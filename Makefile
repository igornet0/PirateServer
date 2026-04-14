# PirateServer / deploy workspace — build, install artifacts, test.
# Usage: `make` or `make help`

.PHONY: help all build build-release check test test-unit test-e2e protocol-bench redis-tunnel-docker protocol-load protocol-fuzz protocol-soak protocol-abuse clippy fmt clean \
	client client-release pirate pirate-release server server-release control-api control-api-release \
	local-agent local-agent-release \
	rust rust-release frontend frontend-install ui \
	desktop-ui deploy-dashboard-dev deploy-dashboard-build \
	pirate-desktop pirate-desktop-release pirate-desktop-bundle \
	build-local build-stack-release dist dist-manifest dist-all\
	dist-linux dist-macos dist-macos-dmg dist-windows dist-windows-msi \
	dist-only-windows dist-only-client-windows-msi dist-only-linux dist-only-macos dist-only-macos-dmg \
	dist-client-all dist-client-linux dist-client-macos dist-client-macos-dmg dist-client-windows dist-client-windows-msi \
	dist-desktop-linux dist-desktop-macos dist-desktop-macos-dmg dist-desktop-windows \
	install install-release dist-server \
	bootstrap bootstrap-phase6 e2e local-e2e docker-client-help build-desktop-ui \
	test-dist-arm64-docker-install up-version

CARGO       ?= cargo
NPM         ?= npm
# Optional cross-compile: `make client-release TARGET=x86_64-unknown-linux-gnu`
TARGET      ?=
CARGO_TARGET = $(if $(strip $(TARGET)),--target $(TARGET),)

PREFIX      ?= $(CURDIR)/dist
INSTALL_BIN ?= $(PREFIX)/bin
# Linux tar.gz bundle: 1 = include server-stack/frontend static UI; 0 = binaries only + .bundle-no-ui (install.sh forbids --ui)
UI_BUILD    ?= 1
# Linux desktop bundle arch (dist-linux); desktop client (Tauri) host arch selector.
ARCH        ?= amd64
DIST_RELEASE ?= 0

.DEFAULT_GOAL := help

