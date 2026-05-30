//! HTTP-facing error mapping (no leakage of verifier internals to clients).

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chal_auth_shared::AuthFailureJson;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("configuration error: {0}")]
    Config(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match &self {
            ApiError::BadRequest(reason) => {
                let status = StatusCode::BAD_REQUEST;
                let body = AuthFailureJson {
                    ok: false,
                    reason: reason.clone(),
                };
                (status, Json(body)).into_response()
            }
            ApiError::Unauthorized(reason) => {
                let status = StatusCode::UNAUTHORIZED;
                let body = AuthFailureJson {
                    ok: false,
                    reason: reason.clone(),
                };
                (status, Json(body)).into_response()
            }
            ApiError::Config(msg) => {
                let status = StatusCode::INTERNAL_SERVER_ERROR;
                let body = AuthFailureJson {
                    ok: false,
                    reason: format!("configuration: {}", msg),
                };
                (status, Json(body)).into_response()
            }
        }
    }
}
