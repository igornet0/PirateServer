//! CORS policy: permissive only when `CONTROL_API_CORS_ALLOW_ANY=1`; otherwise optional allow-list.

use axum::http::{HeaderValue, Method};
use std::env;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Build CORS layer from environment.
///
/// - `CONTROL_API_CORS_ALLOW_ANY=1` — mirror previous `CorsLayer::permissive()` (dev / open installs).
/// - Else `CONTROL_API_CORS_ORIGINS` — comma-separated list (e.g. `http://localhost:5173,https://example.com`).
/// - If neither applies origins, emit a restrictive layer (same-origin browser traffic does not need CORS).
pub fn build_cors_layer() -> CorsLayer {
    if env::var("CONTROL_API_CORS_ALLOW_ANY").ok().as_deref() == Some("1") {
        return CorsLayer::permissive();
    }

    let raw = env::var("CONTROL_API_CORS_ORIGINS").unwrap_or_default();
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();

    if origins.is_empty() {
        CorsLayer::new()
    } else {
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::OPTIONS,
                Method::HEAD,
            ])
            .allow_headers(tower_http::cors::Any)
    }
}