help:
	@echo "PirateServer — Makefile targets"
	@echo ""
	@echo "Rust workspace (debug):"
	@echo "  make build          - cargo build --workspace (dev, local PC)"
	@echo "  make check          - cargo check --workspace"
	@echo "  make rust           - same as build"
	@echo ""
	@echo "Rust release (server / production binaries):"
	@echo "  make build-release  - release all crates"
	@echo "  make rust-release   - same"
	@echo "  make server-release - deploy-server only (release)"
	@echo "  make client-release - CLI 'client' only (release)"
	@echo "  make pirate-release - CLI 'pirate' only (release; auth/board)"
	@echo "  make control-api-release - control-api only (release)"
	@echo "  make build-stack-release - server + client + control-api (release, no npm)"

	@echo "Bundles:\n -Server stack:"
	@echo "  make dist                                                     - rust-release + frontend + dist/release-manifest.json (see VERSION)"
	@echo "  make dist-linux [ARCH=amd64|arm64] [UI_BUILD=1]               - Linux tar.gz (pirate-linux-<ARCH>-<VERSION>-<date>.tar.gz)"
	@echo "    macOS: uses Docker (rust:bookworm) for Rust link; LINUX_BUNDLE_HOST_BUILD=1 forces host cargo/zigbuild"
	@echo "  make dist-macos [ARCH=amd64|arm64] [UI_BUILD=1]               - macOS tar.gz (pirate-macos-<ARCH>-<VERSION>-<date>.tar.gz)"
	@echo "  make dist-macos-dmg [ARCH=amd64|arm64] [UI_BUILD=1]           - macOS DMG (pirate-macos-<ARCH>-<VERSION>-<date>.dmg)"
	@echo "  make dist-windows [ARCH=amd64|arm64] [UI_BUILD=1]             - Windows zip (pirate-windows-<ARCH>-<VERSION>-<date>.zip)"
	@echo "  make dist-only-windows                                         - all server Windows zips (amd64/arm64 × UI_BUILD 1/0); no server MSI"
	@echo "  make dist-windows-msi [ARCH=amd64|arm64] [UI_BUILD=1]         - server MSI: not implemented (use dist-windows); desktop MSI: dist-client-windows-msi"
	@echo "Bundles:\n -Client stack:"
	@echo "  make dist-client-linux [ARCH=amd64|arm64] [UI_BUILD=1]        - Linux bundle (pirate-client-linux-<ARCH>-<VERSION>-<date>.tar.gz)"
	@echo "  make dist-client-macos [ARCH=amd64|arm64] [UI_BUILD=1]        - macOS bundle (pirate-client-macos-<ARCH>-<VERSION>-<date>.tar.gz)"
	@echo "  make dist-client-macos-dmg [ARCH=amd64|arm64] [UI_BUILD=1]    - macOS DMG (pirate-client-macos-<ARCH>-<VERSION>-<date>.dmg)"
	@echo "  make dist-client-windows [ARCH=amd64|arm64] [UI_BUILD=1]      - Windows bundle (pirate-client-windows-<ARCH>-<VERSION>-<date>.zip)"
	@echo "  make dist-client-windows-msi [ARCH=amd64|arm64] [UI_BUILD=1]  - Windows: WiX MSI (.msi); macOS/Linux: NSIS cross → *-nsis.zip (clang + cargo-xwin + makensis)"
	@echo "  note: UI_BUILD=0 — без статики дашборда, архив с .bundle-no-ui (установка UI недоступна)"
	@echo "Bundles:\n -Deploy dashboard desktop (server-stack/desktop-ui Tauri):"
	@echo "  make dist-desktop-linux [ARCH=amd64|arm64] [UI_BUILD=1]       - Linux (deploy-dashboard-desktop-linux-<ARCH>-<VERSION>-<date>.tar.gz; .deb inside; host: Linux)"
	@echo "  make dist-desktop-macos [ARCH=amd64|arm64] [UI_BUILD=1]       - macOS app tarball (deploy-dashboard-desktop-macos-<ARCH>-<VERSION>-<date>.tar.gz; host: macOS)"
	@echo "  make dist-desktop-macos-dmg [ARCH=amd64|arm64] [UI_BUILD=1]   - macOS DMG (deploy-dashboard-desktop-macos-<ARCH>-<VERSION>-<date>.dmg; host: macOS)"
	@echo "  make dist-desktop-windows [ARCH=amd64|arm64] [UI_BUILD=1]     - Windows zip (deploy-dashboard-desktop-windows-<ARCH>-<VERSION>-<date>.zip)"
	@echo "  note: UI_BUILD в Tauri (dist-client-* / dist-desktop-*) — флаг игнорируется; встроенный UI всегда собирается"
	@echo "  Cross-compile:  TARGET=x86_64-unknown-linux-gnu make client-release"
	@echo ""
	@echo "Docker + Linux arm64 bundle (install.sh + client pair + version):"
	@echo "  make test-dist-arm64-docker-install   - make dist-arm64-linux, build arm64 image, run install.sh in container"
	@echo "    SKIP_DIST_BUILD=1 — reuse existing dist/pirate-linux-aarch64-*.tar.gz"
	@echo ""
	@echo "Single crates (debug):"
	@echo "  make server | client | pirate | control-api | local-agent"
	@echo "  make pirate-desktop - Tauri binary pirate-client (Vite build + cargo debug)"
	@echo "  make pirate-desktop-bundle - Tauri bundle/installer (npm run tauri:build)"
	@echo ""
	@echo "Frontend (dashboard):"
	@echo "  make frontend          - npm install + vite build → server-stack/frontend/dist"
	@echo "  make ui                - alias"
	@echo "  make build-desktop-ui  - build Pirate Client web assets only → local-stack/desktop-ui/dist"
	@echo "  make desktop-ui        - Pirate Client bundle running Tauri (npm run tauri:build)"
	@echo "  make deploy-dashboard-dev   - Tauri deploy dashboard (server-stack/desktop-ui, tauri dev; Vite proxy /api → 127.0.0.1:8080)"
	@echo "  make deploy-dashboard-build - Tauri deploy dashboard release bundle (npm run tauri:build)"
	@echo ""
	@echo "Full local dev build:"
	@echo "  make build-local    - Rust debug workspace + frontend"
	@echo ""
	@echo "Install release binaries to PREFIX/bin (default: ./dist/bin):"
	@echo "  make install-release"
	@echo "  PREFIX=/opt/deploy make install-release"
	@echo ""
	@echo "Test:"
	@echo "  make test           - unit tests (cargo) + E2E script"
	@echo "  make test-unit      - cargo test --workspace"
	@echo "  make test-e2e | e2e | local-e2e - scripts/local-e2e.sh"
	@echo "  make protocol-bench - Docker: gRPC + ProxyTunnel/wire throughput + security matrix (see scripts/run-protocol-bench.sh)"
	@echo "  make redis-tunnel-docker - Docker: Redis + DEPLOY_REDIS_URL + metrics + ProxyTunnel smoke (scripts/run-redis-tunnel-docker-tests.sh)"
	@echo "  make protocol-load  - Docker: latency p50/p95/p99 + RPS burst + /metrics (scripts/run-protocol-load.sh)"
	@echo "  make protocol-fuzz  - Docker: malformed unary fuzz + metrics smoke (scripts/run-protocol-fuzz.sh)"
	@echo "  make protocol-soak  - Docker: long status loop (scripts/run-protocol-soak.sh)"
	@echo "  make protocol-abuse - Docker: limited TCP probes (scripts/run-protocol-abuse.sh)"
	@echo ""
	@echo "Docker (тестовый стек, клиент с хоста на gRPC в контейнере):"
	@echo "  make docker-client-help   — кратко про Makefile.docker (compose: tests/docker/)"
	@echo ""
	@echo "Other:"
	@echo "  make clippy | fmt | clean"
	@echo "  make bootstrap-phase6 - scripts/bootstrap-phase6.sh (hints + build)"
	@echo "  make up-version PROJECT=... VERSION=...  - bump one version source"
	@echo "    PROJECT=client|deploy_server|control_api|dashboard_ui|release (then: make dist-manifest or make dist)"
	@echo ""

