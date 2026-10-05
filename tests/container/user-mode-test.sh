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
container_runtime build -t mihomo-cli-user-mode -f tests/container/Dockerfile.user-mode .
container_id=$(container_runtime run \
    --detach \
    --privileged \
    --hostname mihomo-user-test \
    --add-host mihomo-user-test:127.0.0.1 \
    --cgroupns private \
    --network none \
    --tmpfs /run \
    --tmpfs /run/lock \
    --entrypoint /usr/bin/systemd \
    mihomo-cli-user-mode)

cleanup() {
    container_runtime rm -f "$container_id" >/dev/null 2>&1 || true
}
trap cleanup EXIT

diagnose_container() {
    echo "--- user-mode container systemd diagnostics ---" >&2
    container_runtime exec "$container_id" systemctl is-system-running >&2 2>/dev/null || true
    container_runtime exec "$container_id" systemctl --failed --no-pager >&2 2>/dev/null || true
    container_runtime exec "$container_id" journalctl -b --no-pager -n 80 >&2 2>/dev/null || true
}

ready=false
for _ in $(seq 1 100); do
    state=$(container_runtime exec "$container_id" systemctl is-system-running 2>/dev/null || true)
    if [[ "$state" == "running" || "$state" == "degraded" ]]; then
        ready=true
        break
    fi
    sleep 0.1
done
if [ "$ready" != "true" ]; then
    diagnose_container
    exit 1
fi

container_runtime exec "$container_id" /tests/scripts/test-user-mode.sh || {
    diagnose_container
    exit 1
}
