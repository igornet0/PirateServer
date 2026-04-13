//! Bundled traffic rules from JSON (`server-stack/default-rules` schema): `_block`, `_pass`, `_our`.

use crate::bypass::BypassMatcher;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Paths to optional JSON rule files (relative to `settings.json` directory or absolute).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DefaultRulesPaths {
    #[serde(default)]
    pub block_json: Option<String>,
    #[serde(default)]
    pub pass_json: Option<String>,
    #[serde(default)]
    pub our_json: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HostRuleSet {
    bypass: BypassMatcher,
    /// Patterns that cannot be expressed as `BypassMatcher` rules (e.g. multiple `*`).
    glob_regex: Vec<Regex>,
    /// True if at least one rule was compiled in.
    has_rules: bool,
}

impl HostRuleSet {
    pub fn matches_host(&self, host: &str) -> bool {
        if self.bypass.matches_host(host) {
            return true;
        }
        let h = host.trim();
        for re in &self.glob_regex {
            if re.is_match(h) {
                return true;
            }
        }
        false
    }

    pub fn has_rules(&self) -> bool {
        self.has_rules
    }

    pub fn compile_from_strings(lines: &[String]) -> Result<Self, String> {
        let mut simple: Vec<String> = Vec::new();
        let mut glob_regex: Vec<Regex> = Vec::new();

        for raw in lines {
            let r = raw.trim();
            if r.is_empty() {
                continue;
            }
            if needs_glob_regex(r) {
                glob_regex.push(glob_pattern_to_regex(r)?);
            } else {
                simple.extend(expand_simple_rule(r));
            }
        }

        let has_rules = !simple.is_empty() || !glob_regex.is_empty();
        let bypass = BypassMatcher::from_rules(&simple)?;
        Ok(HostRuleSet {
            bypass,
            glob_regex,
            has_rules,
        })
    }
}

impl Default for HostRuleSet {
    fn default() -> Self {
        Self::compile_from_strings(&[]).expect("empty host rule set")
    }
}

fn needs_glob_regex(s: &str) -> bool {
    let c = s.chars().filter(|ch| *ch == '*').count();
    c > 1 || (c == 1 && !s.starts_with('*'))
}

fn expand_simple_rule(raw: &str) -> Vec<String> {
    let r = raw.trim();
    if r.is_empty() {
        return vec![];
    }
    if r.contains('/') && !r.contains('*') {
        return vec![r.to_string()];
    }
    if r.parse::<std::net::IpAddr>().is_ok() {
        return vec![r.to_string()];
    }
    if r.starts_with('*') && !r.starts_with("*.") {
        let rest = &r[1..];
        if !rest.is_empty() && rest.contains('.') && !rest.contains('*') {
            return vec![format!("*.{}", rest), rest.to_string()];
        }
    }
    vec![r.to_string()]
}

fn glob_pattern_to_regex(pattern: &str) -> Result<Regex, String> {
    let p = pattern.trim();
    let mut out = String::from("(?i)^");
    for ch in p.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '.' => out.push_str("\\."),
            '?' => out.push('.'),
            _ => out.push(ch),
        }
    }
    out.push('$');
    Regex::new(&out).map_err(|e| format!("regex {p}: {e}"))
}

fn flatten_json_values(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => {
            for x in a {
                out.extend(flatten_json_values(x));
            }
        }
        Value::Object(o) => {
            for (_, x) in o {
                out.extend(flatten_json_values(x));
            }
        }
        _ => {}
    }
    out
}

fn strings_from_key(obj: &serde_json::Map<String, Value>, key: &str) -> Vec<String> {
    obj.get(key).map(flatten_json_values).unwrap_or_default()
}

/// Compiled matchers for default rule bundles.
#[derive(Debug, Clone, Default)]
pub struct CompiledDefaultRules {
    pub block: HostRuleSet,
    pub pass: HostRuleSet,
    pub our: HostRuleSet,
}

