//! Request bus routing, HTTP execution, and journal append.

use crate::state::SharedState;
use crate::types::{
    BusRouteDecisionKind, RequestBusJournalEntry, StackTunRouteRule, TunnelMode, TunnelRole,
};
use deploy_auth::now_unix_ms;
use deploy_proto::stack_tun::{BusKv, BusRequestEnvelope};
use reqwest::header::{HeaderMap as ReqHeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use std::sync::Arc;

pub const JOURNAL_CAP: usize = 512;

/// Sort rules highest priority first, stable by id for ties.
pub fn sorted_rules(rules: &[StackTunRouteRule]) -> Vec<StackTunRouteRule> {
    let mut out: Vec<StackTunRouteRule> = rules.to_vec();
    out.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

pub(crate) fn rule_matches(rule: &StackTunRouteRule, host: &str, path: &str, method: &str) -> bool {
    if let Some(pref) = &rule.host_contains {
        if !pref.is_empty() && !host.contains(pref.as_str()) {
            return false;
        }
    }
    if let Some(pref) = &rule.path_prefix {
        if !pref.is_empty() && !path.starts_with(pref.as_str()) {
            return false;
        }
    }
    if let Some(m) = &rule.method {
        let mm = m.trim();
        if !mm.is_empty() && !method.eq_ignore_ascii_case(mm) {
            return false;
        }
    }
    true
}

pub fn matching_rule<'a>(
    sorted_rules: &'a [StackTunRouteRule],
    host: &str,
    path: &str,
    method: &str,
) -> Option<&'a StackTunRouteRule> {
    sorted_rules
        .iter()
        .find(|r| rule_matches(r, host, path, method))
}

fn build_url(method: &str, scheme: &str, host: &str, path: &str) -> Result<String, String> {
    let _ = method;
    let sch = scheme.trim();
    let sch = if sch.is_empty() { "http" } else { sch };
    let h = host.trim();
    let pth = path.trim();
    let pth = if pth.starts_with('/') {
        pth.to_string()
    } else {
        format!("/{pth}")
    };
    Ok(format!("{}://{}{}", sch, h.trim_start_matches('/'), pth))
}

fn headers_map_to_reqwest(map: HashMap<String, String>) -> Result<ReqHeaderMap, String> {
    let mut hm = ReqHeaderMap::new();
    for (k, v) in map {
        let nk = HeaderName::from_bytes(k.trim().as_bytes())
            .map_err(|_| format!("invalid header name `{k}`"))?;
        let nv = HeaderValue::from_str(v.trim()).map_err(|_| format!("invalid header `{k}`"))?;
        hm.append(nk, nv);
    }
    Ok(hm)
}

