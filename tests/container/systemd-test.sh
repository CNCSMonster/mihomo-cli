#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/container-runtime.sh"

container_runtime_detect || {
    container_runtime_diagnostic
    exit 1
}

cd "$PROJECT_ROOT"
bash tests/container/prepare-artifacts.sh
container_runtime build -t mihomo-cli-systemd-contract -f tests/container/Dockerfile.tun-contract .
container_id=$(container_runtime run \
    --detach \
    --privileged \
    --hostname mihomo-test \
    --add-host mihomo-test:127.0.0.1 \
    --cgroupns private \
    --network none \
    --tmpfs /run \
    --tmpfs /run/lock \
    --entrypoint /usr/bin/systemd \
    mihomo-cli-systemd-contract)

cleanup() {
    container_runtime rm -f "$container_id" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 100); do
    if container_runtime exec "$container_id" systemctl is-system-running --wait >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

container_runtime exec "$container_id" /tests/scripts/test-systemd-contract.sh
