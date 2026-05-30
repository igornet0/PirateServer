//! Local registry: `[project].name` → project metadata and absolute root path.
//!
//! Stored as `pirate-projects.json` under [`crate::config::config_dir`].
//! Legacy format: `{ "projects": { "name": "/abs/path" } }` is migrated on load.

use deploy_core::pirate_project::PirateManifest;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const REGISTRY_FILE: &str = "pirate-projects.json";
const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRegistryEntry {
    pub name: String,
    pub path: String,
    pub local_version: String,
    pub deploy_project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_deploy_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_deployed_version: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistryFileV2 {
    #[serde(default)]
    schema_version: u32,
    entries: BTreeMap<String, ProjectRegistryEntry>,
}

fn registry_path() -> Result<PathBuf, String> {
    crate::config::config_dir()
        .ok_or_else(|| "no config directory (set XDG_CONFIG_HOME or equivalent)".to_string())
        .map(|d| d.join(REGISTRY_FILE))
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Parse legacy or v2 JSON and migrate legacy to v2 on disk when needed.
fn load_raw() -> Result<RegistryFileV2, String> {
    let p = registry_path()?;
    if !p.is_file() {
        return Ok(RegistryFileV2 {
            schema_version: SCHEMA_VERSION,
            entries: BTreeMap::new(),
        });
    }

    let raw = std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", p.display()))?;

    let mut migrated = false;
    let file = if let Some(obj) = v.as_object() {
        if let Some(ent) = obj.get("entries") {
            let map: BTreeMap<String, ProjectRegistryEntry> =
                serde_json::from_value(ent.clone())
                    .map_err(|e| format!("parse entries in {}: {e}", p.display()))?;
            let ver = obj
                .get("schemaVersion")
                .or_else(|| obj.get("schema_version"))
                .and_then(|x| x.as_u64())
                .unwrap_or(SCHEMA_VERSION as u64) as u32;
            RegistryFileV2 {
                schema_version: ver.max(1),
                entries: map,
            }
        } else if let Some(proj) = obj.get("projects") {
            // Legacy: projects is map of name -> path string
            let legacy: BTreeMap<String, String> = serde_json::from_value(proj.clone())
                .map_err(|e| format!("parse legacy projects in {}: {e}", p.display()))?;
            migrated = !legacy.is_empty();
            RegistryFileV2 {
                schema_version: SCHEMA_VERSION,
                entries: migrate_legacy_entries(legacy)?,
            }
        } else {
            RegistryFileV2 {
                schema_version: SCHEMA_VERSION,
                entries: BTreeMap::new(),
            }
        }
    } else {
        return Err(format!("invalid {}: root must be object", p.display()));
    };

    if migrated && !file.entries.is_empty() {
        let _ = save_raw(&file);
    }

    Ok(file)
}

fn migrate_legacy_entries(legacy: BTreeMap<String, String>) -> Result<BTreeMap<String, ProjectRegistryEntry>, String> {
    let mut out = BTreeMap::new();
    for (name, path_str) in legacy {
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let pb = PathBuf::from(path_str.trim());
        let canon = pb
            .canonicalize()
            .map(|x| x.display().to_string())
            .unwrap_or_else(|_| pb.display().to_string());
        let manifest_path = PathBuf::from(&canon).join("pirate.toml");
        let (local_version, deploy_project_id) = match PirateManifest::read_file(&manifest_path) {
            Ok(m) => (
                m.project.version.trim().to_string(),
                m.project.deploy_target_project_id(),
            ),
            Err(_) => (String::new(), String::new()),
        };
        out.insert(
            name.clone(),
            ProjectRegistryEntry {
                name,
                path: canon,
                local_version,
                deploy_project_id,
                last_deploy_at_ms: None,
                last_deployed_version: None,
            },
        );
    }
    Ok(out)
}

fn save_raw(r: &RegistryFileV2) -> Result<(), String> {
    let p = registry_path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension("json.tmp");
    let body = RegistryFileV2 {
        schema_version: SCHEMA_VERSION,
        entries: r.entries.clone(),
    };
    let body = serde_json::to_string_pretty(&body).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

fn merge_preserve_deploy_meta(new: ProjectRegistryEntry, old: Option<&ProjectRegistryEntry>) -> ProjectRegistryEntry {
    let Some(prev) = old else {
        return new;
    };
    if prev.path != new.path {
        return new;
    }
    ProjectRegistryEntry {
        last_deploy_at_ms: prev.last_deploy_at_ms,
        last_deployed_version: prev.last_deployed_version.clone(),
        ..new
    }
}

fn entry_from_manifest(name: String, canon_path: String, manifest: &PirateManifest) -> ProjectRegistryEntry {
    ProjectRegistryEntry {
        name,
        path: canon_path,
        local_version: manifest.project.version.trim().to_string(),
        deploy_project_id: manifest.project.deploy_target_project_id(),
        last_deploy_at_ms: None,
        last_deployed_version: None,
    }
}

/// Register/update using manifest fields; preserves deploy timestamps when name+path unchanged.
pub fn upsert_registry_entry(entry: ProjectRegistryEntry) -> Result<(), String> {
    let mut r = load_raw()?;
    let name = entry.name.trim().to_string();
    if name.is_empty() {
        return Err("project name must not be empty".to_string());
    }
    let old = r.entries.get(&name);
    let mut merged = merge_preserve_deploy_meta(entry, old);
    merged.name = name.clone();
    merged.path = merged.path.trim().to_string();
    r.entries.insert(name, merged);
    save_raw(&r)
}

/// Register `name` → canonical `root` (reads `pirate.toml` once for metadata).
pub fn register(name: &str, root: &Path) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("project name must not be empty".to_string());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }
    let manifest_path = root.join("pirate.toml");
    let manifest = PirateManifest::read_file(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let canon = root.display().to_string();
    upsert_registry_entry(entry_from_manifest(name.to_string(), canon, &manifest))
}

/// Read `pirate.toml` and register `[project].name` → directory with cached metadata.
pub fn register_from_pirate_toml_dir(root: &Path) -> Result<String, String> {
    let p = root.join("pirate.toml");
    let m = PirateManifest::read_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let n = m.project.name.trim();
    if n.is_empty() {
        return Err("[project].name is empty in pirate.toml".to_string());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root.display()));
    }
    let canon = root.display().to_string();
    upsert_registry_entry(entry_from_manifest(n.to_string(), canon, &m))?;
    Ok(n.to_string())
}

