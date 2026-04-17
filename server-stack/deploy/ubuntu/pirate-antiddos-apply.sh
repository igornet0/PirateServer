#!/usr/bin/env bash
# Apply Anti-DDoS: nginx zones + snippets, sysctl, nft, fail2ban. Idempotent. Root only.
# Usage: pirate-antiddos-apply.sh [STATE_DIR]
set -euo pipefail

die() {
  echo "pirate-antiddos-apply: $*" >&2
  exit 1
}

[[ "${EUID:-0}" -eq 0 ]] || die "must run as root"

STATE_DIR="${1:-/var/lib/pirate/antiddos}"
HOST_JSON="$STATE_DIR/host.json"
PROJECTS_DIR="$STATE_DIR/projects"
# Optional 2nd arg: vhost path (must match control-api CONTROL_API_NGINX_SITE_PATH).
NGINX_SITE="${2:-${NGINX_SITE:-/etc/nginx/sites-available/pirate}}"
ZONES="/etc/nginx/conf.d/99-pirate-antiddos-zones.conf"
SNIPPET="/etc/nginx/snippets/pirate-antiddos-limits.conf"
SYSCTL="/etc/sysctl.d/99-pirate-antiddos.conf"
MARK_BEGIN="# PIRATE_ANTIDDOS_BEGIN"
MARK_END="# PIRATE_ANTIDDOS_END"

install -d -m 0755 "$STATE_DIR" "$PROJECTS_DIR" /etc/nginx/snippets
chown pirate:pirate "$STATE_DIR" "$PROJECTS_DIR" 2>/dev/null || true

if [[ ! -f "$HOST_JSON" ]]; then
  : >"$HOST_JSON"
  chown pirate:pirate "$HOST_JSON" 2>/dev/null || true
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FW_SCRIPT="$SCRIPT_DIR/pirate-antiddos-firewall.sh"
F2B_SCRIPT="$SCRIPT_DIR/pirate-antiddos-fail2ban.sh"
[[ -x "$FW_SCRIPT" ]] || FW_SCRIPT="/usr/local/lib/pirate/pirate-antiddos-firewall.sh"
[[ -x "$F2B_SCRIPT" ]] || F2B_SCRIPT="/usr/local/lib/pirate/pirate-antiddos-fail2ban.sh"

python3 <<PY
import json, os, re, subprocess, sys
from pathlib import Path

state_dir = "${STATE_DIR}"
host_path = "${HOST_JSON}"
projects_dir = Path(state_dir) / "projects"
nginx_site = "${NGINX_SITE}"
zones_path = "${ZONES}"
snippet_path = "${SNIPPET}"
sysctl_path = "${SYSCTL}"
mark_b = "${MARK_BEGIN}"
mark_e = "${MARK_END}"

def default_host():
    return {
        "schema_version": 1,
        "engine": "nginx_nft_fail2ban",
        "enabled": False,
        "aggressive": False,
        "rate_limit_rps": 10.0,
        "burst": 20,
        "max_connections_per_ip": 30,
        "client_body_timeout_sec": 12,
        "keepalive_timeout_sec": 20,
        "send_timeout_sec": 10,
        "whitelist_cidrs": [],
        "fail2ban": {"enabled": True, "bantime_sec": 600, "findtime_sec": 120, "maxretry": 10},
        "firewall": {"enabled": True},
        "lockdown_app_ports": {"enabled": False, "tcp_ports": []},
    }

def load_host():
    if not os.path.isfile(host_path):
        return default_host()
    raw = Path(host_path).read_text(encoding="utf-8", errors="replace").strip()
    if not raw:
        return default_host()
    return json.loads(raw)

def slug(s: str) -> str:
    t = re.sub(r"[^a-zA-Z0-9_]", "_", (s or "").strip())[:64]
    return t or "proj"

def clamp_rps(x) -> float:
    try:
        v = float(x)
    except (TypeError, ValueError):
        v = 10.0
    return max(0.1, min(1000.0, v))

def clamp_int(x, lo, hi, dflt):
    try:
        v = int(x)
    except (TypeError, ValueError):
        v = dflt
    return max(lo, min(hi, v))

host = load_host()
enabled = bool(host.get("enabled"))
agg = bool(host.get("aggressive"))
rps = clamp_rps(host.get("rate_limit_rps", 10))
burst = clamp_int(host.get("burst", 20), 1, 1000, 20)
mconn = clamp_int(host.get("max_connections_per_ip", 30), 1, 10000, 30)
if agg:
    rps = max(0.1, rps * 0.5)
    burst = max(1, int(burst * 0.7))
    mconn = max(1, int(mconn * 0.7))

cbt = clamp_int(host.get("client_body_timeout_sec", 12), 1, 600, 12)
kat = clamp_int(host.get("keepalive_timeout_sec", 20), 1, 3600, 20)
snt = clamp_int(host.get("send_timeout_sec", 10), 1, 600, 10)

