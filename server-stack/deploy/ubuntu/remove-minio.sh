#!/usr/bin/env bash
# Remove MinIO installed by install-minio.sh. Run as root.
set -euo pipefail
systemctl stop pirate-minio 2>/dev/null || true
systemctl disable pirate-minio 2>/dev/null || true
rm -f /etc/systemd/system/pirate-minio.service
rm -f /lib/systemd/system/pirate-minio.service
systemctl daemon-reload 2>/dev/null || true
rm -f /usr/local/bin/minio
# Optional data removal (uncomment to wipe): rm -rf /var/lib/pirate/minio
echo "MinIO binary and unit removed. Data kept at /var/lib/pirate/minio; env at /etc/pirate-minio.env (remove manually if desired)."
