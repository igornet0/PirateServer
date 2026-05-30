//! TCP accept loops, queues, connectors, peer registry.

use crate::queue::StreamQueue;
use crate::store::ConfigStore;
use crate::task_queue::TaskQueue;
use crate::types::{GlobalStats, PersistRoot, TunnelMode, TunnelProfile, TunnelRole};
use deploy_auth::{load_authorized_peers, AuthConfig, NonceTracker};
use ed25519_dalek::{SigningKey, VerifyingKey};
use parking_lot::Mutex as PMutex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

pub const META_TUN_PROFILE: &str = "x-stack-tun-profile";

pub struct SharedState {
    pub store: ConfigStore,
    pub peers_path: PathBuf,
    pub peers: RwLock<HashSet<[u8; 32]>>,
    pub auth_cfg: AuthConfig,
    pub nonce: Arc<NonceTracker>,
    pub stats: Arc<GlobalStats>,
    pub queues: Arc<PMutex<HashMap<String, Arc<StreamQueue>>>>,
    pub task_queues: Arc<PMutex<HashMap<String, Arc<TaskQueue>>>>,
    pub signing_key: SigningKey,
    pub rest_bearer_token: Option<String>,
    pub audit: Arc<PMutex<std::collections::VecDeque<crate::types::AuditEntry>>>,
    pub request_journal: Arc<PMutex<std::collections::VecDeque<crate::types::RequestBusJournalEntry>>>,
    listener_tasks: Arc<PMutex<HashMap<String, JoinHandle<()>>>>,
    connector_tasks: Arc<PMutex<HashMap<String, JoinHandle<()>>>>,
}

fn addr_for_listen(p: &TunnelProfile) -> Result<std::net::SocketAddr, String> {
    let raw = p
        .listen_addr
        .as_ref()
        .ok_or_else(|| "listen_addr missing".to_string())?;
    raw.trim()
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())
}

impl SharedState {
    pub async fn bootstrap(
        state_dir: PathBuf,
        peers_path_override: Option<PathBuf>,
        rest_bearer_token: Option<String>,
        identity_path_override: Option<PathBuf>,
        allow_unauthenticated: bool,
    ) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(&state_dir).map_err(|e| e.to_string())?;
        let store_path = state_dir.join("profiles.json");
        let peers_path = peers_path_override.unwrap_or_else(|| state_dir.join("authorized_peers.json"));
        let id_path = identity_path_override.unwrap_or_else(|| state_dir.join("identity.json"));
        let sk = deploy_auth::load_or_create_identity(&id_path).map_err(|e| e.to_string())?;
        let peers_set = load_authorized_peers(&peers_path).map_err(|e| e.to_string())?;
        let store = ConfigStore::load(store_path).await?;

        let st = Arc::new(Self {
            peers_path,
            peers: RwLock::new(peers_set),
            auth_cfg: AuthConfig {
                allow_unauthenticated,
                ..Default::default()
            },
            nonce: Arc::new(NonceTracker::default()),
            stats: Arc::new(GlobalStats::default()),
            queues: Arc::new(PMutex::new(HashMap::new())),
            task_queues: Arc::new(PMutex::new(HashMap::new())),
            store,
            rest_bearer_token,
            signing_key: sk,
            audit: Arc::new(PMutex::new(std::collections::VecDeque::with_capacity(256))),
            request_journal: Arc::new(PMutex::new(std::collections::VecDeque::with_capacity(256))),
            listener_tasks: Arc::new(PMutex::new(HashMap::new())),
            connector_tasks: Arc::new(PMutex::new(HashMap::new())),
        });

