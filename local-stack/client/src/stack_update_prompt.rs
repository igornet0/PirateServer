//! Interactive / env-driven `StackApplyOptions` for `pirate update` transitions.

use deploy_client::BundleProfile;
use deploy_proto::deploy::{StackApplyMode, StackApplyOptions};
use std::io::{IsTerminal, Write};

fn parse_bool_env_default(key: &str, default: bool) -> Result<bool, String> {
    match std::env::var(key) {
        Ok(v) => {
            let v = v.trim().to_lowercase();
            match v.as_str() {
                "1" | "true" | "yes" | "y" | "д" | "да" => Ok(true),
                "0" | "false" | "no" | "n" | "" => Ok(false),
                _ => Err(format!("{key}: expected 0/1 or true/false, got {v:?}")),
            }
        }
        Err(_) => Ok(default),
    }
}

fn read_line_trimmed() -> Result<String, String> {
    let mut s = String::new();
    std::io::stdin()
        .read_line(&mut s)
        .map_err(|e| e.to_string())?;
    Ok(s.trim().to_string())
}

/// Decide whether we need transition options and build [`StackApplyOptions`].
pub fn resolve_stack_apply_options(
    host_dashboard: bool,
    bundle: &BundleProfile,
) -> Result<Option<StackApplyOptions>, String> {
    let bundle_ui = bundle.effective_has_ui();
    let tty = std::io::stdin().is_terminal();

    if !host_dashboard && bundle_ui {
        return Ok(Some(if tty {
            enable_ui_interactive()?
        } else {
            enable_ui_from_env()?
        }));
    }

    if host_dashboard && !bundle_ui {
        return Ok(Some(if tty {
            disable_ui_interactive()?
        } else {
            disable_ui_from_env()?
        }));
    }

    Ok(None)
}

fn enable_ui_interactive() -> Result<StackApplyOptions, String> {
    let mut stderr = std::io::stderr();
    let _ = writeln!(
        stderr,
        "Переход на UI-стек: укажите параметры (как при install.sh --ui)."
    );
    let _ = write!(
        stderr,
        "Домен для веб-интерфейса (Enter — без домена, по IP): "
    );
    let _ = stderr.flush();
    let domain = read_line_trimmed()?;

    let _ = write!(stderr, "Имя пользователя веб-дашборда [admin]: ");
    let _ = stderr.flush();
    let user_in = read_line_trimmed()?;
    let ui_admin_username = if user_in.is_empty() {
        "admin".to_string()
    } else {
        user_in
    };

    let ui_admin_password =
        rpassword::prompt_password("Пароль веб-дашборда (Enter — случайный): ")
            .map_err(|e| e.to_string())?;

    let _ = write!(
        stderr,
        "Разрешить OTA server-stack через gRPC (DEPLOY_ALLOW_SERVER_STACK_UPDATE)? [y/N]: "
    );
    let _ = stderr.flush();
    let deploy_allow_server_stack_update = matches!(
        read_line_trimmed()?.to_lowercase().as_str(),
        "y" | "yes" | "1" | "true" | "д" | "да"
    );

    let _ = writeln!(
        stderr,
        "Предупреждение: метрики хостов (SERIES/STREAM) увеличивают нагрузку на слабом сервере."
    );
    let _ = write!(
        stderr,
        "Включить CONTROL_API_HOST_STATS_SERIES (графики)? [y/N]: "
    );
    let _ = stderr.flush();
    let control_api_host_stats_series = matches!(
        read_line_trimmed()?.to_lowercase().as_str(),
        "y" | "yes" | "1" | "true" | "д" | "да"
    );

    let _ = write!(
        stderr,
        "Включить CONTROL_API_HOST_STATS_STREAM (онлайн в UI)? [y/N]: "
    );
    let _ = stderr.flush();
    let control_api_host_stats_stream = matches!(
        read_line_trimmed()?.to_lowercase().as_str(),
        "y" | "yes" | "1" | "true" | "д" | "да"
    );

    let _ = write!(stderr, "Установить/настроить nginx для UI? [y/N]: ");
    let _ = stderr.flush();
    let install_nginx = matches!(
        read_line_trimmed()?.to_lowercase().as_str(),
        "y" | "yes" | "1" | "true" | "д" | "да"
    );

    let ui_admin_password = if ui_admin_password.is_empty() {
        random_password()
    } else {
        ui_admin_password
    };

    Ok(StackApplyOptions {
        mode: StackApplyMode::EnableUi as i32,
        domain,
        ui_admin_username,
        ui_admin_password,
        install_nginx,
        deploy_allow_server_stack_update,
        control_api_host_stats_series,
        control_api_host_stats_stream,
        nginx_keep_api_proxy: false,
    })
}

