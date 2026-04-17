//! PATH augmentation for GUI-launched subprocesses (Tauri).

/// GUI/Tauri often inherits a minimal PATH — prepend common locations for `npm` / `node` / dev tools.
pub fn path_for_dev_shell() -> String {
    #[cfg(windows)]
    {
        let base = std::env::var("PATH").unwrap_or_default();
        let extra = r"C:\Program Files\nodejs;C:\Program Files (x86)\nodejs";
        format!("{extra};{base}")
    }
    #[cfg(unix)]
    {
        let base = std::env::var("PATH").unwrap_or_default();
        let extra = "/usr/local/bin:/opt/homebrew/bin:/opt/homebrew/sbin";
        format!("{extra}:{base}")
    }
}