# --- Full workspace ---

all: build

build: rust
rust:
	$(CARGO) build --workspace $(CARGO_TARGET)

build-release: rust-release
rust-release:
	$(CARGO) build --workspace --release $(CARGO_TARGET)

check:
	$(CARGO) check --workspace $(CARGO_TARGET)

# --- Per-crate (debug) ---

server:
	$(CARGO) build -p deploy-server $(CARGO_TARGET)

client:
	$(CARGO) build -p deploy-client --bin client $(CARGO_TARGET)

pirate:
	$(CARGO) build -p deploy-client --bin pirate $(CARGO_TARGET)

control-api:
	$(CARGO) build -p control-api $(CARGO_TARGET)

local-agent:
	$(CARGO) build -p local-agent --bin local-agent $(CARGO_TARGET)

# --- Per-crate (release) ---

server-release:
	$(CARGO) build -p deploy-server --release $(CARGO_TARGET)

client-release:
	$(CARGO) build -p deploy-client --bin client --release $(CARGO_TARGET)

pirate-release:
	$(CARGO) build -p deploy-client --bin pirate --release $(CARGO_TARGET)

control-api-release:
	$(CARGO) build -p control-api --release $(CARGO_TARGET)

local-agent-release:
	$(CARGO) build -p local-agent --bin local-agent --release $(CARGO_TARGET)

