//! gRPC `StackTunService` (TCP tunnel stream + structured request bus).

use crate::relay::tunnel_server_loop;
use crate::request_bus::execute_bus_request;
use crate::state::SharedState;
use crate::types::{BusRouteDecisionKind, TunnelMode};
use deploy_proto::stack_tun::stack_tun_service_server::StackTunService;
use deploy_proto::stack_tun::{
    bus_client_msg, bus_server_msg, task_client_msg, task_server_msg, BusClientMsg, BusKv,
    BusRequestEnvelope, BusResponseEnvelope, BusRouteDecision, BusServerMsg, TunClientMsg,
    TunServerMsg, tun_server_msg,
    TaskAssigned, TaskAck, TaskClaimPull, TaskClientMsg as TqClientMsg, TaskServerMsg as TqServerMsg,
    TaskStatusUpdate,
};
use reqwest::header::HeaderMap as ReqHeaderMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

#[derive(Clone)]
pub struct TunGrpc {
    pub inner: Arc<SharedState>,
}

impl TunGrpc {
    pub fn new(inner: Arc<SharedState>) -> Self {
        Self { inner }
    }
}

type TunnelStreamResp = ReceiverStream<Result<TunServerMsg, Status>>;
type RequestBusStreamResp = ReceiverStream<Result<BusServerMsg, Status>>;
type TaskQueueStreamResp = ReceiverStream<Result<TqServerMsg, Status>>;

const RESP_CHUNK: usize = 48 * 1024;

#[tonic::async_trait]
impl StackTunService for TunGrpc {
    type TunnelStreamStream = TunnelStreamResp;
    type RequestBusStreamStream = RequestBusStreamResp;
    type TaskQueueStreamStream = TaskQueueStreamResp;

    async fn tunnel_stream(
        &self,
        request: Request<Streaming<TunClientMsg>>,
    ) -> Result<Response<Self::TunnelStreamStream>, Status> {
        let meta = request.metadata().clone();
        let key = tonic::metadata::MetadataKey::from_static("x-stack-tun-profile");
        let pid = meta
            .get(&key)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::permission_denied("missing x-stack-tun-profile header"))?;

        let vk = self.inner.grpc_verify_tunnel_meta(&meta, pid).await?;
        let prof = self.inner.listener_profile_clone(pid).await?;
        if prof.mode != TunnelMode::TcpRelay {
            return Err(Status::failed_precondition(
                "listener profile uses requestBus mode; use RequestBusStream instead",
            ));
        }

        let pk_bytes: [u8; 32] = *vk.as_bytes();
        if !self.inner.connector_allowed(&prof, &pk_bytes) {
            return Err(Status::permission_denied(
                "public key not in connector allow-list for profile",
            ));
        }

        let (tx, rx) = mpsc::channel::<Result<TunServerMsg, Status>>(128);
        if tx
            .send(Ok(TunServerMsg {
                msg: Some(tun_server_msg::Msg::Ready(true)),
            }))
            .await
            .is_err()
        {
            return Err(Status::internal("stream closed"));
        }

        let inbound = request.into_inner();
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            tunnel_server_loop(inner, prof, inbound, tx.clone()).await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn request_bus_stream(
        &self,
        request: Request<Streaming<BusClientMsg>>,
    ) -> Result<Response<Self::RequestBusStreamStream>, Status> {
        let meta = request.metadata().clone();
        let key = tonic::metadata::MetadataKey::from_static("x-stack-tun-profile");
        let pid = meta
            .get(&key)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::permission_denied("missing x-stack-tun-profile header"))?;

        let vk = self.inner.grpc_verify_request_bus_meta(&meta, pid).await?;
        let prof = self.inner.listener_profile_clone(pid).await?;
        if prof.mode != TunnelMode::RequestBus {
            return Err(Status::failed_precondition(
                "listener profile mode must be requestBus",
            ));
        }

        let pk_bytes: [u8; 32] = *vk.as_bytes();
        if !self.inner.connector_allowed(&prof, &pk_bytes) {
            return Err(Status::permission_denied(
                "public key not in connector allow-list for profile",
            ));
        }

        let (tx, rx) = mpsc::channel::<Result<BusServerMsg, Status>>(128);
        if tx
            .send(Ok(BusServerMsg {
                msg: Some(bus_server_msg::Msg::Ready(true)),
            }))
            .await
            .is_err()
        {
            return Err(Status::internal("stream closed"));
        }