impl CompiledDefaultRules {
    pub fn our_has_rules(&self) -> bool {
        self.our.has_rules()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    Block,
    Pass,
    Our,
}

/// Load one JSON file and collect known domain / pattern / ip keys into flat string lists.
fn collect_rule_lines(raw: &str, kind: RuleKind) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("JSON: {e}"))?;
    let obj = v
        .as_object()
        .ok_or("rule JSON must be an object")?;

    let (domains_key, patterns_key, ips_key) = match kind {
        RuleKind::Block => (
            "domains_block",
            "domain_patterns_block",
            "ips_block",
        ),
        RuleKind::Pass => (
            "domains_pass",
            "domain_patterns_pass",
            "ips_pass",
        ),
        RuleKind::Our => (
            "domains_our",
            "domain_patterns_our",
            "ips_our",
        ),
    };

    let mut lines = Vec::new();
    lines.extend(strings_from_key(obj, domains_key));
    lines.extend(strings_from_key(obj, patterns_key));
    lines.extend(strings_from_key(obj, ips_key));

    if kind == RuleKind::Our {
        if let Some(c) = obj.get("categories_our") {
            lines.extend(flatten_json_values(c));
        }
    }

    Ok(lines)
}

fn resolve_path(base_dir: &Path, p: &str) -> PathBuf {
    let p = p.trim();
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

/// Load and compile all configured rule files. Missing files are skipped (not an error).
pub fn compile_default_rules(
    paths: &DefaultRulesPaths,
    settings_parent: &Path,
) -> Result<Option<CompiledDefaultRules>, String> {
    if paths.block_json.is_none() && paths.pass_json.is_none() && paths.our_json.is_none() {
        return Ok(None);
    }

    let mut compiled = CompiledDefaultRules::default();

    if let Some(ref p) = paths.block_json {
        let path = resolve_path(settings_parent, p);
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            let lines = collect_rule_lines(&raw, RuleKind::Block)?;
            compiled.block = HostRuleSet::compile_from_strings(&lines)?;
        }
    }
    if let Some(ref p) = paths.pass_json {
        let path = resolve_path(settings_parent, p);
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            let lines = collect_rule_lines(&raw, RuleKind::Pass)?;
            compiled.pass = HostRuleSet::compile_from_strings(&lines)?;
        }
    }
    if let Some(ref p) = paths.our_json {
        let path = resolve_path(settings_parent, p);
        if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            let lines = collect_rule_lines(&raw, RuleKind::Our)?;
            compiled.our = HostRuleSet::compile_from_strings(&lines)?;
        }
    }

    Ok(Some(compiled))
}

/// Editable slice of a rule JSON file (desktop form).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleBundleEdit {
    pub domains: Vec<String>,
    pub domain_patterns: Vec<String>,
    pub ips: Vec<String>,
    #[serde(default)]
    pub categories_our: Option<Value>,
}

/// Parse a rule JSON file into editable fields (`kind`: `block` | `pass` | `our`).
pub fn parse_rule_bundle_json(raw: &str, kind: &str) -> Result<RuleBundleEdit, String> {
    let k = match kind.trim() {
        "block" => RuleKind::Block,
        "pass" => RuleKind::Pass,
        "our" => RuleKind::Our,
        _ => return Err(format!("kind must be block, pass, or our, got {kind:?}")),
    };
    let v: Value = if raw.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_str(raw).map_err(|e| format!("JSON: {e}"))?
    };
    let obj = v.as_object().ok_or("rule JSON must be an object")?;
    let (dk, pk, ik) = match k {
        RuleKind::Block => (
            "domains_block",
            "domain_patterns_block",
            "ips_block",
        ),
        RuleKind::Pass => (
            "domains_pass",
            "domain_patterns_pass",
            "ips_pass",
        ),
        RuleKind::Our => (
            "domains_our",
            "domain_patterns_our",
            "ips_our",
        ),
    };
    let domains = strings_from_key(obj, dk);
    let domain_patterns = strings_from_key(obj, pk);
    let ips = strings_from_key(obj, ik);
    let categories_our = if k == RuleKind::Our {
        obj.get("categories_our").cloned()
    } else {
        None
    };
    Ok(RuleBundleEdit {
        domains,
        domain_patterns,
        ips,
        categories_our,
    })
}

