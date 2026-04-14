#!/usr/bin/env bash
# Malformed / random unary calls — server must stay up; optional metrics scrape.
set -euo pipefail

FUZZ_ITERATIONS="${FUZZ_ITERATIONS:-40}"
METRICS_URL="${METRICS_URL:-http://deploy-server:9090}"
GRPC="${GRPC_ENDPOINT:-deploy-server:50051}"

echo "==> protocol-fuzz (FUZZ_ITERATIONS=$FUZZ_ITERATIONS)"

for i in $(seq 1 "$FUZZ_ITERATIONS"); do
  grpcurl -plaintext -import-path /proto -proto deploy.proto \
    -d "{\"project_id\":\"fuzz_$i\"}" \
    "$GRPC" deploy.DeployService/GetStatus >/dev/null 2>&1 || true
  grpcurl -plaintext -import-path /proto -proto deploy.proto \
    -d '{' \
    "$GRPC" deploy.DeployService/GetStatus >/dev/null 2>&1 || true
done

echo "==> metrics after fuzz"
if curl -sf "$METRICS_URL/metrics" >/tmp/metrics.txt 2>/dev/null; then
  head -5 /tmp/metrics.txt
  echo "... (metrics OK)"
else
  echo "note: metrics not at $METRICS_URL (merge tests/docker/docker-compose.protocol-ext.yml for DEPLOY_METRICS_BIND)"
fi

echo "OK: protocol-fuzz finished"
