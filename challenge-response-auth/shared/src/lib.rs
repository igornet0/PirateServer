#![forbid(unsafe_code)]
//! Shared challenge–response primitives: canonical HMAC input, nonce generation,
//! and JSON-facing types for HTTP examples.

pub mod crypto;
pub mod error;
pub mod time;
pub mod wire;

/// Acceptable drift between client's echoed `challenge.timestamp` and server `now()`
/// plus network latency (milliseconds).
pub const CLOCK_SKEW_MS: i64 = 120_000;

/// How long an unused challenge session remains redeemable after issuance.
pub const CHALLENGE_TTL_MS: i64 = 120_000;

pub use crypto::{
    canonical_hmac_message, hmac_equal, hmac_sha256_sign, hmac_sha256_verify_hex,
    nonce_from_base64_fixed, nonce_to_base64_fixed, random_nonce, MAX_SESSION_ID_BYTES, NONCE_LEN,
    RECOMMENDED_MIN_SECRET_BYTES,
};
pub use error::{AuthError, CanonicalError};
pub use time::unix_timestamp_ms_now;
pub use wire::{AuthAttemptJson, AuthFailureJson, AuthSuccessJson, ChallengeJson};
