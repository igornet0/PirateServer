//! `pirate tunnel` — stack-tun-api TaskQueue worker (Pull / Assigned / upstream / Complete).

use clap::Parser;
use deploy_auth::{attach_auth_metadata, load_identity, pubkey_b64_url};
use deploy_client::config::identity_path;
use deploy_proto::stack_tun::stack_tun_service_client::StackTunServiceClient;
use deploy_proto::stack_tun::{
    task_client_msg, task_server_msg, BusKv, BusRequestEnvelope, TaskClaimPull,
    TaskClientMsg, TaskComplete, TaskPhase, TaskServerMsg, TaskStatusUpdate,
};
use reqwest::header::{HeaderMap as ReqHeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Status};
use tonic::metadata::{AsciiMetadataValue, MetadataKey};

const COMPLETE_CHUNK: usize = 48 * 1024;

#[derive(Clone, Parser, Debug)]
#[command(about = "Run a stack-tun TaskQueue worker (claim HTTP tasks, proxy to local --target).")]
pub struct TunnelArgs {
    /// stack-tun HTTP control base (`http://host:9380` or `https://…`).
    #[arg(long)]
    pub url: String,
    #[arg(long, default_value = "local")]
    pub mode: String,
    #[arg(long)]
    pub bearer: Option<String>,
    /// stack-tun gRPC endpoint (defaults to swapping `:9380` → `:9381` on `--url`).
    #[arg(long)]
    pub grpc: Option<String>,
    /// Listener RequestBus profile id on the remote stack-tun-api.
    #[arg(long)]
    pub listen_profile_id: String,
    /// Upstream socket for Assigned requests (`127.0.0.1:8080`).
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub target: String,
    #[arg(long)]
    pub identity: Option<std::path::PathBuf>,
    #[arg(long, default_value_t = 30_000)]
    pub pull_wait_ms: u64,
}

fn normalize_http_base(raw: &str) -> String {
    let t = raw.trim().trim_end_matches('/');
    t.to_string()
}

fn bearer_client(bearer: Option<&str>) -> Client {
    let mut b = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120));
    if let Some(tok) = bearer.filter(|x| !x.trim().is_empty()) {
        let mut headers = ReqHeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", tok.trim())) {
            headers.insert(
                HeaderName::from_static("authorization"),
                v,
            );
        }
        b = b.default_headers(headers);
    }
    b.build().expect("reqwest client")
}

fn infer_grpc_url(http_base: &str) -> Option<String> {
    let t = http_base.trim_end_matches('/');
    if let Some(head) = t.strip_suffix(":9380") {
        return Some(format!("{head}:9381"));
    }
    None
}

fn parse_sock(target: &str) -> Result<(String, u16), String> {
    let s = target.trim();
    if s.is_empty() {
        return Err("target empty".into());
    }
    if let Ok(addr) = s.parse::<std::net::SocketAddr>() {
        return match addr {
            std::net::SocketAddr::V4(v) => Ok((v.ip().to_string(), v.port())),
            std::net::SocketAddr::V6(v) => Ok((format!("[{}]", v.ip()), v.port())),
        };
    }
    let (host, port_s) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("bad target `{s}` — use host:port"))?;
    if host.is_empty() {
        return Err("target host empty".into());
    }
    let port: u16 = port_s
        .parse()
        .map_err(|_| format!("bad target port `{port_s}`"))?;
    Ok((host.to_string(), port))
}

fn link_kind(mode: &str) -> &'static str {
    match mode.trim().to_ascii_lowercase().as_str() {
        "public" | "publicauth" => "publicAuth",
        _ => "local",
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct IdentityPkResp {
    #[serde(rename = "publicKeyB64")]
    public_key_b64: String,
}

async fn stack_tun_get_text(client: &Client, base: &str, path: &str) -> Result<String, String> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let r = client.get(&url).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        let t = r.text().await.unwrap_or_default();
        return Err(format!("GET {path} {}", t));
    }
    r.text().await.map_err(|e| e.to_string())
}

async fn stack_tun_get_json(client: &Client, base: &str, path: &str) -> Result<Value, String> {
    let s = stack_tun_get_text(client, base, path).await?;
    serde_json::from_str(&s).map_err(|e| format!("json {path}: {e}"))
}

