//! Local GUI / desktop session heuristics for `pirate gui-check` (aligned with install.sh probe).
//!
//! **Why `monitor_count` can be 0 on a Linux server:** `xcap` lists monitors through the
//! OS capture API (X11/Wayland session). Over SSH, under `systemd`, or during `sudo ./install.sh`,
//! there is often no `DISPLAY` / compositor, so enumeration returns nothing even if a cable is
//! plugged in. On Linux we also read **`/sys/class/drm/card*-*/status`** (`drm_connectors_connected`):
//! counts physical DRM connectors reporting `connected` without a GUI session (still not capture
//! until a client runs in a logged-in desktop with Screen Recording / permissions).

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct GuiProbeResult {
    pub gui_detected: bool,
    pub reasons: Vec<String>,
    /// When `xcap` can enumerate monitors (may be 0 on headless even if session exists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_count: Option<u32>,
    /// Linux: DRM sysfs connectors with `status==connected` (works without X11/Wayland; SSH/install).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drm_connectors_connected: Option<u32>,
}

impl GuiProbeResult {
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// Heuristic probe for the current machine (typically the deploy host).
pub fn probe_local() -> GuiProbeResult {
    #[cfg(target_os = "linux")]
    {
        probe_linux()
    }
    #[cfg(target_os = "macos")]
    {
        probe_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        GuiProbeResult {
            gui_detected: false,
            reasons: vec!["unsupported_os".to_string()],
            monitor_count: None,
            drm_connectors_connected: None,
        }
    }
}

/// Count `/sys/class/drm/card*-*/status` with body `connected` (no X11 session required).
#[cfg(target_os = "linux")]
fn drm_sysfs_connected_count_linux() -> u32 {
    let Ok(rd) = std::fs::read_dir("/sys/class/drm") else {
        return 0;
    };
    let mut n = 0u32;
    for ent in rd.flatten() {
        let path = ent.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        // Connectors: `card0-HDMI-A-1` — skip bare `card0` (no hyphen).
        if !name.starts_with("card") || !name.contains('-') {
            continue;
        }
        let status = path.join("status");
        if let Ok(s) = std::fs::read_to_string(&status) {
            if s.trim() == "connected" {
                n += 1;
            }
        }
    }
    n
}

#[cfg(target_os = "linux")]
fn probe_linux() -> GuiProbeResult {
    let mut reasons = Vec::new();

    if let Ok(def) = std::process::Command::new("systemctl")
        .args(["get-default"])
        .output()
    {
        if def.status.success() {
            let s = String::from_utf8_lossy(&def.stdout);
            let line = s.lines().next().unwrap_or("").trim();
            if line == "graphical.target" {
                reasons.push("systemd_default_graphical".to_string());
            }
        }
    }

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        reasons.push("wayland_display".to_string());
    }
    if std::env::var_os("DISPLAY").is_some() {
        reasons.push("display_env".to_string());
    }
    if let Ok(st) = std::env::var("XDG_SESSION_TYPE") {
        let st = st.to_lowercase();
        if st == "wayland" || st == "x11" || st == "tty" {
            reasons.push(format!("xdg_session_type_{st}"));
        }
    }

    if let Ok(out) = std::process::Command::new("loginctl")
        .args(["show-session", "self", "-p", "Type", "-p", "Desktop"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            for line in s.lines() {
                if let Some(rest) = line.strip_prefix("Desktop=") {
                    let d = rest.trim();
                    if !d.is_empty() && d != "(null)" {
                        reasons.push("loginctl_desktop_session".to_string());
                        break;
                    }
                }
            }
        }
    }

    let monitor_count = match xcap::Monitor::all() {
        Ok(mons) => Some(mons.len() as u32),
        Err(_) => None,
    };

    if let Some(n) = monitor_count {
        if n > 0 {
            reasons.push("xcap_monitors".to_string());
        }
    }

    let drm_n = drm_sysfs_connected_count_linux();
    let drm_connectors_connected = if drm_n > 0 { Some(drm_n) } else { None };
    if drm_n > 0 {
        reasons.push("drm_sysfs_connected".to_string());
    }

    let mut gui_detected = !reasons.is_empty();
    if !gui_detected {
        gui_detected = monitor_count.is_some_and(|n| n > 0);
    }
    if !gui_detected {
        gui_detected = drm_n > 0;
    }

    GuiProbeResult {
        gui_detected,
        reasons,
        monitor_count,
        drm_connectors_connected,
    }
}

#[cfg(target_os = "macos")]
fn probe_macos() -> GuiProbeResult {
    let mut reasons = Vec::new();
    // CGSessionCopyCurrentDictionary would be ideal; keep a lightweight check.
    if let Ok(out) = std::process::Command::new("/bin/ps")
        .args(["-ax", "-o", "comm="])
        .output()
    {
        let s = String::from_utf8_lossy(&out.stdout);
        if s.lines().any(|l| l.trim() == "WindowServer") {
            reasons.push("windowserver_running".to_string());
        }
    }
    let monitor_count = xcap::Monitor::all().ok().map(|m| m.len() as u32);
    if let Some(n) = monitor_count {
        if n > 0 {
            reasons.push("xcap_monitors".to_string());
        }
    }
    let mut gui_detected = !reasons.is_empty();
    if !gui_detected {
        gui_detected = monitor_count.is_some_and(|n| n > 0);
    }
    GuiProbeResult {
        gui_detected,
        reasons,
        monitor_count,
        drm_connectors_connected: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_serializes() {
        let r = GuiProbeResult {
            gui_detected: false,
            reasons: vec!["test".to_string()],
            monitor_count: None,
            drm_connectors_connected: None,
        };
        let j = r.to_json_line().unwrap();
        assert!(j.contains("gui_detected"));
        assert!(j.contains("test"));
    }
}
