//! `pirate --version` and optional server stack info via `GetServerStackInfo`.

use crate::config::{load_or_create_identity, normalize_endpoint, use_signed_requests};
use crate::Cli;
use deploy_client::fetch_server_stack_info;
use serde_json::Value;

pub fn local_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Remote info only when `--endpoint` / `--url` is set explicitly.
fn explicit_endpoint(cli: &Cli) -> Option<String> {
    cli.endpoint
        .as_ref()
        .map(|s| normalize_endpoint(s))
        .filter(|s| !s.is_empty())
}

pub async fn run_version(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    println!("client={}", local_version());
    if let Some(ep) = explicit_endpoint(cli) {
        if !ep.starts_with("http://") && !ep.starts_with("https://") {
            return Err("endpoint must start with http:// or https://".into());
        }
        print_remote_versions(&ep).await?;
    }
    Ok(())
}

async fn print_remote_versions(endpoint: &str) -> Result<(), Box<dyn std::error::Error>> {
    let sk = if use_signed_requests(endpoint) {
        Some(load_or_create_identity()?)
    } else {
        None
    };
    let info = fetch_server_stack_info(endpoint, sk.as_ref())
        .await
        .map_err(|e| format!("GetServerStackInfo: {e}"))?;

    if let Some(ref v) = info.deploy_server_binary_version {
        println!("deploy_server_binary={v}");
    }
    if !info.bundle_version.is_empty() {
        println!("bundle={}", info.bundle_version);
    }

    let Some(ref raw) = info.manifest_json else {
        eprintln!("note: no server-stack-manifest.json on server; control_api and dashboard_ui unknown");
        return Ok(());
    };

    let val: Value = serde_json::from_str(raw).map_err(|e| format!("manifest JSON: {e}"))?;
    if let Some(s) = val.get("release").and_then(|v| v.as_str()) {
        println!("release={s}");
    }
    if let Some(s) = val.get("deploy_server").and_then(|v| v.as_str()) {
        println!("deploy_server_manifest={s}");
    }
    if let Some(s) = val.get("control_api").and_then(|v| v.as_str()) {
        println!("control_api={s}");
    }

    match val.get("dashboard_ui_bundled").and_then(|v| v.as_bool()) {
        Some(false) => println!("dashboard_ui=(not bundled)"),
        Some(true) => {
            if let Some(s) = val.get("dashboard_ui").and_then(|v| v.as_str()) {
                println!("dashboard_ui={s}");
            }
        }
        None => {
            if let Some(s) = val.get("dashboard_ui").and_then(|v| v.as_str()) {
                println!("dashboard_ui={s}");
            }
        }
    }

    Ok(())
}
