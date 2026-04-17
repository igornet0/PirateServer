//! Compare `pirate.toml` required host service ids with `GET /api/v1/host-services`.

use deploy_core::host_service_requirements::required_host_service_ids;
use deploy_core::pirate_project::PirateManifest;
use serde::Deserialize;
use serde::Serialize;

use crate::connection::load_control_api_base;
use crate::control_api::{control_api_fetch_host_services_json, control_api_session_active};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostServicesCompatSummary {
    /// `none` | `skipped` | `checked` | `error`
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    pub required_host_service_ids: Vec<String>,
    pub missing_host_service_ids: Vec<String>,
    pub satisfied_host_service_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch_script_present: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct HostServiceRowDto {
    id: String,
    #[serde(default)]
    installed: bool,
}

#[derive(Debug, Deserialize)]
struct HostServicesViewDto {
    services: Vec<HostServiceRowDto>,
    #[serde(default)]
    dispatch_script_present: bool,
}

/// Parse host-services JSON and compute missing ids.
pub fn host_services_gap(
    required: &[String],
    json: &str,
) -> Result<(Vec<String>, Vec<String>, bool), String> {
    let v: HostServicesViewDto =
        serde_json::from_str(json).map_err(|e| format!("host-services JSON: {e}"))?;
    let mut missing = Vec::<String>::new();
    let mut satisfied = Vec::<String>::new();
    for id in required {
        let found = v
            .services
            .iter()
            .find(|r| r.id == *id)
            .map(|r| r.installed)
            .unwrap_or(false);
        if found {
            satisfied.push(id.clone());
        } else {
            missing.push(id.clone());
        }
    }
    Ok((missing, satisfied, v.dispatch_script_present))
}

/// Full summary for UI / preflight (control-api optional).
pub fn summarize_host_services_for_manifest(manifest: &PirateManifest) -> HostServicesCompatSummary {
    let required = required_host_service_ids(manifest);
    if required.is_empty() {
        return HostServicesCompatSummary {
            status: "none".into(),
            skip_reason: None,
            required_host_service_ids: required,
            missing_host_service_ids: vec![],
            satisfied_host_service_ids: vec![],
            dispatch_script_present: None,
        };
    }

    let skipped_template =
        |reason: &'static str| HostServicesCompatSummary {
            status: "skipped".into(),
            skip_reason: Some(reason.into()),
            required_host_service_ids: required.clone(),
            missing_host_service_ids: vec![],
            satisfied_host_service_ids: vec![],
            dispatch_script_present: None,
        };

    let base = load_control_api_base();
    if base.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
        return skipped_template(
            "control-api base URL is not set (configure in server bookmark, Connection tab).",
        );
    }

    if !control_api_session_active() {
        return skipped_template(
            "Not signed in to control-api; cannot verify installed packages on the host.",
        );
    }

    let json = match control_api_fetch_host_services_json() {
        Ok(j) => j,
        Err(e) => {
            return HostServicesCompatSummary {
                status: "error".into(),
                skip_reason: Some(e),
                required_host_service_ids: required.clone(),
                missing_host_service_ids: required.clone(),
                satisfied_host_service_ids: vec![],
                dispatch_script_present: None,
            };
        }
    };

    match host_services_gap(&required, &json) {
        Ok((missing, satisfied, dispatch)) => HostServicesCompatSummary {
            status: "checked".into(),
            skip_reason: None,
            required_host_service_ids: required,
            missing_host_service_ids: missing.clone(),
            satisfied_host_service_ids: satisfied,
            dispatch_script_present: Some(dispatch),
        },
        Err(e) => HostServicesCompatSummary {
            status: "error".into(),
            skip_reason: Some(e),
            required_host_service_ids: required.clone(),
            missing_host_service_ids: required.clone(),
            satisfied_host_service_ids: vec![],
            dispatch_script_present: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gap_finds_missing() {
        // API JSON matches deploy-control `Serialize` (snake_case fields).
        let json = r#"{"services":[{"id":"redis","installed":true},{"id":"postgresql","installed":false}],"dispatch_script_present":true}"#;
        let req = vec!["postgresql".into(), "redis".into()];
        let (m, s, d) = host_services_gap(&req, json).unwrap();
        assert_eq!(m, vec!["postgresql"]);
        assert_eq!(s, vec!["redis"]);
        assert!(d);
    }
}
