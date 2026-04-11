#!/usr/bin/env bash
# Install MongoDB from official repo (Ubuntu LTS). Run as root.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get install -y -qq curl gnupg
. /etc/os-release
CODENAME="${VERSION_CODENAME:-jammy}"
curl -fsSL "https://www.mongodb.org/static/pgp/server-7.0.asc" | gpg -o /usr/share/keyrings/mongodb-server-7.0.gpg --dearmor
echo "deb [ signed-by=/usr/share/keyrings/mongodb-server-7.0.gpg ] https://repo.mongodb.org/apt/ubuntu ${CODENAME}/mongodb-org/7.0 multiverse" >/etc/apt/sources.list.d/mongodb-org-7.0.list
apt-get update -qq
apt-get install -y -qq mongodb-org
systemctl enable mongod
systemctl restart mongod
echo "MongoDB installed. Default bind: see /etc/mongod.conf"