build-stack-release: server-release client-release control-api-release
	@echo "Release binaries under target/$(if $(TARGET),$(TARGET)/,)release/: deploy-server, client, control-api"

# --- Frontend ---

frontend: ui
ui:
	cd server-stack/frontend && $(NPM) install && $(NPM) run build

frontend-install:
	cd server-stack/frontend && $(NPM) ci 2>/dev/null || $(NPM) install


desktop-ui:
	make build-desktop-ui && cd local-stack/desktop-ui && $(NPM) install && $(NPM) run tauri:build

# --- Pirate Client build bundle (desktop UI, 127.0.0.1) ---

build-desktop-ui:
	cd local-stack/desktop-ui && $(NPM) install && $(NPM) run build

pirate-desktop:
	cd local-stack/desktop-ui && $(NPM) install && $(NPM) run build && cd ../.. && $(CARGO) build -p pirate-client $(CARGO_TARGET)

pirate-desktop-release:
	cd local-stack/desktop-ui && $(NPM) install && $(NPM) run build && cd ../.. && $(CARGO) build -p pirate-client --release $(CARGO_TARGET)

# Same as pirate-desktop (Vite dist is required for the Tauri frontend).
pirate-desktop-all: pirate-desktop

pirate-desktop-bundle:
	cd local-stack/desktop-ui && $(NPM) install && $(NPM) run tauri:build

# --- Deploy dashboard (Tauri; same UI as server-stack/frontend) ---

deploy-dashboard-dev:
	cd server-stack/desktop-ui && $(NPM) install && $(NPM) run tauri:dev

deploy-dashboard-build:
	cd server-stack/desktop-ui && $(NPM) install && $(NPM) run tauri:build

# --- Combined ---

build-local: build frontend

# Release Rust workspace + dashboard static files (for nginx root) + dist/release-manifest.json.
dist: rust-release frontend dist-manifest
	@echo "Artifacts: target/.../release/*, server-stack/frontend/dist/, dist/release-manifest.json"

dist-manifest:
	@chmod +x scripts/write-release-manifest.sh scripts/read-version.sh
	./scripts/write-release-manifest.sh

dist-linux:
	@chmod +x scripts/build-linux-bundle.sh scripts/linux-bundle-build.sh scripts/linux-bundle-build-rust-in-docker.sh scripts/linux-bundle-rust-docker-entry.sh scripts/read-version.sh scripts/write-server-stack-manifest.sh
	UI_BUILD=$(UI_BUILD) ARCH=$(ARCH) ./scripts/build-linux-bundle.sh

dist-macos:
	@chmod +x scripts/macos-bundle-build.sh scripts/read-version.sh scripts/write-server-stack-manifest.sh
	UI_BUILD=$(UI_BUILD) ARCH=$(ARCH) ./scripts/macos-bundle-build.sh

dist-macos-dmg:
	@chmod +x scripts/macos-bundle-build.sh scripts/read-version.sh scripts/write-server-stack-manifest.sh
	MAKE_DMG=1 UI_BUILD=$(UI_BUILD) ARCH=$(ARCH) ./scripts/macos-bundle-build.sh

dist-windows:
	@chmod +x scripts/windows-bundle-build.sh scripts/read-version.sh scripts/write-server-stack-manifest.sh
	UI_BUILD=$(UI_BUILD) ARCH=$(ARCH) ./scripts/windows-bundle-build.sh

dist-windows-msi:
	@echo "Server bundle MSI is not implemented. Use: make dist-windows (zip). For desktop MSI: make dist-client-windows-msi."
	@exit 1

dist-desktop-linux:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	DESKTOP_UI=$(CURDIR)/server-stack/desktop-ui DIST_ARTIFACT_PREFIX=deploy-dashboard-desktop WIN_EXE=deploy-dashboard-desktop.exe ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh linux-tgz