        Self::reload_peers_disk(&st).await?;
        st.resync_all().await;
        Ok(st)
    }

    pub async fn reload_peers_disk(self: &Arc<Self>) -> Result<(), String> {
        let set = load_authorized_peers(&self.peers_path).map_err(|e| e.to_string())?;
        let mut g = self.peers.write().await;
        *g = set;
        Ok(())
    }

    fn upsert_listener_queue(self: &Arc<Self>, p: &TunnelProfile) -> Arc<StreamQueue> {
        let ttl = Duration::from_secs(p.stream_offer_ttl_secs.max(1));
        let cap = p.max_pending_streams.max(1);
        let mut mq = self.queues.lock();
        if let Some(q) = mq.get(&p.id) {
            return q.clone();
        }
        let q = StreamQueue::new(cap, ttl);
        mq.insert(p.id.clone(), q.clone());
        q
    }

    fn upsert_listener_task_queue(self: &Arc<Self>, p: &TunnelProfile) -> Arc<TaskQueue> {
        let cap = p
            .max_pending_tasks
            .unwrap_or(p.max_pending_streams)
            .max(1);
        let ttl = Duration::from_secs(
            p.task_queue_ttl_secs
                .unwrap_or(p.stream_offer_ttl_secs)
                .max(1),
        );
        let lease =
            Duration::from_secs((p.task_claim_lease_secs.unwrap_or(300)).max(30));
        let mut mq = self.task_queues.lock();
        if let Some(q) = mq.get(&p.id) {
            return Arc::clone(q);
        }
        let q = TaskQueue::new(p.id.trim(), cap, ttl, lease);
        mq.insert(p.id.clone(), Arc::clone(&q));
        q
    }

    pub async fn resync_all(self: &Arc<Self>) {
        let root = self.store.read_root().await;
        self.sync_listen(&root).await;
        self.sync_connector(&root).await;
    }

    async fn sync_listen(self: &Arc<Self>, root: &PersistRoot) {
        let mut desired: HashSet<String> = HashSet::new();
        for p in &root.profiles {
            if p.enabled && matches!(p.role, TunnelRole::Listen) && p.mode == TunnelMode::TcpRelay {
                desired.insert(p.id.clone());
                self.upsert_listener_queue(p);
            }
        }
        let mut lm = self.listener_tasks.lock();
        lm.retain(|id, handle| {
            if desired.contains(id) {
                return true;
            }
            handle.abort();
            false
        });
        drop(lm);

        let mut desired_tq: HashSet<String> = HashSet::new();
        for p in &root.profiles {
            if p.enabled
                && matches!(p.role, TunnelRole::Listen)
                && p.mode == TunnelMode::RequestBus
            {
                desired_tq.insert(p.id.clone());
                self.upsert_listener_task_queue(p);
            }
        }
        let mut tm = self.task_queues.lock();
        tm.retain(|id, _| desired_tq.contains(id));
        drop(tm);

        let mut lm = self.listener_tasks.lock();
        for p in &root.profiles {
            if !(p.enabled && matches!(p.role, TunnelRole::Listen)) {
                continue;
            }
            if p.mode != TunnelMode::TcpRelay {
                continue;
            }
            if lm.contains_key(&p.id) {
                continue;
            }
            match addr_for_listen(p) {
                Ok(addr) => {
                    let st = Arc::clone(self);
                    let prof = p.clone();
                    let handle =
                        tokio::spawn(async move { listener_acceptor_loop(st, prof, addr).await });
                    lm.insert(p.id.clone(), handle);
                }
                Err(e) => {
                    tracing::error!("listener {} bind parse error: {e}", p.id);
                }
            }
        }
    }

    async fn sync_connector(self: &Arc<Self>, root: &PersistRoot) {
        let mut desired: HashSet<String> = HashSet::new();
        for p in &root.profiles {
            if p.enabled && matches!(p.role, TunnelRole::Connector) {
                desired.insert(p.id.clone());
            }
        }
        let mut cm = self.connector_tasks.lock();
        cm.retain(|id, handle| {
            if desired.contains(id) {
                return true;
            }
            handle.abort();
            false
        });
        drop(cm);

        let mut cm = self.connector_tasks.lock();
        for p in &root.profiles {
            if !(p.enabled && matches!(p.role, TunnelRole::Connector)) {
                continue;
            }
            if cm.contains_key(&p.id) {
                continue;
            }
            let st = Arc::clone(self);
            let prof = p.clone();
            let handle = tokio::spawn(async move { connector_loop_forever(st, prof).await });
            cm.insert(p.id.clone(), handle);
        }
    }

    pub fn connector_allowed(&self, prof: &TunnelProfile, pubkey: &[u8; 32]) -> bool {
        if prof.connector_allow_pubkey_b64.is_empty() {
            return true;
        }
        for b64 in &prof.connector_allow_pubkey_b64 {
            if let Ok(vk) = deploy_auth::parse_verifying_key_b64(b64) {
                if vk.as_bytes() == pubkey {
                    return true;
                }
            }
        }
        false
    }

    pub async fn grpc_verify_tunnel_meta(
        &self,
        meta: &tonic::metadata::MetadataMap,
        listen_profile_id: &str,
    ) -> Result<VerifyingKey, tonic::Status> {
        self.grpc_verify_signed_stack_meta(meta, listen_profile_id, "StackTunTunnel")
            .await
    }

    pub async fn grpc_verify_request_bus_meta(
        &self,
        meta: &tonic::metadata::MetadataMap,
        listen_profile_id: &str,
    ) -> Result<VerifyingKey, tonic::Status> {
        self.grpc_verify_signed_stack_meta(meta, listen_profile_id, "StackTunRequestBus")
            .await
    }

    pub async fn grpc_verify_task_queue_meta(
        &self,
        meta: &tonic::metadata::MetadataMap,
        listen_profile_id: &str,
    ) -> Result<VerifyingKey, tonic::Status> {
        self.grpc_verify_signed_stack_meta(meta, listen_profile_id, "StackTunTaskQueue")
            .await
    }

    async fn grpc_verify_signed_stack_meta(
        &self,
        meta: &tonic::metadata::MetadataMap,
        listen_profile_id: &str,
        signing_method: &str,
    ) -> Result<VerifyingKey, tonic::Status> {
        let profile_hdr_raw = tonic::metadata::MetadataKey::from_static("x-stack-tun-profile");
        let hdr = meta
            .get(&profile_hdr_raw)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| tonic::Status::permission_denied("missing x-stack-tun-profile"))?;
        if hdr.trim() != listen_profile_id.trim() {
            return Err(tonic::Status::permission_denied(
                "profile metadata mismatch canonical payload",
            ));
        }

        deploy_auth::verify_rpc_metadata(
            meta,
            &*self.peers.read().await,
            signing_method,
            listen_profile_id,
            &self.auth_cfg,
            &self.nonce,
        )
        .map_err(|e| tonic::Status::permission_denied(e.to_string()))?;

        let pk = meta
            .get(tonic::metadata::MetadataKey::from_static("x-deploy-pubkey"))
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| tonic::Status::permission_denied("missing x-deploy-pubkey"))?;
        deploy_auth::parse_verifying_key_b64(pk)
            .map_err(|e| tonic::Status::permission_denied(format!("invalid pubkey {e}")))
    }

    pub async fn listener_profile_clone(
        &self,
        id: &str,
    ) -> Result<TunnelProfile, tonic::Status> {
        let root = self.store.read_root().await;
        let hit = root
            .profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| tonic::Status::not_found("tunnel profile"))?
            .clone();
        if !hit.enabled || !matches!(hit.role, TunnelRole::Listen) {
            return Err(tonic::Status::failed_precondition("not an enabled listener"));
        }
        Ok(hit)
    }
}

