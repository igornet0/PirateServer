# Удобные цели для CLI `client` на хосте с установленным стеком (или из распакованного архива).
#
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

# endpoint: явный ENDPOINT или GRPC_ENDPOINT из env-файла, иначе как в client (loopback IPv6)
GRPC_ENDPOINT ?= $(shell test -n "$(ENV_FILE)" && test -f "$(ENV_FILE)" && grep -sE '^GRPC_ENDPOINT=' "$(ENV_FILE)" | sed 's/^GRPC_ENDPOINT=//' | tr -d '\r' || true)
CLIENT_ENDPOINT = $(if $(strip $(ENDPOINT)),$(ENDPOINT),$(if $(strip $(GRPC_ENDPOINT)),$(GRPC_ENDPOINT),http://[::1]:50051))

EP := --endpoint $(CLIENT_ENDPOINT)

.PHONY: help status pair print-bundle deploy rollback dashboard-user-add dashboard-user-add-interactive

help:
	@echo "Pirate — client на сервере"
	@echo ""
	@echo "  make status          — client status (gRPC к deploy-server)"
	@echo "  make pair BUNDLE=…   — pair с JSON из print-bundle или файла"
	@echo "  make print-bundle    — JSON для pair (нужен установленный стек, пользователь deploy)"
	@echo "  make deploy DIR=… VERSION=… — загрузить каталог как релиз"
	@echo "  make rollback VERSION=… — откат на версию под releases/"
	@echo "  make dashboard-user-add DASH_USER=… DASH_PASS=… — пользователь веб-дашборда (Argon2 в БД)"
	@echo "  make dashboard-user-add-interactive — то же, запрос имени и пароля в терминале"
	@echo ""
	@echo "До install.sh: CLIENT=./bin/client ENV_FILE= make status"
	@echo "Свой endpoint: ENDPOINT=http://127.0.0.1:50051 make status"

status:
	@"$(CLIENT)" $(EP) --project "$(PROJECT)" status

# BUNDLE — путь к .json или строка JSON в кавычках
pair:
	@if [[ -z "$(BUNDLE)" ]]; then echo "Задайте BUNDLE=/path/to/bundle.json или JSON-строку"; exit 1; fi
	@"$(CLIENT)" $(EP) --project "$(PROJECT)" pair --bundle "$(BUNDLE)"

print-bundle:
	@sudo -u deploy bash -c 'set -a; [[ -f "$(ENV_FILE)" ]] && . "$(ENV_FILE)"; set +a; exec "$(DEPLOY_SERVER)" --root "$(DEPLOY_ROOT)" print-install-bundle'

deploy:
	@if [[ -z "$(DIR)" ]] || [[ -z "$(VERSION)" ]]; then echo "Нужны DIR=путь и VERSION=метка"; exit 1; fi
	@"$(CLIENT)" $(EP) --project "$(PROJECT)" deploy "$(DIR)" --version "$(VERSION)"

rollback:
	@if [[ -z "$(VERSION)" ]]; then echo "Задайте VERSION=имя_каталога_в_releases"; exit 1; fi
	@"$(CLIENT)" $(EP) --project "$(PROJECT)" rollback "$(VERSION)"

# Имя и пароль пользователя веб-дашборда (control-api JWT + таблица dashboard_users).
# Альтернатива: DEPLOY_DASHBOARD_PASSWORD=… sudo -u deploy … dashboard-add-user --username u (без DASH_PASS).
DASH_USER ?=
DASH_PASS ?=

dashboard-user-add:
	@if [[ -z "$(DASH_USER)" ]] || [[ -z "$(DASH_PASS)" ]]; then \
		echo >&2 "Укажите DASH_USER=логин DASH_PASS=пароль (или см. dashboard-user-add-interactive)"; \
		exit 1; \
	fi
	@sudo -u deploy bash -c 'set -a; [[ -f "$(ENV_FILE)" ]] && . "$(ENV_FILE)"; set +a; exec "$(DEPLOY_SERVER)" --root "$(DEPLOY_ROOT)" dashboard-add-user --username "$(DASH_USER)" --password "$(DASH_PASS)"'

dashboard-user-add-interactive:
	@sudo -u deploy bash -c 'set -a; [[ -f "$(ENV_FILE)" ]] && . "$(ENV_FILE)"; set +a; \
	read -r -p "Имя пользователя дашборда: " _du; \
	read -r -s -p "Пароль: " _dp; echo; \
	exec "$(DEPLOY_SERVER)" --root "$(DEPLOY_ROOT)" dashboard-add-user --username "$$_du" --password "$$_dp"'
