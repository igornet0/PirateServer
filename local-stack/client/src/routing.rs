//! Hostname → board id using suffix rules and default.

use crate::settings::RoutingRule;

pub fn resolve_board_for_host(host: &str, rules: &[RoutingRule], default_board: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    for r in rules {
        let suf = r.suffix.trim().to_ascii_lowercase();
        if suf.is_empty() {
            continue;
        }
        if h == suf || h.ends_with(&format!(".{suf}")) {
            return r.board.clone();
        }
    }
    default_board.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_routing() {
        let rules = vec![
            RoutingRule {
                suffix: "corp.internal".into(),
                board: "work".into(),
            },
            RoutingRule {
                suffix: "home.io".into(),
                board: "home".into(),
            },
        ];
        assert_eq!(
            resolve_board_for_host("x.corp.internal", &rules, "def"),
            "work"
        );
        assert_eq!(resolve_board_for_host("a.b.home.io", &rules, "def"), "home");
        assert_eq!(resolve_board_for_host("other.com", &rules, "def"), "def");
    }
}
