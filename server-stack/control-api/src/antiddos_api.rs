//! `/api/v1/antiddos` — host JSON + sudo apply script.

use crate::error::ApiError;
use crate::{check_api_bearer, ApiState};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use deploy_control::{
    apply_antiddos_via_sudo, collect_antiddos_stats, read_host_json, validate_host_config,
    validate_project_config, write_host_json, write_project_json, AntiddosApplyResultView,
    AntiddosHostConfig, AntiddosProjectConfig, AntiddosStatsView,
};
use std::path::PathBuf;

fn safe_project_file_id(id: &str) -> Result<String, ApiError> {
    let t = id.trim();
    if t.is_empty() || t.contains('/') || t.contains("..") || t.contains('\\') {
        return Err(ApiError::bad_request("invalid project id"));
    }
    if t.len() > 128 {
        return Err(ApiError::bad_request("project id too long"));
    }
    Ok(t.to_string())
}

#[derive(serde::Serialize)]
pub struct AntiddosGetResponse {
    pub config: AntiddosHostConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_apply: Option<AntiddosApplyResultView>,
}

fn host_json_path(state: &ApiState) -> PathBuf {
    state.antiddos_state_dir.join("host.json")
}

fn project_json_path(state: &ApiState, id: &str) -> PathBuf {
    state.antiddos_state_dir.join("projects").join(format!("{id}.json"))
}

fn last_apply_path(state: &ApiState) -> PathBuf {
    state.antiddos_state_dir.join(".last_apply.json")
}

async fn save_last_apply(state: &ApiState, res: &AntiddosApplyResultView) {
    let p = last_apply_path(state);
    if let Ok(raw) = serde_json::to_string_pretty(res) {
        let _ = tokio::fs::write(&p, raw).await;
    }
}

pub async fn api_antiddos_get(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<AntiddosGetResponse>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let path = host_json_path(&s);
    let config = read_host_json(&path)?;
    let last_apply = if last_apply_path(&s).is_file() {
        let raw = tokio::fs::read_to_string(last_apply_path(&s))
            .await
            .ok()
            .unwrap_or_default();
        serde_json::from_str(&raw).ok()
    } else {
        None
    };
    Ok(Json(AntiddosGetResponse { config, last_apply }))
}

pub async fn api_antiddos_put(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<AntiddosHostConfig>,
) -> Result<Json<AntiddosApplyResultView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    validate_host_config(&body).map_err(ApiError::bad_request)?;
    let path = host_json_path(&s);
    write_host_json(&path, &body)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let res = apply_antiddos_via_sudo(
        &s.antiddos_apply_script,
        &s.antiddos_state_dir,
        &s.nginx_site_path,
    )?;
    save_last_apply(&s, &res).await;
    Ok(Json(res))
}

pub async fn api_antiddos_enable(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<AntiddosApplyResultView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let path = host_json_path(&s);
    let mut cfg = read_host_json(&path)?;
    cfg.enabled = true;
    write_host_json(&path, &cfg)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let res = apply_antiddos_via_sudo(
        &s.antiddos_apply_script,
        &s.antiddos_state_dir,
        &s.nginx_site_path,
    )?;
    save_last_apply(&s, &res).await;
    Ok(Json(res))
}

pub async fn api_antiddos_disable(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<AntiddosApplyResultView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let path = host_json_path(&s);
    let mut cfg = read_host_json(&path)?;
    cfg.enabled = false;
    write_host_json(&path, &cfg)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let res = apply_antiddos_via_sudo(
        &s.antiddos_apply_script,
        &s.antiddos_state_dir,
        &s.nginx_site_path,
    )?;
    save_last_apply(&s, &res).await;
    Ok(Json(res))
}

pub async fn api_antiddos_apply(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<AntiddosApplyResultView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let res = apply_antiddos_via_sudo(
        &s.antiddos_apply_script,
        &s.antiddos_state_dir,
        &s.nginx_site_path,
    )?;
    save_last_apply(&s, &res).await;
    Ok(Json(res))
}

pub async fn api_antiddos_project_put(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(mut body): Json<AntiddosProjectConfig>,
) -> Result<Json<AntiddosApplyResultView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let pid = safe_project_file_id(&project_id)?;
    body.project_id = pid.clone();
    validate_project_config(&body).map_err(ApiError::bad_request)?;
    let path = project_json_path(&s, &pid);
    write_project_json(&path, &body)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let res = apply_antiddos_via_sudo(
        &s.antiddos_apply_script,
        &s.antiddos_state_dir,
        &s.nginx_site_path,
    )?;
    save_last_apply(&s, &res).await;
    Ok(Json(res))
}

pub async fn api_antiddos_project_delete(
    State(s): State<ApiState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<AntiddosApplyResultView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    let pid = safe_project_file_id(&project_id)?;
    let path = project_json_path(&s, &pid);
    let _ = tokio::fs::remove_file(&path).await;
    let res = apply_antiddos_via_sudo(
        &s.antiddos_apply_script,
        &s.antiddos_state_dir,
        &s.nginx_site_path,
    )?;
    save_last_apply(&s, &res).await;
    Ok(Json(res))
}

pub async fn api_antiddos_stats(
    State(s): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<AntiddosStatsView>, ApiError> {
    check_api_bearer(&s, &headers)?;
    Ok(Json(collect_antiddos_stats(&s.antiddos_limit_log_path)))
}