        let inbound = request.into_inner();
        let st = Arc::clone(&self.inner);
        let profile_id = prof.id.clone();
        tokio::spawn(async move {
            bus_session_loop(st, inbound, tx, profile_id).await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn task_queue_stream(
        &self,
        request: Request<Streaming<TqClientMsg>>,
    ) -> Result<Response<Self::TaskQueueStreamStream>, Status> {
        let meta = request.metadata().clone();
        let key = tonic::metadata::MetadataKey::from_static("x-stack-tun-profile");
        let pid = meta
            .get(&key)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::permission_denied("missing x-stack-tun-profile header"))?;

        let vk = self.inner.grpc_verify_task_queue_meta(&meta, pid).await?;
        let prof = self.inner.listener_profile_clone(pid).await?;
        if prof.mode != TunnelMode::RequestBus {
            return Err(Status::failed_precondition(
                "listener profile mode must be requestBus for task queue worker",
            ));
        }

        let pk_bytes: [u8; 32] = *vk.as_bytes();
        if !self.inner.connector_allowed(&prof, &pk_bytes) {
            return Err(Status::permission_denied(
                "public key not in connector allow-list for profile",
            ));
        }

        let (tx, rx) = mpsc::channel::<Result<TqServerMsg, Status>>(128);
        if tx
            .send(Ok(TqServerMsg {
                msg: Some(task_server_msg::Msg::Ready(true)),
            }))
            .await
            .is_err()
        {
            return Err(Status::internal("stream closed"));
        }

        let inbound = request.into_inner();
        let st = Arc::clone(&self.inner);
        let profile_id = prof.id.clone();
        tokio::spawn(async move {
            task_queue_session_loop(st, inbound, tx, profile_id, pk_bytes).await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

#[derive(Clone, Default)]
struct BusAcc {
    request_id: String,
    trace_id: String,
    hop_id: u32,
    source_node_id: String,
    target_node_id: String,
    method: String,
    scheme: String,
    host: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl BusAcc {
    fn apply_part(&mut self, env: &BusRequestEnvelope) {
        if !env.request_id.is_empty() {
            self.request_id.clone_from(&env.request_id);
        }
        if !env.trace_id.is_empty() {
            self.trace_id.clone_from(&env.trace_id);
        }
        self.hop_id = env.hop_id;
        if !env.source_node_id.is_empty() {
            self.source_node_id.clone_from(&env.source_node_id);
        }
        if !env.target_node_id.is_empty() {
            self.target_node_id.clone_from(&env.target_node_id);
        }
        if !env.method.is_empty() {
            self.method.clone_from(&env.method);
        }
        if !env.scheme.is_empty() {
            self.scheme.clone_from(&env.scheme);
        }
        if !env.host.is_empty() {
            self.host.clone_from(&env.host);
        }
        if !env.path.is_empty() {
            self.path.clone_from(&env.path);
        }
        for hv in &env.headers {
            let k = hv.key.trim().to_ascii_lowercase();
            if !k.is_empty() {
                self.headers.insert(k, hv.value.clone());
            }
        }
        self.body.extend_from_slice(&env.body_chunk);
    }
}

fn hdr_map_to_kv(hm: &ReqHeaderMap) -> Vec<BusKv> {
    hm.iter()
        .filter_map(|(name, v)| {
            Some(BusKv {
                key: name.as_str().to_string(),
                value: v.to_str().ok()?.to_string(),
            })
        })
        .collect()
}

async fn stream_response_chunks(
    tx: &mpsc::Sender<Result<BusServerMsg, Status>>,
    request_id: &str,
    status: u16,
    hm: ReqHeaderMap,
    body: Vec<u8>,
) -> Result<(), Status> {
    let rid = request_id.to_string();

    tx.send(Ok(BusServerMsg {
        msg: Some(bus_server_msg::Msg::ResponsePart(BusResponseEnvelope {
            request_id: rid.clone(),
            status: status as u32,
            headers: hdr_map_to_kv(&hm),
            body_chunk: vec![],
            finished: false,
            error: String::new(),
        })),
    }))
    .await
    .map_err(|_| Status::cancelled("client disconnected"))?;

    if body.is_empty() {
        let _ = tx
            .send(Ok(BusServerMsg {
                msg: Some(bus_server_msg::Msg::ResponsePart(BusResponseEnvelope {
                    request_id: rid,
                    status: status as u32,
                    headers: vec![],
                    body_chunk: vec![],
                    finished: true,
                    error: String::new(),
                })),
            }))
            .await;
        return Ok(());
    }

    let chunk_count = (body.len().saturating_add(RESP_CHUNK - 1)).div_euclid(RESP_CHUNK);
    let mut ci = 0usize;
    for ch in body.chunks(RESP_CHUNK) {
        ci += 1;
        let finished = ci == chunk_count;
        if tx
            .send(Ok(BusServerMsg {
                msg: Some(bus_server_msg::Msg::ResponsePart(BusResponseEnvelope {
                    request_id: rid.clone(),
                    status: status as u32,
                    headers: vec![],
                    body_chunk: ch.to_vec(),
                    finished,
                    error: String::new(),
                })),
            }))
            .await
            .is_err()
        {
            break;
        }
    }
    Ok(())
}

async fn finalize_one_bus_request(
    st: Arc<SharedState>,
    tx: &mpsc::Sender<Result<BusServerMsg, Status>>,
    profile_id: &str,
    acc: BusAcc,
) {
    st.stats
        .request_bus_received
        .fetch_add(1, AtomicOrdering::Relaxed);

    let rid = acc.request_id.clone();

    let rules = {
        let root = st.store.read_root().await;
        root.routes.clone()
    };

    let exec = execute_bus_request(
        &st,
        &rules,
        profile_id,
        rid.clone(),
        acc.trace_id.clone(),
        acc.source_node_id.clone(),
        acc.target_node_id.clone(),
        acc.hop_id,
        acc.method.clone(),
        acc.scheme.clone(),
        acc.host.clone(),
        acc.path.clone(),
        acc.headers.clone(),
        acc.body.clone(),
    )
    .await;

    match exec {
        Ok((status, hm, bytes, decision)) => {
            if matches!(decision, BusRouteDecisionKind::Deny) {
                st.stats
                    .request_bus_blocked
                    .fetch_add(1, AtomicOrdering::Relaxed);
            }

            let dec_str = format!("{:?}", decision).to_lowercase();

            let _ = tx
                .send(Ok(BusServerMsg {
                    msg: Some(bus_server_msg::Msg::RouteDecision(BusRouteDecision {
                        request_id: rid.clone(),
                        decision: dec_str.clone(),
                        detail: if matches!(decision, BusRouteDecisionKind::Queue) {
                            "queued".into()
                        } else {
                            "executed".into()
                        },
                    })),
                }))
                .await;

            if let Err(status) = stream_response_chunks(tx, &rid, status, hm, bytes).await {
                let _ = tx.send(Err(status)).await;
            }

            let is_async_queue = matches!(decision, BusRouteDecisionKind::Queue);
            if !is_async_queue && status >= 400 {
                st.stats
                    .request_bus_errors
                    .fetch_add(1, AtomicOrdering::Relaxed);
            }

            if !is_async_queue {
                st.stats
                    .request_bus_completed
                    .fetch_add(1, AtomicOrdering::Relaxed);
            }
        }
        Err(e) => {
            st.stats
                .request_bus_errors
                .fetch_add(1, AtomicOrdering::Relaxed);

            let _ = tx
                .send(Ok(BusServerMsg {
                    msg: Some(bus_server_msg::Msg::RouteDecision(BusRouteDecision {
                        request_id: rid.clone(),
                        decision: "error".into(),
                        detail: e.clone(),
                    })),
                }))
                .await;

            let _ = tx
                .send(Ok(BusServerMsg {
                    msg: Some(bus_server_msg::Msg::ResponsePart(BusResponseEnvelope {
                        request_id: rid,
                        status: 502,
                        headers: vec![],
                        body_chunk: vec![],
                        finished: true,
                        error: e,
                    })),
                }))
                .await;
        }
    }
}

async fn bus_session_loop(
    st: Arc<SharedState>,
    mut inbound: Streaming<BusClientMsg>,
    tx: mpsc::Sender<Result<BusServerMsg, Status>>,
    profile_id: String,
) {
    let mut acc = BusAcc::default();

    loop {
        let incoming = inbound.message().await;
        match incoming {
            Ok(Some(BusClientMsg {
                msg: Some(bus_client_msg::Msg::RequestPart(env)),
            })) => {
                let finished = env.body_finished;
                acc.apply_part(&env);
                if finished {
                    let next = BusAcc::default();
                    let cur = std::mem::replace(&mut acc, next);
                    finalize_one_bus_request(Arc::clone(&st), &tx, &profile_id, cur).await;
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                let _ = tx
                    .send(Ok(BusServerMsg {
                        msg: Some(bus_server_msg::Msg::FatalError(format!("read stream: {e}"))),
                    }))
                    .await;
                break;
            }
        }
    }
}

async fn task_queue_session_loop(
    st: Arc<SharedState>,
    mut inbound: Streaming<TqClientMsg>,
    tx: mpsc::Sender<Result<TqServerMsg, Status>>,
    profile_id: String,
    pk: [u8; 32],
) {
    loop {
        match inbound.message().await {
            Ok(Some(TqClientMsg {
                msg: Some(task_client_msg::Msg::ClaimPull(TaskClaimPull { wait_ms })),
            })) => {
                let queue = {
                    let mq = st.task_queues.lock();
                    mq.get(&profile_id).cloned()
                };
                let Some(queue) = queue else {
                    let _ = tx
                        .send(Ok(TqServerMsg {
                            msg: Some(task_server_msg::Msg::Error(
                                "no task queue for profile".into(),
                            )),
                        }))
                        .await;
                    continue;
                };
                let dur = Duration::from_millis((wait_ms as u64).clamp(100, 120_000));
                match Arc::clone(&queue).claim_next(&pk, dur, &st.stats).await {
                    Ok(env) => {
                        let _ = tx
                            .send(Ok(TqServerMsg {
                                msg: Some(task_server_msg::Msg::Assigned(TaskAssigned {
                                    envelope: Some(env),
                                })),
                            }))
                            .await;
                    }
                    Err(status) => {
                        if status.code() == tonic::Code::Cancelled {
                            let _ = tx
                                .send(Ok(TqServerMsg {
                                    msg: Some(task_server_msg::Msg::Error("pull_timeout".into())),
                                }))
                                .await;
                        } else {
                            let _ = tx.send(Err(status)).await;
                            break;
                        }
                    }
                }
            }
            Ok(Some(TqClientMsg {
                msg: Some(task_client_msg::Msg::StatusUpdate(TaskStatusUpdate {
                    request_id,
                    detail,
                    progress_pct,
                    ..
                })),
            })) => {
                let queue = {
                    let mq = st.task_queues.lock();
                    mq.get(&profile_id).cloned()
                };
                let Some(queue) = queue else {
                    let _ = tx
                        .send(Ok(TqServerMsg {
                            msg: Some(task_server_msg::Msg::Error(
                                "no task queue for profile".into(),
                            )),
                        }))
                        .await;
                    continue;
                };
                let rid = request_id.trim().to_string();
                let dd = detail.trim();
                match queue.apply_status(
                    &pk,
                    rid.as_str(),
                    (!dd.is_empty()).then_some(dd),
                    progress_pct,
                    &st.stats,
                ) {
                    Ok(()) => {
                        let _ = tx
                            .send(Ok(TqServerMsg {
                                msg: Some(task_server_msg::Msg::Ack(TaskAck {
                                    request_id: rid,
                                    ack_kind: "status".into(),
                                })),
                            }))
                            .await;
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                    }
                }
            }
            Ok(Some(TqClientMsg {
                msg: Some(task_client_msg::Msg::CompletePart(part)),
            })) => {
                let rid_for_ack = part.request_id.trim().to_string();
                let queue = {
                    let mq = st.task_queues.lock();
                    mq.get(&profile_id).cloned()
                };
                let Some(queue) = queue else {
                    let _ = tx
                        .send(Ok(TqServerMsg {
                            msg: Some(task_server_msg::Msg::Error(
                                "no task queue for profile".into(),
                            )),
                        }))
                        .await;
                    continue;
                };
                match queue.apply_complete(&pk, part, &st.stats) {
                    Ok(()) => {
                        let _ = tx
                            .send(Ok(TqServerMsg {
                                msg: Some(task_server_msg::Msg::Ack(TaskAck {
                                    request_id: rid_for_ack,
                                    ack_kind: "complete".into(),
                                })),
                            }))
                            .await;
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                    }
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                let _ = tx
                    .send(Ok(TqServerMsg {
                        msg: Some(task_server_msg::Msg::Error(format!("read stream: {e}"))),
                    }))
                    .await;
                break;
            }
        }
    }
}
