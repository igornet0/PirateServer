//! Unified JSON error body for `/api/v1/*` responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use deploy_control::ControlError;
use deploy_db::DbError;
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorPayload,
}

#[derive(Serialize)]
pub struct ApiErrorPayload {
    pub code: String,
    pub message: String,
    /// `DEPLOY_MAX_UPLOAD_BYTES` in control-api (when this error is about artifact size).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configured_limit: Option<u64>,
    /// Last `max_upload_bytes` from deploy-server `GetStatus` (when known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_limit: Option<u64>,
    /// `min(configured_limit, grpc_limit)` (or configured only when gRPC unknown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_limit: Option<u64>,
}

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    configured_limit: Option<u64>,
    grpc_limit: Option<u64>,
    effective_limit: Option<u64>,
}

impl ApiError {
    pub fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "bad_gateway",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
        }
    }

    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "service_unavailable",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
        }
    }

    /// Bad request with artifact upload limit context (JSON includes `configured_limit` / `grpc_limit` / `effective_limit`).
    pub fn bad_request_artifact_limit(
        message: impl Into<String>,
        configured_limit: u64,
        grpc_limit: Option<u64>,
        effective_limit: u64,
    ) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
            configured_limit: Some(configured_limit),
            grpc_limit,
            effective_limit: Some(effective_limit),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
        }
    }
}

impl From<ControlError> for ApiError {
    fn from(e: ControlError) -> Self {
        match e {
            ControlError::Grpc(msg) => ApiError::bad_gateway(msg),
            ControlError::HostDeployEnv(msg) => {
                if msg.contains("exceeds") || msg.contains("NUL bytes") {
                    ApiError::bad_request(msg)
                } else {
                    ApiError::bad_gateway(msg)
                }
            }
            ControlError::NginxOp(msg) => {
                if msg.contains("mode must be") || msg.contains("exceeds") || msg.contains("NUL") {
                    ApiError::bad_request(msg)
                } else {
                    ApiError::bad_gateway(msg)
                }
            }
            ControlError::HostServiceOp(msg) => {
                if msg.contains("unknown service")
                    || msg.contains("must be install")
                    || msg.contains("cannot be installed")
                    || msg.contains("dispatcher not found")
                {
                    ApiError::bad_request(msg)
                } else {
                    ApiError::bad_gateway(msg)
                }
            }
            ControlError::Antiddos(msg) => {
                if msg.contains("invalid") || msg.contains("must be") || msg.contains("out of range") {
                    ApiError::bad_request(msg)
                } else {
                    ApiError::bad_gateway(msg)
                }
            }
            ControlError::Io(err) => ApiError::internal(err.to_string()),
            ControlError::Db(err) => match err {
                DbError::InvalidIdentifier(msg) => ApiError::bad_request(msg),
                other => ApiError::internal(other.to_string()),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            error: ApiErrorPayload {
                code: self.code.to_string(),
                message: self.message,
                configured_limit: self.configured_limit,
                grpc_limit: self.grpc_limit,
                effective_limit: self.effective_limit,
            },
        };
        (self.status, Json(body)).into_response()
    }
}
