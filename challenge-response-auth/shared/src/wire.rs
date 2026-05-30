//! Serde structs for JSON over HTTP/TCP.

use serde::{Deserialize, Serialize};

/// Issued challenge (ЭТАП 2 wire).
///
/// Wire uses `nonce` as **standard base64** (no pad stripping surprises) and
/// `timestamp` as Unix epoch **milliseconds**.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChallengeJson {
    /// Base64-encoded nonce (`NONCE_LEN` bytes).
    pub nonce: String,
    /// Unix time in milliseconds (server clock when the challenge was created).
    pub timestamp: i64,
    #[serde(rename = "session_id")]
    pub session_id: String,
}

/// Client proof (ЭТАП 3 wire).
///
/// Matches the fields from ЭТАП 3 (`session_id`, `nonce`, `timestamp`, `signature`).
/// `signature` is **lowercase hex** encoding of the 32-byte HMAC tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthAttemptJson {
    #[serde(rename = "session_id")]
    pub session_id: String,
    /// Same base64 nonce as returned in the challenge.
    pub nonce: String,
    /// Same `timestamp` as in the challenge.
    pub timestamp: i64,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSuccessJson {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthFailureJson {
    pub ok: bool,
    pub reason: String,
}