async fn stack_tun_put_profiles(
    client: &Client,
    base: &str,
    profiles: Vec<Value>,
) -> Result<(), String> {
    let url = format!("{}/api/v1/config", base.trim_end_matches('/'));
    let body = serde_json::json!({ "profiles": profiles });
    let r = client
        .put(&url)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        let t = r.text().await.unwrap_or_default();
        return Err(format!("PUT /config {}", t));
    }
    Ok(())
}

fn insert_profile_meta<T>(
    req: &mut Request<T>,
    listen_profile_id: &str,
) -> Result<(), deploy_auth::AuthError> {
    let k = MetadataKey::from_static("x-stack-tun-profile");
    let v = AsciiMetadataValue::try_from(listen_profile_id.trim())
        .map_err(|_| deploy_auth::AuthError::InvalidMetadata(
            "x-stack-tun-profile value".into(),
        ))?;
    req.metadata_mut().insert(k, v);
    Ok(())
}

fn kv_to_req_headers(env: &BusRequestEnvelope) -> Result<ReqHeaderMap, String> {
    let mut hm = ReqHeaderMap::new();
    for kv in &env.headers {
        let name = kv.key.trim();
        if name.is_empty() {
            continue;
        }
        let lk = name.to_ascii_lowercase();
        if matches!(lk.as_str(), "host" | "connection" | "content-length") {
            continue;
        }
        let hn = HeaderName::from_bytes(lk.as_bytes())
            .map_err(|_| format!("invalid header name `{name}`"))?;
        let hv = HeaderValue::from_str(kv.value.trim())
            .map_err(|_| format!("invalid header `{name}`"))?;
        hm.append(hn, hv);
    }
    Ok(hm)
}

async fn run_upstream(envelope: &BusRequestEnvelope, host: &str, port: u16) -> Result<(u16, ReqHeaderMap, Vec<u8>), String> {
    let method = envelope.method.trim().to_uppercase();
    let mth = Method::from_bytes(method.as_bytes())
        .map_err(|_| format!("unsupported method `{method}`"))?;
    let path = envelope.path.trim();
    let path_norm = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let sch = {
        let s = envelope.scheme.trim();
        if s.is_empty() {
            "http"
        } else {
            s
        }
    };
    let url = format!("{sch}://{}:{}{path_norm}", host, port);

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let mut rb = client.request(mth.clone(), url).headers(kv_to_req_headers(envelope)?);
    if !(mth == Method::GET || mth == Method::HEAD) || !envelope.body_chunk.is_empty() {
        rb = rb.body(envelope.body_chunk.clone());
    }
    let resp = rb.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok((status, headers, body))
}

fn req_headers_to_bus_kv(hm: &ReqHeaderMap) -> Vec<BusKv> {
    hm.iter()
        .filter_map(|(n, v)| {
            Some(BusKv {
                key: n.as_str().to_string(),
                value: v.to_str().ok()?.to_string(),
            })
        })
        .collect()
}

async fn pump_completes(
    inbound: &mut tonic::Streaming<TaskServerMsg>,
    tx_out: &mpsc::Sender<TaskClientMsg>,
    rid: &str,
    status: u32,
    headers: ReqHeaderMap,
    body: Vec<u8>,
) -> Result<(), String> {
    let hdrs = req_headers_to_bus_kv(&headers);
    let first = TaskComplete {
        request_id: rid.to_string(),
        status,
        headers: hdrs,
        body_chunk: vec![],
        finished: false,
        error: String::new(),
    };
    tx_out
        .send(TaskClientMsg {
            msg: Some(task_client_msg::Msg::CompletePart(first)),
        })
        .await
        .map_err(|_| "outbound closed")?;
    discard_until_ack_rx(inbound).await.ok();

    if body.is_empty() {
        let fin = TaskComplete {
            request_id: rid.to_string(),
            status,
            headers: vec![],
            body_chunk: vec![],
            finished: true,
            error: String::new(),
        };
        tx_out
            .send(TaskClientMsg {
                msg: Some(task_client_msg::Msg::CompletePart(fin)),
            })
            .await
            .map_err(|_| "outbound closed")?;
        discard_until_ack_rx(inbound).await.ok();
        return Ok(());
    }

    let chunks: Vec<&[u8]> = body.chunks(COMPLETE_CHUNK).collect();
    let n = chunks.len();
    for (i, ch) in chunks.into_iter().enumerate() {
        let finished = i + 1 == n;
        let part = TaskComplete {
            request_id: rid.to_string(),
            status: if i == 0 { status } else { 0 },
            headers: vec![],
            body_chunk: ch.to_vec(),
            finished,
            error: String::new(),
        };
        tx_out
            .send(TaskClientMsg {
                msg: Some(task_client_msg::Msg::CompletePart(part)),
            })
            .await
            .map_err(|_| "outbound closed")?;
        discard_until_ack_rx(inbound).await.ok();
    }
    Ok(())
}

