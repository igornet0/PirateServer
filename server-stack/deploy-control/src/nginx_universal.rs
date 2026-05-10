//! Full-host nginx inventory, preflight diagnostics, and safe sudo-backed actions.
use crate::types::{
    NginxActionBody, NginxActionPostCheckView, NginxActionResponseView, NginxPreflightProposed, NginxPreflightView,
    NginxProblemView, NginxSiteEntryView, NginxSitesView,
};
use crate::nginx_probe::{
    https_probe_failure_warrants_rollback, https_probe_localhost_resolve_with_retries,
    openssl_x509_checkhost_pem, HttpsLocalProbe,
};
use crate::acme_issue::privileged_path_is_file;
use crate::ControlError;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const MAX_FILE_BYTES: usize = 256 * 1024;

const SITES_AVAILABLE: &str = "/etc/nginx/sites-available";
const SITES_ENABLED: &str = "/etc/nginx/sites-enabled";
const CONF_D: &str = "/etc/nginx/conf.d";
const NGINX_MAIN: &str = "/etc/nginx/nginx.conf";

fn is_pirate_marker(content: &str) -> bool {
    content
        .trim_start()
        .lines()
        .any(|l| l.trim().starts_with("# Pirate:"))
}

/// Stable id for a config file.
pub fn nginx_site_id_for_path(path: &Path) -> String {
    format!("file:{}", path.display())
}

fn nginx_ops_merged_output(out: &std::process::Output) -> String {
    [String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)].concat()
}

fn sudo_nginx_ops(ops_script: &Path, args: &[&str]) -> Result<std::process::Output, ControlError> {
    if !ops_script.is_file() {
        return Err(ControlError::NginxOp(format!(
            "nginx ops script not found: {} (install pirate-nginx-ops.sh)",
            ops_script.display()
        )));
    }
    let mut c = Command::new("sudo");
    c.arg("-n");
    c.arg(
        ops_script
            .to_str()
            .ok_or_else(|| ControlError::NginxOp("invalid nginx ops path".into()))?,
    );
    for a in args {
        c.arg(a);
    }
    c.output()
        .map_err(|e| ControlError::NginxOp(format!("sudo nginx-ops: {e}")))
}

/// `nginx -t` as root via [`pirate-nginx-ops.sh validate`].
fn nginx_test_output_via_ops(ops_script: &Path) -> (bool, String) {
    match sudo_nginx_ops(ops_script, &["validate"]) {
        Ok(o) => (o.status.success(), nginx_ops_merged_output(&o)),
        Err(e) => (false, e.to_string()),
    }
}

fn run_systemctl_reload_nginx_via_ops(ops_script: &Path) -> Result<String, ControlError> {
    let o = sudo_nginx_ops(ops_script, &["reload"])?;
    let text = nginx_ops_merged_output(&o);
    if !o.status.success() {
        return Err(ControlError::NginxOp(format!(
            "systemctl reload nginx failed: {text}"
        )));
    }
    Ok(text)
}

