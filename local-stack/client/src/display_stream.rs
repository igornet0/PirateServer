//! Producer: capture screen and POST JPEG to consumer ingest URL (best-effort).

use std::time::Duration;

use deploy_auth::attach_auth_metadata;
use deploy_core::display_stream::{
    DisplayStreamConfig, DisplayStreamEncrypt, DisplayStreamRole,
};
use deploy_proto::deploy::{DisplayTopologyDisplay, ReportDisplayTopologyRequest};
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use tonic::Request;
use xcap::Monitor;

fn encode_jpeg_rgba(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    let enc = JpegEncoder::new_with_quality(&mut out, quality);
    enc
        .write_image(
            &rgb,
            width,
            height,
            ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn list_displays() -> Result<(), String> {
    let mons = Monitor::all().map_err(|e| e.to_string())?;
    for (i, m) in mons.iter().enumerate() {
        let w = m.width();
        let h = m.height();
        let name = m.name();
        println!("{i}\t{name}\t{w}x{h}");
    }
    Ok(())
}

/// Push current monitor list to deploy-server (dashboard display-stream UI).
pub async fn send_display_topology(
    endpoint: &str,
    project_id: &str,
    sk: &SigningKey,
) -> Result<(), String> {
    let mons = Monitor::all().map_err(|e| e.to_string())?;
    let displays: Vec<DisplayTopologyDisplay> = mons
        .iter()
        .enumerate()
        .map(|(i, m)| DisplayTopologyDisplay {
            index: i as u32,
            label: m.name().to_string(),
            width: m.width(),
            height: m.height(),
        })
        .collect();
    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| e.to_string())?;
    let mut req = Request::new(ReportDisplayTopologyRequest {
        project_id: project_id.to_string(),
        displays,
        stream_capable: true,
    });
    attach_auth_metadata(
        &mut req,
        sk,
        "ReportDisplayTopology",
        project_id,
        "",
    )
    .map_err(|e| e.to_string())?;
    client
        .report_display_topology(req)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Optional `(gRPC endpoint, project_id, signing key)` to push monitor list to deploy-server for the dashboard.
pub async fn run_producer(
    cfg: DisplayStreamConfig,
    topology_report: Option<(String, String, SigningKey)>,
) -> Result<(), String> {
    cfg.validate()?;
    if cfg.role != DisplayStreamRole::Producer {
        return Err("config role must be producer".into());
    }
    if cfg.encrypt == DisplayStreamEncrypt::Tls && !cfg.ingest_base_url.starts_with("https://") {
        return Err("encrypt=tls requires https:// ingest URL".into());
    }

    if let Some((endpoint, project_id, sk)) = topology_report {
        let endpoint = endpoint.clone();
        let project_id = project_id.clone();
        let sk = sk.clone();
        tokio::spawn(async move {
            let mut intv = tokio::time::interval(Duration::from_secs(30));
            loop {
                intv.tick().await;
                if let Err(e) =
                    send_display_topology(&endpoint, &project_id, &sk).await
                {
                    eprintln!("report display topology: {e}");
                }
            }
        });
    }

    let mons = Monitor::all().map_err(|e| e.to_string())?;
    let idx = cfg.display_index as usize;
    let mon = mons.get(idx).ok_or_else(|| {
        format!(
            "display_index {} out of range ({} monitors); use `display-stream list-displays`",
            idx,
            mons.len()
        )
    })?;

    let interval = Duration::from_secs_f64(1.0 / f64::from(cfg.fps.max(1)));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .pool_idle_timeout(Some(Duration::from_secs(2)))
        .build()
        .map_err(|e| e.to_string())?;

    let url = cfg.ingest_base_url.trim().to_string();
    let token = cfg.token.clone();
    let quality = cfg.quality;

    loop {
        let t0 = std::time::Instant::now();
        let cap = match mon.capture_image() {
            Ok(img) => img,
            Err(e) => {
                eprintln!("capture: {e}");
                tokio::time::sleep(interval).await;
                continue;
            }
        };
        let width = cap.width();
        let height = cap.height();
        let rgba = cap.as_raw();
        let jpeg = match encode_jpeg_rgba(rgba, width, height, quality) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("jpeg: {e}");
                tokio::time::sleep(interval).await;
                continue;
            }
        };

        let mut req = client
            .post(&url)
            .header(CONTENT_TYPE, "image/jpeg")
            .body(jpeg);
        if !token.is_empty() {
            req = req.header(
                AUTHORIZATION,
                format!("Bearer {}", token.trim()),
            );
        }

        let fut = async move {
            let _ = req.send().await;
        };
        tokio::spawn(fut);

        let elapsed = t0.elapsed();
        if elapsed < interval {
            tokio::time::sleep(interval - elapsed).await;
        }
    }
}
