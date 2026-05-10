#!/usr/bin/env bash
# Remove Meilisearch installed by install-meilisearch.sh. Run as root.
set -euo pipefail
systemctl stop pirate-meilisearch 2>/dev/null || true
systemctl disable pirate-meilisearch 2>/dev/null || true
rm -f /etc/systemd/system/pirate-meilisearch.service
rm -f /lib/systemd/system/pirate-meilisearch.service
systemctl daemon-reload 2>/dev/null || true
rm -f /usr/local/bin/meilisearch
echo "Meilisearch binary and unit removed. Data at /var/lib/pirate/meili; env at /etc/pirate-meilisearch.env (remove manually if desired)."