/// Write arbitrary nginx config path via `sudo -n pirate-nginx-ops.sh apply-config` (test + reload as root).
pub fn write_nginx_path_via_sudo_tee(
    ops_script: &Path,
    path: &Path,
    content: &str,
) -> Result<String, ControlError> {
    if content.len() > MAX_FILE_BYTES {
        return Err(ControlError::NginxOp(format!(
            "content exceeds {MAX_FILE_BYTES} bytes"
        )));
    }
    if content.as_bytes().contains(&0) {
        return Err(ControlError::NginxOp("content must not contain NUL".into()));
    }
    let p = path
        .to_str()
        .ok_or_else(|| ControlError::NginxOp("invalid path".into()))?;
    let ops = ops_script
        .to_str()
        .ok_or_else(|| ControlError::NginxOp("invalid ops path".into()))?;
    let mut child = Command::new("sudo")
        .args(["-n", ops, "apply-config", p])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ControlError::NginxOp(format!("sudo apply-config: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ControlError::NginxOp("no stdin for apply-config".into()))?;
    stdin
        .write_all(content.as_bytes())
        .map_err(|e| ControlError::NginxOp(format!("stdin: {e}")))?;
    drop(stdin);
    let out = child
        .wait_with_output()
        .map_err(|e| ControlError::NginxOp(format!("apply-config wait: {e}")))?;
    let merged = nginx_ops_merged_output(&out);
    if !out.status.success() {
        return Err(ControlError::NginxOp(format!(
            "apply-config failed: {merged}"
        )));
    }
    Ok(merged)
}

fn read_file_limited(path: &Path) -> Result<String, ControlError> {
    let meta = fs::metadata(path).map_err(|e| ControlError::NginxOp(e.to_string()))?;
    if meta.len() as usize > MAX_FILE_BYTES {
        return Err(ControlError::NginxOp("file too large to read in control-api".into()));
    }
    fs::read_to_string(path).map_err(|e| ControlError::NginxOp(e.to_string()))
}

fn strip_line_comment(s: &str) -> &str {
    s.split('#').next().unwrap_or("").trim()
}

const SSL_CERTIFICATE_DIRECTIVE: &str = "ssl_certificate";

/// Returns the first path token for `ssl_certificate` (not `ssl_certificate_key`), preserving path case.
/// Skips `ssl_certificate_key` lines. Used by [`parse_file_diagnostics`].
fn extract_ssl_certificate_path_token(line: &str) -> Option<String> {
    let s = strip_line_comment(line).trim();
    if s.is_empty() {
        return None;
    }
    let low = s.to_ascii_lowercase();
    if low.starts_with("ssl_certificate_key") {
        return None;
    }
    if !low.starts_with(SSL_CERTIFICATE_DIRECTIVE) {
        return None;
    }
    if s.len() < SSL_CERTIFICATE_DIRECTIVE.len() {
        return None;
    }
    if !s
        .get(0..SSL_CERTIFICATE_DIRECTIVE.len())
        .is_some_and(|h| h.eq_ignore_ascii_case(SSL_CERTIFICATE_DIRECTIVE))
    {
        return None;
    }
    // Directive must be followed by whitespace (e.g. not `ssl_certificates` typo).
    let after_word = s.get(SSL_CERTIFICATE_DIRECTIVE.len()..)?;
    if !after_word
        .as_bytes()
        .first()
        .map_or(true, u8::is_ascii_whitespace)
    {
        return None;
    }
    let rest = after_word.trim_start();
    if rest.is_empty() {
        return None;
    }
    let token = rest
        .split_whitespace()
        .next()?
        .trim_end_matches(';');
    if token.is_empty() {
        return None;
    }
    let token = token.trim_matches('\'').trim_matches('"');
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

fn parse_server_name_line(line: &str) -> Vec<String> {
    let t = strip_line_comment(line).trim();
    if !t.to_ascii_lowercase().starts_with("server_name") {
        return vec![];
    }
    let rest = t[11..].trim();
    let rest = rest.strip_suffix(';').unwrap_or(rest).trim();
    if rest.is_empty() {
        return vec![];
    }
    rest.split_whitespace()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != ";" && s != "default_server")
        .collect()
}

fn collect_listen_info(line: &str) -> (Vec<u16>, bool) {
    let t = strip_line_comment(line).trim().to_ascii_lowercase();
    if !t.contains("listen") {
        return (vec![], false);
    }
    let ssl = t.contains(" ssl");
    let mut ports = vec![];
    for tok in t.split_whitespace() {
        if tok == "listen" || tok == "ssl" || tok == "http2" || tok == "quic" {
            continue;
        }
        if let Some(p) = tok.split('[').next().and_then(|s| s.split(':').last()) {
            if let Ok(n) = p.parse::<u16>() {
                ports.push(n);
            }
        } else if let Ok(n) = tok.parse::<u16>() {
            ports.push(n);
        }
    }
    (ports, ssl)
}

fn has_ssl_directives(content: &str) -> bool {
    for line in content.lines() {
        let t = strip_line_comment(line).trim();
        if t.is_empty() {
            continue;
        }
        let l = t.to_ascii_lowercase();
        if l.contains("listen") && l.contains("ssl") {
            return true;
        }
        if l.starts_with("ssl_certificate") {
            return true;
        }
    }
    false
}

/// Find `server` block: returns (index after `{`, index of matching `}`).
fn find_first_server_block_inner_range(content: &str) -> Option<(usize, usize)> {
    let bytes = content.as_bytes();
    let mut i: usize = 0;
    while i + 6 <= bytes.len() {
        if i > 0 {
            let prev = bytes[i - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'-' {
                i += 1;
                continue;
            }
        }
        if &bytes[i..i + 6] == b"server" {
            let after = i + 6;
            if after < bytes.len() {
                let c = bytes[after];
                if c.is_ascii_alphanumeric() || c == b'_' {
                    i += 1;
                    continue;
                }
            }
            let mut j = after;
            while j < bytes.len() && bytes[j] as char != '{' {
                if bytes[j] == b'#' {
                    while j < bytes.len() && bytes[j] as char != '\n' {
                        j += 1;
                    }
                }
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'{' {
                let open = j;
                let mut depth: i32 = 0;
                let mut k = open;
                while k < bytes.len() {
                    match bytes[k] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some((open + 1, k));
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
            }
        }
        i += 1;
    }
    None
}

/// Replace first `server_name` line in first `server` block inner.
pub fn set_server_name_in_content(content: &str, new_name: &str) -> Result<String, ControlError> {
    let (inner_start, close_idx) = find_first_server_block_inner_range(content)
        .ok_or_else(|| ControlError::NginxOp("no server { } block found".into()))?;
    let before = &content[..inner_start];
    let block = &content[inner_start..close_idx];
    let after = &content[close_idx..];
    let new_line = format!("    server_name {new_name};");
    let lines: Vec<&str> = block.lines().collect();
    let mut done = false;
    let mut out = String::new();
    for line in &lines {
        if !done
            && strip_line_comment(line)
                .trim()
                .to_ascii_lowercase()
                .starts_with("server_name")
        {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&new_line);
            done = true;
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    if !done {
        if lines.is_empty() {
            return Err(ControlError::NginxOp("empty server block".into()));
        }
        out = String::new();
        for (idx, line) in lines.iter().enumerate() {
            if idx == 0 {
                out.push_str(line);
                out.push('\n');
                out.push_str(&new_line);
                out.push('\n');
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if block.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(format!("{before}{out}{after}"))
}

/// Remove SSL listens and cert directives in first server block.
pub fn strip_ssl_from_first_server(content: &str) -> Result<String, ControlError> {
    let (inner_start, close_idx) = find_first_server_block_inner_range(content)
        .ok_or_else(|| ControlError::NginxOp("no server { } block found".into()))?;
    let before = &content[..inner_start];
    let block = &content[inner_start..close_idx];
    let after = &content[close_idx..];
    let mut new_block = String::new();
    for line in block.lines() {
        let t = strip_line_comment(line).trim();
        let low = t.to_ascii_lowercase();
        if low.is_empty() {
            if !new_block.is_empty() {
                new_block.push_str(line);
                new_block.push('\n');
            }
            continue;
        }
        if low.contains("listen") && low.contains("ssl") {
            continue;
        }
        if low.contains("http2")
            && low.contains("listen")
            && (low.contains("443") || low.contains("[::]:443"))
            && low.contains("ssl")
        {
            continue;
        }
        if low.starts_with("ssl_certificate") {
            continue;
        }
        if low.starts_with("ssl_certificate_key") {
            continue;
        }
        if low.starts_with("ssl_trusted_certificate") {
            continue;
        }
        if low.starts_with("include") && low.contains("ssl") {
            continue;
        }
        new_block.push_str(line);
        new_block.push('\n');
    }
    Ok(format!("{before}{new_block}{after}"))
}

/// Add or update SSL in first server block.
pub fn add_ssl_to_first_server(
    content: &str,
    cert: &str,
    key: &str,
) -> Result<String, ControlError> {
    let (inner_start, close_idx) = find_first_server_block_inner_range(content)
        .ok_or_else(|| ControlError::NginxOp("no server { } block found".into()))?;
    let before = &content[..inner_start];
    let block = &content[inner_start..close_idx];
    let after = &content[close_idx..];
    if has_ssl_directives(block) {
        let lines: Vec<String> = block
            .lines()
            .map(|l| {
                let t = strip_line_comment(l).trim();
                let low = t.to_ascii_lowercase();
                if low.starts_with("ssl_certificate_key") {
                    return format!("    ssl_certificate_key {key};");
                }
                if low.starts_with("ssl_certificate") && !low.contains("key") {
                    return format!("    ssl_certificate {cert};");
                }
                l.to_string()
            })
            .collect();
        return Ok(format!("{before}{}{after}", lines.join("\n"),));
    }
    let mut insert = String::new();
    insert.push_str("    listen 443 ssl http2;\n");
    insert.push_str("    listen [::]:443 ssl http2;\n");
    insert.push_str(&format!("    ssl_certificate {cert};\n"));
    insert.push_str(&format!("    ssl_certificate_key {key};\n"));
    let block_lines: Vec<&str> = block.lines().collect();
    if block_lines.is_empty() {
        return Ok(format!("{before}{insert}{after}"));
    }
    let mut out = String::new();
    let mut inserted = false;
    for line in &block_lines {
        out.push_str(line);
        out.push('\n');
        if !inserted
            && strip_line_comment(line)
                .to_ascii_lowercase()
                .contains("listen 80")
        {
            out.push_str(&insert);
            inserted = true;
        }
    }
    if !inserted {
        // Prepend SSL after server opening if no `listen 80` line (unusual).
        return Ok(format!("{before}{insert}{block}{after}"));
    }
    Ok(format!("{before}{out}{after}"))
}

fn first_domain_in_content(content: &str) -> String {
    for line in content.lines() {
        for t in parse_server_name_line(line) {
            if t != "_" && !t.contains('*') {
                return t;
            }
        }
    }
    String::new()
}

fn resolved_post_check_host(action: &NginxActionBody, content_after_edit: &str) -> Option<String> {
    let host = action
        .post_check_host
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(h) = host {
        return Some(h.to_string());
    }
    let fd = first_domain_in_content(content_after_edit);
    if fd.is_empty() {
        None
    } else {
        Some(fd)
    }
}

fn probe_to_post_check_fields(pr: &HttpsLocalProbe) -> (Option<String>, Option<i32>) {
    let probe_host = if pr.probe_host.is_empty() {
        None
    } else {
        Some(pr.probe_host.clone())
    };
    let curl_exit = if pr.classified == "ok"
        || pr.classified == "skipped_wildcard_or_empty"
        || pr.classified == "curl_unavailable"
        || pr.classified == "skipped"
    {
        None
    } else {
        Some(pr.curl_exit)
    };
    (probe_host, curl_exit)
}

fn parse_file_diagnostics(
    path: &Path,
    content: &str,
    entry_kind: &str,
    pirate_path: &Path,
) -> NginxSiteEntryView {
    let mut dom_set: BTreeSet<String> = BTreeSet::new();
    for line in content.lines() {
        for t in parse_server_name_line(line) {
            dom_set.insert(t);
        }
    }
    let domains: Vec<String> = if dom_set.is_empty() {
        vec!["_".to_string()]
    } else {
        dom_set.into_iter().collect()
    };
    let mut warnings = vec![];
    let mut ports: Vec<u16> = vec![];
    for line in content.lines() {
        let (p, _) = collect_listen_info(line);
        for x in p {
            if !ports.contains(&x) {
                ports.push(x);
            }
        }
    }
    if ports.is_empty() {
        warnings.push("no listen ports detected (may be in includes)".to_string());
    }
    let ssl = has_ssl_directives(content);
    if ssl {
        for line in content.lines() {
            if let Some(p) = extract_ssl_certificate_path_token(line) {
                if p.starts_with('$') {
                    warnings.push(format!(
                        "ssl_certificate path is a variable; file presence not verified: {p}"
                    ));
                } else if !p.starts_with('/') {
                    warnings.push(format!(
                        "ssl_certificate path is not absolute; file presence not verified: {p}"
                    ));
                } else if !privileged_path_is_file(&p) {
                    warnings.push(format!("ssl_certificate file missing: {p}"));
                }
            }
        }
    }
    let managed = is_pirate_marker(content) || path == pirate_path
        || path
            .canonicalize()
            .ok()
            .zip(pirate_path.canonicalize().ok())
            .map(|(a, b)| a == b)
            .unwrap_or(false);
    let is_ui = is_pirate_marker(content)
        && content.contains("root ")
        && content.contains("try_files")
        && content.contains("/api/");
    NginxSiteEntryView {
        site_id: nginx_site_id_for_path(path),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string()),
        path: path.display().to_string(),
        entry_kind: entry_kind.to_string(),
        active: true,
        enabled: true,
        enabled_path: None,
        domains,
        ssl_enabled: ssl,
        listen_ports: ports,
        managed_by: if managed { "pirate".into() } else { "external".into() },
        is_ui_stack: is_ui,
        parse_warnings: warnings,
    }
}

/// Build inventory + conflict preflight.
pub fn collect_nginx_sites(pirate_site_path: &Path, ops_script: &Path) -> Result<NginxSitesView, ControlError> {
    let (ok, test_out) = nginx_test_output_via_ops(ops_script);
    let mut global_warnings = vec![];
    if !ok {
        global_warnings.push(format!("nginx -t: {}", test_out.trim()));
    }
    let mut sites: Vec<NginxSiteEntryView> = vec![];
    if Path::new(NGINX_MAIN).is_file() {
        let c = read_file_limited(Path::new(NGINX_MAIN))?;
        let mut m = parse_file_diagnostics(Path::new(NGINX_MAIN), &c, "main", pirate_site_path);
        m.active = true;
        m.enabled = true;
        sites.push(m);
    } else {
        global_warnings
            .push("missing /etc/nginx/nginx.conf (non-Debian layout?)".to_string());
    }
    if let Ok(dir) = fs::read_dir(CONF_D) {
        for e in dir.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("conf") {
                continue;
            }
            let c = match read_file_limited(&p) {
                Ok(s) => s,
                Err(e) => {
                    global_warnings.push(e.to_string());
                    continue;
                }
            };
            if c.is_empty() {
                continue;
            }
            let mut m = parse_file_diagnostics(&p, &c, "conf_d", pirate_site_path);
            m.active = true;
            m.enabled = true;
            sites.push(m);
        }
    }
    let mut enabled_by_target: HashMap<String, String> = HashMap::new();
    if let Ok(dir) = fs::read_dir(SITES_ENABLED) {
        for e in dir.flatten() {
            let p = e.path();
            if let Ok(t) = fs::read_link(&p) {
                let resolved = if t.is_absolute() {
                    t.clone()
                } else {
                    Path::new(SITES_ENABLED).join(&t)
                };
                if let Ok(can) = resolved.canonicalize() {
                    enabled_by_target
                        .entry(can.to_string_lossy().to_string())
                        .or_insert_with(|| p.display().to_string());
                } else {
                    global_warnings.push(format!(
                        "broken symlink: {} -> {}",
                        p.display(),
                        t.display()
                    ));
                }
            } else if p.is_file() {
                if let Ok(can) = p.canonicalize() {
                    enabled_by_target
                        .entry(can.to_string_lossy().to_string())
                        .or_insert_with(|| p.display().to_string());
                }
            }
        }
    }
    if let Ok(dir) = fs::read_dir(SITES_AVAILABLE) {
        for e in dir.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let c = read_file_limited(&p);
            let c = match c {
                Ok(s) => s,
                Err(e) => {
                    global_warnings.push(e.to_string());
                    continue;
                }
            };
            let mut m = parse_file_diagnostics(&p, &c, "vhost", pirate_site_path);
            let key = p
                .canonicalize()
                .map(|c| c.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.display().to_string());
            m.enabled = enabled_by_target.contains_key(&key);
            m.active = m.enabled;
            m.enabled_path = enabled_by_target.get(&key).cloned();
            sites.push(m);
        }
    }
    sites.sort_by(|a, b| {
        let r = |k: &str| match k {
            "main" => 0u8,
            "conf_d" => 1u8,
            "vhost" => 2u8,
            _ => 3u8,
        };
        r(&a.entry_kind)
            .cmp(&r(&b.entry_kind))
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut global_conflicts: Vec<NginxProblemView> = vec![];
    let mut by_domain: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for s in &sites {
        if s.entry_kind != "vhost" || !s.active {
            continue;
        }
        for d in &s.domains {
            if d == "_" {
                continue;
            }
            by_domain
                .entry(d.to_lowercase())
                .or_default()
                .insert(s.path.clone());
        }
    }
    for (dom, paths) in by_domain {
        if paths.len() > 1 {
            global_conflicts.push(NginxProblemView {
                level: "conflict".into(),
                code: "duplicate_server_name".into(),
                message: format!("duplicate server_name '{dom}': {}", paths.iter().cloned().collect::<Vec<_>>().join(", ")),
            });
        }
    }
    Ok(NginxSitesView {
        ok,
        nginx_test_output: if test_out.is_empty() {
            None
        } else {
            Some(test_out)
        },
        global_warnings,
        global_conflicts,
        sites,
    })
}

/// Preflight with optional proposed action.
pub fn preflight_nginx(
    pirate_site_path: &Path,
    proposed: &NginxPreflightProposed,
    ops_script: &Path,
) -> Result<NginxPreflightView, ControlError> {
    let mut inv = collect_nginx_sites(pirate_site_path, ops_script)?;
    let extra = match proposed.action.as_deref() {
        Some("set_server_name") if proposed.path.is_none() => {
            Some(NginxProblemView {
                level: "conflict".into(),
                code: "missing_path".into(),
                message: "path required for set_server_name".into(),
            })
        }
        Some("set_ssl") if proposed.path.is_none() => Some(NginxProblemView {
            level: "conflict".into(),
            code: "missing_path".into(),
            message: "path required for set_ssl".into(),
        }),
        _ => None,
    };
    if let Some(e) = extra {
        inv.global_conflicts.push(e.clone());
    }
    let blockers: Vec<NginxProblemView> = inv
        .global_conflicts
        .iter()
        .filter(|c| c.level == "conflict")
        .cloned()
        .collect();
    for s in &inv.sites {
        for w in &s.parse_warnings {
            if !w.is_empty() {
                inv.global_warnings
                    .push(format!("[{}] {w}", s.path));
            }
        }
    }
    let ok = inv.ok && blockers.is_empty();
    Ok(NginxPreflightView {
        ok,
        inventory: inv,
        blockers,
    })
}

fn apply_or_tee(
    pirate_apply_script: &Path,
    ops_script: &Path,
    path: &Path,
    content: &str,
) -> Result<NginxActionResponseView, ControlError> {
    let lossy = path.to_string_lossy();
    if lossy.contains("/sites-available/") {
        let r = crate::apply_nginx_site_via_sudo(path, content, pirate_apply_script)?;
        return Ok(NginxActionResponseView {
            ok: r.ok,
            action: "write_site".into(),
            message: r.message,
            detail: r.test_output,
            post_check: None,
        });
    }
    let detail = write_nginx_path_via_sudo_tee(ops_script, path, content)?;
    Ok(NginxActionResponseView {
        ok: true,
        action: "write_path".into(),
        message: "config written, tested, reloaded".into(),
        detail: Some(detail),
        post_check: None,
    })
}

/// After `set_ssl` with HTTPS on: probe through nginx. On failure, restore `backup` content when policy says so.
fn set_ssl_post_check_and_maybe_rollback(
    pirate_apply_script: &Path,
    ops_script: &Path,
    path: &Path,
    backup: &str,
    new_c: &str,
    action: &NginxActionBody,
    res: NginxActionResponseView,
) -> Result<NginxActionResponseView, ControlError> {
    if !action.post_check_enabled.unwrap_or(true) {
        return Ok(NginxActionResponseView {
            post_check: Some(NginxActionPostCheckView {
                ok: true,
                http_status: None,
                probe_host: None,
                curl_exit: None,
                summary: "post_check disabled in request".into(),
                rollback_performed: false,
                classified: "skipped".into(),
                details: vec![],
            }),
            ..res
        });
    }
    let host = resolved_post_check_host(action, new_c).unwrap_or_default();
    if host.is_empty() {
        return Ok(NginxActionResponseView {
            post_check: Some(NginxActionPostCheckView {
                ok: true,
                http_status: None,
                probe_host: None,
                curl_exit: None,
                summary: "no server_name for SNI; HTTPS probe skipped".into(),
                rollback_performed: false,
                classified: "skipped".into(),
                details: vec!["set post_check_host or add server_name to vhost".into()],
            }),
            ..res
        });
    }
    if host.starts_with("*.") {
        return Ok(NginxActionResponseView {
            post_check: Some(NginxActionPostCheckView {
                ok: true,
                http_status: None,
                probe_host: Some(host.clone()),
                curl_exit: None,
                summary: "wildcard server_name: HTTPS probe skipped (use a concrete post_check_host)"
                    .into(),
                rollback_performed: false,
                classified: "skipped_wildcard".into(),
                details: vec![],
            }),
            ..res
        });
    }
    let path_probe = action.post_check_path.as_deref().unwrap_or("/");
    let port = action.post_check_port.unwrap_or(443);
    let loopback = action.post_check_loopback.as_deref().unwrap_or("127.0.0.1");
    let pr = https_probe_localhost_resolve_with_retries(&host, path_probe, port, loopback, 3, 200);
    let (ph, ce) = probe_to_post_check_fields(&pr);
    let details = vec![format!(
        "probe host={} class={} detail={}",
        pr.probe_host, pr.classified, pr.detail
    )];
    if pr.classified == "curl_unavailable" {
        return Ok(NginxActionResponseView {
            post_check: Some(NginxActionPostCheckView {
                ok: true,
                http_status: if pr.http_status == 0 {
                    None
                } else {
                    Some(pr.http_status)
                },
                probe_host: ph,
                curl_exit: ce,
                summary: "curl not installed: install curl on host to validate HTTPS after set_ssl"
                    .into(),
                rollback_performed: false,
                classified: pr.classified.clone(),
                details,
            }),
            action: "set_ssl".into(),
            ..res
        });
    }
    if pr.ok {
        return Ok(NginxActionResponseView {
            post_check: Some(NginxActionPostCheckView {
                ok: true,
                http_status: if pr.http_status == 0 {
                    None
                } else {
                    Some(pr.http_status)
                },
                probe_host: ph,
                curl_exit: None,
                summary: "HTTPS check passed through nginx (local resolve)".into(),
                rollback_performed: false,
                classified: pr.classified.clone(),
                details,
            }),
            action: "set_ssl".into(),
            ..res
        });
    }
    let do_rollback = https_probe_failure_warrants_rollback(&pr.classified);
    let rolled = if do_rollback {
        apply_or_tee(pirate_apply_script, ops_script, path, backup)
    } else {
        Err(ControlError::NginxOp("rollback skipped for this probe class".into()))
    };
    let rollback_performed = do_rollback && matches!(&rolled, Ok(r) if r.ok);
    let rb_msg: String = match &rolled {
        Ok(r) => r.detail.clone().unwrap_or_default(),
        Err(e) if do_rollback => e.to_string(),
        _ => String::new(),
    };
    let summary = if pr.classified == "tls_name_mismatch" {
        "HTTPS probe: certificate hostname/SAN does not match SNI; nginx config left as applied (no rollback)"
            .to_string()
    } else if rollback_performed {
        "HTTPS through nginx did not look healthy; rolled back to previous vhost".to_string()
    } else if do_rollback {
        "HTTPS through nginx did not look healthy; rollback failed or partial".to_string()
    } else {
        "HTTPS through nginx did not look healthy; rollback skipped for this error class".to_string()
    };
    Ok(NginxActionResponseView {
        ok: false,
        action: "set_ssl".into(),
        message: format!(
            "set_ssl: HTTPS check failed ({}); config {}",
            pr.classified,
            if rollback_performed {
                "restored to previous"
            } else if pr.classified == "tls_name_mismatch" {
                "not reverted (fix certificate or post_check_host / server_name)"
            } else if do_rollback {
                "NOT reverted; fix nginx/upstream (rollback failed or partial)"
            } else {
                "not reverted (rollback not used for this probe class)"
            }
        ),
        detail: if rb_msg.is_empty() {
            res.detail
        } else {
            res.detail.map(|d| format!("{d}\nrollback: {rb_msg}"))
        },
        post_check: Some(NginxActionPostCheckView {
            ok: false,
            http_status: if pr.http_status == 0 {
                None
            } else {
                Some(pr.http_status)
            },
            probe_host: ph,
            curl_exit: ce,
            summary,
            rollback_performed,
            classified: pr.classified.clone(),
            details,
        }),
    })
}

/// Apply one action.
pub fn apply_nginx_universal_action(
    pirate_apply_script: &Path,
    ops_script: &Path,
    action: &NginxActionBody,
) -> Result<NginxActionResponseView, ControlError> {
    match action.action.as_str() {
        "enable_site" => {
            let ap = action
                .available_path
                .as_deref()
                .ok_or_else(|| ControlError::NginxOp("available_path required".into()))?;
            let path = Path::new(ap);
            let name = path
                .file_name()
                .ok_or_else(|| ControlError::NginxOp("invalid available_path".into()))?
                .to_string_lossy();
            let enabled = format!("{SITES_ENABLED}/{name}");
            let avail_str = path.to_string_lossy().to_string();
            let out = sudo_nginx_ops(
                ops_script,
                &["enable-site", avail_str.as_str(), enabled.as_str()],
            )?;
            if !out.status.success() {
                return Ok(NginxActionResponseView {
                    ok: false,
                    action: "enable_site".into(),
                    message: "enable-site failed (sudo pirate-nginx-ops.sh enable-site)".into(),
                    detail: Some(nginx_ops_merged_output(&out)),
                    post_check: None,
                });
            }
            let (t_ok, t_msg) = nginx_test_output_via_ops(ops_script);
            if !t_ok {
                let _ = sudo_nginx_ops(ops_script, &["disable-site", enabled.as_str()]);
                return Ok(NginxActionResponseView {
                    ok: false,
                    action: "enable_site".into(),
                    message: "nginx -t failed; symlink removed".into(),
                    detail: Some(t_msg),
                    post_check: None,
                });
            }
            let reload = match run_systemctl_reload_nginx_via_ops(ops_script) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(NginxActionResponseView {
                        ok: false,
                        action: "enable_site".into(),
                        message: format!("nginx -t ok but reload failed: {e}"),
                        detail: Some(t_msg),
                        post_check: None,
                    });
                }
            };
            Ok(NginxActionResponseView {
                ok: true,
                action: "enable_site".into(),
                message: "site enabled and nginx reloaded".into(),
                detail: Some(format!("{t_msg}\n{reload}")),
                post_check: None,
            })
        }
        "disable_site" => {
            let ep = action
                .enabled_path
                .as_deref()
                .or(action.path.as_deref())
                .ok_or_else(|| ControlError::NginxOp("enabled_path or path required".into()))?;
            let out = sudo_nginx_ops(ops_script, &["disable-site", ep])?;
            if !out.status.success() {
                return Ok(NginxActionResponseView {
                    ok: false,
                    action: "disable_site".into(),
                    message: "disable-site failed (sudo pirate-nginx-ops.sh disable-site)".into(),
                    detail: Some(nginx_ops_merged_output(&out)),
                    post_check: None,
                });
            }
            let (t_ok, t_msg) = nginx_test_output_via_ops(ops_script);
            if !t_ok {
                return Ok(NginxActionResponseView {
                    ok: false,
                    action: "disable_site".into(),
                    message: "nginx -t failed after disable".into(),
                    detail: Some(t_msg),
                    post_check: None,
                });
            }
            let reload = run_systemctl_reload_nginx_via_ops(ops_script)?;
            Ok(NginxActionResponseView {
                ok: true,
                action: "disable_site".into(),
                message: "site disabled and nginx reloaded".into(),
                detail: Some(format!("{t_msg}\n{reload}")),
                post_check: None,
            })
        }
        "set_server_name" => {
            let p = action
                .path
                .as_deref()
                .ok_or_else(|| ControlError::NginxOp("path required".into()))?;
            let name = action
                .server_name
                .as_deref()
                .ok_or_else(|| ControlError::NginxOp("server_name required".into()))?;
            let path = Path::new(p);
            let c = read_file_limited(path)?;
            let new_c = set_server_name_in_content(&c, name)?;
            apply_or_tee(pirate_apply_script, ops_script, path, &new_c)
        }
        "set_ssl" => {
            let p = action
                .path
                .as_deref()
                .ok_or_else(|| ControlError::NginxOp("path required".into()))?;
            let path = Path::new(p);
            let c = read_file_limited(path)?;
            let backup = c.clone();
            let on = action.ssl_enabled.unwrap_or(false);
            let mut default_le_paths = false;
            let mut issue_domain = String::new();
            let (mut cert_path, mut key_path) = if on {
                match (
                    action.ssl_cert_path.as_deref(),
                    action.ssl_key_path.as_deref(),
                ) {
                    (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => {
                        (a.to_string(), b.to_string())
                    }
                    _ => {
                        let dom = first_domain_in_content(&c);
                        if dom.is_empty() {
                            return Err(ControlError::NginxOp(
                                "set ssl: provide cert/key paths or set server_name in file first"
                                    .into(),
                            ));
                        }
                        default_le_paths = true;
                        issue_domain = dom.clone();
                        (
                            format!("/etc/letsencrypt/live/{dom}/fullchain.pem"),
                            format!("/etc/letsencrypt/live/{dom}/privkey.pem"),
                        )
                    }
                }
            } else {
                (String::new(), String::new())
            };
            if on {
                if !crate::acme_issue::letsencrypt_pem_pair_present(&cert_path, &key_path) {
                    if default_le_paths && action.issue_certificate_if_missing == Some(true) {
                        let acme_domain = action
                            .post_check_host
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty() && !s.starts_with("*."))
                            .map(String::from)
                            .unwrap_or_else(|| issue_domain.clone());
                        match crate::acme_issue::issue_letsencrypt_certificate(&acme_domain, action) {
                            Ok((c, k)) => {
                                cert_path = c;
                                key_path = k;
                            }
                            Err(e) => {
                                return Ok(NginxActionResponseView {
                                    ok: false,
                                    action: "set_ssl".into(),
                                    message: format!("set_ssl: {e}"),
                                    detail: None,
                                    post_check: None,
                                });
                            }
                        }
                    }
                    if !crate::acme_issue::letsencrypt_pem_pair_present(&cert_path, &key_path) {
                        return Ok(NginxActionResponseView {
                            ok: false,
                            action: "set_ssl".into(),
                            message: format!("set_ssl: certificate or key not found: {cert_path}"),
                            detail: None,
                            post_check: None,
                        });
                    }
                }
            }
            let new_c = if on {
                add_ssl_to_first_server(&c, &cert_path, &key_path)?
            } else {
                strip_ssl_from_first_server(&c)?
            };
            if on {
                if let Some(ph) = resolved_post_check_host(action, &new_c) {
                    let ph = ph.trim();
                    if !ph.is_empty() && !ph.starts_with("*.") {
                        if let Err(e) = openssl_x509_checkhost_pem(&cert_path, ph) {
                            return Ok(NginxActionResponseView {
                                ok: false,
                                action: "set_ssl".into(),
                                message: format!(
                                    "set_ssl: certificate does not include hostname '{ph}': {e}"
                                ),
                                detail: None,
                                post_check: Some(NginxActionPostCheckView {
                                    ok: false,
                                    http_status: None,
                                    probe_host: Some(ph.to_string()),
                                    curl_exit: None,
                                    summary: "preflight: openssl x509 -checkhost failed".into(),
                                    rollback_performed: false,
                                    classified: "tls_name_mismatch".into(),
                                    details: vec![e],
                                }),
                            });
                        }
                    }
                }
            }
            let res = apply_or_tee(pirate_apply_script, ops_script, path, &new_c)?;
            if !on || !res.ok {
                return Ok(res);
            }
            set_ssl_post_check_and_maybe_rollback(
                pirate_apply_script,
                ops_script,
                path,
                &backup,
                &new_c,
                action,
                res,
            )
        }
        "validate" => {
            let (vok, msg) = nginx_test_output_via_ops(ops_script);
            Ok(NginxActionResponseView {
                ok: vok,
                action: "validate".into(),
                message: if vok { "nginx -t ok" } else { "nginx -t failed" }.into(),
                detail: Some(msg),
                post_check: None,
            })
        }
        "reload" => {
            let detail = run_systemctl_reload_nginx_via_ops(ops_script)?;
            Ok(NginxActionResponseView {
                ok: true,
                action: "reload".into(),
                message: "nginx reloaded".into(),
                detail: Some(detail),
                post_check: None,
            })
        }
        _ => Err(ControlError::NginxOp(format!(
            "unknown action: {}",
            action.action
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_server_replaces() {
        let c = "server { server_name a; }";
        let n = set_server_name_in_content(c, "b.com").unwrap();
        assert!(n.contains("server_name b.com"));
    }

    #[test]
    fn find_server_block() {
        let c = "http { } server { listen 80; }";
        assert!(find_first_server_block_inner_range(c).is_some());
    }

    #[test]
    fn extract_ssl_certificate_preserves_path_case() {
        let p = extract_ssl_certificate_path_token(
            "  SSL_CERTIFICATE /etc/letsencrypt/live/Example.COM/fullchain.pem;",
        )
        .unwrap();
        assert_eq!(p, "/etc/letsencrypt/live/Example.COM/fullchain.pem");
    }

    #[test]
    fn extract_ssl_certificate_skips_key_directive() {
        assert!(extract_ssl_certificate_path_token("ssl_certificate_key /k.pem;").is_none());
    }

    #[test]
    fn extract_ssl_certificate_variable_path() {
        let p = extract_ssl_certificate_path_token("ssl_certificate $ssl_client_cert;").unwrap();
        assert_eq!(p, "$ssl_client_cert");
    }
}
