# shellcheck shell=bash
# Shared helpers for /etc/pirate-deploy.env (sourced by pirate-*.sh).
# Not intended to be executed directly.

pirate_env_normalize_path() {
  local f="${1:-}"
  if [[ -z "$f" ]]; then
    echo "/etc/pirate-deploy.env"
  else
    echo "$f"
  fi
}

pirate_env_get_raw() {
  local file="$1" key="$2"
  [[ -f "$file" ]] || return 1
  local line
  line="$(grep -E "^${key}=" "$file" 2>/dev/null | tail -n 1)" || return 1
  line="${line#"${key}"=}"
  # shellcheck disable=SC2001
  echo "$line" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//'
}

pirate_env_upsert() {
  local file="$1" key="$2" value="$3"
  if [[ ! -f "$file" ]]; then
    echo "pirate_env_upsert: file not found: $file" >&2
    return 1
  fi
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$file" "$key" "$value" <<'PY' || return 1
import sys
path, key, val = sys.argv[1], sys.argv[2], sys.argv[3]
prefix = key + "="
lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
out, seen = [], False
for line in lines:
    s = line.strip()
    if s and not s.startswith("#") and s.startswith(prefix):
        out.append(key + "=" + val)
        seen = True
    else:
        out.append(line)
if not seen:
    out.append(key + "=" + val)
with open(path, "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(out) + ("\n" if out else ""))
PY
  else
    echo "pirate_env_upsert: python3 required to safely edit $file" >&2
    return 1
  fi
  chmod 0640 "$file"
  chown root:pirate "$file" 2>/dev/null || true
}

pirate_restart_stack_services() {
  if command -v systemctl >/dev/null 2>&1; then
    systemctl restart deploy-server.service
    sleep 1
    systemctl restart control-api.service
  else
    echo "pirate_restart_stack_services: systemctl not found; restart deploy-server and control-api manually" >&2
  fi
}

pirate_restart_control_api_only() {
  if command -v systemctl >/dev/null 2>&1; then
    systemctl restart control-api.service
  else
    echo "pirate_restart_control_api_only: systemctl not found; restart control-api manually" >&2
  fi
}
