//! Structured preflight checks for the Projects / deploy flow (desktop UI).

use crate::connection::{load_endpoint, load_signing_key_for_endpoint};
use crate::host_services_compat::{
    summarize_host_services_for_manifest, HostServicesCompatSummary,
};
use deploy_client::validate_version_label;
use deploy_core::manifest_diagnose::{diagnose_manifest, network_access_summary};
use deploy_core::pirate_project::PirateManifest;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightItem {
    pub id: &'static str,
    pub ok: bool,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectsPreflightReport {
    pub ready: bool,
    pub checks: Vec<PreflightItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_control_api: Option<String>,
    /// Full host-services compare when `pirate.toml` was read (for deploy confirm UI).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_services_compat: Option<HostServicesCompatSummary>,
}

/// Infer a typical control-api HTTP base from a gRPC endpoint URL (best-effort).
fn suggest_control_api_from_grpc(endpoint: &str) -> Option<String> {
    let s = endpoint.trim();
    if s.is_empty() {
        return None;
    }
    let host_part = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
        .unwrap_or(s)
        .split('/')
        .next()?
        .trim();
    if host_part.is_empty() {
        return None;
    }
    let out = if host_part.contains(":50051") {
        format!("http://{}", host_part.replace(":50051", ":8080"))
    } else if !host_part.contains(':') {
        format!("http://{host_part}:8080")
    } else if let Some((h, _p)) = host_part.rsplit_once(':') {
        format!("http://{h}:8080")
    } else {
        format!("http://{host_part}:8080")
    };
    Some(out)
}

fn push_network_checks(checks: &mut Vec<PreflightItem>, manifest: &PirateManifest) {
    let issues = diagnose_manifest(manifest);
    let blocking: Vec<_> = issues.iter().filter(|i| i.blocking).collect();
    let fixable: Vec<_> = issues
        .iter()
        .filter(|i| i.auto_fixable && !i.blocking)
        .collect();

    if blocking.is_empty() && fixable.is_empty() {
        checks.push(PreflightItem {
            id: "network_access",
            ok: true,
            title: "Network & Access".to_string(),
            detail: network_access_summary(manifest),
            hint: None,
            field: None,
            fix_id: None,
            fix_label: None,
        });
        return;
    }

    for issue in blocking {
        checks.push(PreflightItem {
            id: "network_access",
            ok: false,
            title: "Network & Access".to_string(),
            detail: issue.message.clone(),
            hint: Some(issue.hint.clone()),
            field: Some(issue.field.clone()),
            fix_id: None,
            fix_label: None,
        });
    }

    for issue in fixable {
        checks.push(PreflightItem {
            id: "network_access",
            ok: false,
            title: "Network & Access".to_string(),
            detail: issue.message.clone(),
            hint: Some(issue.hint.clone()),
            field: Some(issue.field.clone()),
            fix_id: Some(issue.id.to_string()),
            fix_label: Some(issue.fix_label.clone()),
        });
    }
}

/// Run checks before deploy / pipeline (no network except optional gRPC verify is skipped here).
pub fn run_projects_preflight(project_dir: PathBuf, version: &str) -> ProjectsPreflightReport {
    let mut checks: Vec<PreflightItem> = Vec::new();
    let mut host_services_compat: Option<HostServicesCompatSummary> = None;

    // Connection
    let ep = load_endpoint();
    let ep_str = ep.as_deref().unwrap_or("");
    if ep.is_none() {
        checks.push(PreflightItem {
            id: "connection",
            ok: false,
            title: "Соединение с deploy-server".to_string(),
            detail: "Нет сохранённого gRPC endpoint. Откройте вкладку «Соединение» и подключитесь."
                .to_string(),
            hint: Some("Вставьте install JSON или URL в мастере подключения.".to_string()),
            field: None,
            fix_id: None,
            fix_label: None,
        });
    } else {
        checks.push(PreflightItem {
            id: "connection",
            ok: true,
            title: "Соединение с deploy-server".to_string(),
            detail: format!("Endpoint: {ep_str}"),
            hint: None,
            field: None,
            fix_id: None,
            fix_label: None,
        });
    }

    let suggested_control_api = ep.as_ref().and_then(|e| suggest_control_api_from_grpc(e));

    // Signing key (pairing)
    if let Some(ref e) = ep {
        match load_signing_key_for_endpoint(e) {
            Ok(Some(_)) => {
                checks.push(PreflightItem {
                    id: "signing",
                    ok: true,
                    title: "Ключ подписи (pairing)".to_string(),
                    detail: "Найден ключ для этого сервера.".to_string(),
                    hint: None,
                    field: None,
                    fix_id: None,
                    fix_label: None,
                });
            }
            Ok(None) => {
                checks.push(PreflightItem {
                    id: "signing",
                    ok: false,
                    title: "Ключ подписи (pairing)".to_string(),
                    detail: "Нет ключа Ed25519 для подписи deploy.".to_string(),
                    hint: Some(
                        "Выполните `pirate auth` с install JSON или подключитесь через мастер."
                            .to_string(),
                    ),
                    field: None,
                    fix_id: None,
                    fix_label: None,
                });
            }
            Err(err) => {
                checks.push(PreflightItem {
                    id: "signing",
                    ok: false,
                    title: "Ключ подписи (pairing)".to_string(),
                    detail: err,
                    hint: None,
                    field: None,
                    fix_id: None,
                    fix_label: None,
                });
            }
        }
    }

    // Version label
    match validate_version_label(version) {
        Ok(()) => {
            checks.push(PreflightItem {
                id: "version",
                ok: true,
                title: "Метка версии".to_string(),
                detail: format!("`{version}` допустима."),
                hint: None,
                field: None,
                fix_id: None,
                fix_label: None,
            });
        }
        Err(e) => {
            checks.push(PreflightItem {
                id: "version",
                ok: false,
                title: "Метка версии".to_string(),
                detail: e.to_string(),
                hint: Some("Используйте только [a-zA-Z0-9._-], без пустой строки.".to_string()),
                field: None,
                fix_id: None,
                fix_label: None,
            });
        }
    }

    // Project path
    if !project_dir.as_os_str().is_empty() {
        match project_dir.canonicalize() {
            Ok(p) => {
                if p.is_dir() {
                    let pirate = p.join("pirate.toml");
                    let has_toml = pirate.is_file();
                    checks.push(PreflightItem {
                        id: "project_path",
                        ok: true,
                        title: "Папка проекта".to_string(),
                        detail: p.display().to_string(),
                        hint: if has_toml {
                            None
                        } else {
                            Some(
                                "Файл pirate.toml не найден — Init project или создайте вручную."
                                    .to_string(),
                            )
                        },
                        field: None,
                        fix_id: None,
                        fix_label: None,
                    });
                    if has_toml {
                        match PirateManifest::read_file(&pirate) {
                            Ok(manifest) => {
                                push_network_checks(&mut checks, &manifest);

                                let outputs = manifest.release_output_paths();
                                if outputs.is_empty() {
                                    checks.push(PreflightItem {
                                        id: "build_output",
                                        ok: true,
                                        title: "Build output paths".to_string(),
                                        detail: "Не заданы: будет fallback на корень проекта + .pirateignore."
                                            .to_string(),
                                        hint: Some(
                                            "Добавьте [build].output_path (или output_paths) чтобы в релиз попадал только build-артефакт."
                                                .to_string(),
                                        ),
                                        field: None,
                                        fix_id: None,
                                        fix_label: None,
                                    });
                                } else {
                                    let mut missing = Vec::<String>::new();
                                    for rel in &outputs {
                                        let candidate = p.join(rel);
                                        match candidate.canonicalize() {
                                            Ok(can) if can.starts_with(&p) => {}
                                            Ok(_) => missing.push(format!(
                                                "{rel} (выходит за пределы проекта)"
                                            )),
                                            Err(_) => missing.push(format!(
                                                "{rel} (не найден, сначала выполните build)"
                                            )),
                                        }
                                    }
                                    if missing.is_empty() {
                                        checks.push(PreflightItem {
                                            id: "build_output",
                                            ok: true,
                                            title: "Build output paths".to_string(),
                                            detail: format!(
                                                "Будут отправлены: {}",
                                                outputs.join(", ")
                                            ),
                                            hint: None,
                                            field: None,
                                            fix_id: None,
                                            fix_label: None,
                                        });
                                    } else {
                                        checks.push(PreflightItem {
                                            id: "build_output",
                                            ok: false,
                                            title: "Build output paths".to_string(),
                                            detail: format!(
                                                "Некорректные output path(s): {}",
                                                missing.join("; ")
                                            ),
                                            hint: Some(
                                                "Проверьте [build].output_path(s) и выполните build перед deploy."
                                                    .to_string(),
                                            ),
                                            field: None,
                                            fix_id: None,
                                            fix_label: None,
                                        });
                                    }
                                }

                                let hs = summarize_host_services_for_manifest(&manifest);
                                let (hs_ok, hs_detail, hs_hint): (bool, String, Option<String>) =
                                    match hs.status.as_str() {
                                        "none" => (
                                            true,
                                            "Пакеты на хосте не требуются манифестом (секции [services], [runtime], [proxy])."
                                                .to_string(),
                                            None,
                                        ),
                                        "skipped" => (
                                            true,
                                            format!(
                                                "Требуются на хосте: {}. {}",
                                                hs.required_host_service_ids.join(", "),
                                                hs.skip_reason.as_deref().unwrap_or("")
                                            ),
                                            Some(
                                                "Задайте URL control-api и войдите (закладка сервера), чтобы сверить установленные пакеты."
                                                    .to_string(),
                                            ),
                                        ),
                                        "checked" => {
                                            if hs.missing_host_service_ids.is_empty() {
                                                (
                                                    true,
                                                    format!(
                                                        "Все требуемые сервисы на хосте: {}.",
                                                        hs.satisfied_host_service_ids.join(", ")
                                                    ),
                                                    None,
                                                )
                                            } else {
                                                (
                                                    false,
                                                    format!(
                                                        "Не установлены на сервере: {} (нужно: {}).",
                                                        hs.missing_host_service_ids.join(", "),
                                                        hs.required_host_service_ids.join(", ")
                                                    ),
                                                    Some(
                                                        "Установите через закладку сервера → вкладка «Сервисы»."
                                                            .to_string(),
                                                    ),
                                                )
                                            }
                                        }
                                        "error" => (
                                            false,
                                            format!(
                                                "Не удалось получить список пакетов: {}",
                                                hs.skip_reason.as_deref().unwrap_or("error")
                                            ),
                                            None,
                                        ),
                                        _ => (
                                            true,
                                            "host services".to_string(),
                                            None,
                                        ),
                                    };
                                checks.push(PreflightItem {
                                    id: "host_services",
                                    ok: hs_ok,
                                    title: "Хост-сервисы".to_string(),
                                    detail: hs_detail,
                                    hint: hs_hint,
                                    field: None,
                                    fix_id: None,
                                    fix_label: None,
                                });
                                host_services_compat = Some(hs);
                            }
                            Err(e) => checks.push(PreflightItem {
                                id: "build_output",
                                ok: false,
                                title: "Build output paths".to_string(),
                                detail: format!("Не удалось прочитать pirate.toml: {e}"),
                                hint: None,
                                field: None,
                                fix_id: None,
                                fix_label: None,
                            }),
                        }
                    }
                } else {
                    checks.push(PreflightItem {
                        id: "project_path",
                        ok: false,
                        title: "Папка проекта".to_string(),
                        detail: "Путь не является каталогом.".to_string(),
                        hint: None,
                        field: None,
                        fix_id: None,
                        fix_label: None,
                    });
                }
            }
            Err(e) => {
                checks.push(PreflightItem {
                    id: "project_path",
                    ok: false,
                    title: "Папка проекта".to_string(),
                    detail: format!("Не удалось открыть папку: {e}"),
                    hint: Some("Выберите существующий каталог с проектом.".to_string()),
                    field: None,
                    fix_id: None,
                    fix_label: None,
                });
            }
        }
    } else {
        checks.push(PreflightItem {
            id: "project_path",
            ok: false,
            title: "Папка проекта".to_string(),
            detail: "Папка не выбрана.".to_string(),
            hint: Some("Нажмите «Выбрать папку…».".to_string()),
            field: None,
            fix_id: None,
            fix_label: None,
        });
    }

    let ready = checks
        .iter()
        .filter(|c| c.id != "host_services")
        .all(|c| {
            if !c.ok && c.id == "network_access" && c.fix_id.is_some() {
                return true;
            }
            c.ok
        });
    ProjectsPreflightReport {
        ready,
        checks,
        suggested_control_api,
        host_services_compat,
    }
}
