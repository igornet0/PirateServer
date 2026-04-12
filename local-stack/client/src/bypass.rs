//! Bypass rules: exact host, `*.suffix`, IPv4/IPv6 literal, CIDR.

use ipnet::IpNet;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv6Addr};

#[derive(Debug)]
pub struct BypassMatcher {
    exact: HashSet<String>,
    /// Lowercase suffixes including leading dot, e.g. `.google.com` for `*.google.com`.
    suffixes: Vec<String>,
    cidrs: Vec<IpNet>,
}

impl BypassMatcher {
    pub fn from_rules(rules: &[String]) -> Result<Self, String> {
        let mut exact = HashSet::new();
        let mut suffixes = Vec::new();
        let mut cidrs = Vec::new();
        for raw in rules {
            let r = raw.trim();
            if r.is_empty() {
                continue;
            }
            if let Ok(net) = r.parse::<IpNet>() {
                cidrs.push(net);
                continue;
            }
            if r.contains('/') {
                return Err(format!("invalid CIDR: {r}"));
            }
            if let Ok(ip) = r.parse::<IpAddr>() {
                exact.insert(match ip {
                    IpAddr::V4(v4) => v4.to_string(),
                    IpAddr::V6(v6) => normalize_ipv6_for_match(v6),
                });
                continue;
            }
            let lc = r.to_ascii_lowercase();
            if let Some(rest) = lc.strip_prefix("*.") {
                if rest.is_empty() {
                    return Err("invalid bypass *. pattern".into());
                }
                suffixes.push(format!(".{rest}"));
                exact.insert(rest.to_string());
            } else {
                exact.insert(lc);
            }
        }
        Ok(Self {
            exact,
            suffixes,
            cidrs,
        })
    }

    pub fn matches_host(&self, host: &str) -> bool {
        let h = host.trim().to_ascii_lowercase();
        if self.exact.contains(&h) {
            return true;
        }
        for suf in &self.suffixes {
            if h.ends_with(suf.as_str()) {
                return true;
            }
        }
        if let Ok(ip) = h.parse::<IpAddr>() {
            let ip = match ip {
                IpAddr::V4(v4) => IpAddr::V4(v4),
                IpAddr::V6(v6) => IpAddr::V6(v6),
            };
            for c in &self.cidrs {
                if c.contains(&ip) {
                    return true;
                }
            }
        }
        false
    }
}

fn normalize_ipv6_for_match(ip: Ipv6Addr) -> String {
    ip.to_string().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_suffix() {
        let m = BypassMatcher::from_rules(&[
            "localhost".into(),
            "*.example.com".into(),
        ])
        .unwrap();
        assert!(m.matches_host("localhost"));
        assert!(m.matches_host("foo.example.com"));
        assert!(m.matches_host("example.com"));
        assert!(!m.matches_host("other.com"));
    }

    #[test]
    fn cidr_v4() {
        let m = BypassMatcher::from_rules(&["10.0.0.0/8".into()]).unwrap();
        assert!(m.matches_host("10.1.2.3"));
        assert!(!m.matches_host("192.168.0.1"));
    }
}
