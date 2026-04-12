# PirateServer / deploy workspace — build, install artifacts, test.
# Usage: `make` or `make help`

.PHONY: help all build build-release check test test-unit test-e2e clippy fmt clean \
	client client-release pirate pirate-release server server-release control-api control-api-release \
	local-agent local-agent-release \
	rust rust-release frontend frontend-install ui \
	desktop-ui pirate-desktop pirate-desktop-release pirate-desktop-bundle \
	build-local build-stack-release dist dist-manifest dist-linux dist-arm64-linux install install-release \
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
	@echo "  make dist                           - rust-release + frontend + dist/release-manifest.json (see VERSION)"
	@echo "  make dist-linux [UI_BUILD=1]        - Linux x86_64 tar.gz (pirate-linux-amd64-<VERSION>-<date>.tar.gz)"
	@echo "  make dist-arm64-linux [UI_BUILD=1]  - Linux aarch64 tar.gz (pirate-linux-aarch64-<VERSION>-<date>.tar.gz)"
	@echo "    UI_BUILD=0 — без статики дашборда, архив с .bundle-no-ui (установка UI недоступна)"
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
	@echo ""
	@echo "Docker (тестовый стек, клиент с хоста на gRPC в контейнере):"
	@echo "  make docker-client-help   — кратко про Makefile.docker"
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

# --- Combined ---

build-local: build frontend

# Release Rust workspace + dashboard static files (for nginx root) + dist/release-manifest.json.
dist: rust-release frontend dist-manifest
	@echo "Artifacts: target/.../release/*, server-stack/frontend/dist/, dist/release-manifest.json"

dist-manifest:
	@chmod +x scripts/write-release-manifest.sh scripts/read-version.sh
	./scripts/write-release-manifest.sh

dist-linux:
	@chmod +x scripts/build-linux-bundle.sh scripts/linux-bundle-build.sh scripts/read-version.sh scripts/write-server-stack-manifest.sh
	UI_BUILD=$(UI_BUILD) ./scripts/build-linux-bundle.sh

dist-arm64-linux:
	@chmod +x scripts/build-arm64-linux-bundle.sh scripts/linux-bundle-build.sh scripts/read-version.sh scripts/write-server-stack-manifest.sh
	UI_BUILD=$(UI_BUILD) ./scripts/build-arm64-linux-bundle.sh

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
local-e2e:
	@chmod +x scripts/local-e2e.sh examples/test-app/build/run.sh 2>/dev/null || true
	./scripts/local-e2e.sh

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
