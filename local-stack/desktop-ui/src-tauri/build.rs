//! Stage `pirate` CLI (deploy-client) into `bundled/cli/pirate` for Tauri `bundle.resources`.
//! Run `cargo build -p deploy-client --bin pirate` with the same profile/target before `tauri build`.

use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let bundled = manifest_dir.join("bundled/cli/pirate");
    if let Some(parent) = bundled.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    if let Some(src) = find_pirate_binary() {
        match std::fs::copy(&src, &bundled) {
            Ok(_) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(m) = std::fs::metadata(&bundled) {
                        let mut p = m.permissions();
                        p.set_mode(0o755);
                        let _ = std::fs::set_permissions(&bundled, p);
                    }
                }
                println!("cargo:rerun-if-changed={}", src.display());
            }
            Err(e) => println!("cargo:warning=could not copy pirate CLI: {e}"),
        }
    } else {
        println!("cargo:warning=pirate CLI not in target dir; run: cargo build -p deploy-client --bin pirate (same PROFILE/target).");
        if profile == "release" {
            eprintln!(
                "error: deploy-client `pirate` binary missing before pirate-client release build. \
Run from repo root: cargo build -p deploy-client --bin pirate --release \
(with the same --target as tauri build, if any). \
Or use: make dist-client-* / scripts/build-desktop-client-dist.sh (builds pirate first)."
            );
            std::process::exit(1);
        }
        println!("cargo:warning=debug build: using empty stub for bundled/cli/pirate (install CLI in app will fail until you build deploy-client).");
        if !bundled.exists() {
            let _ = std::fs::write(&bundled, []);
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
    tauri_build::build();
}

fn find_pirate_binary() -> Option<PathBuf> {
    let target_dir = std::env::var("CARGO_TARGET_DIR").ok()?;
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    // Cargo sets `TARGET` for build scripts (artifact triple). `CARGO_BUILD_TARGET` is often unset here.
    let triple = std::env::var("TARGET")
        .or_else(|_| std::env::var("CARGO_BUILD_TARGET"))
        .unwrap_or_default();

    let base = Path::new(&target_dir);
    let with_triple = |root: &Path| -> PathBuf {
        if triple.is_empty() {
            root.join(&profile)
        } else {
            root.join(&triple).join(&profile)
        }
    };

    let dir = with_triple(base);
    // Use the *artifact* target triple, not cfg(windows): when cross-compiling from macOS/Linux
    // the host is Unix but the binary is still pirate.exe under .../x86_64-pc-windows-msvc/release/.
    let name = if triple.contains("windows") {
        "pirate.exe"
    } else {
        "pirate"
    };

    let p = dir.join(name);
    if p.is_file() {
        return Some(p);
    }
    None
}
