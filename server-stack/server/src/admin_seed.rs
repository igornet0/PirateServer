//! Optional dashboard admin user from env after migrations (Argon2 hash in DB).

use argon2::password_hash::{PasswordHasher, SaltString};
use argon2::Argon2;
use deploy_db::DbStore;
use password_hash::rand_core::OsRng;
use tracing::info;

/// PHC string suitable for `dashboard_users.password_hash`.
pub fn hash_dashboard_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| e.to_string())
        .map(|h| h.to_string())
}

pub async fn seed_dashboard_admin(store: &DbStore) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let user = match std::env::var("CONTROL_UI_ADMIN_USERNAME") {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return Ok(()),
    };
    let password = match std::env::var("CONTROL_UI_ADMIN_PASSWORD") {
        Ok(s) if !s.is_empty() => s,
        _ => return Ok(()),
    };
    let force_reset = std::env::var("CONTROL_UI_ADMIN_PASSWORD_RESET")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if let Some(_existing) = store.find_dashboard_user_by_username(&user).await? {
        if !force_reset {
            info!(username = %user, "dashboard admin user exists; skip seed (set CONTROL_UI_ADMIN_PASSWORD_RESET=1 to overwrite)");
            return Ok(());
        }
        info!(username = %user, "dashboard admin password reset from env");
    } else {
        info!(username = %user, "creating dashboard admin user from env");
    }

    let hash = hash_dashboard_password(&password).map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        e.into()
    })?;

    store.upsert_dashboard_user(&user, &hash).await?;
    Ok(())
}
