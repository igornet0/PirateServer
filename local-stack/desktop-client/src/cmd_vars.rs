//! `pirate.toml` command placeholders for desktop UI.

use deploy_core::cmd_template::{merge_cmd_placeholders, CmdPlaceholder};
use deploy_core::pirate_project::PirateManifest;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn project_cmd_placeholders(
    path: &Path,
    phases: &[&str],
) -> Result<Vec<CmdPlaceholder>, String> {
    let manifest_path = path.join("pirate.toml");
    let m = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let mut cmds = Vec::<&str>::new();
    for phase in phases {
        let cmd = match *phase {
            "build" => m.build.cmd.as_str(),
            "test" => m.test.cmd.as_str(),
            "start" => m.start.cmd.as_str(),
            other => {
                return Err(format!(
                    "unknown cmd phase `{other}` (expected build|test|start)"
                ));
            }
        };
        if !cmd.trim().is_empty() {
            cmds.push(cmd);
        }
    }
    Ok(merge_cmd_placeholders(&cmds))
}

pub fn cmd_vars_map_from_json(
    vars: Option<std::collections::HashMap<String, String>>,
) -> Option<BTreeMap<String, String>> {
    vars.map(|m| m.into_iter().collect())
}

pub fn run_project_build(
    path: PathBuf,
    cmd_vars: Option<BTreeMap<String, String>>,
) -> Result<deploy_client::StepResult, String> {
    deploy_client::run_build(&path, cmd_vars.as_ref())
}

pub fn run_project_test(
    path: PathBuf,
    cmd_vars: Option<BTreeMap<String, String>>,
) -> Result<deploy_client::StepResult, String> {
    deploy_client::run_test(&path, cmd_vars.as_ref())
}
