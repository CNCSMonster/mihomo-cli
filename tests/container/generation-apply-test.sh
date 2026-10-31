#!/usr/bin/env bash
# 代际升级应用 E2E 入口：systemd + fake core 容器内顺序执行三件套。
# 与 systemd-test.sh 同构（privileged + cgroupns private + --network none）。
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
container_runtime build -t mihomo-cli-generation-e2e -f tests/container/Dockerfile.tun-contract .

container_id=$(container_runtime run \
    --detach \
    --privileged \
    --hostname mihomo-generation \
    --add-host mihomo-generation:127.0.0.1 \
    --cgroupns private \
    --network none \
    --tmpfs /run \
    --tmpfs /run/lock \
    --entrypoint /usr/bin/systemd \
    mihomo-cli-generation-e2e)

cleanup() {
    container_runtime rm -f "$container_id" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 200); do
    if container_runtime exec "$container_id" systemctl is-system-running --wait >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

failures=0
for script in \
    test-generation-apply-failure-recovery.sh \
    test-generation-apply-reentry.sh \
    test-generation-apply-withdrawn-gate.sh; do
    echo "──── ${script} ────"
    if container_runtime exec "$container_id" "/tests/scripts/${script}"; then
        echo "PASS ${script}"
    else
        echo "FAIL ${script}"
        failures=$((failures + 1))
    fi
done

[ "$failures" -eq 0 ]