fn random_password() -> String {
    use rand_core::{OsRng, RngCore};
    let mut b = [0u8; 18];
    OsRng.fill_bytes(&mut b);
    hex::encode(b).chars().take(24).collect()
}

fn enable_ui_from_env() -> Result<StackApplyOptions, String> {
    let domain = std::env::var("PIRATE_UPDATE_DOMAIN").unwrap_or_default();
    let ui_admin_username =
        std::env::var("PIRATE_UPDATE_UI_ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let ui_admin_password = match std::env::var("PIRATE_UPDATE_UI_ADMIN_PASSWORD") {
        Ok(s) if !s.is_empty() => s,
        _ => random_password(),
    };
    Ok(StackApplyOptions {
        mode: StackApplyMode::EnableUi as i32,
        domain,
        ui_admin_username,
        ui_admin_password,
        install_nginx: parse_bool_env_default("PIRATE_UPDATE_INSTALL_NGINX", false)?,
        deploy_allow_server_stack_update: parse_bool_env_default(
            "PIRATE_UPDATE_DEPLOY_ALLOW_SERVER_STACK_UPDATE",
            false,
        )?,
        control_api_host_stats_series: parse_bool_env_default(
            "PIRATE_UPDATE_CONTROL_API_HOST_STATS_SERIES",
            false,
        )?,
        control_api_host_stats_stream: parse_bool_env_default(
            "PIRATE_UPDATE_CONTROL_API_HOST_STATS_STREAM",
            false,
        )?,
        nginx_keep_api_proxy: false,
    })
}

fn disable_ui_interactive() -> Result<StackApplyOptions, String> {
    let mut stderr = std::io::stderr();
    let _ = writeln!(
        stderr,
        "Переход на backend-only: статика UI и JWT будут отключены."
    );
    let _ = write!(
        stderr,
        "Оставить nginx в режиме только API (прокси /api)? [y/N]: "
    );
    let _ = stderr.flush();
    let nginx_keep_api_proxy = matches!(
        read_line_trimmed()?.to_lowercase().as_str(),
        "y" | "yes" | "1" | "true" | "д" | "да"
    );

    Ok(StackApplyOptions {
        mode: StackApplyMode::DisableUi as i32,
        domain: String::new(),
        ui_admin_username: String::new(),
        ui_admin_password: String::new(),
        install_nginx: false,
        deploy_allow_server_stack_update: false,
        control_api_host_stats_series: false,
        control_api_host_stats_stream: false,
        nginx_keep_api_proxy,
    })
}

fn disable_ui_from_env() -> Result<StackApplyOptions, String> {
    let nginx_keep_api_proxy = parse_bool_env_default("PIRATE_UPDATE_NGINX_KEEP_API_PROXY", false)?;
    Ok(StackApplyOptions {
        mode: StackApplyMode::DisableUi as i32,
        domain: String::new(),
        ui_admin_username: String::new(),
        ui_admin_password: String::new(),
        install_nginx: false,
        deploy_allow_server_stack_update: false,
        control_api_host_stats_series: false,
        control_api_host_stats_stream: false,
        nginx_keep_api_proxy,
    })
}
