use deploy_db::DeployEventRow;
use serde::{Deserialize, Serialize};

/// Deploy status exposed to the HTTP API (and dashboard).
/// One deployable unit on the host. Today only a single implicit `default` project (see ROADMAP multi-project).
#[derive(Debug, Clone, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub deploy_root: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectsView {
    pub projects: Vec<ProjectView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusView {
    pub current_version: String,
    pub state: String,
    /// `grpc` when live gRPC succeeded; `database` when falling back to snapshot.
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleasesView {
    pub releases: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryView {
    pub events: Vec<DeployEventRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NginxConfigView {
    pub path: String,
    pub content: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct NginxConfigPut {
    pub content: String,
}

/// Result of gRPC `Rollback`.
#[derive(Debug, Clone, Serialize)]
pub struct RollbackView {
    pub status: String,
    pub active_version: String,
}

/// Result of `StopProcess` / `RestartProcess` (same shape as deploy status).
#[derive(Debug, Clone, Serialize)]
pub struct ProcessControlView {
    pub current_version: String,
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct RollbackBody {
    pub version: String,
    /// Empty or omitted means `default`.
    #[serde(default)]
    pub project_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NginxPutResponseView {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reload_output: Option<String>,
}