fn fallback_connector_port(root: &crate::types::PersistRoot, listen_profile_id: &str) -> u16 {
    root.profiles
        .iter()
        .find(|c| matches!(c.role, TunnelRole::Connector) && c.listen_profile_id.as_deref() == Some(listen_profile_id))
        .map(|c| c.target_port)
        .unwrap_or(8080)
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_bus_request(
    st: &Arc<SharedState>,
    rules_slice: &[StackTunRouteRule],
    profile_id: &str,
    request_id: String,
    trace_id: String,
    source_node_id: String,
    target_node_id: String,
    hop_id: u32,
    method: String,
    scheme: String,
    host: String,
    path: String,
    headers_map: HashMap<String, String>,
    body: Vec<u8>,
) -> Result<(u16, ReqHeaderMap, Vec<u8>, BusRouteDecisionKind), String> {
    let root = st.store.read_root().await;

    let default_fallback = root
        .profiles
        .iter()
        .find(|p| {
            p.id == profile_id
                && p.role == TunnelRole::Listen
                && p.mode == TunnelMode::RequestBus
        })
        .and_then(|p| p.default_bus_decision)
        .unwrap_or(BusRouteDecisionKind::Allow);

    let sorted = sorted_rules(rules_slice);
    let host_t = host.trim();
    let path_t = path.trim();
    let method_t = method.trim();

    let matched = matching_rule(&sorted, host_t, path_t, method_t);
    let raw_decision = matched.map(|r| r.decision).unwrap_or(default_fallback);

    let journal_stub = RequestBusJournalEntry {
        ts_unix_ms: now_unix_ms(),
        hop_id,
        request_id: request_id.clone(),
        trace_id: trace_id.clone(),
        source_node_id: source_node_id.clone(),
        target_node_id: target_node_id.clone(),
        profile_id: profile_id.to_string(),
        method: method.clone(),
        host: host.clone(),
        path: path.clone(),
        status: 0,
        decision: "".into(),
        error: None,
        bytes_in: body.len(),
        bytes_out: 0,
    };

    match raw_decision {
        BusRouteDecisionKind::Deny => {
            append_journal_complete(
                st.as_ref(),
                journal_stub,
                403,
                "deny",
                None,
                0,
                Some("blocked"),
            );
            Ok((
                403,
                ReqHeaderMap::new(),
                b"Forbidden by route policy".to_vec(),
                raw_decision,
            ))
        }
        BusRouteDecisionKind::Allow => {
            let url = build_url(&method, &scheme, &host_t, &path_t)?;
            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| e.to_string())?;

            let mut rb = match method_t.to_uppercase().as_str() {
                "GET" => client.get(&url),
                "HEAD" => client.head(&url),
                "POST" => client.post(&url),
                "PUT" => client.put(&url),
                "PATCH" => client.patch(&url),
                "DELETE" => client.delete(&url),
                other => client.request(other.parse().map_err(|_| "unsupported HTTP method")?, &url),
            };

            let hm = headers_map_to_reqwest(headers_map)?;
            rb = rb.headers(hm);

            if !body.is_empty() || !matches!(method.to_uppercase().as_str(), "GET" | "HEAD") {
                rb = rb.body(body.clone());
            }

            match rb.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let rh = resp.headers().clone();
                    let bytes_out = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
                    append_journal_complete(
                        st.as_ref(),
                        journal_stub,
                        status as u32,
                        "allow",
                        None,
                        bytes_out.len(),
                        None,
                    );
                    Ok((status, rh, bytes_out, raw_decision))
                }
                Err(e) => {
                    append_journal_complete(
                        st.as_ref(),
                        journal_stub,
                        502,
                        "allow",
                        Some(e.to_string()),
                        0,
                        None,
                    );
                    Err(e.to_string())
                }
            }
        }
        BusRouteDecisionKind::Forward => {
            let fwd_url = matched.and_then(|r| r.forward_url.clone()).unwrap_or_default();
            if fwd_url.trim().is_empty() {
                return Err("forwardUrl missing on matching rule".into());
            }
            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| e.to_string())?;

            let rb = client
                .request(
                    method_t
                        .parse()
                        .map_err(|_| format!("unsupported method `{method_t}`"))?,
                    fwd_url.trim(),
                )
                .headers(headers_map_to_reqwest(headers_map)?)
                .body(body.clone());

            match rb.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let rh = resp.headers().clone();
                    let bytes_out = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
                    append_journal_complete(
                        st.as_ref(),
                        journal_stub,
                        status as u32,
                        "forward",
                        None,
                        bytes_out.len(),
                        None,
                    );
                    Ok((status, rh, bytes_out, raw_decision))
                }
                Err(e) => {
                    append_journal_complete(
                        st.as_ref(),
                        journal_stub,
                        502,
                        "forward",
                        Some(e.to_string()),
                        0,
                        None,
                    );
                    Err(e.to_string())
                }
            }
        }
        BusRouteDecisionKind::LocalHandle => {
            let (lh, lp) = if let Some(rr) = matched {
                let h = rr
                    .local_host
                    .clone()
                    .unwrap_or_else(|| "127.0.0.1".into())
                    .trim()
                    .to_string();
                let p = rr.local_port.unwrap_or_else(|| fallback_connector_port(&root, profile_id));
                (h, p)
            } else {
                ("127.0.0.1".into(), fallback_connector_port(&root, profile_id))
            };

            let path_part = if path_t.starts_with('/') {
                path_t.to_string()
            } else {
                format!("/{path_t}")
            };
            let sch = scheme.trim();
            let sch = if sch.is_empty() { "http" } else { sch };
            let url = format!("{sch}://{lh}:{lp}{path_part}");

            let client = reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(15))
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .map_err(|e| e.to_string())?;

            let mut rb = match method_t.to_uppercase().as_str() {
                "GET" => client.get(&url),
                "HEAD" => client.head(&url),
                "POST" => client.post(&url),
                "PUT" => client.put(&url),
                "PATCH" => client.patch(&url),
                "DELETE" => client.delete(&url),
                other => client.request(other.parse().map_err(|_| "unsupported HTTP method")?, &url),
            };
            rb = rb.headers(headers_map_to_reqwest(headers_map)?).body(body.clone());

            match rb.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let rh = resp.headers().clone();
                    let bytes_out = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
                    append_journal_complete(
                        st.as_ref(),
                        journal_stub,
                        status as u32,
                        "localHandle",
                        None,
                        bytes_out.len(),
                        None,
                    );
                    Ok((status, rh, bytes_out, raw_decision))
                }
                Err(e) => {
                    append_journal_complete(
                        st.as_ref(),
                        journal_stub,
                        502,
                        "localHandle",
                        Some(e.to_string()),
                        0,
                        None,
                    );
                    Err(e.to_string())
                }
            }
        }
        BusRouteDecisionKind::Queue => {
            let tq = {
                let mq = st.task_queues.lock();
                mq.get(profile_id).cloned()
            };
            let Some(tq) = tq else {
                append_journal_complete(
                    st.as_ref(),
                    journal_stub,
                    503,
                    "queue",
                    Some("task queue not initialized for listener profile".into()),
                    0,
                    None,
                );
                return Err("task queue unavailable for listener profile".into());
            };

            let headers_kv: Vec<BusKv> = headers_map
                .iter()
                .map(|(k, v)| BusKv {
                    key: k.clone(),
                    value: v.clone(),
                })
                .collect();

            let env = BusRequestEnvelope {
                request_id: request_id.clone(),
                trace_id: trace_id.clone(),
                hop_id,
                source_node_id: source_node_id.clone(),
                target_node_id: target_node_id.clone(),
                method: method.clone(),
                scheme: scheme.clone(),
                host: host.clone(),
                path: path.clone(),
                headers: headers_kv,
                body_chunk: body.clone(),
                body_finished: true,
                created_at_ms: now_unix_ms(),
            };

            let rid = match tq.enqueue(st.stats.as_ref(), env) {
                Ok(rid) => rid,
                Err("task_queue_full") => {
                    append_journal_complete(
                        st.as_ref(),
                        journal_stub,
                        503,
                        "queue",
                        Some("task queue full".into()),
                        0,
                        None,
                    );
                    return Err("task queue full".into());
                }
                Err(_) => {
                    append_journal_complete(
                        st.as_ref(),
                        journal_stub,
                        400,
                        "queue",
                        Some("enqueue failed".into()),
                        0,
                        None,
                    );
                    return Err("enqueue failed".into());
                }
            };

            let note = serde_json::json!({
                "requestId": rid,
                "phase": "queued",
                "profileId": profile_id,
            });
            let payload = serde_json::to_vec(&note).unwrap_or_else(|_| b"{\"error\":\"json\"}".to_vec());

            append_journal_complete(
                st.as_ref(),
                journal_stub,
                202,
                "queue",
                None,
                payload.len(),
                None,
            );

            Ok((
                202,
                ReqHeaderMap::new(),
                payload,
                raw_decision,
            ))
        }
    }
}

