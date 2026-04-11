//! Dashboard control plane: aggregate gRPC status, filesystem releases, PostgreSQL history, nginx file ops.

mod nginx;
mod service;
mod types;

pub use nginx::{apply_nginx_put, read_nginx_config, NginxPutOutcome};
pub use service::{ControlError, ControlPlane};
pub use types::{
    HistoryView, NginxConfigPut, NginxConfigView, NginxPutResponseView, ProcessControlView,
    ProjectView, ProjectsView, ReleasesView, RollbackBody, RollbackView, StatusView,
};
