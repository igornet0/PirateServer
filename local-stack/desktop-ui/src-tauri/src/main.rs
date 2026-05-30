//! Pirate Client — Tauri shell (embedded WebView, no loopback HTTP server).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::path::BaseDirectory;
use tauri::{Emitter, Manager};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn init_tracing() -> tracing_appender::non_blocking::WorkerGuard {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PirateClient")
        .join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::never(&log_dir, "pirate-client.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stdout)
                .with_ansi(true),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false),
        )
        .init();

    tracing::info!(log_dir = %log_dir.display(), "logging to file and stdout");
    guard
}

fn pirate_resolves_in_path() -> bool {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "where", "pirate"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v pirate >/dev/null 2>&1")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// True if `pirate` resolves in a typical shell `PATH`, or the managed user-bin copy exists
/// (after silent sync a new terminal will usually pick it up; `sh -c` may not read `.zshrc`).
#[tauri::command]
fn is_pirate_cli_available() -> bool {
    if pirate_resolves_in_path() {
        return true;
    }
    match pirate_user_cli_bin() {
        Ok(p) => p.is_file(),
        Err(_) => false,
    }
}

/// `pirate --version` prints a `client=` line; used to detect stale PATH installs after app updates.
fn parse_pirate_client_version(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let t = line.trim();
        t.strip_prefix("client=").map(|s| s.trim().to_string())
    })
}

fn pirate_version_from_bin(bin: &Path) -> Option<String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_pirate_client_version(&String::from_utf8_lossy(&out.stdout))
}

fn path_to_pirate_in_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let out = std::process::Command::new("cmd")
            .args(["/C", "where", "pirate"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let line = String::from_utf8_lossy(&out.stdout).lines().next()?.trim();
        if line.is_empty() {
            return None;
        }
        let p = PathBuf::from(line);
        if p.is_file() {
            Some(p)
        } else {
            None
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v pirate 2>/dev/null")
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            return None;
        }
        let p = PathBuf::from(s);
        if p.is_file() {
            Some(p)
        } else {
            None
        }
    }
}

/// Windows: `%LOCALAPPDATA%\\PirateClient\\bin`. macOS/Linux: `PirateClient/bin` under [`dirs::data_dir`].
fn pirate_user_cli_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    let root = dirs::data_local_dir();
    #[cfg(not(windows))]
    let root = dirs::data_dir();
    let root =
        root.ok_or_else(|| "не удалось определить каталог данных пользователя".to_string())?;
    Ok(root.join("PirateClient").join("bin"))
}

fn pirate_user_cli_bin() -> Result<PathBuf, String> {
    let dir = pirate_user_cli_dir()?;
    #[cfg(windows)]
    {
        Ok(dir.join("pirate.exe"))
    }
    #[cfg(not(windows))]
    {
        Ok(dir.join("pirate"))
    }
}

#[cfg(unix)]
fn chmod755_path(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(p)
        .map_err(|e| e.to_string())?
        .permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(p, perm).map_err(|e| e.to_string())
}

const PIRATE_PATH_BLOCK_BEGIN: &str = "# PirateClient PATH begin";
const PIRATE_PATH_BLOCK_END: &str = "# PirateClient PATH end";

fn unix_path_export_line(dir: &Path) -> Result<String, String> {
    let d = dir
        .to_str()
        .ok_or_else(|| "некорректный путь к bin".to_string())?;
    // Prepend so this `pirate` wins over older copies (e.g. /usr/local/bin) after a new app install.
    // `hash -r`: zsh/bash may have cached `pirate` → /usr/local/bin before PATH changed (same session / re-source).
    Ok(format!(
        "{}\nexport PATH={}:$PATH\nhash -r 2>/dev/null || true\n{}\n",
        PIRATE_PATH_BLOCK_BEGIN,
        sh_single_quote_unix(d),
        PIRATE_PATH_BLOCK_END
    ))
}

/// Remove a prior PirateClient PATH block so we can rewrite it (prepend order, path changes, migrations).
#[cfg(unix)]
fn remove_pirate_client_path_block(content: &str) -> String {
    let Some(start) = content.find(PIRATE_PATH_BLOCK_BEGIN) else {
        return content.to_string();
    };
    let tail = &content[start..];
    let Some(rel) = tail.find(PIRATE_PATH_BLOCK_END) else {
        return content[..start].trim_end().to_string();
    };
    let end = start + rel + PIRATE_PATH_BLOCK_END.len();
    let after = content[end..].trim_start_matches('\n');
    let before = content[..start].trim_end();
    match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => after.to_string(),
        (false, true) => before.to_string(),
        (false, false) => format!("{before}\n{after}"),
    }
}

