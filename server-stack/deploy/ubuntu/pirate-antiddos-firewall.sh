#!/usr/bin/env bash
# Idempotent nftables rules for web ports (80/443) and optional app-port lockdown. Does not touch SSH.
# Invoked by pirate-antiddos-apply.sh as root.
set -euo pipefail

TABLE="inet pirate_antiddos"
CHAIN="input"

die() {
  echo "pirate-antiddos-firewall: $*" >&2
  exit 1
}

[[ "${EUID:-0}" -eq 0 ]] || die "must run as root"

if ! command -v nft >/dev/null 2>&1; then
  echo "pirate-antiddos-firewall: nft not installed; skipping nft rules" >&2
  exit 0
fi

ENABLED="${1:-0}"
FW_JSON="${2:-}"

if [[ "$ENABLED" != "1" ]]; then
  nft delete table inet pirate_antiddos 2>/dev/null || true
  echo "ok: antiddos nft table removed (disabled)"
  exit 0
fi

if [[ -z "$FW_JSON" || ! -f "$FW_JSON" ]]; then
  die "firewall json path missing"
fi

python3 <<PY
import json, subprocess, sys, ipaddress

path = "${FW_JSON}"
with open(path, "r", encoding="utf-8") as f:
    j = json.load(f)

fw = j.get("firewall") or {}
if not fw.get("enabled", True):
    subprocess.run(
        ["nft", "delete", "table", "inet", "pirate_antiddos"],
        stderr=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
    )
    print("ok: firewall disabled in json")
    sys.exit(0)

wl = j.get("whitelist_cidrs") or []
v4 = []
v6 = []
for c in wl:
    c = (c or "").strip()
    if not c:
        continue
    try:
        net = ipaddress.ip_network(c, strict=False)
    except ValueError:
        continue
    if net.version == 4:
        v4.append(str(net))
    else:
        v6.append(str(net))

lock = j.get("lockdown_app_ports") or {}
ports = []
if lock.get("enabled") and lock.get("tcp_ports"):
    for p in lock["tcp_ports"]:
        try:
            pi = int(p)
            if 1 <= pi <= 65535:
                ports.append(pi)
        except (TypeError, ValueError):
            pass

TABLE = "inet pirate_antiddos"
CHAIN = "input"
subprocess.run(
    ["nft", "delete", "table", "inet", "pirate_antiddos"],
    stderr=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
)
lines = [
    f"add table {TABLE}",
    f"add set {TABLE} whitelist4 {{ type ipv4_addr; flags interval; }}",
    f"add set {TABLE} whitelist6 {{ type ipv6_addr; flags interval; }}",
]
for c in v4:
    lines.append(f"add element {TABLE} whitelist4 {{ {c} }}")
for c in v6:
    lines.append(f"add element {TABLE} whitelist6 {{ {c} }}")

pd = "{ 80, 443 }"
lines.append(
    f"add chain {TABLE} {CHAIN} {{ type filter hook input priority -50; policy accept; }}"
)
lines.append(
    f"add rule {TABLE} {CHAIN} tcp dport {pd} ct state established,related accept"
)
lines.append(
    f"add rule {TABLE} {CHAIN} tcp dport {pd} ip saddr @whitelist4 accept"
)
lines.append(
    f"add rule {TABLE} {CHAIN} tcp dport {pd} ip6 saddr @whitelist6 accept"
)
lines.append(
    f"add rule {TABLE} {CHAIN} tcp dport {pd} ct state new limit rate 200/second burst 400 packets accept"
)
lines.append(f"add rule {TABLE} {CHAIN} tcp dport {pd} ct state new drop")

if ports:
    app_pd = "{ " + ", ".join(str(p) for p in sorted(set(ports))) + " }"
    lines.append(
        f"add rule {TABLE} {CHAIN} tcp dport {app_pd} iifname lo accept"
    )
    lines.append(
        f"add rule {TABLE} {CHAIN} tcp dport {app_pd} iifname != lo drop"
    )

script = "\n".join(lines) + "\n"
subprocess.run(["nft", "-f", "-"], input=script.encode(), check=True)
print("ok")
PY

echo "ok: pirate-antiddos-firewall applied"
