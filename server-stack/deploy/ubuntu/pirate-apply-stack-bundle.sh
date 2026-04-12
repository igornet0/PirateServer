#!/usr/bin/env bash
# Apply a staged Pirate server-stack bundle (root only). Invoked via sudo from deploy-server.
# Usage: pirate-apply-stack-bundle.sh <bundle_root_abs> <version_label> [apply_options.json]
# bundle_root: directory containing bin/deploy-server, bin/control-api (e.g. .../pirate-linux-amd64).
# Optional apply_options.json (OTA UI transitions): written by deploy-server; mode enable_ui|disable_ui.

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

# Reject path traversal / unexpected roots (deploy-server only passes paths under /var/lib/pirate).
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
if [[ "$HOST_ARCH" == "aarch64" ]] && [[ "$BIN_ARCH" == *"x86-64"* ]]; then
  echo "pirate-apply-stack-bundle.sh: bundle is x86_64 but host is aarch64" >&2
  exit 1
fi

echo "==> install binaries -> /usr/local/bin"
install -m 0755 "$BIN_DIR/deploy-server" /usr/local/bin/deploy-server
install -m 0755 "$BIN_DIR/control-api" /usr/local/bin/control-api
install -m 0755 "$BIN_DIR/client" /usr/local/bin/client
# Must match install.sh: OTA previously refreshed only `client`, leaving a stale `pirate` ELF
# when the host was installed from a bundle that shipped both names.
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

SYSTEMD_SRC="$BUNDLE_ROOT/systemd"
if [[ -d "$SYSTEMD_SRC" ]]; then
  for u in deploy-server.service control-api.service; do
    if [[ -f "$SYSTEMD_SRC/$u" ]]; then
      install -m 0644 "$SYSTEMD_SRC/$u" "/etc/systemd/system/$u"
    fi
  done
  systemctl daemon-reload
fi

echo "$VERSION_LABEL" > /var/lib/pirate/server-stack-version
chown pirate:pirate /var/lib/pirate/server-stack-version
chmod 0644 /var/lib/pirate/server-stack-version

if [[ -f "$BUNDLE_ROOT/server-stack-manifest.json" ]]; then
  install -m 0644 "$BUNDLE_ROOT/server-stack-manifest.json" /var/lib/pirate/server-stack-manifest.json
  chown pirate:pirate /var/lib/pirate/server-stack-manifest.json
fi

# Keep OTA helper in sync with the bundle (so fixes to this script ship with the next tarball).
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
import json, os, subprocess, sys

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

ENV_PATH = "/etc/pirate-deploy.env"