pub fn insert_profile_meta<T>(
    req: &mut tonic::Request<T>,
    listen_profile_id: &str,
) -> Result<(), deploy_auth::AuthError> {
    let k = tonic::metadata::MetadataKey::from_static("x-stack-tun-profile");
    let v =
        tonic::metadata::AsciiMetadataValue::try_from(listen_profile_id.trim()).map_err(|_| {
            deploy_auth::AuthError::InvalidMetadata("x-stack-tun-profile value".into())
        })?;
    req.metadata_mut().insert(k, v);
    Ok(())
}

pub fn signing_attach_task_queue<T>(
    req: &mut tonic::Request<T>,
    sk: &SigningKey,
    listener_profile_id: &str,
) -> Result<(), deploy_auth::AuthError> {
    deploy_auth::attach_auth_metadata(
        req,
        sk,
        "StackTunTaskQueue",
        "default",
        listener_profile_id,
    )?;
    insert_profile_meta(req, listener_profile_id)?;
    Ok(())
}

pub fn signing_attach_tunnel<T>(
    req: &mut tonic::Request<T>,
    sk: &SigningKey,
    listener_profile_id: &str,
) -> Result<(), deploy_auth::AuthError> {
    deploy_auth::attach_auth_metadata(
        req,
        sk,
        "StackTunTunnel",
        "default",
        listener_profile_id,
    )?;
    insert_profile_meta(req, listener_profile_id)?;
    Ok(())
}

async fn listener_acceptor_loop(st: Arc<SharedState>, profile: TunnelProfile, addr: std::net::SocketAddr) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(
                profile = %profile.id,
                listen = %addr,
                error = %e,
                "TcpListener bind failed; sleeping"
            );
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    };
    tracing::info!(profile = %profile.id, listen = %addr, "tunnel listener accepting");
    loop {
        match listener.accept().await {
            Ok((sock, peer)) => {
                let root_now = st.store.read_root().await;
                let p_now = root_now.profiles.iter().find(|p| p.id == profile.id).cloned();
                let Some(active) =
                    p_now.filter(|pp| pp.enabled && matches!(pp.role, TunnelRole::Listen))
                else {
                    drop(sock);
                    continue;
                };
                let queue = {
                    let qm = st.queues.lock();
                    qm.get(&active.id).cloned()
                };
                let Some(queue) = queue else {
                    drop(sock);
                    continue;
                };
                tracing::trace!(profile = %active.id, peer = ?peer, "accepted public tcp");
                if !queue.try_enqueue(&st.stats, sock) {
                    tracing::warn!(profile = %active.id, peer = ?peer, "queue full dropped");
                }
            }
            Err(e) => {
                tracing::error!(profile = %profile.id, error = %e, "accept loop error");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn connector_loop_forever(st: Arc<SharedState>, mut profile: TunnelProfile) {
    loop {
        let refreshed = st.store.read_root().await;
        if let Some(hit) = refreshed.profiles.iter().find(|p| p.id == profile.id).cloned() {
            profile = hit;
        }
        if !(profile.enabled && matches!(profile.role, TunnelRole::Connector)) {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        if profile.mode != TunnelMode::TcpRelay {
            tracing::trace!(
                profile_id = %profile.id,
                "connector profile is requestBus/task-queue; TunnelStream daemon connector skipped",
            );
            tokio::time::sleep(Duration::from_secs(4)).await;
            continue;
        }
        let remote = match profile.remote_grpc_endpoint.clone() {
            Some(r) if !r.trim().is_empty() => r.trim().to_string(),
            _ => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let listen_prof_id = profile
            .listen_profile_id
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();

        match crate::relay::connector_run_session(&st, &st.signing_key, remote, &profile, &listen_prof_id).await {
            Ok(()) => {}
            Err(e) => {
                st.stats
                    .relay_errors
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tracing::warn!(profile_id = %profile.id, error = %e, "connector relay session ended");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}