/// Serialize [`RuleBundleEdit`] to the JSON shape expected by [`compile_default_rules`].
pub fn serialize_rule_bundle_json(
    edit: &RuleBundleEdit,
    kind: &str,
    version: u32,
    last_updated: &str,
) -> Result<String, String> {
    let k = match kind.trim() {
        "block" => RuleKind::Block,
        "pass" => RuleKind::Pass,
        "our" => RuleKind::Our,
        _ => return Err(format!("kind must be block, pass, or our, got {kind:?}")),
    };
    let mut map = serde_json::Map::new();
    map.insert(
        "version".into(),
        Value::Number(serde_json::Number::from(version)),
    );
    map.insert(
        "last_updated".into(),
        Value::String(last_updated.to_string()),
    );
    match k {
        RuleKind::Block => {
            map.insert("domains_block".into(), Value::Array(
                edit.domains.iter().cloned().map(Value::String).collect(),
            ));
            map.insert(
                "domain_patterns_block".into(),
                Value::Array(
                    edit.domain_patterns
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            map.insert(
                "ips_block".into(),
                Value::Array(edit.ips.iter().cloned().map(Value::String).collect()),
            );
        }
        RuleKind::Pass => {
            map.insert("domains_pass".into(), Value::Array(
                edit.domains.iter().cloned().map(Value::String).collect(),
            ));
            map.insert(
                "domain_patterns_pass".into(),
                Value::Array(
                    edit.domain_patterns
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            map.insert(
                "ips_pass".into(),
                Value::Array(edit.ips.iter().cloned().map(Value::String).collect()),
            );
        }
        RuleKind::Our => {
            map.insert("domains_our".into(), Value::Array(
                edit.domains.iter().cloned().map(Value::String).collect(),
            ));
            map.insert(
                "domain_patterns_our".into(),
                Value::Array(
                    edit.domain_patterns
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            map.insert(
                "ips_our".into(),
                Value::Array(edit.ips.iter().cloned().map(Value::String).collect()),
            );
            if let Some(ref c) = edit.categories_our {
                map.insert("categories_our".into(), c.clone());
            }
        }
    }
    serde_json::to_string_pretty(&Value::Object(map)).map_err(|e| e.to_string())
}

/// Ensure JSON compiles to matchers (same checks as [`compile_default_rules`]).
pub fn validate_default_rules_json(kind: &str, raw: &str) -> Result<(), String> {
    let k = match kind.trim() {
        "block" => RuleKind::Block,
        "pass" => RuleKind::Pass,
        "our" => RuleKind::Our,
        _ => return Err(format!("kind must be block, pass, or our, got {kind:?}")),
    };
    let lines = collect_rule_lines(raw, k)?;
    HostRuleSet::compile_from_strings(&lines).map(|_| ())
}

/// Read a rule file relative to `settings_parent`, or empty edit if path missing / file missing.
pub fn read_rule_bundle_file(
    settings_parent: &std::path::Path,
    rel: Option<&str>,
    kind: &str,
) -> Result<RuleBundleEdit, String> {
    let rel = rel.map(str::trim).filter(|s| !s.is_empty());
    let Some(rel) = rel else {
        return Ok(RuleBundleEdit::default());
    };
    let path = resolve_path(settings_parent, rel);
    if !path.exists() {
        return Ok(RuleBundleEdit::default());
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_rule_bundle_json(&raw, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_matches_domain() {
        let raw = r#"{"domains_block":["ads.example.com"],"domain_patterns_block":[],"ips_block":[]}"#;
        let lines = collect_rule_lines(raw, RuleKind::Block).unwrap();
        let set = HostRuleSet::compile_from_strings(&lines).unwrap();
        assert!(set.matches_host("ads.example.com"));
        assert!(!set.matches_host("ok.com"));
    }

    #[test]
    fn our_glob_regex() {
        let lines = vec!["*.bbc.*".to_string()];
        let set = HostRuleSet::compile_from_strings(&lines).unwrap();
        assert!(set.matches_host("news.bbc.co.uk"));
    }

    #[test]
    fn rule_bundle_serialize_parse_roundtrip() {
        let e = RuleBundleEdit {
            domains: vec!["ads.example.com".into()],
            domain_patterns: vec!["*.ad.*".into()],
            ips: vec![],
            categories_our: None,
        };
        let s = serialize_rule_bundle_json(&e, "block", 1, "2026-01-01").unwrap();
        validate_default_rules_json("block", &s).unwrap();
        let p = parse_rule_bundle_json(&s, "block").unwrap();
        assert_eq!(p.domains, e.domains);
        assert_eq!(p.domain_patterns, e.domain_patterns);
    }
}
