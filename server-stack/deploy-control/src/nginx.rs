use crate::types::{NginxConfigView, NginxPutResponseView};
use std::path::Path;
use std::process::Output;

fn output_text(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    format!("{stdout}{stderr}")
}

/// Read nginx config file for API response.
pub async fn read_nginx_config(path: &Path) -> Result<NginxConfigView, std::io::Error> {
    let content = tokio::fs::read_to_string(path).await?;
    Ok(NginxConfigView {
        path: path.display().to_string(),
        content,
        enabled: true,
    })
}

pub struct NginxPutOutcome {
    pub response: NginxPutResponseView,
}

/// Write config, `nginx -t`, optionally `nginx -s reload`. On test failure, returns `revert_to` content.
pub async fn apply_nginx_put(
    path: &Path,
    content: &str,
    test_full_config: bool,
) -> Result<NginxPutOutcome, std::io::Error> {
    let previous = tokio::fs::read_to_string(path).await.unwrap_or_default();
    tokio::fs::write(path, content).await?;

    let path_owned = path.to_path_buf();
    let test_full = test_full_config;
    let nginx_test_output_result = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("nginx");
        cmd.arg("-t");
        if test_full {
            cmd.arg("-c").arg(&path_owned);
        }
        cmd.output()
    })
    .await??;

    let test_out = output_text(&nginx_test_output_result);

    if !nginx_test_output_result.status.success() {
        let _ = tokio::fs::write(path, &previous).await;
        return Ok(NginxPutOutcome {
            response: NginxPutResponseView {
                ok: false,
                message: "nginx -t failed; file reverted".into(),
                test_output: Some(test_out),
                reload_output: None,
            },
        });
    }

    let reload_path = path.to_path_buf();
    let reload_full = test_full_config;
    let reload_res = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("nginx");
        if reload_full {
            cmd.arg("-c").arg(&reload_path);
        }
        cmd.arg("-s").arg("reload").output()
    })
    .await??;

    let reload_out = output_text(&reload_res);

    if !reload_res.status.success() {
        tracing::warn!(%reload_out, "nginx reload failed");
        return Ok(NginxPutOutcome {
            response: NginxPutResponseView {
                ok: false,
                message: "nginx -t ok but nginx -s reload failed (config left on disk)".into(),
                test_output: Some(test_out),
                reload_output: Some(reload_out),
            },
        });
    }

    tracing::info!(path = %path.display(), "nginx config updated and reloaded");
    Ok(NginxPutOutcome {
        response: NginxPutResponseView {
            ok: true,
            message: "nginx config written, nginx -t passed, reload sent".into(),
            test_output: Some(test_out),
            reload_output: Some(reload_out),
        },
    })
}
