Bare-metal layout (example)
===========================

Install binaries (release build) to /usr/local/bin:

  deploy-server   — from cargo build -p deploy-server --release
  control-api     — from cargo build -p control-api --release
  tunnel-gateway  — optional; cargo build -p tunnel-gateway --release

Systemd units (adjust User, paths, DATABASE_URL):

  server-stack/deploy/systemd/deploy-server.service
  server-stack/deploy/systemd/control-api.service
  server-stack/deploy/systemd/tunnel-gateway.service

Typical directories:

  /deploy              — DEPLOY_ROOT (releases/, current)
  /deploy/.keys        — deploy-server keys (or DEPLOY_KEYS_DIR)

Order: PostgreSQL (if used) → deploy-server (applies DB migrations) → control-api → nginx.

See docs/PHASE6.md for environment variables.
