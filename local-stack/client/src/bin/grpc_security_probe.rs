//! gRPC auth regression checks: nonce replay, invalid metadata, ConnectionProbe project_id mismatch.
//! Used by Docker protocol test stack (`grpc-security-probe` in `pirate-bench-runtime`).

use clap::Parser;
use deploy_auth::{
    load_identity, pubkey_b64_url, rpc_canonical, sign_bytes, signing_payload, META_NONCE,
    META_PUBKEY, META_SIG, META_TS,
};
use deploy_proto::deploy::{ConnectionProbeChunk, StatusRequest};
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::{MetadataMap, MetadataValue};
use tonic::{Code, Request};

fn insert_ascii(m: &mut MetadataMap, key: &str, val: &str) -> Result<(), String> {
    let k = tonic::metadata::MetadataKey::from_bytes(key.as_bytes())
        .map_err(|_| "metadata key".to_string())?;
    let v = MetadataValue::try_from(val).map_err(|_| "metadata value must be ascii".to_string())?;
    m.insert(k, v);
    Ok(())
}

fn auth_metadata(
    sk: &SigningKey,
    method: &str,
    project_id: &str,
    ts_ms: i64,
    nonce: &str,
) -> Result<MetadataMap, String> {
    let payload = signing_payload(method, project_id, "");
    let msg = rpc_canonical(method, ts_ms, nonce, &payload);
    let sig_b64 = sign_bytes(sk, &msg);
    let mut m = MetadataMap::new();
    insert_ascii(&mut m, META_PUBKEY, &pubkey_b64_url(sk))?;
    insert_ascii(&mut m, META_TS, &ts_ms.to_string())?;
    insert_ascii(&mut m, META_NONCE, nonce)?;
    insert_ascii(&mut m, META_SIG, &sig_b64)?;
    Ok(m)
}

#[derive(Parser, Debug)]
#[command(name = "grpc-security-probe")]
struct Args {
    #[arg(long)]
    endpoint: String,
    #[arg(long, default_value = "default")]
    project: String,
    #[arg(long)]
    identity: Option<PathBuf>,
}

fn identity_path(cli: &Args) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(p) = &cli.identity {
        return Ok(p.clone());
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".config")
        });
    Ok(base.join("pirate-client").join("identity.json"))
}

async fn test_replay_nonce(
    endpoint: &str,
    project_id: &str,
    sk: &SigningKey,
) -> Result<(), String> {
    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| e.to_string())?;
    let ts_ms = deploy_auth::now_unix_ms();
    let nonce = "replay_probe_fixed_nonce_01";
    let map = auth_metadata(sk, "GetStatus", project_id, ts_ms, nonce)?;

    let mut req1 = Request::new(StatusRequest {
        project_id: project_id.to_string(),
    });
    *req1.metadata_mut() = map.clone();

    client
        .get_status(req1)
        .await
        .map_err(|e| format!("first GetStatus: {e}"))?;

    let mut req2 = Request::new(StatusRequest {
        project_id: project_id.to_string(),
    });
    *req2.metadata_mut() = map;
    let err = client
        .get_status(req2)
        .await
        .err()
        .ok_or_else(|| "second GetStatus should fail (replay)".to_string())?;
    let st = err;
    if st.code() != Code::Unauthenticated {
        return Err(format!("expected Unauthenticated, got {:?}", st.code()));
    }
    let msg = st.message();
    if !msg.contains("replay") && !msg.contains("Replay") && !msg.contains("nonce") {
        return Err(format!("unexpected error message: {msg}"));
    }
    Ok(())
}

async fn test_bad_timestamp(
    endpoint: &str,
    project_id: &str,
    sk: &SigningKey,
) -> Result<(), String> {
    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| e.to_string())?;
    let mut map = auth_metadata(sk, "GetStatus", project_id, deploy_auth::now_unix_ms(), "n1")?;
    map.insert(
        tonic::metadata::MetadataKey::from_static("x-deploy-ts"),
        MetadataValue::try_from("not_a_number").unwrap(),
    );

    let mut req = Request::new(StatusRequest {
        project_id: project_id.to_string(),
    });
    *req.metadata_mut() = map;
    let err = client
        .get_status(req)
        .await
        .err()
        .ok_or_else(|| "expected failure for bad timestamp".to_string())?;
    if err.code() != Code::Unauthenticated && err.code() != Code::InvalidArgument {
        return Err(format!("expected Unauthenticated/InvalidArgument: {err}"));
    }
    Ok(())
}

