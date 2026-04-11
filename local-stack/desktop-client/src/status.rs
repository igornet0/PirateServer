use crate::hosts;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AppStatus {
    pub hostname: String,
    pub hosts_entry_ok: bool,
    pub shell: &'static str,
}

pub fn app_status() -> AppStatus {
    AppStatus {
        hostname: hosts::HOSTNAME.to_string(),
        hosts_entry_ok: hosts::hosts_mapping_present(),
        shell: "tauri",
    }
}
