//! Regression: `DEPLOY_ALLOW_SERVER_STACK_UPDATE=1` must not break clap parse (bool env values are only `true`/`false` in clap 4).

use std::process::Command;

fn deploy_server_exe() -> &'static str {
    option_env!("CARGO_BIN_EXE_deploy-server").expect(
        "integration tests require the binary; run: cargo test -p deploy-server --tests",
    )
}

#[test]
fn deploy_server_help_ok_when_env_is_numeric_one() {
    let out = Command::new(deploy_server_exe())
        .env("DEPLOY_ALLOW_SERVER_STACK_UPDATE", "1")
        .arg("--help")
        .output()
        .expect("spawn deploy-server --help");
    assert!(
        out.status.success(),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn deploy_server_help_ok_when_env_is_true() {
    let out = Command::new(deploy_server_exe())
        .env("DEPLOY_ALLOW_SERVER_STACK_UPDATE", "true")
        .arg("--help")
        .output()
        .expect("spawn deploy-server --help");
    assert!(out.status.success());
}
