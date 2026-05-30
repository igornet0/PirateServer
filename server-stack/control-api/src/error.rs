//! Unified JSON error body for `/api/v1/*` responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use deploy_control::{ControlError, PirateStorageError, StorageBindError};
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
    /// Storage-relative path in conflict (archive extract `abort` mode).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_path: Option<String>,
}

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    configured_limit: Option<u64>,
    grpc_limit: Option<u64>,
    effective_limit: Option<u64>,
    conflict_path: Option<String>,
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
            conflict_path: None,
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
            conflict_path: None,
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
            conflict_path: None,
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
            conflict_path: None,
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
            conflict_path: None,
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
            conflict_path: None,
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
            conflict_path: None,
        }
    }

    /// Feature disabled (e.g. `CONTROL_API_HOST_DATABASES=0`) or not allowed.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
            conflict_path: None,
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
            conflict_path: None,
        }
    }

    pub fn precondition_failed(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PRECONDITION_FAILED,
            code: "precondition_failed",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
            conflict_path: None,
        }
    }

    pub fn elevation_required(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "elevation_required",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
            conflict_path: None,
        }
    }

    pub fn elevation_failed(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "elevation_failed",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
            conflict_path: None,
        }
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "not_implemented",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
            conflict_path: None,
        }
    }

    /// HTTP 507 — storage quota (Pirate file manager).
    pub fn storage_quota_exceeded(message: impl Into<String>, max_bytes: u64, would_be_used: u64) -> Self {
        Self {
            status: StatusCode::INSUFFICIENT_STORAGE,
            code: "storage_quota",
            message: message.into(),
            configured_limit: Some(max_bytes),
            grpc_limit: None,
            effective_limit: Some(would_be_used),
            conflict_path: None,
        }
    }

    /// Extract would overwrite; retry with `overwrite` or `delete_and_overwrite`.
    pub fn extract_conflict(path: String, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "extract_conflict",
            message: message.into(),
            configured_limit: None,
            grpc_limit: None,
            effective_limit: None,
            conflict_path: Some(path),
        }
    }
}

impl From<PirateStorageError> for ApiError {
    fn from(e: PirateStorageError) -> Self {
        match e {
            PirateStorageError::NotFound(msg) => ApiError::not_found(msg),
            PirateStorageError::AlreadyExists(msg) => ApiError::conflict(msg),
            PirateStorageError::ExtractConflict { path } => {
                ApiError::extract_conflict(
                    path,
                    "extraction path conflict: choose overwrite or delete_and_overwrite",
                )
            }
            PirateStorageError::UnsupportedArchive(msg) => {
                ApiError::bad_request(format!("unsupported archive: {msg}"))
            }
            PirateStorageError::CorruptArchive(msg) => ApiError::bad_request(format!("corrupt archive: {msg}")),
            PirateStorageError::Quota { used, max, .. } => {
                ApiError::storage_quota_exceeded(
                    format!("storage quota: would be {used} bytes; max {max}"),
                    max,
                    used,
                )
            }
            PirateStorageError::InvalidPath(msg) | PirateStorageError::NameTooLong(msg) => {
                ApiError::bad_request(msg)
            }
            PirateStorageError::NotConfigured => ApiError::service_unavailable(
                "Pirate storage is not configured (PIRATE_STORAGE_ROOT)",
            ),
            PirateStorageError::Io(e) => ApiError::internal(e.to_string()),
            PirateStorageError::Db(m) => ApiError::internal(m),
        }
    }
}

impl From<StorageBindError> for ApiError {
    fn from(e: StorageBindError) -> Self {
        match e {
            StorageBindError::InvalidScript => {
                ApiError::service_unavailable("storage bind helper script is missing on this host")
            }
            StorageBindError::SudoFailed(m) => ApiError::bad_gateway(m),
            StorageBindError::Io(e) => ApiError::internal(e.to_string()),
            StorageBindError::Json(e) => ApiError::internal(e.to_string()),
        }
    }
}

fn map_grpc_control_error(msg: String) -> ApiError {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("failed precondition")
        || lower.contains("no active release")
        || lower.contains("ports still listening")
        || lower.contains("release directory")
        || lower.contains("neither run.sh")
    {
        ApiError::precondition_failed(msg)
    } else {
        ApiError::bad_gateway(msg)
    }
}

impl From<ControlError> for ApiError {
    fn from(e: ControlError) -> Self {
        match e {
            ControlError::Grpc(msg) => map_grpc_control_error(msg),
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
            ControlError::HostDb(msg) => ApiError::bad_request(msg),
            ControlError::ProcessListeners(msg) => {
                if msg.contains("scope must be") || msg.contains("signal must be") || msg.contains("invalid pid") {
                    ApiError::bad_request(msg)
                } else if msg.contains("only supported on Linux") {
                    ApiError::not_implemented(msg)
                } else {
                    ApiError::bad_gateway(msg)
                }
            }
            ControlError::ElevationRequired(msg) => ApiError::elevation_required(msg),
            ControlError::ElevationFailed(msg) => ApiError::elevation_failed(msg),
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
                conflict_path: self.conflict_path,
            },
        };
        (self.status, Json(body)).into_response()
    }
}
