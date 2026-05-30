//! HMAC-SHA256 over a canonical byte string (no string tricks, no partial compare).

use base64::{engine::general_purpose::STANDARD as B64_STANDARD, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::error::{AuthError, CanonicalError};

type HmacSha256 = Hmac<Sha256>;

/// Minimum recommended shared secret length (bytes). HMAC allows any key size; short keys are weak.
pub const RECOMMENDED_MIN_SECRET_BYTES: usize = 32;

/// Nonce size in bytes (128 bits).
pub const NONCE_LEN: usize = 16;

/// Magic + version prefix for the HMAC input domain separation.
const DOMAIN_V1: &[u8] = b"HCR1-v1\0";

/// Maximum UTF-8 length of `session_id` included in the HMAC input.
pub const MAX_SESSION_ID_BYTES: usize = 128;

/// Maximum nonce length accepted when building HMAC input (fixed 16 in this protocol).
pub const MAX_NONCE_BYTES: usize = NONCE_LEN;

/// Canonical message: `DOMAIN_V1 || u16_le(nonce_len) || nonce || i64_be(ts) || u16_le(sid_len) || sid_utf8`.
///
/// Length prefixes prevent concatenation ambiguity between fields.
pub fn canonical_hmac_message(
    nonce: &[u8],
    timestamp_ms: i64,
    session_id: &str,
) -> Result<Vec<u8>, CanonicalError> {
    if nonce.len() > MAX_NONCE_BYTES {
        return Err(CanonicalError::NonceTooLong {
            max: MAX_NONCE_BYTES,
        });
    }
    let sid = session_id.as_bytes();
    if sid.len() > MAX_SESSION_ID_BYTES {
        return Err(CanonicalError::SessionIdTooLong {
            max: MAX_SESSION_ID_BYTES,
        });
    }
    let mut out = Vec::with_capacity(
        DOMAIN_V1.len() + 2 + nonce.len() + 8 + 2 + sid.len(),
    );
    out.extend_from_slice(DOMAIN_V1);
    out.extend_from_slice(&(nonce.len() as u16).to_le_bytes());
    out.extend_from_slice(nonce);
    out.extend_from_slice(&timestamp_ms.to_be_bytes());
    out.extend_from_slice(&(sid.len() as u16).to_le_bytes());
    out.extend_from_slice(sid);
    Ok(out)
}

/// Compute `HMAC-SHA256(secret, canonical_message)`.
pub fn hmac_sha256_sign(
    secret: &[u8],
    nonce: &[u8],
    timestamp_ms: i64,
    session_id: &str,
) -> Result<[u8; 32], AuthError> {
    let msg = canonical_hmac_message(nonce, timestamp_ms, session_id)
        .map_err(|e| match e {
            CanonicalError::SessionIdTooLong { .. } => AuthError::SessionIdTooLong {
                max: MAX_SESSION_ID_BYTES,
            },
            CanonicalError::NonceTooLong { .. } => AuthError::NonceLength {
                expected: MAX_NONCE_BYTES,
                got: nonce.len(),
            },
        })?;
    let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| AuthError::InvalidKeyLength)?;
    mac.update(&msg);
    let tag = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&tag);
    Ok(out)
}

/// Constant-time equality of two 32-byte MACs.
#[must_use]
pub fn hmac_equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    bool::from(a.ct_eq(b))
}

/// Verify `signature` (hex) matches `HMAC-SHA256(secret, canonical(...))`.
pub fn hmac_sha256_verify_hex(
    secret: &[u8],
    nonce: &[u8],
    timestamp_ms: i64,
    session_id: &str,
    signature_hex: &str,
) -> Result<bool, AuthError> {
    let decoded = hex::decode(signature_hex)?;
    if decoded.len() != 32 {
        return Err(AuthError::SignatureLength {
            expected: 32,
            got: decoded.len(),
        });
    }
    let mut sig = [0u8; 32];
    sig.copy_from_slice(&decoded);
    let expected = hmac_sha256_sign(secret, nonce, timestamp_ms, session_id)?;
    Ok(hmac_equal(&sig, &expected))
}

/// Fill `out` with OS CSPRNG bytes (nonce).
pub fn random_nonce(out: &mut [u8; NONCE_LEN]) {
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(out);
}

/// Encode a fixed nonce as standard Base64 (JSON wire).
#[must_use]
pub fn nonce_to_base64_fixed(nonce: &[u8; NONCE_LEN]) -> String {
    B64_STANDARD.encode(nonce.as_slice())
}

/// Decode nonce from Base64 — must decode to exactly `NONCE_LEN` bytes.
pub fn nonce_from_base64_fixed(s: &str) -> Result<[u8; NONCE_LEN], AuthError> {
    let decoded = B64_STANDARD.decode(s.trim())?;
    if decoded.len() != NONCE_LEN {
        return Err(AuthError::NonceLength {
            expected: NONCE_LEN,
            got: decoded.len(),
        });
    }
    let mut out = [0u8; NONCE_LEN];
    out.copy_from_slice(&decoded);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_accepts_matching_hex() {
        let secret = b"0123456789abcdef0123456789abcdef"; // 32 bytes
        let nonce = [0xABu8; NONCE_LEN];
        let ts: i64 = 1_700_000_000_123;
        let sid = "01234567-89ab-cdef-0123-456789abcdef";
        let tag = hmac_sha256_sign(secret, &nonce, ts, sid).unwrap();
        let hex_sig = hex::encode(tag);
        assert!(hmac_sha256_verify_hex(secret, &nonce, ts, sid, &hex_sig).unwrap());
    }

    #[test]
    fn wrong_secret_rejected() {
        let good = [0x11u8; 32];
        let bad = [0xEEu8; 32];
        let nonce = [9u8; NONCE_LEN];
        let ts: i64 = 42;
        let sid = "550e8400-e29b-41d4-a716-446655440000";
        let tag = hmac_sha256_sign(&good[..], &nonce, ts, sid).unwrap();
        let hex_sig = hex::encode(tag);
        assert!(!hmac_sha256_verify_hex(&bad[..], &nonce, ts, sid, &hex_sig).unwrap());
    }

    #[test]
    fn tampered_signature_rejected() {
        let secret = b"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let nonce = [7u8; NONCE_LEN];
        let sid = "00000000-0000-4000-a000-000000000042";
        let tag = hmac_sha256_sign(secret, &nonce, 99, sid).unwrap();
        let mut tag2 = tag;
        tag2[31] ^= 0x03;
        let hex_bad = hex::encode(tag2);
        assert!(matches!(
            hmac_sha256_verify_hex(secret, &nonce, 99, sid, &hex_bad),
            Ok(false)
        ));
    }
}