/// Resolve registered project name to root path.
pub fn resolve_path(name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("project name must not be empty".to_string());
    }
    let r = load_raw()?;
    let entry = r.entries.get(name).ok_or_else(|| {
        format!(
            "unknown project name '{name}': run `pirate projects add <path>` or `pirate init-project`"
        )
    })?;
    let pb = PathBuf::from(&entry.path);
    if !pb.is_dir() {
        return Err(format!(
            "registered path for '{name}' is missing: {}",
            pb.display()
        ));
    }
    Ok(pb)
}

/// Name → absolute path (CLI / compatibility).
pub fn list_projects() -> Result<BTreeMap<String, String>, String> {
    let r = load_raw()?;
    Ok(r
        .entries
        .values()
        .map(|e| (e.name.clone(), e.path.clone()))
        .collect())
}

/// Full cached rows for desktop UI (sorted by name).
pub fn list_project_registry_entries() -> Result<Vec<ProjectRegistryEntry>, String> {
    let r = load_raw()?;
    let mut v: Vec<ProjectRegistryEntry> = r.entries.into_values().collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(v)
}

/// After a successful deploy from `project_root`, update last deploy fields and optional local version cache.
pub fn record_deploy_for_project_root(
    project_root: &Path,
    deployed_version: &str,
    local_version_after_deploy: Option<&str>,
) -> Result<(), String> {
    let root = project_root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", project_root.display()))?;
    let canon = root.display().to_string();
    let mut r = load_raw()?;
    let mut found: Option<String> = None;
    for (k, e) in r.entries.iter() {
        if e.path == canon {
            found = Some(k.clone());
            break;
        }
    }
    let Some(key) = found else {
        return Ok(());
    };
    let entry = r.entries.get_mut(&key).expect("key from iter");
    entry.last_deploy_at_ms = Some(now_ms());
    let dv = deployed_version.trim();
    if !dv.is_empty() {
        entry.last_deployed_version = Some(dv.to_string());
    }
    if let Some(lv) = local_version_after_deploy {
        let lv = lv.trim();
        if !lv.is_empty() {
            entry.local_version = lv.to_string();
        }
    }
    save_raw(&r)
}

pub fn remove(name: &str) -> Result<bool, String> {
    let name = name.trim();
    let mut r = load_raw()?;
    let ok = r.entries.remove(name).is_some();
    save_raw(&r)?;
    Ok(ok)
}
