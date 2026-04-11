//! Shared deploy client logic (CLI binary + desktop).

pub mod ops;
pub mod upload;

pub use ops::{
    build_chunks, default_version, pack_directory, read_or_pack_bundle,
    validate_version as validate_version_label,
};
pub use upload::{
    deploy_directory, fetch_server_stack_info, upload_artifact, upload_server_stack_artifact,
    upload_server_stack_artifact_with_progress, DeploySummary,
};
