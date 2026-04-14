//! Local PaaS helpers: init/scan/build/test/docker (reuse `deploy-client`).

use deploy_client::{
    apply_generated_files, default_version, deploy_directory, init_project, run_build, run_test,
    scan_project, test_local_docker, validate_version_label, ScanReport, StepResult,
};
use serde::Serialize;
use std::path::PathBuf;

use crate::connection::{load_endpoint, load_project_id, load_signing_key_for_endpoint};
use crate::deploy::runtime;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineOutcome {
    pub init_path: Option<String>,
    pub scan: Option<ScanReport>,
    pub build: Option<StepResult>,
    pub test: Option<StepResult>,
    pub test_local: Option<StepResult>,
    pub deploy: Option<DeploySummaryOutcome>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploySummaryOutcome {
    pub status: String,
    pub deployed_version: String,
    pub artifact_bytes: u64,
    pub chunk_count: usize,
}

pub fn run_init_project(path: PathBuf, name: Option<String>) -> Result<String, String> {
    init_project(&path, name.as_deref()).map(|p| p.display().to_string())
}

pub fn run_scan_project(path: PathBuf, dry_run: bool) -> Result<ScanReport, String> {
    scan_project(&path, dry_run)
}

pub fn run_project_build(path: PathBuf) -> Result<StepResult, String> {
    run_build(&path)
}

pub fn run_project_test(path: PathBuf) -> Result<StepResult, String> {
    run_test(&path)
}

pub fn run_test_local(path: PathBuf, image: String) -> Result<StepResult, String> {
    test_local_docker(&path, &image)
}

pub fn run_apply_gen(path: PathBuf) -> Result<(), String> {
    apply_generated_files(&path)
}

/// Full pipeline: optional init, scan, build, test, optional test-local, deploy.
pub fn run_pipeline(
    path: PathBuf,
    do_init: bool,
    name: Option<String>,
    skip_test_local: bool,
    version: Option<String>,
    chunk_size: usize,
) -> Result<PipelineOutcome, String> {
    let mut out = PipelineOutcome {
        init_path: None,
        scan: None,
        build: None,
        test: None,
        test_local: None,
        deploy: None,
    };
    if do_init {
        let p = init_project(&path, name.as_deref())?;
        out.init_path = Some(p.display().to_string());
    }
    out.scan = Some(scan_project(&path, false)?);
    let b = run_build(&path)?;
    if !b.ok {
        out.build = Some(b);
        return Ok(out);
    }
    out.build = Some(b);
    let t = run_test(&path)?;
    if !t.ok {
        out.test = Some(t);
        return Ok(out);
    }
    out.test = Some(t);
    if !skip_test_local {
        out.test_local = Some(test_local_docker(&path, "pirate-pipeline-local")?);
    }
    let ver = version.unwrap_or_else(default_version);
    validate_version_label(&ver).map_err(|e| e.to_string())?;
    let endpoint = load_endpoint().ok_or_else(|| "no saved connection; connect first".to_string())?;
    let sk = load_signing_key_for_endpoint(&endpoint)?;
    let project = load_project_id();
    let rt = runtime()?;
    let resp = rt.block_on(deploy_directory(
        &endpoint,
        &path,
        &ver,
        &project,
        chunk_size,
        sk.as_ref(),
    ))?;
    out.deploy = Some(DeploySummaryOutcome {
        status: resp.response.status,
        deployed_version: resp.response.deployed_version,
        artifact_bytes: resp.artifact_bytes,
        chunk_count: resp.chunk_count,
    });
    Ok(out)
}
