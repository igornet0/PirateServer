//! Inspect a server-stack bundle (directory or `.tar.gz`) without loading the whole archive twice.

use flate2::read::GzDecoder;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// UI / manifest signals from a local bundle (same rules as `install.sh` / server unpack).
#[derive(Debug, Clone)]
pub struct BundleProfile {
    pub has_ui_static: bool,
    pub has_bundle_no_ui_marker: bool,
    pub manifest_json: Option<String>,
}

impl BundleProfile {
    /// Effective UI static for OTA transitions (marker forbids UI even if files exist).
    pub fn effective_has_ui(&self) -> bool {
        self.has_ui_static && !self.has_bundle_no_ui_marker
    }
}

fn bundle_root_has_server_bins(dir: &Path) -> bool {
    dir.join("bin/deploy-server").is_file() && dir.join("bin/control-api").is_file()
}

fn find_bundle_root_in_dir(dir: &Path) -> Option<PathBuf> {
    for name in ["pirate-linux-amd64", "pirate-linux-aarch64"] {
        let d = dir.join(name);
        if bundle_root_has_server_bins(&d) {
            return Some(d);
        }
    }
    if bundle_root_has_server_bins(dir) {
        return Some(dir.to_path_buf());
    }
    // Fallback: any single subdirectory with server binaries (matches deploy-server unpack rules).
    let mut hits: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() && bundle_root_has_server_bins(&p) {
                hits.push(p);
            }
        }
    }
    if hits.len() == 1 {
        return hits.pop();
    }
    None
}

/// Inspect an unpacked bundle directory (same layout as `build-linux-bundle.sh` output).
pub fn inspect_bundle_directory(dir: &Path) -> Option<BundleProfile> {
    let root = find_bundle_root_in_dir(dir)?;
    let marker = root.join(".bundle-no-ui");
    let index = root.join("share/ui/dist/index.html");
    let manifest_path = root.join("server-stack-manifest.json");
    let manifest_json = std::fs::read_to_string(&manifest_path).ok();
    Some(BundleProfile {
        has_ui_static: index.is_file(),
        has_bundle_no_ui_marker: marker.exists(),
        manifest_json,
    })
}

/// Inspect a `.tar.gz` / `.tgz` by streaming headers only (then reads small manifest file).
pub fn inspect_bundle_tar_gz(path: &Path) -> std::io::Result<BundleProfile> {
    let f = File::open(path)?;
    let dec = GzDecoder::new(f);
    let mut archive = tar::Archive::new(dec);
    let mut has_ui_static = false;
    let mut has_bundle_no_ui_marker = false;
    let mut manifest_json: Option<String> = None;

    for ent in archive.entries()? {
        let mut ent = ent?;
        let pth = ent.path()?;
        let s = pth.to_string_lossy();
        if s.ends_with("/share/ui/dist/index.html") || s == "share/ui/dist/index.html" {
            has_ui_static = true;
        }
        if s.ends_with("/.bundle-no-ui") || s == ".bundle-no-ui" {
            has_bundle_no_ui_marker = true;
        }
        if s.ends_with("/server-stack-manifest.json") || s == "server-stack-manifest.json" {
            let size = ent.size();
            if size <= 256 * 1024 {
                let mut buf = Vec::new();
                ent.read_to_end(&mut buf)?;
                manifest_json = String::from_utf8(buf).ok();
            }
        }
    }

    Ok(BundleProfile {
        has_ui_static,
        has_bundle_no_ui_marker,
        manifest_json,
    })
}

/// Directory or `.tar.gz` / `.tgz`.
pub fn inspect_bundle_path(path: &Path) -> std::io::Result<BundleProfile> {
    if path.is_dir() {
        return inspect_bundle_directory(path).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "directory is not a server-stack bundle (missing bin/deploy-server)",
            )
        });
    }
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        return inspect_bundle_tar_gz(path);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "bundle file must be .tar.gz or .tgz",
    ))
}