async fn discard_until_ack_rx(
    inbound: &mut tonic::Streaming<TaskServerMsg>,
) -> Result<(), Status> {
    loop {
        let m = inbound
            .message()
            .await?
            .ok_or_else(|| Status::cancelled("server closed"))?;
        match m.msg {
            Some(task_server_msg::Msg::Ack(_)) => return Ok(()),
            Some(task_server_msg::Msg::Ready(_)) => {}
            Some(task_server_msg::Msg::Error(_)) => {}
            Some(task_server_msg::Msg::Assigned(_)) => {}
            None => {}
        }
    }
}

pub async fn run(args: TunnelArgs) -> Result<(), String> {
    let http_base = normalize_http_base(&args.url);
    if http_base.is_empty() {
        return Err("--url empty".into());
    }

    let pid = args.listen_profile_id.trim();
    if pid.is_empty() {
        return Err("--listen-profile-id required".into());
    }

    let grpc_url = normalize_http_base(
        &args
            .grpc
            .clone()
            .or_else(|| infer_grpc_url(&http_base))
            .ok_or_else(|| {
                "cannot infer gRPC URL; pass --grpc (e.g. http://host:9381)".to_string()
            })?,
    );
    let (target_host, target_port) = parse_sock(&args.target)?;

    let id_path = match &args.identity {
        Some(p) => p.clone(),
        None => identity_path().ok_or_else(|| {
            "cannot resolve identity.json path — use `pirate auth` or pass --identity".to_string()
        })?,
    };
    let sk = load_identity(&id_path).map_err(|e| format!("{e} (run `pirate auth` first)"))?;

    let http = bearer_client(args.bearer.as_deref());

    stack_tun_get_text(&http, &http_base, "/health").await?;

    let id_js = stack_tun_get_json(&http, &http_base, "/api/v1/identity/public-key").await?;
    let _listener_pk: IdentityPkResp =
        serde_json::from_value(id_js).map_err(|e| format!("identity/public-key JSON: {e}"))?;

    let pubkey = pubkey_b64_url(&sk);
    let body_peer = serde_json::json!({ "publicKeyB64": pubkey });
    let peers_url = format!("{}/api/v1/peers", http_base.trim_end_matches('/'));
    let peer_put = http
        .post(&peers_url)
        .header("Content-Type", "application/json")
        .body(body_peer.to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !(peer_put.status().is_success() || peer_put.status().as_u16() == 409) {
        let t = peer_put.text().await.unwrap_or_default();
        return Err(format!("authorize peer {}", t));
    }

    let root = stack_tun_get_json(&http, &http_base, "/api/v1/config").await?;
    let mut profiles = root["profiles"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mode = args.mode.trim();
    let lk = link_kind(mode);

    if lk == "publicAuth" {
        let mut touched = false;
        for prof in profiles.iter_mut() {
            let id_hit = prof
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.trim() == pid.trim())
                .unwrap_or(false);
            let role_listen = prof
                .get("role")
                .and_then(|v| v.as_str())
                == Some("listen");
            if !(id_hit && role_listen) {
                continue;
            }
            touched = true;
            let mut arr: Vec<Value> = prof
                .get("connectorAllowPubkeyB64")
                .and_then(|x| x.as_array().cloned())
                .unwrap_or_default();
            let have = arr.iter().any(|v| v.as_str() == Some(pubkey.as_str()));
            if !have {
                arr.push(serde_json::json!(pubkey));
            }
            if let Some(m) = prof.as_object_mut() {
                m.insert("connectorAllowPubkeyB64".into(), serde_json::json!(arr));
            }
            break;
        }
        if !touched {
            return Err(format!(
                "MODE=publicAuth: listener profile `{pid}` not found in config — create a listen + requestBus profile first"
            ));
        }
    }

    let worker_id = format!("pirate-tq-worker-{}", deploy_auth::now_unix_ms());
    let connector = serde_json::json!({
        "id": worker_id.clone(),
        "name": format!("cli-task-queue-{pid}"),
        "role": "connector",
        "mode": "requestBus",
        "linkKind": lk,
        "remoteGrpcEndpoint": grpc_url.clone(),
        "listenProfileId": pid,
        "targetHost": target_host,
        "targetPort": target_port,
        "maxPendingStreams": 128,
        "streamOfferTtlSecs": 300,
        "pullWaitMs": args.pull_wait_ms.max(500).min(120_000),
        "connectorAllowPubkeyB64": [],
        "enabled": false,
    });

    profiles.retain(|p| p.get("id").and_then(|v| v.as_str()) != Some(worker_id.as_str()));
    profiles.push(connector);

    stack_tun_put_profiles(&http, &http_base, profiles).await?;

    println!(
        "[tunnel] grpc={grpc_url} listen_profile={pid} target={}",
        args.target
    );
    let chan = tonic::transport::Endpoint::from_shared(grpc_url.clone())
        .map_err(|e| e.to_string())?
        .connect_timeout(Duration::from_secs(15))
        .tcp_nodelay(true)
        .connect()
        .await
        .map_err(|e| format!("grpc connect: {e}"))?;

    let mut cli = StackTunServiceClient::new(chan);
    let (tx_out, rx_out) = mpsc::channel::<TaskClientMsg>(64);
    let out_stream = ReceiverStream::new(rx_out);
    let mut req = Request::new(out_stream);
    attach_auth_metadata(
        &mut req,
        &sk,
        "StackTunTaskQueue",
        "default",
        pid,
    )
    .map_err(|e| e.to_string())?;
    insert_profile_meta(&mut req, pid).map_err(|e| e.to_string())?;

    let mut inbound = cli
        .task_queue_stream(req)
        .await
        .map_err(|e| format!("task_queue_stream: {e}"))?
        .into_inner();

    let first = inbound
        .message()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "empty server stream".to_string())?;
    match first.msg {
        Some(task_server_msg::Msg::Ready(true)) => {}
        Some(task_server_msg::Msg::Error(s)) => return Err(format!("server error on open: {s}")),
        other => return Err(format!("unexpected handshake: {:?}", other)),
    }

    loop {
        let wait_ms = args.pull_wait_ms.max(500).min(120_000);
        tx_out
            .send(TaskClientMsg {
                msg: Some(task_client_msg::Msg::ClaimPull(TaskClaimPull { wait_ms })),
            })
            .await
            .map_err(|_| "grpc outbound closed")?;

        let srv = inbound
            .message()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "server stream ended".to_string())?;

        match srv.msg {
            Some(task_server_msg::Msg::Assigned(a)) => {
                let env = match a.envelope {
                    Some(e) => e,
                    None => continue,
                };
                let rid = env.request_id.clone();
                if rid.is_empty() {
                    continue;
                }

                let _ = tx_out
                    .send(TaskClientMsg {
                        msg: Some(task_client_msg::Msg::StatusUpdate(TaskStatusUpdate {
                            request_id: rid.clone(),
                            phase: TaskPhase::Running as i32,
                            detail: format!("upstream {target_host}:{target_port}"),
                            progress_pct: 25,
                        })),
                    })
                    .await;

                discard_until_ack_rx(&mut inbound).await.ok();

                match run_upstream(&env, target_host.trim(), target_port).await {
                    Ok((status, hdr, body)) => {
                        let _ =
                            pump_completes(&mut inbound, &tx_out, &rid, status as u32, hdr, body).await;
                    }
                    Err(e) => {
                        let err_part = TaskComplete {
                            request_id: rid.clone(),
                            status: 502,
                            headers: vec![],
                            body_chunk: vec![],
                            finished: true,
                            error: e,
                        };
                        let _ = tx_out
                            .send(TaskClientMsg {
                                msg: Some(task_client_msg::Msg::CompletePart(err_part)),
                            })
                            .await;
                        discard_until_ack_rx(&mut inbound).await.ok();
                    }
                }
            }
            Some(task_server_msg::Msg::Error(_)) => continue,
            Some(task_server_msg::Msg::Ack(_)) | Some(task_server_msg::Msg::Ready(_)) => {}
            None => {}
        }
    }
}
