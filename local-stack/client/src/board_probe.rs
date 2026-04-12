//! One-shot gRPC probe: GetStatus RTT, ConnectionProbe throughput, ListSessions + audit tail.

use crate::config::{load_connection, normalize_endpoint, use_signed_requests};
use deploy_auth::attach_auth_metadata;
use deploy_auth::endpoints_equivalent_for_signing;
use deploy_auth::pubkey_b64_url;
use deploy_proto::deploy::{
    ConnectionProbeChunk, ListSessionsRequest, QuerySessionLogsRequest, StatusRequest,
};
use deploy_proto::DeployServiceClient;
use ed25519_dalek::SigningKey;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::Code;
use tonic::Request;

const PROBE_MAX_UPLOAD: u64 = 4 * 1024 * 1024;
const PROBE_MAX_DOWNLOAD: u32 = 4 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;

fn display_current_version(v: &str) -> &str {
    if v.is_empty() {
        "(none)"
    } else {
        v
    }
}

fn no_deployed_app_release(current_version: &str) -> bool {
    current_version.is_empty() || current_version.starts_with("stack@")
}

fn hint_unauth(status: &tonic::Status, endpoint: &str) {
    if status.code() != Code::Unauthenticated {
        return;
    }
    let msg = status.message();
    if !msg.contains("x-deploy-pubkey") {
        return;
    }
    eprintln!(
        "hint: run `pirate auth '<install-json>'` (or `pirate pair`) for this host so signed gRPC metadata is sent."
    );
    eprintln!("      Without pair, no x-deploy-pubkey is sent; endpoint was: {endpoint}");
    if let Some(c) = load_connection() {
        let saved = normalize_endpoint(&c.url);
        let want = normalize_endpoint(endpoint);
        if c.paired && !endpoints_equivalent_for_signing(&saved, &want) {
            eprintln!(
                "note: connection.json is paired for URL `{saved}` but this command used `{want}`."
            );
        } else if !c.paired {
            eprintln!("note: connection.json exists but paired=false; complete `pirate auth` or `pirate pair`.");
        }
    } else {
        eprintln!("note: no saved connection in ~/.config/pirate-client/connection.json for this user.");
    }
}

fn kind_label(k: i32) -> &'static str {
    match k {
        1 => "proxy",
        2 => "resource",
        _ => "unspecified",
    }
}

fn mbps(bits: f64, ms: f64) -> f64 {
    if ms <= 0.0 {
        return 0.0;
    }
    (bits / 1_000_000.0) / (ms / 1000.0)
}

