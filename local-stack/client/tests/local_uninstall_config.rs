//! Linux: `dirs` respects `XDG_CONFIG_HOME` for `remove_local_client_config`.

#[cfg(target_os = "linux")]
#[test]
fn remove_local_client_config_respects_xdg_config_home() {
    use deploy_client::local_uninstall::remove_local_client_config;
    use std::fs;

    let tmp = std::env::temp_dir().join(format!("pirate-xdg-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let prev = std::env::var("XDG_CONFIG_HOME").ok();
    std::env::set_var("XDG_CONFIG_HOME", &tmp);
    let cfg = tmp.join("pirate-client");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(cfg.join("connection.json"), "{}").unwrap();
    remove_local_client_config().unwrap();
    assert!(!cfg.exists());
    match prev {
        Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
}