fn append_journal_complete(
    st: &SharedState,
    mut e: RequestBusJournalEntry,
    status: u32,
    decision: &'static str,
    error: Option<String>,
    bytes_out: usize,
    block_detail: Option<&'static str>,
) {
    e.status = status;
    e.decision = decision.into();
    e.error = error.or_else(|| block_detail.map(ToString::to_string));
    e.bytes_out = bytes_out;
    append_journal(st, e);
}

pub fn append_journal(st: &SharedState, mut entry: RequestBusJournalEntry) {
    if entry.request_id.trim().is_empty() {
        entry.request_id = format!(
            "{}",
            deploy_auth::now_unix_ms(),
        );
    }
    let mut q = st.request_journal.lock();
    if q.len() >= JOURNAL_CAP {
        q.pop_front();
    }
    q.push_back(entry);
}

pub fn filtered_journal_snapshot(
    st: &SharedState,
    limit: usize,
    source: Option<&str>,
    target: Option<&str>,
    profile: Option<&str>,
    status: Option<u32>,
    host_contains: Option<&str>,
    path_contains: Option<&str>,
    method_contains: Option<&str>,
    trace_id: Option<&str>,
    request_id: Option<&str>,
    errors_only: bool,
    blocked_only: bool,
) -> Vec<RequestBusJournalEntry> {
    let q = st.request_journal.lock();
    let mut rows: Vec<RequestBusJournalEntry> = q.iter().rev().cloned().collect();
    drop(q);

    if let Some(s) = source.filter(|x| !x.trim().is_empty()) {
        let s = s.trim();
        rows.retain(|e| e.source_node_id.contains(s));
    }
    if let Some(s) = target.filter(|x| !x.trim().is_empty()) {
        let s = s.trim();
        rows.retain(|e| e.target_node_id.contains(s));
    }
    if let Some(p) = profile.filter(|x| !x.trim().is_empty()) {
        rows.retain(|e| e.profile_id == p);
    }
    if let Some(st) = status {
        rows.retain(|e| e.status == st);
    }
    if let Some(h) = host_contains.filter(|x| !x.trim().is_empty()) {
        rows.retain(|e| e.host.contains(h.trim()));
    }
    if let Some(p) = path_contains.filter(|x| !x.trim().is_empty()) {
        rows.retain(|e| e.path.contains(p.trim()));
    }
    if let Some(m) = method_contains.filter(|x| !x.trim().is_empty()) {
        rows.retain(|e| e.method.eq_ignore_ascii_case(m.trim()));
    }
    if let Some(t) = trace_id.filter(|x| !x.trim().is_empty()) {
        rows.retain(|e| e.trace_id.contains(t.trim()));
    }
    if let Some(r) = request_id.filter(|x| !x.trim().is_empty()) {
        rows.retain(|e| e.request_id.contains(r.trim()));
    }
    if errors_only {
        rows.retain(|e| e.error.is_some() || e.status >= 400);
    }
    if blocked_only {
        rows.retain(|e| e.decision.eq_ignore_ascii_case("deny") || e.status == 403);
    }

    rows.truncate(limit.max(1).min(500));
    rows
}