/// Run probe sequence; requires prior pair when signing is enabled for this URL.
pub async fn run_board_test_connect(
    endpoint: &str,
    project_id: &str,
    sk: &SigningKey,
    probe_upload_bytes: u64,
    probe_download_bytes: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    if !use_signed_requests(endpoint) {
        return Err(
            "no paired connection for this URL. Run: pirate auth '<install-json>' first".into(),
        );
    }

    let my_pk = pubkey_b64_url(sk);
    let upload_total = (probe_upload_bytes.min(PROBE_MAX_UPLOAD)) as usize;
    let download_req = probe_download_bytes.min(PROBE_MAX_DOWNLOAD);

    let mut client = DeployServiceClient::connect(endpoint.to_string()).await?;

    // --- GetStatus RTT ---
    let mut status_req = Request::new(StatusRequest {
        project_id: project_id.to_string(),
    });
    attach_auth_metadata(&mut status_req, sk, "GetStatus", project_id, "")?;
    let t0 = Instant::now();
    let status = client
        .get_status(status_req)
        .await
        .map_err(|e| {
            hint_unauth(&e, endpoint);
            e
        })?
        .into_inner();
    let rtt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "get_status rtt_ms={:.2} current_version={} state={}",
        rtt_ms,
        display_current_version(&status.current_version),
        status.state
    );
    if no_deployed_app_release(&status.current_version) {
        eprintln!(
            "note: no app release yet or only stack metadata in current_version; see `pirate auth` help."
        );
    }

    // --- ConnectionProbe ---
    let (tx, rx) = mpsc::channel::<ConnectionProbeChunk>(8);
    let mut probe_req = Request::new(ReceiverStream::new(rx));
    attach_auth_metadata(&mut probe_req, sk, "ConnectionProbe", project_id, "")?;

    let project_owned = project_id.to_string();
    let send_task = tokio::spawn(async move {
        let mut sent = 0usize;
        while sent < upload_total {
            let take = CHUNK.min(upload_total - sent);
            let data = vec![0xCDu8; take];
            let chunk = if sent == 0 {
                ConnectionProbeChunk {
                    project_id: project_owned.clone(),
                    download_request_bytes: download_req,
                    data,
                }
            } else {
                ConnectionProbeChunk {
                    project_id: String::new(),
                    download_request_bytes: 0,
                    data,
                }
            };
            let _ = tx.send(chunk).await;
            sent += take;
        }
        if upload_total == 0 {
            let _ = tx
                .send(ConnectionProbeChunk {
                    project_id: project_owned,
                    download_request_bytes: download_req,
                    data: vec![],
                })
                .await;
        }
    });

    let probe_start = Instant::now();
    let probe = client
        .connection_probe(probe_req)
        .await
        .map_err(|e| {
            hint_unauth(&e, endpoint);
            e
        })?
        .into_inner();
    let client_elapsed_ms = probe_start.elapsed().as_millis() as f64;
    send_task.await?;

    let up_ms = probe.upload_duration_ms.max(1) as f64;
    let upload_bits = (probe.upload_bytes as f64) * 8.0;
    let up_mbps = mbps(upload_bits, up_ms);
    let dl_bytes = probe.download_payload.len() as f64;
    let dl_bits = dl_bytes * 8.0;
    let rest_ms = (client_elapsed_ms - up_ms).max(1.0);
    let dl_mbps = mbps(dl_bits, rest_ms);

    println!(
        "connection_probe upload_bytes={} upload_ms={} upload_mbps≈{:.2}",
        probe.upload_bytes, probe.upload_duration_ms, up_mbps
    );
    println!(
        "connection_probe download_bytes={} client_roundtrip_ms={:.0} download_mbps≈{:.2} (download time estimated as roundtrip − server upload_ms; not a precise split)",
        probe.download_payload.len(),
        client_elapsed_ms,
        dl_mbps
    );
    eprintln!(
        "note: live TCP open/closed is not tracked; use last_seen_ms and session audit (tcp_open/tcp_close) as a heuristic."
    );

    // --- ListSessions (self row) ---
    let mut list_req = Request::new(ListSessionsRequest {
        project_id: project_id.to_string(),
    });
    attach_auth_metadata(&mut list_req, sk, "ListSessions", project_id, "")?;
    let list = match client.list_sessions(list_req).await {
        Ok(r) => r.into_inner(),
        Err(e) => {
            if e.code() == tonic::Code::FailedPrecondition {
                eprintln!(
                    "list_sessions: metadata DB not configured on server (DEPLOY_SQLITE_URL / DATABASE_URL); skipping peer row."
                );
                return Ok(());
            }
            hint_unauth(&e, endpoint);
            return Err(e.into());
        }
    };

    let row = list.peers.iter().find(|p| p.client_public_key_b64 == my_pk);
    if let Some(p) = row {
        println!(
            "this_client last_seen_ms={} last_peer_ip={} last_grpc_method={} kind={} ({}) proxy_bytes_in={} proxy_bytes_out={}",
            p.last_seen_ms,
            p.last_peer_ip,
            p.last_grpc_method,
            p.connection_kind,
            kind_label(p.connection_kind),
            p.proxy_bytes_in_total,
            p.proxy_bytes_out_total
        );
    } else {
        println!("this_client: no row for your public key in ListSessions (not enrolled or auth peer list empty).");
    }

    // --- Short audit tail for this key ---
    let mut log_req = Request::new(QuerySessionLogsRequest {
        project_id: project_id.to_string(),
        limit: 80,
        before_id: 0,
    });
    attach_auth_metadata(&mut log_req, sk, "QuerySessionLogs", project_id, "")?;
    match client.query_session_logs(log_req).await {
        Ok(r) => {
            let evs: Vec<_> = r
                .into_inner()
                .events
                .into_iter()
                .filter(|e| e.client_public_key_b64 == my_pk)
                .take(4)
                .collect();
            if !evs.is_empty() {
                println!("recent_audit (this key, up to 4):");
                for e in evs {
                    println!(
                        "  id={} created_at_ms={} kind={} status={} peer_ip={} method={} detail={}",
                        e.id,
                        e.created_at_ms,
                        e.kind,
                        e.status,
                        e.peer_ip,
                        e.grpc_method,
                        e.detail
                    );
                }
            }
        }
        Err(e) => {
            if e.code() == tonic::Code::FailedPrecondition {
                eprintln!("query_session_logs: metadata DB not configured; skipping audit tail.");
            } else {
                hint_unauth(&e, endpoint);
                return Err(e.into());
            }
        }
    }

    Ok(())
}
