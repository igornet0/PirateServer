//! Built-in traffic rules, optional JSON bundles (`default_rules`), and legacy bypass.
//! See `tunnel_decision` for evaluation order.

use crate::bypass::BypassMatcher;
use crate::default_rules::CompiledDefaultRules;
use crate::settings::{BoardConfig, GlobalSettings, TrafficRuleSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelDecision {
    Tunnel,
    Direct,
    Block,
}

/// Host is treated as RU-web when it is not an IP literal and ends with `.ru`.
pub fn is_ru_web_host(host: &str) -> bool {
    let h = host.trim();
    if h.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    let lc = h.to_ascii_lowercase();
    lc.ends_with(".ru")
}

/// Unified routing: block lists → split-tunnel allowlists → NOT_RU_WEB → pass ∪ bypass → tunnel.
/// [`GlobalSettings::traffic_rule_source`](crate::settings::GlobalSettings::traffic_rule_source) selects
/// whether board inline lists, JSON bundles, or both apply.
pub fn tunnel_decision(
    host: &str,
    board: &BoardConfig,
    global: &GlobalSettings,
    rules: Option<&CompiledDefaultRules>,
) -> Result<TunnelDecision, String> {
    match global.traffic_rule_source() {
        TrafficRuleSource::Merged => tunnel_decision_merged(host, board, global, rules),
        TrafficRuleSource::Bundles => tunnel_decision_bundles(host, board, global, rules),
        TrafficRuleSource::Board => tunnel_decision_board(host, board, global, rules),
    }
}

/// JSON bundles ∪ board lists (original behavior).
fn tunnel_decision_merged(
    host: &str,
    board: &BoardConfig,
    global: &GlobalSettings,
    rules: Option<&CompiledDefaultRules>,
) -> Result<TunnelDecision, String> {
    // 1) Block: board anti-adw ∪ JSON `_block`
    if let Some(r) = rules {
        if r.block.matches_host(host) {
            return Ok(TunnelDecision::Block);
        }
    }

    // 2) Split-tunnel allowlist: JSON `_our` (merged)
    let json_our = rules.map(CompiledDefaultRules::our_has_rules).unwrap_or(false);
    let split_on = json_our;
    if split_on {
        let our_match = rules
            .map(|r| r.our.matches_host(host))
            .unwrap_or(false);
        if !our_match {
            return Ok(TunnelDecision::Direct);
        }
    }

    // 4) JSON `_pass` ∪ global ∪ board bypass
    let g_m = BypassMatcher::from_rules(&global.bypass)?;
    let b_m = BypassMatcher::from_rules(&board.bypass)?;
    if g_m.matches_host(host) || b_m.matches_host(host) {
        return Ok(TunnelDecision::Direct);
    }
    if let Some(r) = rules {
        if r.pass.matches_host(host) {
            return Ok(TunnelDecision::Direct);
        }
    }

    Ok(TunnelDecision::Tunnel)
}

/// Only `global.default_rules` JSON; board list fields ignored (not_ru_web still applies).
fn tunnel_decision_bundles(
    host: &str,
    _board: &BoardConfig,
    global: &GlobalSettings,
    rules: Option<&CompiledDefaultRules>,
) -> Result<TunnelDecision, String> {
    if let Some(r) = rules {
        if r.block.matches_host(host) {
            return Ok(TunnelDecision::Block);
        }
    }

    let json_our = rules.map(CompiledDefaultRules::our_has_rules).unwrap_or(false);
    if json_our {
        let our_match = rules
            .map(|r| r.our.matches_host(host))
            .unwrap_or(false);
        if !our_match {
            return Ok(TunnelDecision::Direct);
        }
    }

    let g_m = BypassMatcher::from_rules(&global.bypass)?;
    if g_m.matches_host(host) {
        return Ok(TunnelDecision::Direct);
    }
    if let Some(r) = rules {
        if r.pass.matches_host(host) {
            return Ok(TunnelDecision::Direct);
        }
    }

    Ok(TunnelDecision::Tunnel)
}

/// Only board inline lists + `global.bypass`; JSON bundles ignored.
fn tunnel_decision_board(
    host: &str,
    board: &BoardConfig,
    global: &GlobalSettings,
    _rules: Option<&CompiledDefaultRules>,
) -> Result<TunnelDecision, String> {
    let g_m = BypassMatcher::from_rules(&global.bypass)?;
    let b_m = BypassMatcher::from_rules(&board.bypass)?;
    if g_m.matches_host(host) || b_m.matches_host(host) {
        return Ok(TunnelDecision::Direct);
    }

    Ok(TunnelDecision::Tunnel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::default_rules::{CompiledDefaultRules, HostRuleSet};

    fn board() -> BoardConfig {
        BoardConfig::default()
    }

    fn global() -> GlobalSettings {
        GlobalSettings::default()
    }

    fn global_source(source: &str) -> GlobalSettings {
        let mut g = GlobalSettings::default();
        g.traffic_rule_source = source.to_string();
        g
    }

    // fn sample_compiled_rules() -> CompiledDefaultRules {
    //     let mut rules = CompiledDefaultRules::default();
    //     rules.block = HostRuleSet::compile_from_strings(&["json.blocked".into()]).unwrap();
    //     rules.pass = HostRuleSet::compile_from_strings(&["json.pass".into()]).unwrap();
    //     rules.our = HostRuleSet::compile_from_strings(&["json.tunnel".into()]).unwrap();
    //     rules
    // }

    #[test]
    fn all_disabled_tunnels() {
        let b = board();
        assert_eq!(
            tunnel_decision("example.com", &b, &global(), None).unwrap(),
            TunnelDecision::Tunnel
        );
        assert_eq!(
            tunnel_decision("x.ru", &b, &global(), None).unwrap(),
            TunnelDecision::Tunnel
        );
    }

    #[test]
    fn legacy_bypass_direct() {
        let mut g = global();
        g.bypass = vec!["local.test".into()];
        let b = board();
        assert_eq!(
            tunnel_decision("local.test", &b, &g, None).unwrap(),
            TunnelDecision::Direct
        );
    }

    #[test]
    fn bundles_ignores_board_bypass_but_keeps_global_bypass() {
        let mut rules = CompiledDefaultRules::default();
        rules.block = HostRuleSet::compile_from_strings(&["json.blocked".into()]).unwrap();
        rules.pass = HostRuleSet::compile_from_strings(&["json.pass".into()]).unwrap();
        rules.our = HostRuleSet::compile_from_strings(&[]).unwrap();
        assert!(!rules.our_has_rules());

        let g = global_source("bundles");
        let mut b = board();
        b.bypass = vec!["board.bypass.host".into()];
        assert_eq!(
            tunnel_decision("board.bypass.host", &b, &g, Some(&rules)).unwrap(),
            TunnelDecision::Tunnel,
            "bundles: board bypass ignored; no split; not in pass → tunnel"
        );

        let mut g2 = global_source("bundles");
        g2.bypass = vec!["global.bypass.host".into()];
        assert_eq!(
            tunnel_decision("global.bypass.host", &b, &g2, Some(&rules)).unwrap(),
            TunnelDecision::Direct,
            "global bypass still applies in bundles mode"
        );
    }

    #[test]
    fn json_block_and_pass_with_compiled_rules() {
        let g = global();
        let b = board();

        let mut rules = CompiledDefaultRules::default();
        rules.block = HostRuleSet::compile_from_strings(&["blocked.bad".into()]).unwrap();
        rules.pass = HostRuleSet::compile_from_strings(&["pass.example".into()]).unwrap();
        rules.our = HostRuleSet::compile_from_strings(&["tunnel.only".into()]).unwrap();

        assert_eq!(
            tunnel_decision("blocked.bad", &b, &g, Some(&rules)).unwrap(),
            TunnelDecision::Block
        );
        assert_eq!(
            tunnel_decision("pass.example", &b, &g, Some(&rules)).unwrap(),
            TunnelDecision::Direct
        );
        assert_eq!(
            tunnel_decision("tunnel.only", &b, &g, Some(&rules)).unwrap(),
            TunnelDecision::Tunnel
        );
        assert_eq!(
            tunnel_decision("other.com", &b, &g, Some(&rules)).unwrap(),
            TunnelDecision::Direct
        );
    }
}