async fn test_nonce_too_long(
    endpoint: &str,
    project_id: &str,
    sk: &SigningKey,
) -> Result<(), String> {
    let long_nonce: String = (0..130).map(|_| 'a').collect();
    let r = auth_metadata(
        sk,
        "GetStatus",
        project_id,
        deploy_auth::now_unix_ms(),
        &long_nonce,
    );
    if r.is_ok() {
        // signing path may still produce metadata; server must reject
        let mut client = DeployServiceClient::connect(endpoint.to_string())
            .await
            .map_err(|e| e.to_string())?;
        let mut req = Request::new(StatusRequest {
            project_id: project_id.to_string(),
        });
        *req.metadata_mut() = r.unwrap();
        let err = client
            .get_status(req)
            .await
            .err()
            .ok_or_else(|| "expected failure for nonce > 128".to_string())?;
        if err.code() == Code::Ok {
            return Err("nonce length not rejected".to_string());
        }
        return Ok(());
    }
    Ok(())
}

async fn test_pubkey_garbage(endpoint: &str, project_id: &str) -> Result<(), String> {
    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| e.to_string())?;
    let ts_ms = deploy_auth::now_unix_ms();
    let nonce = "n2";
    let mut m = MetadataMap::new();
    insert_ascii(&mut m, META_PUBKEY, "' OR 1=1 --")?;
    insert_ascii(&mut m, META_TS, &ts_ms.to_string())?;
    insert_ascii(&mut m, META_NONCE, nonce)?;
    insert_ascii(&mut m, META_SIG, "AAAA")?;

    let mut req = Request::new(StatusRequest {
        project_id: project_id.to_string(),
    });
    *req.metadata_mut() = m;
    let err = client
        .get_status(req)
        .await
        .err()
        .ok_or_else(|| "expected failure for garbage pubkey".to_string())?;
    if err.code() == Code::Ok {
        return Err("garbage pubkey accepted".to_string());
    }
    Ok(())
}

async fn test_connection_probe_project_mismatch(
    endpoint: &str,
    project_id: &str,
    sk: &SigningKey,
) -> Result<(), String> {
    let mut client = DeployServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| e.to_string())?;

    let (tx, rx) = mpsc::channel::<ConnectionProbeChunk>(4);
    let pid = project_id.to_string();
    tokio::spawn(async move {
        let _ = tx
            .send(ConnectionProbeChunk {
                project_id: pid.clone(),
                download_request_bytes: 64,
                data: vec![0xcd; 32],
            })
            .await;
        let _ = tx
            .send(ConnectionProbeChunk {
                project_id: "other_project_mismatch".to_string(),
                download_request_bytes: 0,
                data: vec![0u8],
            })
            .await;
    });

    let mut req = Request::new(ReceiverStream::new(rx));
    let map = auth_metadata(sk, "ConnectionProbe", project_id, deploy_auth::now_unix_ms(), "n3")?;
    *req.metadata_mut() = map;

    let err = client
        .connection_probe(req)
        .await
        .err()
        .ok_or_else(|| "expected ConnectionProbe failure for project_id mismatch".to_string())?;
    if err.code() != Code::InvalidArgument {
        return Err(format!("expected InvalidArgument, got {:?}", err.code()));
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let path = identity_path(&args)?;
    let sk = load_identity(&path).map_err(|e| format!("load identity {:?}: {}", path, e))?;

    let mut failed = 0usize;

    print!("  replay_nonce ... ");
    match test_replay_nonce(&args.endpoint, &args.project, &sk).await {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL ({e})");
            failed += 1;
        }
    }
    print!("  bad_timestamp ... ");
    match test_bad_timestamp(&args.endpoint, &args.project, &sk).await {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL ({e})");
            failed += 1;
        }
    }
    print!("  nonce_too_long ... ");
    match test_nonce_too_long(&args.endpoint, &args.project, &sk).await {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL ({e})");
            failed += 1;
        }
    }

    print!("  pubkey_injection ... ");
    match test_pubkey_garbage(&args.endpoint, &args.project).await {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL ({e})");
            failed += 1;
        }
    }

    print!("  connection_probe_project_mismatch ... ");
    match test_connection_probe_project_mismatch(&args.endpoint, &args.project, &sk).await {
        Ok(()) => println!("PASS"),
        Err(e) => {
            println!("FAIL ({e})");
            failed += 1;
        }
    }

    if failed > 0 {
        eprintln!("grpc-security-probe: {failed} test(s) failed");
        std::process::exit(1);
    }
    Ok(())
}
