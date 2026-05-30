//! In-memory FIFO task queue per listener RequestBus profile (Pull / claim / lease / complete).

use crate::types::GlobalStats;
use base64::Engine;
use deploy_proto::stack_tun::{BusKv, BusRequestEnvelope, TaskComplete};
use parking_lot::Mutex as PMutex;
use rand_core::{OsRng, RngCore};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StoredTaskPhase {
    Queued,
    Claimed,
    Running,
    Completed,
    Failed,
    Expired,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOuterJson {
    pub request_id: String,
    pub profile_id: String,
    #[serde(rename = "phase")]
    pub phase_str: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_pct: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

struct TaskEntry {
    profile_id: String,
    envelope: BusRequestEnvelope,
    queued_at: Instant,
    phase: StoredTaskPhase,
    claimant: Option<[u8; 32]>,
    lease_until: Option<Instant>,
    detail: Option<String>,
    progress_pct: u32,
    response_status: Option<u32>,
    response_headers: Vec<BusKv>,
    response_body: Vec<u8>,
    terminal_error: Option<String>,
    done: Arc<Notify>,
}

#[derive(Clone)]
pub struct TaskQueue(pub Arc<TaskQueueInner>);

pub struct TaskQueueInner {
    profile_id: String,
    cap: usize,
    ttl: Duration,
    lease: Duration,
    deque_ids: PMutex<VecDeque<String>>,
    tasks: PMutex<HashMap<String, TaskEntry>>,
    notify_claim: Notify,
}

enum ClaimAttempt {
    Taken(BusRequestEnvelope),
    Skip,
    RequeueFront,
}

fn phase_str(p: StoredTaskPhase) -> &'static str {
    match p {
        StoredTaskPhase::Queued => "queued",
        StoredTaskPhase::Claimed => "claimed",
        StoredTaskPhase::Running => "running",
        StoredTaskPhase::Completed => "completed",
        StoredTaskPhase::Failed => "failed",
        StoredTaskPhase::Expired => "expired",
    }
}

impl TaskQueue {
    pub fn new(profile_id: &str, cap: usize, ttl: Duration, lease: Duration) -> Arc<Self> {
        Arc::new(Self(Arc::new(TaskQueueInner {
            profile_id: profile_id.trim().to_string(),
            cap: cap.max(1),
            ttl,
            lease,
            deque_ids: PMutex::new(VecDeque::new()),
            tasks: PMutex::new(HashMap::new()),
            notify_claim: Notify::new(),
        })))
    }

    #[inline]
    pub fn profile_id(&self) -> &str {
        self.0.profile_id.as_str()
    }

    /// Returns request_id.
    pub fn enqueue(self: &Arc<Self>, stats: &GlobalStats, mut envelope: BusRequestEnvelope) -> Result<String, &'static str> {
        self.0.prune_all(stats);
        {
            let q = self.0.deque_ids.lock();
            if q.len() >= self.0.cap {
                return Err("task_queue_full");
            }
        }
        let rid = if envelope.request_id.trim().is_empty() {
            format!(
                "tq-{}-{:016x}",
                deploy_auth::now_unix_ms(),
                OsRng.next_u64()
            )
        } else {
            envelope.request_id.trim().to_string()
        };

        let mut hm = self.0.tasks.lock();
        if hm.contains_key(&rid) {
            return Err("duplicate_request_id");
        }
        envelope.request_id.clone_from(&rid);
        envelope.body_finished = true;
        if envelope.created_at_ms == 0 {
            envelope.created_at_ms = deploy_auth::now_unix_ms();
        }
        let done = Arc::new(Notify::new());
        hm.insert(
            rid.clone(),
            TaskEntry {
                profile_id: self.0.profile_id.clone(),
                envelope,
                queued_at: Instant::now(),
                phase: StoredTaskPhase::Queued,
                claimant: None,
                lease_until: None,
                detail: None,
                progress_pct: 0,
                response_status: None,
                response_headers: vec![],
                response_body: vec![],
                terminal_error: None,
                done: done.clone(),
            },
        );
        drop(hm);
        let mut dq = self.0.deque_ids.lock();
        dq.push_back(rid.clone());
        drop(dq);

        stats
            .task_queue_submitted
            .fetch_add(1, AtomicOrdering::Relaxed);
        self.0.notify_claim.notify_waiters();
        Ok(rid)
    }

    pub async fn claim_next(
        self: &Arc<Self>,
        worker_pk: &[u8; 32],
        wait: Duration,
        stats: &GlobalStats,
    ) -> Result<BusRequestEnvelope, tonic::Status> {
        let wait = wait.clamp(Duration::from_millis(50), Duration::from_secs(120));
        let deadline = Instant::now() + wait;
        loop {
            self.0.prune_all(stats);
            let got = self.0.try_claim_one(worker_pk, stats);
            if let Some(env) = got {
                return Ok(env);
            }
            let now = Instant::now();
            if now >= deadline {
                stats
                    .task_queue_claim_timeout
                    .fetch_add(1, AtomicOrdering::Relaxed);
                return Err(tonic::Status::cancelled("pull_timeout"));
            }
            let sleep = Duration::from_millis(250).min(deadline.saturating_duration_since(now));
            let _ = tokio::time::timeout(sleep, self.0.notify_claim.notified()).await;
        }
    }

    pub fn apply_status(
        self: &Arc<Self>,
        worker_pk: &[u8; 32],
        request_id: &str,
        detail: Option<&str>,
        progress_pct: u32,
        stats: &GlobalStats,
    ) -> Result<(), tonic::Status> {
        let rid = request_id.trim().to_string();
        if rid.is_empty() {
            return Err(tonic::Status::invalid_argument("empty request_id"));
        }
        self.0.prune_all(stats);
        let mut hm = self.0.tasks.lock();
        let e = hm.get_mut(&rid).ok_or_else(|| tonic::Status::not_found("unknown task"))?;
        if Some(*worker_pk) != e.claimant {
            return Err(tonic::Status::permission_denied(
                "wrong worker for task status update",
            ));
        }
        if !matches!(
            e.phase,
            StoredTaskPhase::Claimed | StoredTaskPhase::Running
        ) {
            return Err(tonic::Status::failed_precondition("task not claimable/active"));
        }
        if Instant::now() > e.lease_until.unwrap_or(Instant::now()) {
            return Err(tonic::Status::failed_precondition("lease expired"));
        }
        e.phase = StoredTaskPhase::Running;
        e.detail = detail.map(|s| s.to_string());
        e.progress_pct = progress_pct;
        stats
            .task_queue_status_updates
            .fetch_add(1, AtomicOrdering::Relaxed);
        Ok(())
    }

    /// Append response chunks; finalize when [`TaskComplete.finished`] is true.
    pub fn apply_complete(
        self: &Arc<Self>,
        worker_pk: &[u8; 32],
        part: TaskComplete,
        stats: &GlobalStats,
    ) -> Result<(), tonic::Status> {
        let rid = part.request_id.trim().to_string();
        if rid.is_empty() {
            return Err(tonic::Status::invalid_argument("empty request_id"));
        }
        self.0.prune_all(stats);
        let err_trimmed = part.error.trim();
        if part.finished && !err_trimmed.is_empty() {
            let mut hm = self.0.tasks.lock();
            let e = hm
                .get_mut(&rid)
                .ok_or_else(|| tonic::Status::not_found("unknown task"))?;
            if Some(*worker_pk) != e.claimant {
                return Err(tonic::Status::permission_denied(
                    "wrong worker for task complete",
                ));
            }
            e.phase = StoredTaskPhase::Failed;
            e.terminal_error = Some(err_trimmed.to_string());
            e.response_body.extend_from_slice(&part.body_chunk);
            e.done.notify_waiters();
            stats
                .task_queue_failed
                .fetch_add(1, AtomicOrdering::Relaxed);
            return Ok(());
        }

        let mut hm = self.0.tasks.lock();
        let e = hm
            .get_mut(&rid)
            .ok_or_else(|| tonic::Status::not_found("unknown task"))?;
        if Some(*worker_pk) != e.claimant {
            return Err(tonic::Status::permission_denied(
                "wrong worker for task complete",
            ));
        }

        if part.status != 0 {
            e.response_status = Some(part.status);
        }
        if !part.headers.is_empty() {
            e.response_headers = part.headers.clone();
        }
        e.response_body.extend_from_slice(&part.body_chunk);

        if part.finished {
            if e.response_status.is_none() && e.response_body.is_empty() {
                return Err(tonic::Status::invalid_argument(
                    "task complete missing status and body",
                ));
            }
            let code = e.response_status.unwrap_or(200);
            e.response_status = Some(code);
            e.phase = StoredTaskPhase::Completed;
            e.done.notify_waiters();
            stats
                .task_queue_completed
                .fetch_add(1, AtomicOrdering::Relaxed);
        }

        Ok(())
    }

    pub fn snapshot_json(self: &Arc<Self>, id: &str) -> Option<TaskOuterJson> {
        let hm = self.0.tasks.lock();
        hm.get(id).map(|e| Self::entry_to_json(e))
    }

    pub fn snapshot_list_json(
        self: &Arc<Self>,
        phase_filter: Option<StoredTaskPhase>,
        limit: usize,
    ) -> Vec<TaskOuterJson> {
        let hm = self.0.tasks.lock();
        let mut out: Vec<(i64, TaskOuterJson)> = hm
            .values()
            .map(|e| {
                let ord = if e.envelope.created_at_ms > 0 {
                    e.envelope.created_at_ms
                } else {
                    0
                };
                (ord, Self::entry_to_json(e))
            })
            .collect();
        drop(hm);
        if let Some(p) = phase_filter {
            let want = phase_str(p);
            out.retain(|(_, j)| j.phase_str.as_str() == want);
        }
        out.sort_by(|a, b| {
            (b.0)
                .cmp(&a.0)
                .then_with(|| b.1.request_id.cmp(&a.1.request_id))
        });
        out.into_iter()
            .map(|(_, j)| j)
            .take(limit.max(1).min(500))
            .collect()
    }

    pub async fn wait_terminal(
        self: &Arc<Self>,
        id: &str,
        deadline: Instant,
    ) -> Result<TaskOuterJson, tonic::Status> {
        loop {
            if let Some(s) = self.snapshot_json(id) {
                if matches!(
                    s.phase_str.as_str(),
                    "completed" | "failed" | "expired"
                ) {
                    return Ok(s);
                }
            } else {
                return Err(tonic::Status::not_found("unknown task"));
            }
            let done = {
                let hm = self.0.tasks.lock();
                hm.get(id).map(|e| e.done.clone())
            };
            let Some(n) = done else {
                return Err(tonic::Status::not_found("unknown task"));
            };
            let now = Instant::now();
            if now >= deadline {
                return self
                    .snapshot_json(id)
                    .ok_or_else(|| tonic::Status::not_found("unknown task"));
            }
            let sleep = Duration::from_millis(250).min(deadline.saturating_duration_since(now));
            let _ = tokio::time::timeout(sleep, n.notified()).await;
        }
    }

    fn entry_to_json(e: &TaskEntry) -> TaskOuterJson {
        let phase_str_out = phase_str(e.phase).to_string();
        let mut hdr_json = serde_json::Map::new();
        if !e.response_headers.is_empty() && matches!(e.phase, StoredTaskPhase::Completed | StoredTaskPhase::Failed) {
            for kv in &e.response_headers {
                hdr_json.insert(kv.key.clone(), serde_json::Value::String(kv.value.clone()));
            }
        }
        let err_out = match e.phase {
            StoredTaskPhase::Failed => {
                Some(e.terminal_error.clone().unwrap_or_default())
                    .filter(|s| !s.trim().is_empty())
                    .or_else(|| {
                        if !e.response_body.is_empty() {
                            Some(String::from_utf8_lossy(&e.response_body).into_owned())
                        } else {
                            None
                        }
                    })
            }
            StoredTaskPhase::Expired => Some(
                e.terminal_error
                    .clone()
                    .unwrap_or_else(|| "expired".into()),
            ),
            _ => None,
        };

        TaskOuterJson {
            request_id: e.envelope.request_id.clone(),
            profile_id: e.profile_id.clone(),
            phase_str: phase_str_out,
            trace_id: Some(e.envelope.trace_id.clone()).filter(|s| !s.is_empty()),
            method: Some(e.envelope.method.clone()).filter(|s| !s.is_empty()),
            host: Some(e.envelope.host.clone()).filter(|s| !s.is_empty()),
            path: Some(e.envelope.path.clone()).filter(|s| !s.is_empty()),
            detail: e.detail.clone(),
            progress_pct: Some(e.progress_pct).filter(|&p| p > 0),
            http_status: e.response_status,
            response_headers: if hdr_json.is_empty() {
                None
            } else {
                Some(hdr_json)
            },
            body_base64: match e.phase {
                StoredTaskPhase::Completed => Some(base64::engine::general_purpose::STANDARD.encode(
                    &e.response_body,
                )),
                _ => None,
            },
            error: err_out,
        }
    }
}