dist-desktop-macos:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	DESKTOP_UI=$(CURDIR)/server-stack/desktop-ui DIST_ARTIFACT_PREFIX=deploy-dashboard-desktop WIN_EXE=deploy-dashboard-desktop.exe ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh macos-tgz

dist-desktop-macos-dmg:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	DESKTOP_UI=$(CURDIR)/server-stack/desktop-ui DIST_ARTIFACT_PREFIX=deploy-dashboard-desktop WIN_EXE=deploy-dashboard-desktop.exe ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh macos-dmg

dist-desktop-windows:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	DESKTOP_UI=$(CURDIR)/server-stack/desktop-ui DIST_ARTIFACT_PREFIX=deploy-dashboard-desktop WIN_EXE=deploy-dashboard-desktop.exe ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh windows-zip

dist-desktop-linux-all:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-desktop-linux
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-desktop-linux
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-desktop-linux
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-desktop-linux

dist-desktop-macos-all:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-desktop-macos
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-desktop-macos
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-desktop-macos
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-desktop-macos

dist-desktop-macos-dmg-all:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-desktop-macos-dmg
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-desktop-macos-dmg
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-desktop-macos-dmg
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-desktop-macos-dmg

dist-desktop-windows-all:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-desktop-windows
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-desktop-windows
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-desktop-windows
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-desktop-windows

# Server Windows MSI is not implemented (dist-windows-msi exits 1); this target only builds zips.
dist-only-windows:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-windows
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-windows
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-windows
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-windows

dist-only-linux:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-linux
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-linux
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-linux
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-linux

dist-only-macos:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-macos
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-macos
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-macos
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-macos

dist-only-macos-dmg:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-macos-dmg
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-macos-dmg
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-macos-dmg
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-macos-dmg

dist-all: 
	$(MAKE) dist-only-linux
	$(MAKE) dist-only-windows
	$(MAKE) dist-only-macos
	$(MAKE) dist-only-macos-dmg

dist-server: dist-all

dist-client-linux:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh linux-tgz

dist-client-macos:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh macos-tgz

dist-client-macos-dmg:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh macos-dmg

dist-client-windows:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh windows-zip

dist-client-windows-msi:
	@chmod +x scripts/build-desktop-client-dist.sh scripts/read-version.sh
	ARCH=$(ARCH) UI_BUILD=$(UI_BUILD) ./scripts/build-desktop-client-dist.sh windows-msi

dist-only-client-linux:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-client-linux
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-client-linux
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-client-linux
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-client-linux

dist-only-client-macos:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-client-macos
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-client-macos
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-client-macos
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-client-macos

dist-only-client-macos-dmg:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-client-macos-dmg
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-client-macos-dmg
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-client-macos-dmg
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-client-macos-dmg

dist-only-client-windows:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-client-windows
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-client-windows
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-client-windows
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-client-windows

dist-only-client-windows-msi:
	$(MAKE) ARCH=amd64 UI_BUILD=1 dist-client-windows-msi
	$(MAKE) ARCH=arm64 UI_BUILD=1 dist-client-windows-msi
	$(MAKE) ARCH=amd64 UI_BUILD=0 dist-client-windows-msi
	$(MAKE) ARCH=arm64 UI_BUILD=0 dist-client-windows-msi

dist-client-all:
	$(MAKE) dist-only-client-linux
	$(MAKE) dist-only-client-macos
	$(MAKE) dist-only-client-macos-dmg
	$(MAKE) dist-only-client-windows
	$(MAKE) dist-only-client-windows-msi

dist-client: dist-client-all

# --- Install artifacts ---

install-release: server-release client-release control-api-release
	@mkdir -p $(INSTALL_BIN)
	@cp target/$(if $(TARGET),$(TARGET)/,)release/deploy-server $(INSTALL_BIN)/
	@cp target/$(if $(TARGET),$(TARGET)/,)release/client $(INSTALL_BIN)/
	@cp target/$(if $(TARGET),$(TARGET)/,)release/control-api $(INSTALL_BIN)/
	@echo "Installed to $(INSTALL_BIN)/ (deploy-server, client, control-api)"

