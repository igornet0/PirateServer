//! JWT access tokens for dashboard login (HS256).

use argon2::password_hash::{PasswordHash, PasswordVerifier};
use argon2::Argon2;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    /// User id (dashboard_users.id)
    pub sub: String,
    pub exp: i64,
}

pub fn verify_password_against_hash(password: &str, password_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(password_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn encode_access_token(
    user_id: i32,
    secret: &str,
    ttl_secs: u64,
) -> Result<String, String> {
    let exp = chrono::Utc::now().timestamp() + ttl_secs as i64;
    let claims = JwtClaims {
        sub: user_id.to_string(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| e.to_string())
}

pub fn decode_access_token(token: &str, secret: &str) -> Result<JwtClaims, String> {
    let data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|e| e.to_string())?;
    Ok(data.claims)
}
