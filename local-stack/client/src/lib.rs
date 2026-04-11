//! Shared deploy client logic (CLI binary + desktop).

pub mod ops;
pub mod upload;

pub use ops::{
    build_chunks, default_version, pack_directory, validate_version as validate_version_label,
};
pub use upload::{deploy_directory, upload_artifact, DeploySummary};
