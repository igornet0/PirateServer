#!/usr/bin/env bash
# Oracle Database is not installed via a single apt line. This script prints guidance only.
cat <<'EOF'
Oracle Database (XE or full) requires accepting Oracle license terms and often a
separate download from oracle.com, or Oracle Container Registry.

Typical options:
  - Oracle Database XE RPM/deb from Oracle (version-specific instructions).
  - Docker image from container-registry.oracle.com (requires Oracle account).

For client-only access from this host, install oracle-instantclient-* packages
from Oracle Linux yum/apt mirrors if available for your Ubuntu version.

No packages were installed by this script.
EOF
exit 0
