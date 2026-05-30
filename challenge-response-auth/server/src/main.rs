#![forbid(unsafe_code)]
//! Axum demo: `/v1/challenge` → `/v1/authenticate`.

mod error;
mod state;

use std::{env, net::SocketAddr, sync::Arc};

use axum::{extract::State, routing::post, Json, Router};
use chal_auth_shared::{
    hmac_sha256_verify_hex,
    nonce_from_base64_fixed, nonce_to_base64_fixed, random_nonce, unix_timestamp_ms_now,
    AuthAttemptJson, AuthSuccessJson, ChallengeJson, CHALLENGE_TTL_MS, CLOCK_SKEW_MS,
    RECOMMENDED_MIN_SECRET_BYTES,
};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::{ChallengeStore, PendingChallenge};

#[derive(Clone)]
struct AppState {
    secret: Arc<Vec<u8>>,
    store: ChallengeStore,
    ttl_ms: i64,
    skew_ms: i64,
}

fn secret_from_env() -> Result<Vec<u8>, ApiError> {
    let hex_raw = env::var("CHAL_AUTH_SECRET_HEX").map_err(|_| {
        ApiError::Config(
            "set CHAL_AUTH_SECRET_HEX (64 hex chars => 256-bit secret)".into(),
        )
    })?;
    let trimmed = hex_raw.trim().replace([' ', ':', '-'], "");
    hex::decode(&trimmed)
        .map_err(|_| ApiError::Config("CHAL_AUTH_SECRET_HEX is invalid hex".into()))
}

fn millis_env_or_default(env_key: &str, default_ms: i64) -> Result<i64, ApiError> {
    match env::var(env_key) {
        Ok(raw) => raw
            .trim()
            .parse::<i64>()
            .map_err(|_| ApiError::Config(format!("{} must be integer ms", env_key))),
        Err(_) => Ok(default_ms),
    }
}

async fn health() -> &'static str {
    "ok"
}

async fn post_challenge(State(state): State<AppState>) -> Result<Json<ChallengeJson>, ApiError> {
    let mut nonce = [0u8; chal_auth_shared::NONCE_LEN];
    random_nonce(&mut nonce);
    let challenge_timestamp_ms =
        unix_timestamp_ms_now().map_err(|_| ApiError::Config("cannot read clock".into()))?;
    let sid = Uuid::new_v4();
    state
        .store
        .record_challenge(
            sid,
            PendingChallenge {
                nonce,
                challenge_timestamp_ms,
                created_at_wall_ms: challenge_timestamp_ms,
            },
        )
        .await;

    Ok(Json(ChallengeJson {
        nonce: nonce_to_base64_fixed(&nonce),
        timestamp: challenge_timestamp_ms,
        session_id: sid.to_string(),
    }))
}

async fn post_authenticate(
    State(state): State<AppState>,
    Json(body): Json<AuthAttemptJson>,
) -> Result<Json<AuthSuccessJson>, ApiError> {
    let session_id =
        Uuid::parse_str(body.session_id.trim()).map_err(|_| {
            ApiError::BadRequest("session_id must be UUID".into())
        })?;

    let now_wall_ms =
        unix_timestamp_ms_now().map_err(|_| ApiError::Config("cannot read clock".into()))?;

    let pending = state.store.get_pending(&session_id).await.ok_or_else(|| {
        ApiError::Unauthorized("unknown or redeemed session".into())
    })?;

    if now_wall_ms.saturating_sub(pending.created_at_wall_ms) > state.ttl_ms {
        state.store.remove(&session_id).await;
        return Err(ApiError::Unauthorized(
            "challenge expired; request `/v1/challenge` again".into(),
        ));
    }

    if (now_wall_ms - body.timestamp).abs() > state.skew_ms {
        return Err(ApiError::Unauthorized("timestamp skew too large".into()));
    }

    let nonce_fixed = nonce_from_base64_fixed(&body.nonce)
        .map_err(|_| ApiError::BadRequest("nonce is invalid base64 or length".into()))?;

    if !ChallengeStore::nonce_and_timestamp_consistent_with(&pending, &nonce_fixed, body.timestamp)
    {
        return Err(ApiError::Unauthorized(
            "nonce does not match session state".into(),
        ));
    }

    let canonical_sid = session_id.to_string();

    let ok_sig = match hmac_sha256_verify_hex(
        &state.secret,
        &nonce_fixed,
        body.timestamp,
        &canonical_sid,
        body.signature.trim(),
    ) {
        Ok(ok) => ok,
        Err(e) => {
            return Err(ApiError::BadRequest(format!(
                "signature encoding invalid: {}",
                e
            )));
        }
    };

    if !ok_sig {
        return Err(ApiError::Unauthorized("signature mismatch".into()));
    }

    state.store.remove(&session_id).await;
    Ok(Json(AuthSuccessJson {
        ok: true,
        message: "authenticated".into(),
    }))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            tracing_subscriber::EnvFilter::new("chal_auth_server=debug,tower_http=info")
        }))
        .init();

    let secret = match secret_from_env() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(?e);
            std::process::exit(2);
        }
    };

    if secret.len() < RECOMMENDED_MIN_SECRET_BYTES {
        tracing::warn!(
            len = secret.len(),
            RECOMMENDED_MIN_SECRET_BYTES,
            "CHAL_AUTH_SECRET_HEX decodes to fewer than RECOMMENDED_MIN_SECRET_BYTES bytes"
        );
    }

    let ttl_ms = millis_env_or_default("CHAL_AUTH_TTL_MS", CHALLENGE_TTL_MS).unwrap_or_else(|e| {
        tracing::error!(?e);
        std::process::exit(2);
    });
    let skew_ms =
        millis_env_or_default("CHAL_AUTH_CLOCK_SKEW_MS", CLOCK_SKEW_MS)
            .unwrap_or_else(|e| {
                tracing::error!(?e);
                std::process::exit(2);
            });

    let bind: SocketAddr = env::var("CHAL_AUTH_BIND")
        .unwrap_or_else(|_| "127.0.0.1:9393".into())
        .parse()
        .unwrap_or_else(|e| {
            tracing::error!(?e);
            std::process::exit(2);
        });

    let state = AppState {
        secret: Arc::new(secret),
        store: ChallengeStore::new(ttl_ms),
        ttl_ms,
        skew_ms,
    };

    let app = Router::new()
        .route("/health", axum::routing::get(health))
        .route("/v1/challenge", post(post_challenge))
        .route("/v1/authenticate", post(post_authenticate))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!(%bind, ttl_ms, skew_ms, "listening");
    match TcpListener::bind(bind).await {
        Ok(listener) => {
            let make = app.into_make_service();
            match axum::serve(listener, make).await {
                Ok(_) => {}
                Err(e) => tracing::error!(error = ?e),
            }
        }
        Err(e) => tracing::error!(error = ?e),
    }
}
