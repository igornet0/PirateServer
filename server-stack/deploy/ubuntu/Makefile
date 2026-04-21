# Удобные цели для CLI `client` / `pirate` на хосте с установленным стеком (или из распакованного архива).
# После install.sh: /usr/local/bin/pirate → client (один бинарник deploy-client).
# sudo make install - install.sh
# После install.sh (бинарники в /usr/local/bin, env в /etc/pirate-deploy.env):
#   sudo make status
#   sudo make print-bundle
#   sudo make pair BUNDLE=/root/bundle.json
#
# До установки (из каталога pirate-linux-amd64/):
#   make status CLIENT=./bin/client ENV_FILE=
#
# Переменные: CLIENT, ENV_FILE, ENDPOINT (перекрывает GRPC_ENDPOINT из env-файла),
#   DEPLOY_SERVER, DEPLOY_ROOT, PROJECT

SHELL := /bin/bash

CLIENT ?= /usr/local/bin/client
DEPLOY_SERVER ?= /usr/local/bin/deploy-server
ENV_FILE ?= /etc/pirate-deploy.env
DEPLOY_ROOT ?= /var/lib/pirate/deploy
PROJECT ?= default

# Helpers: из распакованного архива (lib/pirate/) или после install.sh → /usr/local/lib/pirate
LIB_PIRATE := $(shell if [ -d "$(CURDIR)/lib/pirate" ]; then echo "$(CURDIR)/lib/pirate"; else echo "/usr/local/lib/pirate"; fi)
PIRATE_DASHBOARD_ADD := $(LIB_PIRATE)/pirate-dashboard-add-user.sh
PIRATE_API_NET_WIZ := $(LIB_PIRATE)/pirate-api-network-wizard.sh

# Маркер из архива UI_BUILD=0 (make install-ui / install-all тогда недоступны).
NO_UI := $(wildcard .bundle-no-ui)

