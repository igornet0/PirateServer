//! Pure-Rust checks for `deploy_client` packing / validation (no running gRPC server).

use deploy_client::{
    build_chunks, build_server_stack_chunks, default_version, pack_directory, pack_release_sources,
    read_or_pack_bundle,
    validate_version_label,
};
use sha2::Digest;
use std::fs;

#[test]
fn validate_version_accepts_common_labels() {
    validate_version_label("v-e2e-1").unwrap();
    validate_version_label("pirate-linux-aarch64-no-ui-0.1.0-20260412").unwrap();
}

#[test]
fn validate_version_rejects_bad_chars() {
    assert!(validate_version_label("bad version").is_err());
    assert!(validate_version_label("../x").is_err());
}

#[test]
fn default_version_is_non_empty() {
    let v = default_version();
    assert!(v.starts_with("v-"));
    validate_version_label(&v).unwrap();
}

#[test]
fn pack_directory_then_read_tar_gz_roundtrip() {
    let pid = std::process::id();
    let root = std::env::temp_dir().join(format!("deploy-client-ops-{pid}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/a.txt"), b"hello").unwrap();

    let packed = pack_directory(&root).unwrap();
    assert!(!packed.is_empty());

    let tgz = std::env::temp_dir().join(format!("deploy-client-ops-{pid}.tar.gz"));
    let _ = fs::remove_file(&tgz);
    fs::write(&tgz, &packed).unwrap();

    let read_back = read_or_pack_bundle(&tgz).unwrap();
    assert_eq!(read_back, packed);

    let _ = fs::remove_file(&tgz);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn build_chunks_single_non_empty_artifact() {
    let data = vec![0u8; 10_000];
    let sha = hex::encode(sha2::Sha256::digest(&data));
    let chunks = build_chunks(&data, "v1", "default", &sha, 4096);
    assert!(chunks.len() >= 2);
    assert!(chunks.last().unwrap().is_last);
    assert_eq!(chunks.last().unwrap().sha256_hex, sha);
}

#[test]
fn build_server_stack_chunks_matches_deploy_shape() {
    let data = vec![1u8; 100];
    let sha = hex::encode(sha2::Sha256::digest(&data));
    let chunks = build_server_stack_chunks(&data, "stack-v1", &sha, 40, None);
    assert!(!chunks.is_empty());
    assert!(chunks.last().unwrap().is_last);
    assert_eq!(chunks.first().unwrap().version, "stack-v1");
}

#[test]
fn read_or_pack_bundle_rejects_non_tar_file() {
    let pid = std::process::id();
    let p = std::env::temp_dir().join(format!("deploy-client-not-tar-{pid}.bin"));
    fs::write(&p, b"x").unwrap();
    assert!(read_or_pack_bundle(&p).is_err());
    let _ = fs::remove_file(&p);
}

#[test]
fn pack_release_sources_uses_selected_outputs_only() {
    let pid = std::process::id();
    let root = std::env::temp_dir().join(format!("deploy-client-release-select-{pid}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("dist")).unwrap();
    fs::write(root.join("dist/app.js"), b"ok").unwrap();
    fs::write(root.join("README.md"), b"skip").unwrap();

    let packed = pack_release_sources(&root, &["dist".to_string()], None).unwrap();
    let out = std::env::temp_dir().join(format!("deploy-client-release-select-{pid}.tar.gz"));
    let _ = fs::remove_file(&out);
    fs::write(&out, packed).unwrap();
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(&out).unwrap()));
    let mut names = Vec::<String>::new();
    for e in ar.entries().unwrap() {
        let e = e.unwrap();
        names.push(e.path().unwrap().to_string_lossy().to_string());
    }
    assert!(names.iter().any(|n| n == "dist/app.js"));
    assert!(!names.iter().any(|n| n == "README.md"));
    let _ = fs::remove_file(&out);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn pack_release_sources_blocks_path_escape() {
    let pid = std::process::id();
    let root = std::env::temp_dir().join(format!("deploy-client-release-escape-{pid}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let err = pack_release_sources(&root, &["../".to_string()], None).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn pack_directory_applies_pirateignore() {
    let pid = std::process::id();
    let root = std::env::temp_dir().join(format!("deploy-client-release-ignore-{pid}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("dist")).unwrap();
    fs::write(root.join("dist/app.js"), b"ok").unwrap();
    fs::write(root.join("secret.txt"), b"nope").unwrap();
    fs::write(root.join(".pirateignore"), "secret.txt\n").unwrap();

    let packed = pack_directory(&root).unwrap();
    let out = std::env::temp_dir().join(format!("deploy-client-release-ignore-{pid}.tar.gz"));
    let _ = fs::remove_file(&out);
    fs::write(&out, packed).unwrap();
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(&out).unwrap()));
    let mut names = Vec::<String>::new();
    for e in ar.entries().unwrap() {
        let e = e.unwrap();
        names.push(e.path().unwrap().to_string_lossy().to_string());
    }
    assert!(!names.iter().any(|n| n == "secret.txt"));
    assert!(names.iter().any(|n| n == "dist/app.js"));
    let _ = fs::remove_file(&out);
    let _ = fs::remove_dir_all(&root);
}
