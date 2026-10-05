#!/usr/bin/env bash
set -euo pipefail

: "${CLASH_CONFIG_URL:?set CLASH_CONFIG_URL to run this external test}"
: "${MIHOMO_REAL_CORE:?set MIHOMO_REAL_CORE to a real Mihomo executable}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REAL_CORE="$(realpath "$MIHOMO_REAL_CORE")"
CLI_BINARY="${MIHOMO_CLI_BINARY:-$PROJECT_ROOT/target/release/mihomo-cli}"
[ -x "$CLI_BINARY" ] || { echo "FAIL: mihomo-cli binary is not executable" >&2; exit 1; }
[ -x "$REAL_CORE" ] || { echo "FAIL: real Mihomo binary is not executable" >&2; exit 1; }
command -v docker >/dev/null || { echo "FAIL: Docker is required" >&2; exit 1; }

cd "$PROJECT_ROOT"
bash tests/container/prepare-artifacts.sh
cp "$REAL_CORE" target/container-test/mihomo-real
chmod 755 target/container-test/mihomo-real

docker build -t mihomo-cli-real-subscription-select \
  -f tests/container/Dockerfile.real-subscription-select . >/dev/null
container_id=$(docker run --detach \
  --privileged \
  --hostname mihomo-real-subscription-select \
  --add-host mihomo-real-subscription-select:127.0.0.1 \
  --device /dev/net/tun \
  --cgroupns private \
  --network bridge \
  --tmpfs /run \
  --tmpfs /run/lock \
  --env CLASH_CONFIG_URL \
  --entrypoint /usr/bin/systemd \
  mihomo-cli-real-subscription-select)
cleanup() { docker rm -f "$container_id" >/dev/null 2>&1 || true; }
trap cleanup EXIT

for _ in $(seq 1 100); do
  if docker exec "$container_id" systemctl is-system-running --wait >/dev/null 2>&1; then break; fi
  sleep 0.1
done
docker exec "$container_id" /tests/test-real-subscription-select-systemd.sh
