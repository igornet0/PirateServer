//! Pure-Rust checks for `deploy_client` packing / validation (no running gRPC server).

use deploy_client::{
    build_chunks, build_server_stack_chunks, default_version, pack_directory, read_or_pack_bundle,
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
