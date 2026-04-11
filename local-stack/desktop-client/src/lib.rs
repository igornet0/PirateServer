//! Pirate Client desktop library (Tauri backend helpers).

pub mod bookmarks;
pub mod connection;
pub mod deploy;
pub mod hosts;
pub mod status;

pub use bookmarks::{load_bookmarks, remove_bookmark, upsert_bookmark, ServerBookmark};
pub use connection::{
    clear_endpoint, connect_from_bundle, load_endpoint, load_project_id,
    parse_grpc_endpoint_from_bundle, save_endpoint, set_active_project, verify_grpc_endpoint,
    GrpcConnectResult,
};
pub use deploy::{DeployOutcome, RollbackOutcome};
pub use status::{app_status, AppStatus};