impl TaskQueueInner {
    fn prune_all(&self, stats: &GlobalStats) {
        self.recycle_expired_leases(stats);
        self.discard_ttl_expired(stats);
    }

    fn discard_ttl_expired(&self, stats: &GlobalStats) {
        let now = Instant::now();
        loop {
            let front_id = { self.deque_ids.lock().front().cloned() };
            let Some(id) = front_id else {
                break;
            };

            let is_stale = {
                let hm = self.tasks.lock();
                hm.get(&id)
                    .map(|e| {
                        e.phase == StoredTaskPhase::Queued && now.duration_since(e.queued_at) > self.ttl
                    })
                    .unwrap_or(false)
            };
            if !is_stale {
                break;
            }

            let popped = self.deque_ids.lock().pop_front();
            if popped.as_ref() != Some(&id) {
                continue;
            }

            if let Some(mut e) = self.tasks.lock().remove(&id) {
                if e.phase == StoredTaskPhase::Queued {
                    e.phase = StoredTaskPhase::Expired;
                    e.terminal_error = Some("queued_ttl_expired".into());
                    e.done.notify_waiters();
                    stats
                        .task_queue_expired
                        .fetch_add(1, AtomicOrdering::Relaxed);
                }
                // Re-insert so GET /tasks/{id} can still see terminal state briefly.
                self.tasks.lock().insert(id, e);
            }
        }
    }

