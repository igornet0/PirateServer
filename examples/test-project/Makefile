# TestProjectWeb — установка, разработка, релизная сборка
NPM ?= npm
PORT ?= 3000
# Почта в /privacy и /terms (см. server.mjs)
PRIVACY_DOMAIN_MAIL ?=
TERMS_DOMAIN_MAIL ?=
RELEASE_DIR := dist/test-project-web

.PHONY: help install dev start release clean

help:
	@echo "Targets:"
	@echo "  make install   — установить зависимости (npm install)"
	@echo "  make dev       — сервер в режиме разработки (node --watch), PORT=$(PORT)"
	@echo "  make start     — продакшен-запуск (node server.mjs), PORT=$(PORT), PRIVACY_DOMAIN_MAIL=…, TERMS_DOMAIN_MAIL=…"
	@echo "  make release   — собрать каталог $(RELEASE_DIR) с production node_modules"
	@echo "  make clean     — удалить $(RELEASE_DIR) и node_modules"
	@echo "  make kill-dev  - kill all dev processes 3000 port"

install:
	$(NPM) install

dev:
	PORT=$(PORT) PRIVACY_DOMAIN_MAIL=$(PRIVACY_DOMAIN_MAIL) TERMS_DOMAIN_MAIL=$(TERMS_DOMAIN_MAIL) $(NPM) run dev

start:
	PORT=$(PORT) PRIVACY_DOMAIN_MAIL=$(PRIVACY_DOMAIN_MAIL) TERMS_DOMAIN_MAIL=$(TERMS_DOMAIN_MAIL) $(NPM) start

# Копирует только нужные файлы и ставит зависимости без dev (сейчас dev-зависимостей нет).
release:
	rm -rf $(RELEASE_DIR)
	mkdir -p $(RELEASE_DIR)
	cp package.json package-lock.json index.html styles.css $(RELEASE_DIR)/
	cd $(RELEASE_DIR) && $(NPM) ci --omit=dev

clean:
	rm -rf $(RELEASE_DIR) node_modules