//! Authentication and wire-format errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("session id length invalid (max {max} bytes)")]
    SessionIdTooLong { max: usize },
    #[error("nonce length invalid (expected {expected} bytes, got {got})")]
    NonceLength { expected: usize, got: usize },
    #[error("invalid hex signature: {0}")]
    InvalidSignatureHex(#[from] hex::FromHexError),
    #[error("signature must be {expected} bytes after decode, got {got}")]
    SignatureLength { expected: usize, got: usize },
    #[error("invalid base64 nonce: {0}")]
    InvalidNonceB64(#[from] base64::DecodeError),
    #[error("HMAC key invalid length for HMAC-SHA256")]
    InvalidKeyLength,
    #[error("system time error: {0}")]
    SystemTime(#[from] std::time::SystemTimeError),
}

#[derive(Debug, Error)]
pub enum CanonicalError {
    #[error("session id too long (max {max} utf-8 bytes)")]
    SessionIdTooLong { max: usize },
    #[error("nonce too long (max {max} bytes)")]
    NonceTooLong { max: usize },
}