install: install-release

# --- Test ---

test-unit:
	$(CARGO) test --workspace $(CARGO_TARGET) -- --nocapture

test-e2e: e2e

e2e: local-e2e

protocol-bench:
	@chmod +x scripts/run-protocol-bench.sh 2>/dev/null || true
	./scripts/run-protocol-bench.sh

redis-tunnel-docker:
	@chmod +x scripts/run-redis-tunnel-docker-tests.sh scripts/redis-tunnel-docker-tests-inner.sh 2>/dev/null || true
	./scripts/run-redis-tunnel-docker-tests.sh

protocol-load:
	@chmod +x scripts/run-protocol-load.sh 2>/dev/null || true
	./scripts/run-protocol-load.sh

protocol-fuzz:
	@chmod +x scripts/run-protocol-fuzz.sh 2>/dev/null || true
	./scripts/run-protocol-fuzz.sh

protocol-soak:
	@chmod +x scripts/run-protocol-soak.sh 2>/dev/null || true
	./scripts/run-protocol-soak.sh

protocol-abuse:
	@chmod +x scripts/run-protocol-abuse.sh 2>/dev/null || true
	./scripts/run-protocol-abuse.sh

protocols:
	@echo "==> run protocols"
	@echo "==> run redis-tunnel-docker"
	make redis-tunnel-docker
	@echo "--------------------------------"
	@echo "==> run protocol-bench"
	make protocol-bench 
	@echo "--------------------------------"
	@echo "==> run protocol-load"
	make protocol-load
	@echo "--------------------------------"
	@echo "==> run protocol-fuzz"
	make protocol-fuzz
	@echo "--------------------------------"
	@echo "==> run protocol-soak"
	make protocol-soak
	@echo "--------------------------------"
	@echo "==> run protocol-abuse"
	make protocol-abuse
	@echo "--------------------------------"

local-e2e:
	@echo "==> run local e2e"
	@chmod +x scripts/local-e2e.sh examples/test-app/build/run.sh 2>/dev/null || true
	./scripts/local-e2e.sh
	@echo "==> run routing e2e"
	@chmod +x scripts/run-routing-e2e.sh
	./scripts/run-routing-e2e.sh
	@echo "==> run wire e2e"
	@chmod +x scripts/run-wire-tunnel-e2e.sh
	./scripts/run-wire-tunnel-e2e.sh
	@echo "==> run bearer e2e"


test: test-unit test-e2e

# --- Quality ---

clippy:
	$(CARGO) clippy --workspace --all-targets $(CARGO_TARGET)

fmt:
	$(CARGO) fmt --all

# --- Bootstrap scripts ---

bootstrap: bootstrap-phase6
bootstrap-phase6:
	@chmod +x scripts/bootstrap-phase6.sh 2>/dev/null || true
	./scripts/bootstrap-phase6.sh

docker-client-help:
	@echo "Поднять стек и дернуть хостовый client (без up будет connection refused):"
	@echo "  make -f Makefile.docker up"
	@echo "  make -f Makefile.docker connection   # подсказка по URL"
	@echo "  make -f Makefile.docker client-status"
	@echo "  make -f Makefile.docker client-deploy   # опционально: тестовый деплой"
	@echo "См. также: make -f Makefile.docker help"
	@echo "Сборка aarch64 бандла + install.sh в Docker: make test-dist-arm64-docker-install"

test-dist-arm64-docker-install:
	@chmod +x scripts/docker-dist-arm64-install-e2e.sh
	./scripts/docker-dist-arm64-install-e2e.sh

up-version:
	@chmod +x scripts/up-version.sh
	./scripts/up-version.sh

# --- Clean ---

clean:
	$(CARGO) clean
	@rm -rf server-stack/frontend/dist
	@rm -rf local-stack/desktop-ui/dist