    fn recycle_expired_leases(&self, stats: &GlobalStats) {
        let now = Instant::now();
        let mut requeue: Vec<String> = vec![];
        {
            let mut hm = self.tasks.lock();
            for (id, e) in hm.iter_mut() {
                if matches!(e.phase, StoredTaskPhase::Claimed | StoredTaskPhase::Running) {
                    if let Some(until) = e.lease_until {
                        if now > until {
                            e.phase = StoredTaskPhase::Queued;
                            e.claimant = None;
                            e.lease_until = None;
                            e.detail = Some("lease_expired_requeued".into());
                            requeue.push(id.clone());
                            stats
                                .task_queue_lease_requeue
                                .fetch_add(1, AtomicOrdering::Relaxed);
                        }
                    }
                }
            }
        }
        if requeue.is_empty() {
            return;
        }
        let mut dq = self.deque_ids.lock();
        for id in requeue {
            if !dq.contains(&id) {
                dq.push_front(id);
            }
        }
        self.notify_claim.notify_waiters();
    }

    fn try_claim_one(&self, worker_pk: &[u8; 32], stats: &GlobalStats) -> Option<BusRequestEnvelope> {
        loop {
            let id = self.deque_ids.lock().pop_front()?;
            let claimed = self.attempt_claim_id(&id, worker_pk, stats);
            match claimed {
                ClaimAttempt::Taken(env) => {
                    stats
                        .task_queue_claimed
                        .fetch_add(1, AtomicOrdering::Relaxed);
                    return Some(env);
                }
                ClaimAttempt::Skip => {}
                ClaimAttempt::RequeueFront => {
                    self.deque_ids.lock().push_front(id);
                    continue;
                }
            }
        }
    }