proj_cfgs = []
if projects_dir.is_dir():
    for p in sorted(projects_dir.glob("*.json")):
        try:
            with open(p, "r", encoding="utf-8") as f:
                pj = json.load(f)
            pid = (pj.get("project_id") or p.stem).strip()
            if not pid:
                pid = p.stem
            prps = clamp_rps(pj.get("rate_limit_rps", rps))
            pburst = clamp_int(pj.get("burst", burst), 1, 1000, burst)
            pmconn = clamp_int(pj.get("max_connections_per_ip", mconn), 1, 10000, mconn)
            if pj.get("aggressive"):
                prps = max(0.1, prps * 0.5)
                pburst = max(1, int(pburst * 0.7))
                pmconn = max(1, int(pmconn * 0.7))
            proj_cfgs.append((slug(pid), prps, pburst, pmconn))
        except Exception:
            pass

lines_z = [
    "# PIRATE_ANTIDDOS generated — do not edit by hand",
    "limit_req_zone $binary_remote_addr zone=pirate_rl_main:10m rate=%sr/s;" % rps,
    "limit_conn_zone $binary_remote_addr zone=pirate_conn_main:10m;",
]
for sl, prps, _, _ in proj_cfgs:
    lines_z.append(
        "limit_req_zone $binary_remote_addr zone=pirate_rl_prj_%s:10m rate=%sr/s;"
        % (sl, prps)
    )
    lines_z.append(
        "limit_conn_zone $binary_remote_addr zone=pirate_conn_prj_%s:10m;" % sl
    )

if not enabled:
    Path(zones_path).write_text("# PIRATE_ANTIDDOS disabled\n", encoding="utf-8")
    Path(snippet_path).write_text("# empty\n", encoding="utf-8")
else:
    Path(zones_path).write_text("\n".join(lines_z) + "\n", encoding="utf-8")
    sn = [
        "# PIRATE_ANTIDDOS server limits",
        "error_log /var/log/nginx/pirate-antiddos-error.log warn;",
        "client_body_timeout %ds;" % cbt,
        "keepalive_timeout %ds;" % kat,
        "send_timeout %ds;" % snt,
        "limit_req zone=pirate_rl_main burst=%d nodelay;" % burst,
        "limit_conn pirate_conn_main %d;" % mconn,
    ]
    Path(snippet_path).write_text("\n".join(sn) + "\n", encoding="utf-8")

# Sysctl
if enabled and (host.get("firewall") or {}).get("syn_tuning", True):
    sysctl_txt = """# PIRATE_ANTIDDOS
net.ipv4.tcp_syncookies = 1
net.core.somaxconn = 4096
net.ipv4.tcp_max_syn_backlog = 8192
"""
else:
    sysctl_txt = "# PIRATE_ANTIDDOS disabled\n"
Path(sysctl_path).write_text(sysctl_txt, encoding="utf-8")

# Patch nginx site: include snippet
inc = "include /etc/nginx/snippets/pirate-antiddos-limits.conf;"
block = "\n".join([mark_b, inc, mark_e]) + "\n"
site_path = Path(nginx_site)
if site_path.is_file():
    raw = site_path.read_text(encoding="utf-8", errors="replace")
    if mark_b in raw and mark_e in raw:
        if enabled:
            raw = re.sub(
                re.escape(mark_b) + r"[\s\S]*?" + re.escape(mark_e),
                block.rstrip(),
                raw,
                count=1,
            )
        else:
            raw = re.sub(
                re.escape(mark_b) + r"[\s\S]*?" + re.escape(mark_e) + r"\n?",
                "",
                raw,
            )
    elif enabled:
        raw = raw.replace(
            "server {",
            "server {\n" + block,
            1,
        )
    site_path.write_text(raw, encoding="utf-8")

# Merge host json for sub-scripts (firewall reads full host)
with open(host_path, "w", encoding="utf-8") as f:
    json.dump(host, f, indent=2)
    f.write("\n")
PY

touch /var/log/nginx/pirate-antiddos-error.log
chown www-data:adm /var/log/nginx/pirate-antiddos-error.log 2>/dev/null || chmod 0644 /var/log/nginx/pirate-antiddos-error.log 2>/dev/null || true

# Sub-scripts (1 = host antiddos on; each script reads sub-switches from json)
SUB=0
if python3 -c "import json; h=json.load(open('${HOST_JSON}')); exit(0 if h.get('enabled') else 1)" 2>/dev/null; then
  SUB=1
fi

bash "$FW_SCRIPT" "$SUB" "$HOST_JSON" || true
bash "$F2B_SCRIPT" "$SUB" "$HOST_JSON" || true

if command -v nginx >/dev/null 2>&1; then
  nginx -t
  systemctl reload nginx
fi

sysctl --system >/dev/null 2>&1 || true

echo "ok: pirate-antiddos-apply finished"