#[cfg(unix)]
fn sh_single_quote_unix(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(not(unix))]
fn sh_single_quote_unix(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Ensure a guarded `export PATH=dir:$PATH` block exists in common shell rc files (prepend; rewrite if stale).
#[cfg(unix)]
fn ensure_unix_user_bin_on_path(dir: &Path) -> Result<(), String> {
    let block = unix_path_export_line(dir)?;
    let home = dirs::home_dir().ok_or_else(|| "нет домашнего каталога".to_string())?;
    for rel in [
        ".zprofile", // login zsh: before /etc/zshrc + ~/.zshrc (path_helper already ran from /etc/zprofile)
        ".zshrc",
        ".zlogin", // login zsh: runs *after* ~/.zshrc — wins if OMZ / other rc reset PATH
        ".profile",
        ".bash_profile",
        ".bashrc", // non-login interactive bash (many terminals)
    ] {
        let p = home.join(rel);
        let existing = if p.is_file() {
            std::fs::read_to_string(&p).unwrap_or_default()
        } else {
            String::new()
        };
        let base = remove_pirate_client_path_block(&existing);
        let mut out = base.trim_end().to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&block);
        if existing == out {
            continue;
        }
        std::fs::write(&p, out).map_err(|e| format!("{p:?}: {e}"))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_unix_user_bin_on_path(_dir: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn ensure_windows_user_bin_on_path() -> Result<(), String> {
    let dir = pirate_user_cli_dir()?;
    let dir_json = serde_json::to_string(&dir.to_string_lossy()).map_err(|e| e.to_string())?;
    let ps = format!(
        r#"$ErrorActionPreference = 'Stop'
$destDir = {dir_json}
if (-not (Test-Path -LiteralPath $destDir)) {{ New-Item -ItemType Directory -Force -Path $destDir | Out-Null }}
$u = [Environment]::GetEnvironmentVariable('Path','User')
if ($null -eq $u) {{ $u = '' }}
$sep = ';'
$parts = foreach ($e in $u.Split($sep, [System.StringSplitOptions]::RemoveEmptyEntries)) {{
  if ($e -and ($e -ne $destDir)) {{ $e }}
}}
$newPath = if ($parts.Count -gt 0) {{ $destDir + $sep + ($parts -join $sep) }} else {{ $destDir }}
[Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
Write-Output 'OK'
"#,
    );
    let tmp = std::env::temp_dir().join(format!("pirate-path-env-{}.ps1", std::process::id()));
    std::fs::write(&tmp, ps.as_bytes()).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            tmp.to_str().ok_or_else(|| "temp path".to_string())?,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp);
    if !out.status.success() {
        let err = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&out.stdout).trim()
        );
        return Err(if err.is_empty() {
            "PowerShell (PATH) завершился с ошибкой.".into()
        } else {
            err
        });
    }
    Ok(())
}

fn ensure_user_cli_on_path() -> Result<(), String> {
    #[cfg(windows)]
    {
        ensure_windows_user_bin_on_path()
    }
    #[cfg(not(windows))]
    {
        let dir = pirate_user_cli_dir()?;
        ensure_unix_user_bin_on_path(&dir)
    }
}

/// Copy bundled `pirate` into the managed user `bin` if missing or older than bundled.
fn sync_user_cli_from_bundle(app: &tauri::AppHandle, force_copy: bool) -> Result<(), String> {
    let bundled = resolve_pirate_cli_source(app)?;
    verify_cli_blob(&bundled)?;
    let dir = pirate_user_cli_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = pirate_user_cli_bin()?;
    let bv = pirate_version_from_bin(&bundled);
    let uv = dest
        .is_file()
        .then(|| pirate_version_from_bin(&dest))
        .flatten();
    let need = force_copy
        || !dest.is_file()
        || match (&uv, &bv) {
            (Some(a), Some(b)) => a != b,
            (None, _) if dest.is_file() => true,
            _ => bv.is_some() && uv != bv,
        };
    if need {
        std::fs::copy(&bundled, &dest).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        chmod755_path(&dest)?;
    }
    Ok(())
}

fn silent_sync_user_cli(app: &tauri::AppHandle) -> Result<(), String> {
    let copy_res = sync_user_cli_from_bundle(app, false);
    if let Err(e) = ensure_user_cli_on_path() {
        tracing::warn!(error = %e, "ensure_user_cli_on_path");
    }
    copy_res
}

fn same_executable(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn pirate_cli_default_install_auth_kind() -> &'static str {
    "user_local"
}

fn pirate_cli_system_wide_install_auth_kind() -> Option<&'static str> {
    #[cfg(target_os = "macos")]
    {
        return Some("macos_admin");
    }
    #[cfg(target_os = "linux")]
    {
        return Some("linux_pkexec");
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PirateCliPathInfo {
    path_in_path: Option<String>,
    path_version: Option<String>,
    bundled_version: Option<String>,
    user_bin_path: Option<String>,
    user_bin_version: Option<String>,
    /// Managed user copy is missing or older than bundled.
    needs_update: bool,
    /// `command -v pirate` resolves to a different file than the managed user bin.
    first_on_path_differs_from_user_bin: bool,
    /// Default install (`install_pirate_cli` without elevation): always `user_local`.
    install_auth_kind: String,
    /// OS-specific auth for optional system-wide install (`systemWide: true`).
    system_wide_install_auth_kind: Option<String>,
}

#[tauri::command]
fn pirate_cli_path_info(app: tauri::AppHandle) -> Result<PirateCliPathInfo, String> {
    let bundled = resolve_pirate_cli_source(&app)?;
    verify_cli_blob(&bundled)?;
    let bundled_version = pirate_version_from_bin(&bundled);

    let user_bin = pirate_user_cli_bin()?;
    let user_bin_path = user_bin.is_file().then(|| user_bin.display().to_string());
    let user_bin_version = user_bin
        .is_file()
        .then(|| pirate_version_from_bin(&user_bin))
        .flatten();

    let needs_update = !user_bin.is_file()
        || match (&user_bin_version, &bundled_version) {
            (Some(uv), Some(bv)) => uv != bv,
            (None, _) if user_bin.is_file() => true,
            _ => bundled_version.is_some() && user_bin_version != bundled_version,
        };

    let path_bin = path_to_pirate_in_path();
    let path_in_path = path_bin.as_ref().map(|p| p.to_string_lossy().to_string());
    let path_version = path_bin.as_ref().and_then(|p| pirate_version_from_bin(p));

    let first_on_path_differs_from_user_bin = match (path_bin.as_ref(), user_bin.is_file()) {
        (Some(pb), true) => !same_executable(pb, &user_bin),
        _ => false,
    };

    let install_auth_kind = pirate_cli_default_install_auth_kind().to_string();
    let system_wide_install_auth_kind =
        pirate_cli_system_wide_install_auth_kind().map(String::from);

    tracing::info!(
        path_in_path = ?path_in_path,
        path_version = ?path_version,
        user_bin_path = ?user_bin_path,
        user_bin_version = ?user_bin_version,
        bundled = %bundled.display(),
        bundled_version = ?bundled_version,
        needs_update,
        first_on_path_differs_from_user_bin,
        "pirate_cli_path_info"
    );

    Ok(PirateCliPathInfo {
        path_in_path,
        path_version,
        bundled_version,
        user_bin_path,
        user_bin_version,
        needs_update,
        first_on_path_differs_from_user_bin,
        install_auth_kind,
        system_wide_install_auth_kind,
    })
}

#[cfg(target_os = "macos")]
fn sh_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn verify_cli_blob(p: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    let len = meta.len();
    if len < 4096 {
        return Err(format!(
            "Встроенный CLI не собран ({} байт, путь: {}). Сначала выполните: cargo build -p deploy-client --bin pirate, затем пересоберите клиент (tauri build).",
            len,
            p.display()
        ));
    }
    Ok(())
}

fn resolve_pirate_cli_source(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(p) = app
        .path()
        .resolve("bundled/cli/pirate", BaseDirectory::Resource)
    {
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Ok(rd) = app.path().resource_dir() {
        let p = rd.join("bundled/cli/pirate");
        if p.is_file() {
            return Ok(p);
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "нет каталога исполняемого файла".to_string())?;
    #[cfg(windows)]
    let side = dir.join("pirate.exe");
    #[cfg(not(windows))]
    let side = dir.join("pirate");
    if side.is_file() {
        return Ok(side);
    }
    Err("Не найден встроенный бинарь pirate (bundled/cli/pirate).".into())
}

#[cfg(target_os = "linux")]
fn install_pirate_to_path_linux_system_wide(src: &Path) -> Result<String, String> {
    let src_s = src
        .to_str()
        .ok_or_else(|| "некорректный путь".to_string())?;
    let st = std::process::Command::new("pkexec")
        .args(["install", "-m", "0755", src_s, "/usr/local/bin/pirate"])
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() {
        return Ok(
            "pirate установлен в /usr/local/bin/pirate. Откройте новый терминал и проверьте: pirate --help"
                .into(),
        );
    }
    Err(
        "Не удалось установить через pkexec. Запустите вручную: sudo install -m 0755 <путь> /usr/local/bin/pirate"
            .into(),
    )
}

/// Keep in sync with `identifier` in `tauri.conf.json`.
#[cfg(target_os = "macos")]
const MACOS_APP_BUNDLE_ID: &str = "com.pirate.client";

#[cfg(target_os = "macos")]
fn macos_path_pirate_wrapper_sh() -> String {
    format!(
        r#"#!/bin/sh
set -e
for app in "/Applications/PirateClient.app" $(/usr/bin/mdfind "kMDItemCFBundleIdentifier == '{}'" 2>/dev/null); do
  [ -e "$app" ] || continue
  cli="$app/Contents/Resources/bundled/cli/pirate"
  if [ -x "$cli" ]; then
    exec "$cli" "$@"
  fi
done
echo "pirate: PirateClient.app not found or bundled CLI missing (bundle {})." >&2
exit 127
"#,
        MACOS_APP_BUNDLE_ID, MACOS_APP_BUNDLE_ID
    )
}

#[cfg(target_os = "macos")]
fn format_osascript_output(out: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        "no output".into()
    }
}

/// Escape for use inside AppleScript double-quoted string (e.g. `quoted form of "…"`).
#[cfg(target_os = "macos")]
fn applescript_string_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn install_pirate_to_path_macos_system_wide() -> Result<String, String> {
    let body = macos_path_pirate_wrapper_sh();
    let launcher =
        std::env::temp_dir().join(format!("pirate-path-launcher-{}.sh", std::process::id()));
    std::fs::write(&launcher, body.as_bytes()).map_err(|e| e.to_string())?;
    chmod755_path(&launcher)?;

    let runner = std::env::temp_dir().join(format!(
        "pirate-path-install-runner-{}.sh",
        std::process::id()
    ));
    let lq = sh_single_quote(&launcher.to_string_lossy());
    let runner_body = format!(
        "#!/bin/bash\n\
set -e\n\
LAUNCHER={lq}\n\
cleanup() {{ rm -f \"$LAUNCHER\" \"$0\" 2>/dev/null || true; }}\n\
trap cleanup EXIT\n\
/bin/mkdir -p /usr/local/bin\n\
sudo /usr/bin/install -m 0755 \"$LAUNCHER\" /usr/local/bin/pirate\n\
echo \"[PirateClient] pirate installed to /usr/local/bin/pirate.\"\n\
read -r -p \"Press Enter to close this tab…\" _\n",
    );
    std::fs::write(&runner, runner_body.as_bytes()).map_err(|e| e.to_string())?;
    chmod755_path(&runner)?;

    let rp = applescript_string_escape(&runner.to_string_lossy());
    let apple = format!(
        "tell application id \"com.apple.Terminal\"\n\
activate\n\
do script (\"exec bash \" & quoted form of \"{rp}\")\n\
end tell"
    );

    let out = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &apple])
        .output()
        .map_err(|e| e.to_string())?;

    if !out.status.success() {
        let _ = std::fs::remove_file(&launcher);
        let _ = std::fs::remove_file(&runner);
        let detail = format_osascript_output(&out);
        return Err(format!(
            "Не удалось открыть Terminal для установки: {detail}\n\
Разрешите PirateClient управлять Terminal: «Системные настройки → Конфиденциальность и безопасность → Автоматизация» (или Automation), если macOS блокировал запрос.\n\
Также нужна запись NSAppleEventsUsageDescription в Info.plist (входит в сборку приложения)."
        ));
    }

    Ok(
        "Открылся Terminal: в его окне появится запрос пароля от sudo — введите пароль этого Mac, чтобы записать pirate в /usr/local/bin. \
Дождитесь строки «[PirateClient] pirate installed…», затем Enter.\n\n\
В /usr/local/bin ставится launcher из приложения: после обновления PirateClient версия `pirate --version` в терминале совпадёт с приложением."
            .into(),
    )
}

fn install_pirate_cli_user(app: &tauri::AppHandle) -> Result<String, String> {
    sync_user_cli_from_bundle(app, true)?;
    ensure_user_cli_on_path()?;
    let dest = pirate_user_cli_bin()?;
    Ok(format!(
        "pirate установлен в {} (копия из приложения). Каталог добавлен в PATH пользователя; полностью закройте и снова откройте терминал, затем: pirate --version",
        dest.display()
    ))
}

fn install_pirate_cli_sync(app: tauri::AppHandle, system_wide: bool) -> Result<String, String> {
    let src = resolve_pirate_cli_source(&app)?;
    verify_cli_blob(&src)?;

    if system_wide {
        #[cfg(windows)]
        {
            let _ = src;
            return Err(
                "На Windows используется установка в каталог пользователя без прав администратора."
                    .into(),
            );
        }
        #[cfg(target_os = "linux")]
        {
            return install_pirate_to_path_linux_system_wide(&src);
        }
        #[cfg(target_os = "macos")]
        {
            let _ = src;
            return install_pirate_to_path_macos_system_wide();
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = (app, src);
            return Err("Системная установка на этой ОС не поддерживается.".into());
        }
    }

    install_pirate_cli_user(&app)
}

/// Default: user-local copy + PATH. Pass `systemWide: true` on macOS/Linux for `/usr/local/bin` (password).
#[tauri::command]
async fn install_pirate_cli(
    app: tauri::AppHandle,
    system_wide: Option<bool>,
) -> Result<String, String> {
    let sw = system_wide.unwrap_or(false);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || install_pirate_cli_sync(app, sw))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_status() -> pirate_desktop::AppStatus {
    pirate_desktop::app_status()
}

#[tauri::command]
fn parse_grpc_bundle(bundle: String) -> Result<String, String> {
    pirate_desktop::parse_grpc_endpoint_from_bundle(&bundle)
}

#[tauri::command]
fn connect_grpc_bundle(bundle: String) -> Result<pirate_desktop::GrpcConnectResult, String> {
    pirate_desktop::connect_from_bundle(&bundle)
}

#[tauri::command]
fn get_saved_grpc_endpoint() -> Option<String> {
    pirate_desktop::load_endpoint()
}

#[tauri::command]
fn clear_grpc_connection() -> Result<(), String> {
    pirate_desktop::clear_endpoint()
}

#[tauri::command]
fn test_grpc_endpoint(endpoint: String) -> Result<pirate_desktop::GrpcConnectResult, String> {
    pirate_desktop::verify_grpc_endpoint(&endpoint)
}

#[tauri::command]
fn refresh_grpc_status() -> Result<pirate_desktop::GrpcConnectResult, String> {
    let ep = pirate_desktop::load_endpoint().ok_or_else(|| "no saved connection".to_string())?;
    pirate_desktop::verify_grpc_endpoint(&ep)
}

#[tauri::command]
fn get_control_api_base() -> Option<String> {
    pirate_desktop::load_control_api_base()
}

#[tauri::command]
fn set_control_api_base(url: String) -> Result<(), String> {
    pirate_desktop::set_control_api_base(&url)
}

#[tauri::command]
fn mark_control_api_recent_restart(seconds: Option<i64>) -> Result<(), String> {
    pirate_desktop::mark_control_api_recent_restart(seconds.unwrap_or(90))
}

#[tauri::command]
fn control_api_recent_restart_hint() -> bool {
    pirate_desktop::control_api_recent_restart_hint()
}

#[tauri::command]
fn fetch_remote_host_stats() -> Result<String, String> {
    pirate_desktop::fetch_host_stats_json()
}

#[tauri::command]
fn fetch_remote_host_stats_detail(
    kind: i32,
    top: u32,
    q: String,
    limit: u32,
) -> Result<String, String> {
    pirate_desktop::fetch_host_stats_detail_json(kind, top, q, limit)
}

#[tauri::command]
fn ssl_status_json(grpc_url: String, project_id: String) -> Result<String, String> {
    pirate_desktop::ssl_status_json(&grpc_url, &project_id)
}

#[tauri::command]
fn ssl_create(
    grpc_url: String,
    project_id: String,
    domains: Vec<String>,
    mode: i32,
    webroot_path: String,
    dry_run: bool,
    staging: bool,
) -> Result<String, String> {
    pirate_desktop::ssl_create_json(
        &grpc_url,
        &project_id,
        domains,
        mode,
        &webroot_path,
        dry_run,
        staging,
    )
}

#[tauri::command]
fn ssl_update(
    grpc_url: String,
    project_id: String,
    exact_domain: String,
    glob_pattern: String,
    regex: String,
    dry_run: bool,
) -> Result<String, String> {
    pirate_desktop::ssl_update_json(
        &grpc_url,
        &project_id,
        &exact_domain,
        &glob_pattern,
        &regex,
        dry_run,
    )
}

#[tauri::command]
fn ssl_check_and_renew(
    grpc_url: String,
    project_id: String,
    force_all: bool,
) -> Result<String, String> {
    pirate_desktop::ssl_check_and_renew_json(&grpc_url, &project_id, force_all)
}

/// `GET {base}/api/v1/host-stats/series` for `net_rx` and `net_tx` (control-api; requires
/// `CONTROL_API_HOST_STATS_SERIES=1` on the server). Same base URL as gRPC endpoint (HTTPS).
#[tauri::command]
async fn fetch_remote_host_stats_series(base_url: String, range: String) -> Result<String, String> {
    fn norm_range(s: &str) -> &'static str {
        let r = s.trim().to_lowercase().replace(' ', "");
        match r.as_str() {
            "15m" | "15min" => "15m",
            "1h" | "60m" | "60min" => "1h",
            "24h" | "24hr" | "1d" | "1440m" => "24h",
            "7d" | "1w" | "week" | "168h" | "168hr" => "7d",
            _ => "1h",
        }
    }

    let base = base_url.trim().trim_end_matches('/');
    let enc = norm_range(&range);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;

    let rx_url = format!("{base}/api/v1/host-stats/series?metric=net_rx&range={enc}");
    let tx_url = format!("{base}/api/v1/host-stats/series?metric=net_tx&range={enc}");

    let (rx_res, tx_res) = tokio::join!(client.get(&rx_url).send(), client.get(&tx_url).send(),);

    let rx_res = rx_res.map_err(|e| e.to_string())?;
    let tx_res = tx_res.map_err(|e| e.to_string())?;

    let rx_status = rx_res.status();
    let tx_status = tx_res.status();
    let rx_body = rx_res.text().await.map_err(|e| e.to_string())?;
    let tx_body = tx_res.text().await.map_err(|e| e.to_string())?;

    if !rx_status.is_success() {
        return Err(format!(
            "net_rx HTTP {}: {}",
            rx_status,
            rx_body.chars().take(200).collect::<String>()
        ));
    }
    if !tx_status.is_success() {
        return Err(format!(
            "net_tx HTTP {}: {}",
            tx_status,
            tx_body.chars().take(200).collect::<String>()
        ));
    }

    let net_rx: serde_json::Value = serde_json::from_str(&rx_body).map_err(|e| e.to_string())?;
    let net_tx: serde_json::Value = serde_json::from_str(&tx_body).map_err(|e| e.to_string())?;
    let out = serde_json::json!({ "net_rx": net_rx, "net_tx": net_tx });
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_active_project() -> String {
    pirate_desktop::load_project_id()
}

#[tauri::command]
fn set_active_project(project_id: String) -> Result<(), String> {
    pirate_desktop::set_active_project(project_id)
}

#[tauri::command]
fn pick_deploy_directory() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        .pick_folder()
        .map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
async fn deploy_from_directory(
    app: tauri::AppHandle,
    directory: String,
    version: String,
    chunk_size: Option<u32>,
) -> Result<pirate_desktop::DeployOutcome, String> {
    let chunk = chunk_size.unwrap_or(64 * 1024) as usize;
    let dir = PathBuf::from(directory.clone());
    let registry_version = version.clone();
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || {
        let rt = pirate_desktop::deploy::runtime().map_err(|e| e.to_string())?;
        let out = rt.block_on(pirate_desktop::deploy::run_deploy_with_progress_events(
            dir.clone(),
            version,
            chunk,
            move |ev| {
                let _ = app2.emit("deploy-progress", &ev);
            },
        ))?;
        let _ = pirate_desktop::record_deploy_for_directory(
            dir.as_path(),
            &out.deployed_version,
            Some(registry_version.trim()),
        );
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn rollback_deploy(version: String) -> Result<pirate_desktop::RollbackOutcome, String> {
    pirate_desktop::deploy::run_rollback(version)
}

#[tauri::command]
fn read_release_version_from_manifest(directory: String) -> Result<String, String> {
    pirate_desktop::read_release_version_from_manifest(PathBuf::from(directory).as_path())
}

#[tauri::command]
fn check_project_uploaded(directory: String) -> Result<pirate_desktop::ProjectDeployCheck, String> {
    pirate_desktop::check_project_uploaded(PathBuf::from(directory))
}

#[tauri::command]
fn remove_server_project(
    project_id: String,
) -> Result<pirate_desktop::RemoveProjectOutcome, String> {
    pirate_desktop::deploy::run_remove_project(project_id)
}

#[tauri::command]
fn analyze_network_access(
    directory: String,
    overrides: Option<pirate_desktop::AnalyzeNetworkAccessOverrides>,
) -> Result<pirate_desktop::NetworkAccessAnalysis, String> {
    pirate_desktop::analyze_network_access(PathBuf::from(directory), overrides)
}

#[tauri::command]
fn validate_network_access(directory: String) -> Result<String, String> {
    let r = pirate_desktop::validate_network_access_remote(PathBuf::from(directory))?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_project_network_manifest(
    directory: String,
    input: pirate_desktop::SaveProjectNetworkManifestInput,
) -> Result<(), String> {
    pirate_desktop::save_project_network_manifest(PathBuf::from(directory), input)
}

#[tauri::command]
fn load_project_network_manifest(directory: String) -> Result<pirate_desktop::LoadProjectNetworkManifestView, String> {
    pirate_desktop::load_project_network_manifest(PathBuf::from(directory))
}

#[tauri::command]
fn read_project_local_env(directory: String) -> Result<pirate_desktop::LocalEnvView, String> {
    pirate_desktop::read_project_local_env(PathBuf::from(directory))
}

#[tauri::command]
fn write_project_local_env(directory: String, content: String) -> Result<(), String> {
    pirate_desktop::write_project_local_env(PathBuf::from(directory), content)
}

#[tauri::command]
fn control_api_apply_project_nginx(directory: String) -> Result<String, String> {
    pirate_desktop::control_api_apply_project_nginx(&PathBuf::from(directory))
}

#[tauri::command]
fn apply_manifest_fix(directory: String, fix_id: String) -> Result<String, String> {
    pirate_desktop::apply_manifest_fix(&PathBuf::from(directory), &fix_id)
}

#[tauri::command]
fn projects_preflight(directory: String, version: String) -> Result<String, String> {
    let report = pirate_desktop::run_projects_preflight(PathBuf::from(directory), &version);
    serde_json::to_string(&report).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_registered_projects() -> Result<Vec<pirate_desktop::RegisteredProject>, String> {
    pirate_desktop::list_registered_projects()
}

#[tauri::command]
fn register_project_from_directory(path: String) -> Result<String, String> {
    pirate_desktop::register_project_from_directory(path)
}

#[tauri::command]
fn remove_registered_project(name: String) -> Result<bool, String> {
    pirate_desktop::remove_registered_project(name)
}

#[tauri::command]
fn local_dev_start(
    app: tauri::AppHandle,
    path: String,
    cmd_vars: Option<std::collections::HashMap<String, String>>,
) -> Result<(), String> {
    let app = app.clone();
    let emit: std::sync::Arc<dyn Fn(pirate_desktop::LocalDevLogLine) + Send + Sync> =
        std::sync::Arc::new(move |line: pirate_desktop::LocalDevLogLine| {
            let _ = app.emit("local-dev-log", &line);
        });
    let vars = pirate_desktop::cmd_vars_map_from_json(cmd_vars);
    pirate_desktop::start_local_dev_stack(PathBuf::from(path), Some(emit), vars)
}

#[tauri::command]
fn local_dev_stop() -> Result<(), String> {
    pirate_desktop::stop_local_dev_stack()
}

#[tauri::command]
fn local_dev_status() -> pirate_desktop::LocalDevStatus {
    pirate_desktop::local_dev_status()
}

#[tauri::command]
fn probe_local_toolchain() -> pirate_desktop::ToolchainReport {
    pirate_desktop::probe_local_toolchain()
}

#[tauri::command]
fn control_api_login(base_url: String, username: String, password: String) -> Result<(), String> {
    pirate_desktop::control_api_login(&base_url, &username, &password)
}

#[tauri::command]
fn control_api_keychain_save(
    base_url: String,
    username: String,
    password: String,
) -> Result<(), String> {
    pirate_desktop::control_api_keychain_save(&base_url, &username, &password)
}

#[tauri::command]
fn control_api_keychain_load(
    base_url: String,
) -> Result<Option<pirate_desktop::ControlApiKeychainCreds>, String> {
    pirate_desktop::control_api_keychain_load(&base_url)
}

#[tauri::command]
fn control_api_keychain_delete(base_url: String) -> Result<(), String> {
    pirate_desktop::control_api_keychain_delete(&base_url)
}

#[tauri::command]
fn control_api_health_probe(base_url: String) -> Result<String, String> {
    pirate_desktop::control_api_health_probe(&base_url)
}

#[tauri::command]
fn control_api_logout() -> Result<(), String> {
    pirate_desktop::control_api_logout()
}

#[tauri::command]
fn control_api_session_active() -> bool {
    pirate_desktop::control_api_session_active()
}

#[tauri::command]
fn control_api_bearer_token() -> Result<String, String> {
    pirate_desktop::control_api_bearer_token()
}

#[tauri::command]
async fn control_api_fetch_status_json(project_id: String) -> Result<String, String> {
    let pid = project_id;
    tauri::async_runtime::spawn_blocking(move || pirate_desktop::control_api_fetch_status_json(&pid))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn control_api_fetch_project_telemetry_json(
    project_id: String,
    logs_limit: Option<usize>,
) -> Result<String, String> {
    pirate_desktop::control_api_fetch_project_telemetry_json(&project_id, logs_limit.unwrap_or(120))
}

#[tauri::command]
fn control_api_clear_project_runtime_log(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_clear_project_runtime_log(&project_id)
}

#[tauri::command]
fn control_api_fetch_app_env_json(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_fetch_app_env_json(&project_id)
}

#[tauri::command]
fn control_api_put_app_env(project_id: String, content: String) -> Result<(), String> {
    pirate_desktop::control_api_put_app_env(&project_id, &content)
}

#[tauri::command]
fn control_api_fetch_host_deploy_env_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_host_deploy_env_json()
}

#[tauri::command]
fn control_api_put_host_deploy_env(content: String) -> Result<String, String> {
    pirate_desktop::control_api_put_host_deploy_env(&content)
}

#[tauri::command]
fn control_api_fetch_host_deploy_env_template_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_host_deploy_env_template_json()
}

#[tauri::command]
fn control_api_fetch_nginx_status_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_nginx_status_json()
}

#[tauri::command]
fn control_api_fetch_host_services_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_host_services_json()
}

#[tauri::command]
fn control_api_host_service_install(
    id: String,
    install_env_json: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_service_install(&id, install_env_json.as_deref())
}

#[tauri::command]
fn control_api_host_service_remove(id: String) -> Result<String, String> {
    pirate_desktop::control_api_host_service_remove(&id)
}

#[tauri::command]
fn control_api_host_service_runtime_get_json(id: String) -> Result<String, String> {
    pirate_desktop::control_api_host_service_runtime_get_json(&id)
}

#[tauri::command]
fn control_api_host_service_runtime_put_json(
    id: String,
    body_json: String,
) -> Result<String, String> {
    pirate_desktop::control_api_host_service_runtime_put_json(&id, &body_json)
}

#[tauri::command]
fn control_api_host_service_restart(id: String) -> Result<String, String> {
    pirate_desktop::control_api_host_service_restart(&id)
}

#[tauri::command]
fn control_api_host_databases_list_json() -> Result<String, String> {
    pirate_desktop::control_api_host_databases_list_json()
}

#[tauri::command]
fn control_api_host_db_schemas_json(
    instance_id: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_schemas_json(
        &instance_id,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_tables_json(
    instance_id: String,
    schema: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_tables_json(
        &instance_id,
        &schema,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_columns_json(
    instance_id: String,
    schema: String,
    table: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_columns_json(
        &instance_id,
        &schema,
        &table,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_rows_json(
    instance_id: String,
    schema: String,
    table: String,
    limit: u32,
    offset: u32,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_rows_json(
        &instance_id,
        &schema,
        &table,
        limit,
        offset,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_relationships_json(
    instance_id: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_relationships_json(
        &instance_id,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_query_json(
    instance_id: String,
    sql: String,
    max_rows: u32,
    db_user: Option<String>,
    db_password: Option<String>,
    database: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_query_json(
        &instance_id,
        &sql,
        max_rows,
        db_user.as_deref(),
        db_password.as_deref(),
        database.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_redis_keys_json(
    instance_id: String,
    pattern: String,
    cursor: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_redis_keys_json(
        &instance_id,
        &pattern,
        &cursor,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_mongo_databases_json(
    instance_id: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_mongo_databases_json(
        &instance_id,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_mongo_collections_json(
    instance_id: String,
    db: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_mongo_collections_json(
        &instance_id,
        &db,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_mongo_preview_json(
    instance_id: String,
    db: String,
    collection: String,
    limit: u32,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_mongo_preview_json(
        &instance_id,
        &db,
        &collection,
        limit,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_capabilities_json() -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_capabilities_json()
}

#[tauri::command]
fn control_api_host_db_v2_object_tree_json(
    instance_id: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_object_tree_json(
        &instance_id,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_grid_json(
    instance_id: String,
    schema: String,
    table: String,
    limit: u32,
    offset: u32,
    sort_column: Option<String>,
    sort_desc: bool,
    filter_column: Option<String>,
    filter_value: Option<serde_json::Value>,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_grid_json(
        &instance_id,
        &schema,
        &table,
        limit,
        offset,
        sort_column.as_deref(),
        sort_desc,
        filter_column.as_deref(),
        filter_value,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_row_mutate_json(
    instance_id: String,
    op: String,
    schema: String,
    table: String,
    pk: Option<serde_json::Map<String, serde_json::Value>>,
    row: serde_json::Value,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_row_mutate_json(
        &instance_id,
        &op,
        &schema,
        &table,
        pk,
        row,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_sql_job_start_json(
    instance_id: String,
    sql: String,
    max_rows: u32,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_sql_job_start_json(
        &instance_id,
        &sql,
        max_rows,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_sql_job_get_json(
    instance_id: String,
    job_id: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_sql_job_get_json(
        &instance_id,
        &job_id,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_sql_job_cancel_json(
    instance_id: String,
    job_id: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_sql_job_cancel_json(
        &instance_id,
        &job_id,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_migration_status_get_json(
    instance_id: String,
    database: String,
    db_user: Option<String>,
    db_password: Option<String>,
    tools: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_migration_status_get_json(
        &instance_id,
        &database,
        db_user.as_deref(),
        db_password.as_deref(),
        tools.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_migration_status_post_json(
    instance_id: String,
    database: String,
    db_user: Option<String>,
    db_password: Option<String>,
    tools: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_migration_status_post_json(
        &instance_id,
        &database,
        db_user.as_deref(),
        db_password.as_deref(),
        tools.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_admin_create_database_json(
    instance_id: String,
    database: String,
    owner: Option<String>,
    encoding: Option<String>,
    if_not_exists: Option<bool>,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_admin_create_database_json(
        &instance_id,
        &database,
        owner.as_deref(),
        encoding.as_deref(),
        if_not_exists.unwrap_or(false),
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_admin_create_table_json(
    instance_id: String,
    body_json: String,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_admin_create_table_json(&instance_id, &body_json)
}

#[tauri::command]
fn control_api_host_db_v2_admin_create_user_json(
    instance_id: String,
    body_json: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_admin_create_user_json(
        &instance_id,
        &body_json,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_admin_delete_user_json(
    instance_id: String,
    body_json: String,
    db_user: Option<String>,
    db_password: Option<String>,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_admin_delete_user_json(
        &instance_id,
        &body_json,
        db_user.as_deref(),
        db_password.as_deref(),
    )
}

#[tauri::command]
fn control_api_host_db_v2_migration_run_json(
    instance_id: String,
    tool: String,
    workdir: String,
) -> Result<String, String> {
    pirate_desktop::control_api_host_db_v2_migration_run_json(&instance_id, &tool, &workdir)
}

/// Saved DB username / encrypted password file state for host-db viewer (per `instance_id`).
#[tauri::command]
fn db_credentials_get_json(instance_id: String) -> Result<String, String> {
    pirate_desktop::db_credentials_get_json(&instance_id)
}

#[tauri::command]
fn db_credentials_save(
    instance_id: String,
    username: String,
    password: String,
    remember: bool,
) -> Result<(), String> {
    pirate_desktop::db_credentials_save(&instance_id, &username, &password, remember)
}

#[tauri::command]
fn db_credentials_forget(instance_id: String) -> Result<(), String> {
    pirate_desktop::db_credentials_forget(&instance_id)
}

#[tauri::command]
fn db_local_forward_start(target_host: String, target_port: u16) -> Result<u16, String> {
    pirate_desktop::db_local_forward_start(&target_host, target_port)
}

#[tauri::command]
fn db_local_forward_stop() -> Result<(), String> {
    pirate_desktop::db_local_forward_stop()
}

#[tauri::command]
fn db_local_forward_local_port() -> Option<u16> {
    pirate_desktop::db_local_forward_local_port()
}

#[tauri::command]
fn db_tunnel_list_json() -> Result<String, String> {
    pirate_desktop::db_tunnel_list_json()
}

#[tauri::command]
fn db_tunnel_tcp_start(id: String, target_host: String, target_port: u16) -> Result<u16, String> {
    pirate_desktop::db_tunnel_tcp_start(id, &target_host, target_port)
}

#[tauri::command]
fn db_tunnel_tcp_stop(id: String) -> Result<(), String> {
    pirate_desktop::db_tunnel_tcp_stop(&id)
}

#[tauri::command]
fn db_tunnel_ssh_start(
    id: String,
    ssh_host: String,
    ssh_port: u16,
    ssh_user: String,
    remote_host: String,
    remote_port: u16,
    local_port: u16,
    identity_path: Option<String>,
) -> Result<u16, String> {
    pirate_desktop::db_tunnel_ssh_start(
        id,
        &ssh_host,
        ssh_port,
        &ssh_user,
        &remote_host,
        remote_port,
        local_port,
        identity_path.as_deref(),
    )
}

#[tauri::command]
fn db_tunnel_ssh_stop(id: String) -> Result<(), String> {
    pirate_desktop::db_tunnel_ssh_stop(&id)
}

#[tauri::command]
fn stack_tun_health(base_url: String, bearer: Option<String>) -> Result<String, String> {
    pirate_desktop::stack_tun_health(&base_url, bearer.as_deref())
}

#[tauri::command]
fn stack_tun_get_config(base_url: String, bearer: Option<String>) -> Result<String, String> {
    pirate_desktop::stack_tun_get_config_json(&base_url, bearer.as_deref())
}

#[tauri::command]
fn stack_tun_put_config(
    base_url: String,
    bearer: Option<String>,
    json_body: String,
) -> Result<String, String> {
    pirate_desktop::stack_tun_put_config_json(&base_url, bearer.as_deref(), &json_body)
}

#[tauri::command]
fn stack_tun_reload_peers(base_url: String, bearer: Option<String>) -> Result<String, String> {
    pirate_desktop::stack_tun_reload_peers(&base_url, bearer.as_deref())
}

#[tauri::command]
fn stack_tun_stats(base_url: String, bearer: Option<String>) -> Result<String, String> {
    pirate_desktop::stack_tun_stats_json(&base_url, bearer.as_deref())
}

#[tauri::command]
fn stack_tun_identity_public_key(base_url: String, bearer: Option<String>) -> Result<String, String> {
    pirate_desktop::stack_tun_identity_public_key_json(&base_url, bearer.as_deref())
}

#[tauri::command]
fn stack_tun_authorize_peer(
    base_url: String,
    bearer: Option<String>,
    public_key_b64: String,
) -> Result<String, String> {
    pirate_desktop::stack_tun_authorize_peer_json(
        &base_url,
        bearer.as_deref(),
        &public_key_b64,
    )
}

#[tauri::command]
fn stack_tun_list_peers(base_url: String, bearer: Option<String>) -> Result<String, String> {
    pirate_desktop::stack_tun_list_peers_json(&base_url, bearer.as_deref())
}

#[tauri::command]
fn stack_tun_get_routes(base_url: String, bearer: Option<String>) -> Result<String, String> {
    pirate_desktop::stack_tun_get_routes_json(&base_url, bearer.as_deref())
}

#[tauri::command]
fn stack_tun_put_routes(
    base_url: String,
    bearer: Option<String>,
    json_body: String,
) -> Result<String, String> {
    pirate_desktop::stack_tun_put_routes_json(&base_url, bearer.as_deref(), &json_body)
}

#[tauri::command]
fn stack_tun_requests_json(
    base_url: String,
    bearer: Option<String>,
    query: Option<String>,
) -> Result<String, String> {
    pirate_desktop::stack_tun_requests_json(&base_url, bearer.as_deref(), query.as_deref())
}

#[tauri::command]
fn stack_tun_request_bus_invoke(
    base_url: String,
    bearer: Option<String>,
    json_body: String,
) -> Result<String, String> {
    pirate_desktop::stack_tun_request_bus_invoke_json(&base_url, bearer.as_deref(), &json_body)
}

#[tauri::command]
async fn db_direct_test(
    req: pirate_desktop::DirectTestRequest,
) -> Result<pirate_desktop::DirectTestResponse, String> {
    pirate_desktop::direct_test(req).await
}

#[tauri::command]
async fn db_direct_open(
    req: pirate_desktop::DirectOpenRequest,
) -> Result<serde_json::Value, String> {
    pirate_desktop::direct_open(req).await
}

#[tauri::command]
fn db_direct_close(session_id: String) -> Result<serde_json::Value, String> {
    pirate_desktop::direct_close(session_id)
}

#[tauri::command]
async fn db_direct_list_databases(session_id: String) -> Result<String, String> {
    pirate_desktop::direct_list_databases(&session_id).await
}

#[tauri::command]
async fn db_direct_list_schemas(session_id: String) -> Result<String, String> {
    pirate_desktop::direct_list_schemas(&session_id).await
}

#[tauri::command]
async fn db_direct_list_tables(session_id: String, schema: String) -> Result<String, String> {
    pirate_desktop::direct_list_tables(&session_id, &schema).await
}

#[tauri::command]
async fn db_direct_table_preview(
    req: pirate_desktop::DirectPreviewRequest,
) -> Result<String, String> {
    pirate_desktop::direct_table_preview(req).await
}

#[tauri::command]
async fn db_direct_query(req: pirate_desktop::DirectQueryRequest) -> Result<String, String> {
    pirate_desktop::direct_query(req).await
}

#[tauri::command]
async fn db_direct_heartbeat(session_id: String) -> Result<String, String> {
    pirate_desktop::direct_heartbeat(&session_id).await
}

#[tauri::command]
async fn db_direct_pg_stats(session_id: String) -> Result<String, String> {
    pirate_desktop::direct_pg_stats_json(&session_id).await
}

#[tauri::command]
async fn db_direct_pg_structure(
    req: pirate_desktop::DirectStructureRequest,
) -> Result<String, String> {
    pirate_desktop::direct_pg_structure_json(req).await
}

#[tauri::command]
fn db_direct_profile_list_json() -> Result<String, String> {
    pirate_desktop::direct_profile_list_json()
}

#[tauri::command]
fn db_direct_profile_upsert(body: String, password: Option<String>) -> Result<String, String> {
    let u: pirate_desktop::DirectProfileUpsert =
        serde_json::from_str(&body).map_err(|e| e.to_string())?;
    pirate_desktop::direct_profile_upsert(&u, password.as_deref())
}

#[tauri::command]
fn db_direct_profile_delete(id: String) -> Result<(), String> {
    pirate_desktop::direct_profile_delete(&id)
}

#[tauri::command]
fn db_direct_password_set(profile_id: String, password: String) -> Result<(), String> {
    pirate_desktop::direct_password_set(&profile_id, &password)
}

#[tauri::command]
fn db_direct_query_history_list(connection_id: String, limit: i64) -> Result<String, String> {
    pirate_desktop::query_history_list_json(&connection_id, limit)
}

#[tauri::command]
fn control_api_fetch_nginx_site_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_nginx_site_json()
}

#[tauri::command]
fn control_api_put_nginx_site(content: String) -> Result<String, String> {
    pirate_desktop::control_api_put_nginx_site(&content)
}

#[tauri::command]
fn control_api_fetch_nginx_file_json(path: String) -> Result<String, String> {
    pirate_desktop::control_api_fetch_nginx_file_json(&path)
}

#[tauri::command]
fn control_api_put_nginx_file_json(path: String, content: String) -> Result<String, String> {
    pirate_desktop::control_api_put_nginx_file_json(&path, &content)
}

#[tauri::command]
fn control_api_storage_tree_json(path: String) -> Result<String, String> {
    pirate_desktop::control_api_storage_tree_json(&path)
}

#[tauri::command]
fn control_api_storage_usage_json() -> Result<String, String> {
    pirate_desktop::control_api_storage_usage_json()
}

#[tauri::command]
fn control_api_storage_create_folder(path: String) -> Result<(), String> {
    pirate_desktop::control_api_storage_create_folder(&path)
}

#[tauri::command]
fn control_api_storage_delete_file(path: String) -> Result<(), String> {
    pirate_desktop::control_api_storage_delete_file(&path)
}

#[tauri::command]
fn control_api_storage_delete_folder(path: String, recursive: bool) -> Result<(), String> {
    pirate_desktop::control_api_storage_delete_folder(&path, recursive)
}

#[tauri::command]
fn control_api_storage_rename(from: String, to: String) -> Result<(), String> {
    pirate_desktop::control_api_storage_rename(&from, &to)
}

#[tauri::command]
fn control_api_storage_upload_file(
    remote_path: String,
    local_file: String,
) -> Result<String, String> {
    pirate_desktop::control_api_storage_upload_file(&remote_path, &local_file)
}

#[tauri::command]
fn control_api_storage_download_file(
    remote_path: String,
    local_path: String,
) -> Result<(), String> {
    pirate_desktop::control_api_storage_download_file(&remote_path, &local_path)
}

#[tauri::command]
fn control_api_storage_extract_json(
    archive_path: String,
    target_dir: Option<String>,
    conflict_mode: String,
) -> Result<String, String> {
    let td = target_dir.as_deref();
    pirate_desktop::control_api_storage_extract_json(&archive_path, td, &conflict_mode)
}

#[tauri::command]
fn control_api_storage_bind_sources_json() -> Result<String, String> {
    pirate_desktop::control_api_storage_bind_sources_json()
}

#[tauri::command]
fn control_api_storage_bind_json(
    source_path: String,
    volume_name: String,
) -> Result<String, String> {
    pirate_desktop::control_api_storage_bind_json(&source_path, &volume_name)
}

#[tauri::command]
fn control_api_storage_unbind_json(volume_name: String) -> Result<String, String> {
    pirate_desktop::control_api_storage_unbind_json(&volume_name)
}

#[tauri::command]
fn pick_file_for_storage_upload() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        .pick_file()
        .map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
fn pick_files_for_storage_upload() -> Result<Option<Vec<String>>, String> {
    Ok(rfd::FileDialog::new().pick_files().map(|paths| {
        paths
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }))
}

#[tauri::command]
fn pick_save_path_for_storage_download(suggested: String) -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        .set_file_name(&suggested)
        .save_file()
        .map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
fn control_api_ensure_nginx(mode: String) -> Result<String, String> {
    pirate_desktop::control_api_ensure_nginx(&mode)
}

#[tauri::command]
fn control_api_fetch_nginx_sites_json() -> Result<String, String> {
    pirate_desktop::control_api_fetch_nginx_sites_json()
}

#[tauri::command]
fn control_api_nginx_preflight_json(body: String) -> Result<String, String> {
    pirate_desktop::control_api_nginx_preflight_json(&body)
}

#[tauri::command]
fn control_api_nginx_action_json(body: String) -> Result<String, String> {
    pirate_desktop::control_api_nginx_action_json(&body)
}

#[tauri::command]
fn control_api_restart_process_json(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_restart_process_json(&project_id)
}

#[tauri::command]
fn control_api_stop_process_json(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_stop_process_json(&project_id)
}

#[tauri::command]
fn control_api_fetch_process_listeners_json(project_id: String, scope: String) -> Result<String, String> {
    pirate_desktop::control_api_fetch_process_listeners_json(&project_id, &scope)
}

#[tauri::command]
fn control_api_kill_process_listener_json(
    project_id: String,
    pid: u32,
    signal: String,
    port: Option<u16>,
    root_password: Option<String>,
    allow_foreign: bool,
) -> Result<String, String> {
    pirate_desktop::control_api_kill_process_listener_json(
        &project_id,
        pid,
        &signal,
        port,
        root_password.as_deref(),
        allow_foreign,
    )
}

#[tauri::command]
fn control_api_antiddos_get_json() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_get_json()
}

#[tauri::command]
fn control_api_antiddos_put_json(content: String) -> Result<String, String> {
    pirate_desktop::control_api_antiddos_put_json(&content)
}

#[tauri::command]
fn control_api_antiddos_enable() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_enable()
}

#[tauri::command]
fn control_api_antiddos_disable() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_disable()
}

#[tauri::command]
fn control_api_antiddos_apply() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_apply()
}

#[tauri::command]
fn control_api_antiddos_stats_json() -> Result<String, String> {
    pirate_desktop::control_api_antiddos_stats_json()
}

#[tauri::command]
fn control_api_antiddos_project_put_json(
    project_id: String,
    content: String,
) -> Result<String, String> {
    pirate_desktop::control_api_antiddos_project_put_json(&project_id, &content)
}

#[tauri::command]
fn control_api_antiddos_project_delete(project_id: String) -> Result<String, String> {
    pirate_desktop::control_api_antiddos_project_delete(&project_id)
}

#[tauri::command]
async fn fetch_server_projects_overview(
) -> Result<pirate_desktop::ServerProjectsOverview, String> {
    tauri::async_runtime::spawn_blocking(pirate_desktop::fetch_server_projects_overview)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn desktop_perf_snapshot() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(pirate_desktop::desktop_perf_snapshot_json)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn ensure_deploy_project_id_for_deploy(path: String) -> Result<String, String> {
    pirate_desktop::ensure_deploy_project_id_for_deploy(PathBuf::from(path))
}

/// Open a project directory in the system file manager (Finder, Explorer, …).
#[tauri::command]
fn open_project_folder(path: String) -> Result<(), String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("path is empty".into());
    }
    let pb = PathBuf::from(p);
    if !pb.is_dir() {
        return Err(format!("not a directory: {}", pb.display()));
    }
    open::that(&pb).map_err(|e| e.to_string())
}

#[tauri::command]
fn deploy_upload_cancel() {
    pirate_desktop::deploy_upload_cancel();
}

#[tauri::command]
fn server_stack_upload_cancel() {
    pirate_desktop::server_stack_upload_cancel();
}

#[tauri::command]
fn list_server_bookmarks() -> Vec<pirate_desktop::ServerBookmark> {
    pirate_desktop::load_bookmarks()
}

#[tauri::command]
fn delete_server_bookmark(id: String) -> Result<(), String> {
    pirate_desktop::remove_bookmark(&id)
}

#[tauri::command]
fn add_server_bookmark(url: String) -> Result<pirate_desktop::ServerBookmark, String> {
    let id = pirate_desktop::add_bookmark_from_input(&url)?;
    pirate_desktop::load_bookmarks()
        .into_iter()
        .find(|b| b.id == id)
        .ok_or_else(|| "bookmark not found after insert".to_string())
}

#[tauri::command]
fn activate_server_bookmark(url: String) -> Result<pirate_desktop::GrpcConnectResult, String> {
    pirate_desktop::connection::activate_bookmark_url(&url)
}

#[tauri::command]
fn migrate_grpc_public_endpoint(
    old_url: String,
    new_url: String,
) -> Result<pirate_desktop::GrpcConnectResult, String> {
    pirate_desktop::migrate_grpc_public_endpoint(&old_url, &new_url)
}

#[tauri::command]
fn rename_server_bookmark(id: String, label: String) -> Result<(), String> {
    pirate_desktop::set_bookmark_label(&id, label)
}

#[tauri::command]
fn save_bookmark_host_agent(
    id: String,
    host_agent_base_url: String,
    host_agent_token: String,
) -> Result<(), String> {
    pirate_desktop::set_bookmark_host_agent(&id, &host_agent_base_url, &host_agent_token)
}

#[tauri::command]
fn host_agent_health_json(base_url: String) -> Result<String, String> {
    pirate_desktop::host_agent_health_json(&base_url)
}

#[tauri::command]
fn host_agent_status_json(base_url: String, token: String) -> Result<String, String> {
    pirate_desktop::host_agent_status_json(&base_url, &token)
}

#[tauri::command]
fn host_agent_reboot_json(
    base_url: String,
    token: String,
    delay_sec: u64,
) -> Result<String, String> {
    pirate_desktop::host_agent_reboot_json(&base_url, &token, delay_sec, None)
}

#[tauri::command]
fn host_agent_upload_server_stack_cmd(
    base_url: String,
    token: String,
    path: String,
    version: String,
) -> Result<String, String> {
    pirate_desktop::host_agent_upload_server_stack(
        &base_url,
        &token,
        std::path::Path::new(&path),
        &version,
    )
}

#[tauri::command]
fn monitoring_api_base() -> Option<String> {
    pirate_desktop::monitoring_api_base()
}

#[tauri::command]
fn monitoring_set_economy(enabled: bool) -> bool {
    pirate_desktop::monitoring_set_economy_mode(enabled)
}

#[tauri::command]
fn start_display_ingest(token: Option<String>) -> Result<u16, String> {
    pirate_desktop::spawn_display_ingest_server(token).map_err(|e| e.to_string())
}

#[tauri::command]
fn display_ingest_base() -> Option<String> {
    pirate_desktop::display_ingest_api_base()
}

/// `data:application/json;base64,...` for consumer role (paste / share with producer host).
#[tauri::command]
fn display_ingest_export_consumer_config(token: Option<String>) -> Result<String, String> {
    let base = pirate_desktop::display_ingest_api_base()
        .ok_or_else(|| "start display ingest first".to_string())?;
    let url = format!("{}/ingest", base.trim_end_matches('/'));
    let mut cfg = deploy_core::display_stream::DisplayStreamConfig::example_consumer(&url);
    if let Some(t) = token.filter(|s| !s.trim().is_empty()) {
        cfg.token = t;
    }
    cfg.to_data_url().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_display_stream_prefs() -> [bool; 2] {
    let (a, b) = pirate_desktop::get_display_stream_prefs();
    [a, b]
}

#[tauri::command]
fn set_display_stream_prefs(allow_receive: bool, allow_send: bool) -> Result<(), String> {
    pirate_desktop::set_display_stream_prefs(allow_receive, allow_send)
}

#[tauri::command]
fn internet_proxy_start(listen: Option<String>) -> Result<(), String> {
    pirate_desktop::internet_proxy_start(listen)
}

#[tauri::command]
fn internet_proxy_stop() -> Result<(), String> {
    pirate_desktop::internet_proxy_stop()
}

#[tauri::command]
fn internet_proxy_status() -> pirate_desktop::InternetProxyStatus {
    pirate_desktop::internet_proxy_status()
}

#[tauri::command]
fn internet_proxy_logs() -> Vec<pirate_desktop::ProxyTraceEntry> {
    pirate_desktop::internet_proxy_logs()
}

#[tauri::command]
fn internet_proxy_logs_clear() {
    pirate_desktop::internet_proxy_logs_clear();
}

#[tauri::command]
fn load_client_settings_json() -> Result<String, String> {
    pirate_desktop::load_settings_json()
}

#[tauri::command]
fn save_client_settings_json(text: String) -> Result<(), String> {
    pirate_desktop::save_settings_json(&text)
}

#[tauri::command]
fn apply_default_rules_preset_cmd(preset: String) -> Result<(), String> {
    pirate_desktop::apply_default_rules_preset_to_disk(&preset)
}

#[tauri::command]
fn load_default_rules_bundles_form() -> Result<pirate_desktop::DefaultRulesBundlesForm, String> {
    pirate_desktop::load_default_rules_bundles_form()
}

#[tauri::command]
fn save_default_rules_bundles_form(
    form: pirate_desktop::DefaultRulesBundlesForm,
) -> Result<(), String> {
    pirate_desktop::save_default_rules_bundles_form(form)
}

#[tauri::command]
fn load_board_rules_form() -> Result<pirate_desktop::BoardRulesForm, String> {
    pirate_desktop::load_board_rules_form()
}

#[tauri::command]
fn save_board_rules_form(form: pirate_desktop::BoardRulesForm) -> Result<(), String> {
    pirate_desktop::save_board_rules_form(form)
}

#[tauri::command]
fn pick_server_stack_tar_gz() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        // Native dialogs usually filter by final extension, so include "gz" too.
        .add_filter("Tarball", &["tar", "tgz", "gz"])
        .pick_file()
        .map(|p| p.to_string_lossy().to_string()))
}

#[tauri::command]
fn fetch_server_stack_info_cmd() -> Result<String, String> {
    pirate_desktop::fetch_server_stack_info_json()
}

#[tauri::command]
fn paas_init_project(path: String, name: Option<String>) -> Result<String, String> {
    pirate_desktop::run_init_project(PathBuf::from(path), name)
}

#[tauri::command]
fn paas_scan_project(path: String, dry_run: bool) -> Result<String, String> {
    let r = pirate_desktop::run_scan_project(PathBuf::from(path), dry_run)?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn project_cmd_placeholders(path: String, phases: Vec<String>) -> Result<String, String> {
    let phase_refs: Vec<&str> = phases.iter().map(String::as_str).collect();
    let list = pirate_desktop::project_cmd_placeholders(&PathBuf::from(path), &phase_refs)?;
    serde_json::to_string(&list).map_err(|e| e.to_string())
}

#[tauri::command]
fn paas_project_build(
    path: String,
    cmd_vars: Option<std::collections::HashMap<String, String>>,
) -> Result<String, String> {
    let vars = pirate_desktop::cmd_vars_map_from_json(cmd_vars);
    let r = pirate_desktop::run_project_build(PathBuf::from(path), vars)?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn paas_project_test(
    path: String,
    cmd_vars: Option<std::collections::HashMap<String, String>>,
) -> Result<String, String> {
    let vars = pirate_desktop::cmd_vars_map_from_json(cmd_vars);
    let r = pirate_desktop::run_project_test(PathBuf::from(path), vars)?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn paas_test_local(path: String, image: Option<String>) -> Result<String, String> {
    let r = pirate_desktop::run_test_local(
        PathBuf::from(path),
        image.unwrap_or_else(|| "pirate-local-test".to_string()),
    )?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
fn paas_apply_gen(path: String) -> Result<(), String> {
    pirate_desktop::run_apply_gen(PathBuf::from(path))
}

#[tauri::command]
fn paas_pipeline(
    path: String,
    do_init: bool,
    name: Option<String>,
    skip_test_local: bool,
    version: Option<String>,
    chunk_size: Option<u32>,
    cmd_vars: Option<std::collections::HashMap<String, String>>,
) -> Result<String, String> {
    let vars = pirate_desktop::cmd_vars_map_from_json(cmd_vars);
    let r = pirate_desktop::run_pipeline(
        PathBuf::from(path),
        do_init,
        name,
        skip_test_local,
        version,
        chunk_size.unwrap_or(64 * 1024) as usize,
        vars,
    )?;
    serde_json::to_string(&r).map_err(|e| e.to_string())
}

#[tauri::command]
async fn apply_server_stack_update(
    app: tauri::AppHandle,
    path: String,
    version: String,
    chunk_size: Option<u32>,
) -> Result<pirate_desktop::ServerStackOutcome, String> {
    let chunk = chunk_size.unwrap_or(64 * 1024) as usize;
    let path = PathBuf::from(path);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        pirate_desktop::run_server_stack_update_with_progress(
            path,
            version,
            chunk,
            move |sent, total| {
                let _ = app.emit(
                    "server_stack_upload_progress",
                    serde_json::json!({ "sent": sent, "total": total }),
                );
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    let _guard = init_tracing();
    pirate_desktop::ensure_unified_client_config_migrated();
    if let Err(e) = pirate_desktop::spawn_monitoring_server() {
        tracing::warn!(%e, "monitoring HTTP server not started");
    } else {
        tracing::info!(
            base = ?pirate_desktop::monitoring_api_base(),
            "monitoring API"
        );
    }
    tauri::Builder::default()
        .setup(|app| {
            if let Err(e) = silent_sync_user_cli(&app.handle()) {
                tracing::warn!(error = %e, "silent_sync_user_cli");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            is_pirate_cli_available,
            pirate_cli_path_info,
            install_pirate_cli,
            get_status,                                      // app status
            parse_grpc_bundle,                               // parse install JSON from bundle
            connect_grpc_bundle,                             // connect from bundle
            get_saved_grpc_endpoint,                         // saved gRPC endpoint
            clear_grpc_connection,                           // clear saved gRPC endpoint
            test_grpc_endpoint,                              // test gRPC endpoint
            refresh_grpc_status,                             // refresh gRPC endpoint status
            get_control_api_base,                            // control-api base URL
            set_control_api_base,                            // set control-api base URL
            mark_control_api_recent_restart, // mark restart window for diagnostics/retry
            control_api_recent_restart_hint, // true while restart window is active
            get_active_project,              // active project ID
            set_active_project,              // set active project ID
            pick_deploy_directory,           // pick deploy directory
            deploy_from_directory,           // deploy from directory
            rollback_deploy,                 // rollback deploy
            read_release_version_from_manifest, // read [project].version from pirate.toml
            check_project_uploaded,          // project deployed status for selected path
            remove_server_project, // remove project on server (stop process + delete files + db rows)
            analyze_network_access, // local detect services + nginx preview from manifest
            validate_network_access, // server-side deploy validation blockers/warnings
            save_project_network_manifest,
            load_project_network_manifest,
            read_project_local_env,   // read env file path from pirate.toml and return content
            write_project_local_env,  // write content to env file path from pirate.toml
            control_api_apply_project_nginx,
            apply_manifest_fix,                  // preflight auto-fix pirate.toml
            projects_preflight,    // projects preflight
            list_registered_projects, // list registered projects
            register_project_from_directory, // register project from directory
            remove_registered_project, // remove registered project
            local_dev_start,       // local stack: compose + start.cmd
            local_dev_stop,        // local stack: compose + stop.cmd
            local_dev_status,      // local stack: status
            probe_local_toolchain, // local CLI toolchain probe
            control_api_login,     // control-api JWT login
            control_api_keychain_save, // OS keychain: save control-api credentials
            control_api_keychain_load, // OS keychain: load control-api credentials
            control_api_keychain_delete, // OS keychain: remove control-api credentials
            control_api_health_probe, // quick GET /health probe
            control_api_logout,    // clear control-api JWT
            control_api_session_active, // JWT present and not expired (for UI)
            control_api_bearer_token, // JWT for WebSocket access_token (host terminal)
            control_api_fetch_status_json, // GET /api/v1/status (JWT)
            control_api_fetch_project_telemetry_json, // GET /api/v1/projects/telemetry (JWT)
            control_api_clear_project_runtime_log, // POST /api/v1/projects/telemetry/clear (JWT)
            control_api_fetch_app_env_json, // GET /api/v1/app-env (JWT)
            control_api_put_app_env, // PUT /api/v1/app-env (JWT)
            control_api_fetch_host_deploy_env_json, // GET /api/v1/host-deploy-env (JWT)
            control_api_put_host_deploy_env, // PUT /api/v1/host-deploy-env (JWT)
            control_api_fetch_host_deploy_env_template_json, // GET reference env.example (JWT)
            control_api_fetch_nginx_status_json, // GET /api/v1/nginx/status (JWT)
            control_api_fetch_host_services_json, // GET /api/v1/host-services (JWT)
            control_api_host_service_install, // POST /api/v1/host-services/:id/install (JWT)
            control_api_host_service_remove, // POST /api/v1/host-services/:id/remove (JWT)
            control_api_host_service_runtime_get_json, // GET /api/v1/host-services/:id/runtime-config (JWT)
            control_api_host_service_runtime_put_json, // PUT /api/v1/host-services/:id/runtime-config (JWT)
            control_api_host_service_restart, // POST /api/v1/host-services/:id/restart (JWT)
            control_api_host_databases_list_json,
            control_api_host_db_schemas_json,
            control_api_host_db_tables_json,
            control_api_host_db_columns_json,
            control_api_host_db_rows_json,
            control_api_host_db_relationships_json,
            control_api_host_db_query_json,
            control_api_host_db_redis_keys_json,
            control_api_host_db_mongo_databases_json,
            control_api_host_db_mongo_collections_json,
            control_api_host_db_mongo_preview_json,
            control_api_host_db_v2_capabilities_json,
            control_api_host_db_v2_object_tree_json,
            control_api_host_db_v2_grid_json,
            control_api_host_db_v2_row_mutate_json,
            control_api_host_db_v2_sql_job_start_json,
            control_api_host_db_v2_sql_job_get_json,
            control_api_host_db_v2_sql_job_cancel_json,
            control_api_host_db_v2_migration_status_get_json,
            control_api_host_db_v2_migration_status_post_json,
            control_api_host_db_v2_admin_create_database_json,
            control_api_host_db_v2_admin_create_table_json,
            control_api_host_db_v2_admin_create_user_json,
            control_api_host_db_v2_admin_delete_user_json,
            control_api_host_db_v2_migration_run_json,
            db_credentials_get_json,
            db_credentials_save,
            db_credentials_forget,
            db_local_forward_start,
            db_local_forward_stop,
            db_local_forward_local_port,
            db_tunnel_list_json,
            db_tunnel_tcp_start,
            db_tunnel_tcp_stop,
            db_tunnel_ssh_start,
            db_tunnel_ssh_stop,
            stack_tun_health,
            stack_tun_get_config,
            stack_tun_put_config,
            stack_tun_reload_peers,
            stack_tun_stats,
            stack_tun_identity_public_key,
            stack_tun_authorize_peer,
            stack_tun_list_peers,
            stack_tun_get_routes,
            stack_tun_put_routes,
            stack_tun_requests_json,
            stack_tun_request_bus_invoke,
            db_direct_test,
            db_direct_open,
            db_direct_close,
            db_direct_list_databases,
            db_direct_list_schemas,
            db_direct_list_tables,
            db_direct_table_preview,
            db_direct_query,
            db_direct_heartbeat,
            db_direct_pg_stats,
            db_direct_pg_structure,
            db_direct_profile_list_json,
            db_direct_profile_upsert,
            db_direct_profile_delete,
            db_direct_password_set,
            db_direct_query_history_list,
            control_api_fetch_nginx_site_json, // GET /api/v1/nginx/site (JWT)
            control_api_put_nginx_site,        // PUT /api/v1/nginx/site (JWT)
            control_api_fetch_nginx_file_json, // GET /api/v1/nginx/file (JWT)
            control_api_put_nginx_file_json,   // PUT /api/v1/nginx/file (JWT)
            control_api_storage_tree_json,
            control_api_storage_usage_json,
            control_api_storage_create_folder,
            control_api_storage_delete_file,
            control_api_storage_delete_folder,
            control_api_storage_rename,
            control_api_storage_upload_file,
            control_api_storage_download_file,
            control_api_storage_extract_json,
            control_api_storage_bind_sources_json,
            control_api_storage_bind_json,
            control_api_storage_unbind_json,
            pick_file_for_storage_upload,
            pick_files_for_storage_upload,
            pick_save_path_for_storage_download,
            control_api_ensure_nginx, // POST /api/v1/nginx/ensure (JWT)
            control_api_fetch_nginx_sites_json, // GET /api/v1/nginx/sites (JWT)
            control_api_nginx_preflight_json, // POST /api/v1/nginx/preflight (JWT)
            control_api_nginx_action_json, // POST /api/v1/nginx/action (JWT)
            control_api_restart_process_json, // POST /api/v1/process/restart (JWT)
            control_api_stop_process_json, // POST /api/v1/process/stop (JWT)
            control_api_fetch_process_listeners_json,
            control_api_kill_process_listener_json,
            control_api_antiddos_get_json,
            control_api_antiddos_put_json,
            control_api_antiddos_enable,
            control_api_antiddos_disable,
            control_api_antiddos_apply,
            control_api_antiddos_stats_json,
            control_api_antiddos_project_put_json,
            control_api_antiddos_project_delete,
            fetch_server_projects_overview, // projects list + per-project status
            desktop_perf_snapshot,          // baseline metrics JSON (samples count, HTTP pool)
            ensure_deploy_project_id_for_deploy, // resolve deploy slot (default vs allocate) before deploy
            open_project_folder,                 // reveal project folder in file manager
            deploy_upload_cancel,                // cancel deploy upload
            server_stack_upload_cancel,          // cancel server stack upload
            list_server_bookmarks,               // list server bookmarks
            delete_server_bookmark,              // delete server bookmark
            add_server_bookmark,                 // add server bookmark
            activate_server_bookmark,            // activate server bookmark
            migrate_grpc_public_endpoint,        // move saved gRPC URL + pairing to new public URL
            rename_server_bookmark,              // rename server bookmark
            save_bookmark_host_agent,            // out-of-band host-agent URL + token
            host_agent_health_json,
            host_agent_status_json,
            host_agent_reboot_json,
            host_agent_upload_server_stack_cmd,
            monitoring_api_base,                   // monitoring API base URL
            monitoring_set_economy,                // set monitoring economy mode
            start_display_ingest,                  // start display ingest
            display_ingest_base,                   // display ingest base URL
            display_ingest_export_consumer_config, // display ingest export consumer config
            get_display_stream_prefs,              // get display stream prefs
            set_display_stream_prefs,              // set display stream prefs
            internet_proxy_start,                  // start internet proxy
            internet_proxy_stop,                   // stop internet proxy
            internet_proxy_status,                 // internet proxy status
            internet_proxy_logs,                   // internet proxy logs
            internet_proxy_logs_clear,             // internet proxy logs clear
            load_client_settings_json,             // load client settings
            save_client_settings_json,             // save client settings
            apply_default_rules_preset_cmd,        // apply default rules preset
            load_default_rules_bundles_form,       // load default rules bundles form
            save_default_rules_bundles_form,       // save default rules bundles form
            load_board_rules_form,                 // load board rules form
            save_board_rules_form,                 // save board rules form
            fetch_remote_host_stats,               // fetch remote host stats
            fetch_remote_host_stats_detail,        // fetch remote host stats detail
            fetch_remote_host_stats_series,        // fetch remote host stats series
            ssl_status_json,                       // gRPC SslStatus (JSON)
            ssl_create,                            // gRPC SslCreate
            ssl_update,                            // gRPC SslUpdate
            ssl_check_and_renew,                   // gRPC SslCheckAndRenew
            pick_server_stack_tar_gz,              // pick server stack tar.gz
            fetch_server_stack_info_cmd,           // fetch server stack info
            apply_server_stack_update,             // apply server stack update
            paas_init_project,                     // paas init project
            paas_scan_project,                     // paas scan project
            project_cmd_placeholders,              // ${VAR} placeholders in pirate.toml cmds
            paas_project_build,                    // paas project build
            paas_project_test,                     // paas project test
            paas_test_local,                       // paas test local
            paas_apply_gen,                        // paas apply gen
            paas_pipeline,                         // paas pipeline
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