    fn attempt_claim_id(
        &self,
        id: &str,
        worker_pk: &[u8; 32],
        stats: &GlobalStats,
    ) -> ClaimAttempt {
        let now = Instant::now();
        let mut hm = self.tasks.lock();
        let Some(e) = hm.get_mut(id) else {
            return ClaimAttempt::Skip;
        };
        if e.phase != StoredTaskPhase::Queued {
            return ClaimAttempt::RequeueFront;
        }
        if now.duration_since(e.queued_at) > self.ttl {
            e.phase = StoredTaskPhase::Expired;
            e.terminal_error = Some("queued_ttl_expired".into());
            e.done.notify_waiters();
            stats
                .task_queue_expired
                .fetch_add(1, AtomicOrdering::Relaxed);
            return ClaimAttempt::Skip;
        }
        e.phase = StoredTaskPhase::Claimed;
        e.claimant = Some(*worker_pk);
        e.lease_until = Some(now + self.lease);
        ClaimAttempt::Taken(e.envelope.clone())
    }
}

pub fn phase_from_query(s: Option<&str>) -> Option<StoredTaskPhase> {
    let t = s?.trim();
    Some(match t.to_ascii_lowercase().as_str() {
        "queued" => StoredTaskPhase::Queued,
        "claimed" => StoredTaskPhase::Claimed,
        "running" => StoredTaskPhase::Running,
        "completed" => StoredTaskPhase::Completed,
        "failed" => StoredTaskPhase::Failed,
        "expired" => StoredTaskPhase::Expired,
        _ => return None,
    })
}
