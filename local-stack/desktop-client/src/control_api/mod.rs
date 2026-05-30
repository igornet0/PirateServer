//! control-api HTTP: JWT auth and REST endpoints (domain split for maintainability).

mod auth;
mod client;
mod rest;

pub use auth::{
    control_api_bearer_token, control_api_health_probe, control_api_login, control_api_logout,
    control_api_session_active,
};
pub use client::ControlApiClient;
pub use rest::*;