# endpoint: явный ENDPOINT или GRPC_ENDPOINT из env-файла, иначе как в client (loopback IPv6)
GRPC_ENDPOINT ?= $(shell test -n "$(ENV_FILE)" && test -f "$(ENV_FILE)" && grep -sE '^GRPC_ENDPOINT=' "$(ENV_FILE)" | sed 's/^GRPC_ENDPOINT=//' | tr -d '\r' || true)
CLIENT_ENDPOINT = $(if $(strip $(ENDPOINT)),$(ENDPOINT),$(if $(strip $(GRPC_ENDPOINT)),$(GRPC_ENDPOINT),http://[::1]:50051))

EP := --endpoint $(CLIENT_ENDPOINT)

.PHONY: help status pair print-bundle deploy rollback dashboard-user-add dashboard-user-add-interactive api-network-wizard

help:
	@echo "Pirate — client на сервере"
	@echo ""
	@echo "  make install-clear    - install.sh (только сервисы, без nginx и без статики UI)"
	@echo "  make install-nginx    - install.sh --nginx"
	@echo "  make install-ui       - install.sh --ui"
	@echo "  make install-all      - install.sh --nginx --ui (полный веб-стек)"
	@echo "  (если в каталоге есть .bundle-no-ui — install-ui и install-all недоступны)"
	@echo ""
	@echo "После install.sh (бинарники в /usr/local/bin, env в /etc/pirate-deploy.env):"
	@echo "  make status          — client status (gRPC к deploy-server; можно CLIENT=/usr/local/bin/pirate)"
	@echo "  make pair BUNDLE=…   — pair с JSON из print-bundle или файла"
	@echo "  make print-bundle    — JSON для pair (нужен установленный стек, пользователь pirate)"
	@echo "  make deploy DIR=… VERSION=… — загрузить каталог как релиз"
	@echo "  make rollback VERSION=… — откат на версию под releases/"
	@echo "  make dashboard-user-add DASH_USER=… DASH_PASS=… — пользователь дашборда; JWT в env при необходимости + restart control-api"
	@echo "  make dashboard-user-add-interactive — то же, запрос логина/пароля"
	@echo "  sudo make api-network-wizard — доступ API/дашборда: LAN или публично (nginx vs прямой :8080, URL в env)"
	@echo ""
	@echo "До install.sh: CLIENT=./bin/client ENV_FILE= make status"
	@echo "Свой endpoint: ENDPOINT=http://127.0.0.1:50051 make status"

install-clear:
	sudo ./install.sh
	@echo "Установка завершена. Настройка сети API/дашборда..."
	${MAKE} api-network-wizard

install-ui:
ifeq ($(strip $(NO_UI)),)
	sudo ./install.sh --ui
else
	@echo >&2 "Этот архив собран без UI (.bundle-no-ui); установка UI недоступна."
	@exit 1
endif
	${MAKE} api-network-wizard

install-all:
ifeq ($(strip $(NO_UI)),)
	sudo ./install.sh --nginx --ui
else
	@echo >&2 "Этот архив собран без UI (.bundle-no-ui); установка UI недоступна. Используйте: sudo make install-nginx"
	@exit 1
endif
	${MAKE} api-network-wizard

status:
	@"$(CLIENT)" $(EP) --project "$(PROJECT)" status

# BUNDLE — путь к .json или строка JSON в кавычках
pair:
	@if [[ -z "$(BUNDLE)" ]]; then echo "Задайте BUNDLE=/path/to/bundle.json или JSON-строку"; exit 1; fi
	@"$(CLIENT)" $(EP) --project "$(PROJECT)" pair --bundle "$(BUNDLE)"

print-bundle:
	@sudo -u pirate bash -c 'set -a; [[ -f "$(ENV_FILE)" ]] && . "$(ENV_FILE)"; set +a; exec "$(DEPLOY_SERVER)" --root "$(DEPLOY_ROOT)" print-install-bundle'

deploy:
	@if [[ -z "$(DIR)" ]] || [[ -z "$(VERSION)" ]]; then echo "Нужны DIR=путь и VERSION=метка"; exit 1; fi
	@"$(CLIENT)" $(EP) --project "$(PROJECT)" deploy "$(DIR)" --release "$(VERSION)"

rollback:
	@if [[ -z "$(VERSION)" ]]; then echo "Задайте VERSION=имя_каталога_в_releases"; exit 1; fi
	@"$(CLIENT)" $(EP) --project "$(PROJECT)" rollback "$(VERSION)"

# Имя и пароль пользователя веб-дашборда (Argon2 в БД). Скрипт добавляет CONTROL_API_JWT_SECRET при отсутствии и перезапускает control-api.
# Альтернатива: DEPLOY_DASHBOARD_PASSWORD в env для deploy-server (редко нужно).
DASH_USER ?=
DASH_PASS ?=

dashboard-user-add:
	@if [[ -z "$(DASH_USER)" ]] || [[ -z "$(DASH_PASS)" ]]; then \
		echo >&2 "Укажите DASH_USER=логин DASH_PASS=пароль (или см. dashboard-user-add-interactive)"; \
		exit 1; \
	fi
	@test -x "$(PIRATE_DASHBOARD_ADD)" || { echo >&2 "Нет $(PIRATE_DASHBOARD_ADD) — обновите установку или скопируйте скрипты в lib/pirate/"; exit 1; }
	@sudo PIRATE_DEPLOY_ENV="$(ENV_FILE)" DEPLOY_SERVER="$(DEPLOY_SERVER)" DEPLOY_ROOT="$(DEPLOY_ROOT)" \
		"$(PIRATE_DASHBOARD_ADD)" "$(DASH_USER)" "$(DASH_PASS)"

dashboard-user-add-interactive:
	@test -x "$(PIRATE_DASHBOARD_ADD)" || { echo >&2 "Нет $(PIRATE_DASHBOARD_ADD) — обновите установку или скопируйте скрипты в lib/pirate/"; exit 1; }
	@sudo PIRATE_DEPLOY_ENV="$(ENV_FILE)" DEPLOY_SERVER="$(DEPLOY_SERVER)" DEPLOY_ROOT="$(DEPLOY_ROOT)" \
		"$(PIRATE_DASHBOARD_ADD)"

# Интерактивно: LAN vs публичный доступ; если включён сайт nginx pirate — allow/deny в snippet; иначе bind 0.0.0.0 и URL :8080. Пишет DEPLOY_CONTROL_API_* и при необходимости DEPLOY_GRPC_PUBLIC_URL.
api-network-wizard:
	@test -x "$(PIRATE_API_NET_WIZ)" || { echo >&2 "Нет $(PIRATE_API_NET_WIZ) — обновите установку или скопируйте скрипты в lib/pirate/"; exit 1; }
	@sudo PIRATE_DEPLOY_ENV="$(ENV_FILE)" "$(PIRATE_API_NET_WIZ)"
