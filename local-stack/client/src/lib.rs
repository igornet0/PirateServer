//! Shared deploy client logic (CLI binary + desktop).

pub mod board;
pub mod proxy_trace;
pub mod bypass;
pub mod bundle_inspect;
pub mod bootstrap_apply;
pub mod config;
pub mod connection_manager;
pub mod default_rules;
pub mod grpc_transport;
pub mod quic;
pub mod local_uninstall;
pub mod metrics_collector;
pub mod ops;
pub mod routing;
pub mod routing_rules;
pub mod settings;
pub mod tls_profile;
pub mod upload;
pub mod proxy_test;
pub mod project_control;
pub mod local_pipeline;
pub mod project_registry;
pub mod network_access;

/// Stable API for the desktop shell: local HTTP CONNECT proxy (`board`) and settings.
pub mod internet_proxy {
    pub use crate::board::run_board;
    pub use crate::proxy_trace::{
        compact_grpc_endpoint_for_log, trace_log, ProxyTraceBuffer, ProxyTraceEntry,
    };
    pub use crate::connection_manager::ConnectionManager;
    pub use crate::default_rules::{
        compile_default_rules, parse_rule_bundle_json, read_rule_bundle_file, serialize_rule_bundle_json,
        validate_default_rules_json, CompiledDefaultRules, DefaultRulesPaths, RuleBundleEdit,
    };
    pub use crate::routing_rules::{tunnel_decision, TunnelDecision};
    pub use crate::settings::{
        default_settings_path, global_settings, init_global_settings, load_settings_from_path,
        BoardConfig, GlobalSettings, SettingsFile, SettingsSnapshot,
    };
}

pub use bundle_inspect::{inspect_bundle_path, inspect_bundle_tar_gz, BundleProfile};
pub use ops::{
    build_chunks, build_chunks_with_manifest, build_server_stack_chunks, default_version,
    pack_directory, pack_release_sources, read_or_pack_bundle, validate_version as validate_version_label,
};
pub use upload::{
    deploy_directory, fetch_server_stack_info, set_artifact_upload_cancel,
    set_server_stack_upload_cancel, upload_artifact, upload_artifact_with_manifest,
    upload_server_stack_artifact, upload_server_stack_artifact_with_progress, DeploySummary,
};
pub use project_control::{init_project, scan_project, ScanReport};
pub use network_access::{
    apply_detected_services_to_manifest, detect_services, generate_proxy_config, validate_deploy,
    DeployValidationReport, DetectedService, ServiceDetectionReport,
};
pub use local_pipeline::{
    apply_generated_files, ensure_dockerfile, run_build, run_test, test_local_docker, StepResult,
};
pub use project_registry::{
    list_projects, register, register_from_pirate_toml_dir, remove as remove_project_registry,
    resolve_path,
};
