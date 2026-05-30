//! Placeholders in `pirate.toml` command strings: `${VAR}` or `${VAR=opt1|opt2}`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdPlaceholder {
    pub name: String,
    /// When set, UI shows a selector; otherwise a free-text field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

fn valid_placeholder_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parse `${NAME}` or `${NAME=a|b|c}` placeholders (deduped by name, first wins).
pub fn parse_cmd_placeholders(cmd: &str) -> Vec<CmdPlaceholder> {
    let mut out = Vec::<CmdPlaceholder>::new();
    let b = cmd.as_bytes();
    let mut i = 0usize;
    while i + 2 < b.len() {
        if b[i] == b'$' && b[i + 1] == b'{' {
            if let Some((ph, consumed)) = parse_placeholder_inner(&cmd[i + 2..]) {
                if !out.iter().any(|p| p.name == ph.name) {
                    out.push(ph);
                }
                i += 2 + consumed;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Merge placeholders from several command strings (build + test, etc.).
pub fn merge_cmd_placeholders(cmds: &[&str]) -> Vec<CmdPlaceholder> {
    let mut out = Vec::<CmdPlaceholder>::new();
    for cmd in cmds {
        for ph in parse_cmd_placeholders(cmd) {
            if let Some(existing) = out.iter_mut().find(|p| p.name == ph.name) {
                if existing.options.is_none() {
                    existing.options = ph.options.clone();
                }
            } else {
                out.push(ph);
            }
        }
    }
    out
}

fn parse_placeholder_inner(rest: &str) -> Option<(CmdPlaceholder, usize)> {
    let close = rest.find('}')?;
    let inner = rest[..close].trim();
    if inner.is_empty() {
        return None;
    }
    let (name, options) = match inner.split_once('=') {
        Some((n, opts)) => {
            let name = n.trim();
            let list: Vec<String> = opts
                .split('|')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            let options = if list.is_empty() { None } else { Some(list) };
            (name, options)
        }
        None => (inner, None),
    };
    if !valid_placeholder_name(name) {
        return None;
    }
    Some((
        CmdPlaceholder {
            name: name.to_string(),
            options,
        },
        close + 1,
    ))
}

/// Replace all `${…}` segments; `values` must include every placeholder name.
pub fn substitute_cmd(cmd: &str, values: &BTreeMap<String, String>) -> Result<String, String> {
    let placeholders = parse_cmd_placeholders(cmd);
    for p in &placeholders {
        let raw = values.get(&p.name).ok_or_else(|| {
            format!("missing value for parameter `{}`", p.name)
        })?;
        let v = raw.trim();
        if v.is_empty() {
            return Err(format!("empty value for parameter `{}`", p.name));
        }
        if let Some(ref opts) = p.options {
            if !opts.iter().any(|o| o == v) {
                return Err(format!(
                    "invalid value for `{}`: expected one of {}",
                    p.name,
                    opts.join(", ")
                ));
            }
        }
    }

    let mut out = String::new();
    let mut i = 0usize;
    while i < cmd.len() {
        if let Some(rest) = cmd.get(i..) {
            if let Some(after) = rest.strip_prefix("${") {
                if let Some((ph, consumed)) = parse_placeholder_inner(after) {
                    let v = values
                        .get(&ph.name)
                        .map(|s| s.trim())
                        .unwrap_or_default();
                    out.push_str(v);
                    i += 2 + consumed;
                    continue;
                }
            }
        }
        let ch = cmd[i..].chars().next().expect("char");
        out.push(ch);
        i += ch.len_utf8();
    }
    Ok(out)
}

/// Resolve command: no-op when there are no placeholders; otherwise requires `values`.
pub fn resolve_cmd_template(
    cmd: &str,
    values: Option<&BTreeMap<String, String>>,
) -> Result<String, String> {
    let placeholders = parse_cmd_placeholders(cmd);
    if placeholders.is_empty() {
        return Ok(cmd.to_string());
    }
    let map = values.ok_or_else(|| missing_placeholders_message(&placeholders))?;
    substitute_cmd(cmd, map)
}

fn missing_placeholders_message(placeholders: &[CmdPlaceholder]) -> String {
    let names: Vec<&str> = placeholders.iter().map(|p| p.name.as_str()).collect();
    format!(
        "command requires parameters: {} (set via desktop UI or pass values)",
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_selector_and_text_placeholders() {
        let ph = parse_cmd_placeholders(
            "make -f Makefile.build build-linux ${LINUX_ARCH=amd64|arm64} ${EXTRA}",
        );
        assert_eq!(ph.len(), 2);
        assert_eq!(ph[0].name, "LINUX_ARCH");
        assert_eq!(ph[0].options.as_ref().map(|v| v.as_slice()), Some(&["amd64".to_string(), "arm64".to_string()][..]));
        assert_eq!(ph[1].name, "EXTRA");
        assert!(ph[1].options.is_none());
    }

    #[test]
    fn substitute_replaces_placeholders() {
        let mut v = BTreeMap::new();
        v.insert("LINUX_ARCH".into(), "arm64".into());
        v.insert("EXTRA".into(), "--verbose".into());
        let cmd = "make ARCH=${LINUX_ARCH} ${EXTRA}";
        let out = substitute_cmd(cmd, &v).unwrap();
        assert_eq!(out, "make ARCH=arm64 --verbose");
    }

    #[test]
    fn substitute_rejects_invalid_option() {
        let mut v = BTreeMap::new();
        v.insert("LINUX_ARCH".into(), "x86".into());
        let cmd = "make ${LINUX_ARCH=amd64|arm64}";
        assert!(substitute_cmd(cmd, &v).is_err());
    }
}