if mode == "disable_ui":
    subprocess.run(["rm", "-rf", "/var/lib/pirate/ui/dist"], check=False)
    d = read_env(ENV_PATH)
    for k in ("CONTROL_UI_ADMIN_USERNAME", "CONTROL_UI_ADMIN_PASSWORD", "CONTROL_API_JWT_SECRET"):
        d.pop(k, None)
    nginx_keep = bool(j.get("nginx_keep_api_proxy"))
    if not nginx_keep:
        subprocess.run(["rm", "-f", "/etc/nginx/sites-enabled/pirate"], check=False)
        subprocess.run(["systemctl", "try-reload-or-restart", "nginx"], check=False)
    else:
        ng = os.path.join(bundle, "nginx")
        src = os.path.join(ng, "nginx-pirate-api-only.conf")
        if os.path.isfile(src):
            subprocess.run(["install", "-m", "0644", src, "/etc/nginx/sites-available/pirate"], check=True)
            subprocess.run(["ln", "-sf", "/etc/nginx/sites-available/pirate",
                            "/etc/nginx/sites-enabled/pirate"], check=False)
            subprocess.run(["nginx", "-t"], check=True)
            subprocess.run(["systemctl", "reload", "nginx"], check=False)
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
        try:
            ip = subprocess.check_output(["hostname", "-I"]).decode().split()[0].strip()
        except Exception:
            ip = "127.0.0.1"
        d["DEPLOY_GRPC_PUBLIC_URL"] = f"http://{ip}:50051"
    write_env(ENV_PATH, d)
    install_nginx = bool(j.get("install_nginx"))
    if install_nginx:
        _env = {**os.environ, "DEBIAN_FRONTEND": "noninteractive"}
        subprocess.run(["apt-get", "update", "-qq"], env=_env, check=True)
        subprocess.run(
            ["apt-get", "install", "-y", "-qq", "nginx", "openssl", "ca-certificates"],
            env=_env,
            check=True,
        )
        ng = os.path.join(bundle, "nginx")
        if domain and os.path.isfile(os.path.join(ng, "nginx-pirate-site-domain.conf.in")):
            names = domain if domain.startswith("www.") else f"{domain} www.{domain}"
            tmpl = open(os.path.join(ng, "nginx-pirate-site-domain.conf.in"), "r", encoding="utf-8").read()
            open("/etc/nginx/sites-available/pirate", "w", encoding="utf-8").write(
                tmpl.replace("__pirate_SERVER_NAMES__", names)
            )
        elif os.path.isfile(os.path.join(ng, "nginx-pirate-site.conf")):
            subprocess.run(
                ["install", "-m", "0644", os.path.join(ng, "nginx-pirate-site.conf"),
                 "/etc/nginx/sites-available/pirate"],
                check=True,
            )
        if os.path.isfile("/etc/nginx/sites-enabled/default"):
            try:
                os.remove("/etc/nginx/sites-enabled/default")
            except FileNotFoundError:
                pass
        subprocess.run(["ln", "-sf", "/etc/nginx/sites-available/pirate", "/etc/nginx/sites-enabled/pirate"], check=False)
        subprocess.run(["nginx", "-t"], check=True)
        subprocess.run(["systemctl", "enable", "nginx"], check=False)
        subprocess.run(["systemctl", "restart", "nginx"], check=True)
        subprocess.run(["chmod", "o+x", "/var/lib/pirate"], check=False)
    sys.exit(0)

sys.exit(0)
PY
  _py_st=$?
  if [[ "$_py_st" -ne 0 ]]; then
    echo "pirate-apply-stack-bundle.sh: apply json handler failed" >&2
    exit 1
  fi
  _mode="$(python3 -c "import json; print(json.load(open('$APPLY_JSON_PATH')).get('mode',''))")"
  if [[ "$_mode" == "enable_ui" ]]; then
    echo "==> control-api bootstrap-grpc-key (enable_ui)"
    sudo -u pirate env DEPLOY_ROOT=/var/lib/pirate/deploy /usr/local/bin/control-api bootstrap-grpc-key || true
    GRPC_KEY_LINE='GRPC_SIGNING_KEY_PATH=/var/lib/pirate/deploy/.keys/control_api_ed25519.json'
    if grep -q '^GRPC_SIGNING_KEY_PATH=' /etc/pirate-deploy.env 2>/dev/null; then
      sed -i "s|^GRPC_SIGNING_KEY_PATH=.*|${GRPC_KEY_LINE}|" /etc/pirate-deploy.env
    else
      echo "$GRPC_KEY_LINE" >> /etc/pirate-deploy.env
    fi
    chmod 0640 /etc/pirate-deploy.env
    chown root:pirate /etc/pirate-deploy.env
  fi
fi

echo "==> schedule service restarts (delayed so gRPC client can read OK first)"
# Restart deploy-server first (gRPC), then control-api (HTTP).
STAMP="$(date +%s%N)"
systemd-run --unit="pirate-restart-stack-${STAMP}" --on-active=2s \
  /usr/bin/systemctl restart deploy-server.service

systemd-run --unit="pirate-restart-ca-${STAMP}" --on-active=5s \
  /usr/bin/systemctl restart control-api.service

echo "ok: server-stack $VERSION_LABEL staged; services will restart shortly"
