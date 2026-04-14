#!/usr/bin/env bash
# macOS: применение OTA-бандла server-stack (root). Аналог deploy/ubuntu/pirate-apply-stack-bundle.sh.
# Usage: pirate-apply-stack-bundle.sh <bundle_root_abs> <version_label> [apply_options.json]

set -euo pipefail

if [[ "${EUID:-0}" -ne 0 ]]; then
  echo "pirate-apply-stack-bundle.sh: must run as root" >&2
  exit 1
fi

BUNDLE_ROOT="${1:-}"
VERSION_LABEL="${2:-}"
APPLY_JSON_PATH="${3:-}"

if [[ -z "$BUNDLE_ROOT" || -z "$VERSION_LABEL" ]]; then
  echo "usage: pirate-apply-stack-bundle.sh <bundle_root_abs> <version_label> [apply_options.json]" >&2
  exit 1
fi

if [[ -n "$APPLY_JSON_PATH" ]]; then
  case "$APPLY_JSON_PATH" in
    /var/lib/pirate/*) ;;
    *)
      echo "pirate-apply-stack-bundle.sh: apply json must be under /var/lib/pirate/" >&2
      exit 1
      ;;
  esac
  if [[ ! -f "$APPLY_JSON_PATH" ]]; then
    echo "pirate-apply-stack-bundle.sh: not found: $APPLY_JSON_PATH" >&2
    exit 1
  fi
fi

case "$BUNDLE_ROOT" in
  /var/lib/pirate/*) ;;
  *)
    echo "pirate-apply-stack-bundle.sh: bundle_root must be under /var/lib/pirate/" >&2
    exit 1
    ;;
esac

if [[ ! -d "$BUNDLE_ROOT" ]]; then
  echo "pirate-apply-stack-bundle.sh: not a directory: $BUNDLE_ROOT" >&2
  exit 1
fi

BIN_DIR="$BUNDLE_ROOT/bin"
for b in deploy-server control-api client; do
  if [[ ! -f "$BIN_DIR/$b" ]]; then
    echo "pirate-apply-stack-bundle.sh: missing $BIN_DIR/$b" >&2
    exit 1
  fi
done

HOST_ARCH="$(uname -m)"
BIN_ARCH="$(file -b "$BIN_DIR/deploy-server" 2>/dev/null || true)"
if [[ "$HOST_ARCH" == "arm64" ]] && [[ "$BIN_ARCH" == *"x86_64"* ]]; then
  echo "pirate-apply-stack-bundle.sh: bundle is x86_64 but host is arm64" >&2
  exit 1
fi
if [[ "$HOST_ARCH" == "x86_64" ]] && [[ "$BIN_ARCH" == *"arm64"* ]]; then
  echo "pirate-apply-stack-bundle.sh: bundle is arm64 but host is x86_64" >&2
  exit 1
fi

echo "==> install binaries -> /usr/local/bin"
install -m 0755 "$BIN_DIR/deploy-server" /usr/local/bin/deploy-server
install -m 0755 "$BIN_DIR/control-api" /usr/local/bin/control-api
install -m 0755 "$BIN_DIR/client" /usr/local/bin/client
if [[ -f "$BIN_DIR/pirate" ]]; then
  install -m 0755 "$BIN_DIR/pirate" /usr/local/bin/pirate
else
  ( cd /usr/local/bin && ln -sf client pirate )
fi

UI_SRC="$BUNDLE_ROOT/share/ui/dist"
if [[ -f "$UI_SRC/index.html" ]]; then
  echo "==> frontend -> /var/lib/pirate/ui/dist"
  rm -rf /var/lib/pirate/ui/dist
  install -d -o pirate -g pirate -m 0755 /var/lib/pirate/ui
  cp -a "$UI_SRC" /var/lib/pirate/ui/dist
  chown -R pirate:pirate /var/lib/pirate/ui
  if command -v nginx >/dev/null 2>&1; then
    chmod o+x /var/lib/pirate 2>/dev/null || true
  fi
fi

LAUNCHD_SRC="$BUNDLE_ROOT/launchd"
if [[ -d "$LAUNCHD_SRC" ]]; then
  echo "==> launchd plists + libexec"
  install -d -m 0755 /usr/local/libexec/pirate
  for w in run-deploy-server.sh run-control-api.sh; do
    if [[ -f "$BUNDLE_ROOT/lib/pirate/$w" ]]; then
      install -m 0755 "$BUNDLE_ROOT/lib/pirate/$w" "/usr/local/libexec/pirate/$w"
    fi
  done
  for p in com.pirate.deploy-server.plist com.pirate.control-api.plist; do
    if [[ -f "$LAUNCHD_SRC/$p" ]]; then
      install -m 0644 "$LAUNCHD_SRC/$p" "/Library/LaunchDaemons/$p"
    fi
  done
  launchctl bootout system "/Library/LaunchDaemons/com.pirate.deploy-server.plist" 2>/dev/null || true
  launchctl bootout system "/Library/LaunchDaemons/com.pirate.control-api.plist" 2>/dev/null || true
  launchctl bootstrap system "/Library/LaunchDaemons/com.pirate.deploy-server.plist"
  launchctl bootstrap system "/Library/LaunchDaemons/com.pirate.control-api.plist"
fi

echo "$VERSION_LABEL" > /var/lib/pirate/server-stack-version
chown pirate:pirate /var/lib/pirate/server-stack-version
chmod 0644 /var/lib/pirate/server-stack-version

if [[ -f "$BUNDLE_ROOT/server-stack-manifest.json" ]]; then
  install -m 0644 "$BUNDLE_ROOT/server-stack-manifest.json" /var/lib/pirate/server-stack-manifest.json
  chown pirate:pirate /var/lib/pirate/server-stack-manifest.json
fi

NEW_SCRIPT="$BUNDLE_ROOT/lib/pirate/pirate-apply-stack-bundle.sh"
if [[ -f "$NEW_SCRIPT" ]]; then
  echo "==> refresh /usr/local/lib/pirate/pirate-apply-stack-bundle.sh from bundle"
  install -m 0755 "$NEW_SCRIPT" /usr/local/lib/pirate/pirate-apply-stack-bundle.sh
fi

if [[ -n "$APPLY_JSON_PATH" ]] && command -v python3 >/dev/null 2>&1; then
  echo "==> stack apply options (OTA UI transition)"
  export PIRATE_BUNDLE_ROOT="$BUNDLE_ROOT"
  export PIRATE_APPLY_JSON="$APPLY_JSON_PATH"
  python3 <<'PY'
import json, os, shutil, subprocess, sys

bundle = os.environ["PIRATE_BUNDLE_ROOT"]
with open(os.environ["PIRATE_APPLY_JSON"], "r", encoding="utf-8") as f:
    j = json.load(f)
mode = j.get("mode") or ""

def read_env(path):
    out = {}
    try:
        with open(path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#") or "=" not in line:
                    continue
                k, v = line.split("=", 1)
                out[k.strip()] = v
    except FileNotFoundError:
        pass
    return out

def write_env(path, d):
    order = [
        "DEPLOY_SQLITE_URL", "DEPLOY_ROOT", "GRPC_ENDPOINT", "CONTROL_API_PORT", "RUST_LOG",
        "CONTROL_API_BIND", "DEPLOY_ALLOW_SERVER_STACK_UPDATE", "CONTROL_API_HOST_STATS_SERIES",
        "CONTROL_API_HOST_STATS_STREAM", "CONTROL_UI_ADMIN_USERNAME", "CONTROL_UI_ADMIN_PASSWORD",
        "CONTROL_API_JWT_SECRET", "DEPLOY_GRPC_PUBLIC_URL", "GRPC_SIGNING_KEY_PATH",
    ]
    lines = []
    seen = set()
    for k in order:
        if k in d:
            lines.append(f"{k}={d[k]}")
            seen.add(k)
    for k in sorted(d.keys()):
        if k not in seen:
            lines.append(f"{k}={d[k]}")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    os.chmod(path, 0o640)
    subprocess.run(["chown", "root:pirate", path], check=False)

def brew_nginx_servers_dir():
    brew = shutil.which("brew") or ("/opt/homebrew/bin/brew" if os.path.isfile("/opt/homebrew/bin/brew") else None)
    if not brew or not os.path.isfile(brew):
        return None
    try:
        prefix = subprocess.check_output([brew, "--prefix", "nginx"], text=True).strip()
    except subprocess.CalledProcessError:
        return None
    d = os.path.join(prefix, "etc", "nginx", "servers")
    os.makedirs(d, exist_ok=True)
    return d

def nginx_reload():
    exe = shutil.which("nginx")
    if exe:
        subprocess.run([exe, "-t"], check=False)
        subprocess.run([exe, "-s", "reload"], check=False)
    subprocess.run(["brew", "services", "restart", "nginx"], check=False)

def mac_public_ip():
    try:
        out = subprocess.check_output(
            ["bash", "-c", "iface=$(route -n get default 2>/dev/null | awk '/interface:/{print $2}'); ipconfig getifaddr \"$iface\" 2>/dev/null"],
            text=True,
        ).strip()
        if out:
            return out
    except Exception:
        pass
    return "127.0.0.1"

ENV_PATH = "/etc/pirate-deploy.env"
PIRATE_CONF = "pirate.conf"

if mode == "disable_ui":
    subprocess.run(["rm", "-rf", "/var/lib/pirate/ui/dist"], check=False)
    d = read_env(ENV_PATH)
    for k in ("CONTROL_UI_ADMIN_USERNAME", "CONTROL_UI_ADMIN_PASSWORD", "CONTROL_API_JWT_SECRET"):
        d.pop(k, None)
    nginx_keep = bool(j.get("nginx_keep_api_proxy"))
    srv = brew_nginx_servers_dir()
    if not nginx_keep:
        if srv:
            p = os.path.join(srv, PIRATE_CONF)
            if os.path.isfile(p):
                os.remove(p)
        nginx_reload()
    else:
        ng = os.path.join(bundle, "nginx")
        src = os.path.join(ng, "nginx-pirate-api-only.conf")
        if os.path.isfile(src) and srv:
            subprocess.run(["install", "-m", "0644", src, os.path.join(srv, PIRATE_CONF)], check=True)
            nginx_reload()
    write_env(ENV_PATH, d)
    sys.exit(0)

if mode == "enable_ui":
    domain = (j.get("domain") or "").strip()
    user = (j.get("ui_admin_username") or "admin").strip()
    pw = (j.get("ui_admin_password") or "").strip()
    if not pw:
        pw = subprocess.check_output(["openssl", "rand", "-base64", "24"]).decode().strip()
    jwt = subprocess.check_output(["openssl", "rand", "-base64", "48"]).decode().replace("\n", "")
    d = read_env(ENV_PATH)
    d.setdefault("DEPLOY_SQLITE_URL", "sqlite:///var/lib/pirate/deploy/deploy.db")
    d.setdefault("DEPLOY_ROOT", "/var/lib/pirate/deploy")
    d.setdefault("GRPC_ENDPOINT", "http://[::1]:50051")
    d.setdefault("CONTROL_API_PORT", "8080")
    d.setdefault("RUST_LOG", "info")
    d.setdefault("CONTROL_API_BIND", "127.0.0.1")
    d["DEPLOY_ALLOW_SERVER_STACK_UPDATE"] = "1" if j.get("deploy_allow_server_stack_update") else "0"
    d["CONTROL_API_HOST_STATS_SERIES"] = "1" if j.get("control_api_host_stats_series") else "0"
    d["CONTROL_API_HOST_STATS_STREAM"] = "1" if j.get("control_api_host_stats_stream") else "0"
    d["CONTROL_UI_ADMIN_USERNAME"] = user
    d["CONTROL_UI_ADMIN_PASSWORD"] = pw
    d["CONTROL_API_JWT_SECRET"] = jwt
    if domain:
        d["DEPLOY_GRPC_PUBLIC_URL"] = f"http://{domain}:50051"
    else:
        d["DEPLOY_GRPC_PUBLIC_URL"] = f"http://{mac_public_ip()}:50051"
    write_env(ENV_PATH, d)
    install_nginx = bool(j.get("install_nginx"))
    if install_nginx:
        brew = shutil.which("brew") or ("/opt/homebrew/bin/brew" if os.path.isfile("/opt/homebrew/bin/brew") else None)
        if not brew:
            print("enable_ui: Homebrew not found; install nginx/openssl manually", file=sys.stderr)
            sys.exit(1)
        subprocess.run([brew, "install", "nginx", "openssl"], check=True)
        srv = brew_nginx_servers_dir()
        if not srv:
            print("enable_ui: could not resolve nginx servers dir", file=sys.stderr)
            sys.exit(1)
        ng = os.path.join(bundle, "nginx")
        if domain and os.path.isfile(os.path.join(ng, "nginx-pirate-site-domain.conf.in")):
            names = domain if domain.startswith("www.") else f"{domain} www.{domain}"
            tmpl = open(os.path.join(ng, "nginx-pirate-site-domain.conf.in"), "r", encoding="utf-8").read()
            open(os.path.join(srv, PIRATE_CONF), "w", encoding="utf-8").write(
                tmpl.replace("__pirate_SERVER_NAMES__", names)
            )
        elif os.path.isfile(os.path.join(ng, "nginx-pirate-site.conf")):
            subprocess.run(
                ["install", "-m", "0644", os.path.join(ng, "nginx-pirate-site.conf"),
                 os.path.join(srv, PIRATE_CONF)],
                check=True,
            )
        subprocess.run([brew, "services", "start", "nginx"], check=False)
        subprocess.run(["chmod", "o+x", "/var/lib/pirate"], check=False)
        nginx_reload()
    sys.exit(0)

sys.exit(0)
PY
  _py_st=$?
  if [[ "$_py_st" -ne 0 ]]; then
    echo "pirate-apply-stack-bundle.sh: apply json handler failed" >&2
    exit 1
  fi
  _mode="$(PIRATE_APPLY_JSON="$APPLY_JSON_PATH" python3 -c 'import json,os; print(json.load(open(os.environ["PIRATE_APPLY_JSON"])).get("mode",""))')"
  if [[ "$_mode" == "enable_ui" ]]; then
    echo "==> control-api bootstrap-grpc-key (enable_ui)"
    sudo -u pirate env DEPLOY_ROOT=/var/lib/pirate/deploy /usr/local/bin/control-api bootstrap-grpc-key || true
    export GRPC_KEY_LINE='GRPC_SIGNING_KEY_PATH=/var/lib/pirate/deploy/.keys/control_api_ed25519.json'
    python3 <<'PYGRPC'
import os, re
p = "/etc/pirate-deploy.env"
line = os.environ["GRPC_KEY_LINE"]
try:
    with open(p, "r", encoding="utf-8") as f:
        s = f.read()
except FileNotFoundError:
    s = ""
if re.search(r"^GRPC_SIGNING_KEY_PATH=", s, re.M):
    s = re.sub(r"^GRPC_SIGNING_KEY_PATH=.*$", line, s, flags=re.M)
else:
    s = s.rstrip() + ("\n" if s and not s.endswith("\n") else "") + line + "\n"
with open(p, "w", encoding="utf-8") as f:
    f.write(s)
PYGRPC
    chmod 0640 /etc/pirate-deploy.env
    chown root:pirate /etc/pirate-deploy.env
  fi
fi

echo "==> schedule service restarts (launchd)"
(
  sleep 2
  launchctl kickstart -k "system/com.pirate.deploy-server" 2>/dev/null || true
) &
(
  sleep 5
  launchctl kickstart -k "system/com.pirate.control-api" 2>/dev/null || true
) &

echo "ok: server-stack $VERSION_LABEL staged; services will restart shortly"
