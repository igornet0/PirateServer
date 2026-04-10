//! Unified JSON error body for `/api/v1/*` responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use deploy_control::ControlError;
use serde::Serialize;

#[derive(Serialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorPayload,
}

#[derive(Serialize)]
pub struct ApiErrorPayload {
    pub code: String,
    pub message: String,
}

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    pub fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "bad_gateway",
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
        }
    }

    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "service_unavailable",
            message: message.into(),
        }
    }
}

impl From<ControlError> for ApiError {
    fn from(e: ControlError) -> Self {
        match e {
            ControlError::Grpc(msg) => ApiError::bad_gateway(msg),
            ControlError::Io(err) => ApiError::internal(err.to_string()),
            ControlError::Db(err) => ApiError::internal(err.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorBody {
            error: ApiErrorPayload {
                code: self.code.to_string(),
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}
