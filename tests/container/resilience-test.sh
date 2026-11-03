#!/usr/bin/env bash
# Resilience E2E contracts: identity revive, drift
# observability, cache.db hygiene, stale TUN transaction diagnosis.
#
# These scripts need systemd-flavoured, privileged-topology containers (the
# daemon runs as root, /dev/net/tun is expected, 60s diagnostic ticks are
# exercised), so they cannot ride the plain `test-runner.sh` matrix.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/container-runtime.sh"

TESTS=(
    test-selection-identity-revive.sh
    test-selection-drift-observability.sh
    test-cache-db-hygiene.sh
    test-tun-transaction-stale-diagnosis.sh
)

# Optional filter: pass script names (without .sh) or a subset of TESTS.
if [[ $# -gt 0 ]]; then
    SELECTED=()
    for name in "$@"; do
        case "$name" in
            *.sh) SELECTED+=("$name") ;;
            *) SELECTED+=("test-$name.sh") ;;
        esac
    done
    TESTS=("${SELECTED[@]}")
fi

container_runtime_detect || {
    container_runtime_diagnostic
    exit 1
}

cd "$PROJECT_ROOT"
bash tests/container/prepare-artifacts.sh
container_runtime build -t mihomo-cli-resilience -f tests/container/Dockerfile.resilience .

container_id=$(container_runtime run \
    --detach \
    --privileged \
    --cgroupns private \
    --tmpfs /run \
    --tmpfs /run/lock \
    --tmpfs /tmp \
    --entrypoint /lib/systemd/systemd \
    mihomo-cli-resilience)

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

failed=()
for test_script in "${TESTS[@]}"; do
    printf '\n===== %s =====\n' "$test_script"
    if container_runtime exec "$container_id" bash "/tests/scripts/$test_script"; then
        printf '===== %s: PASS =====\n' "$test_script"
    else
        printf '===== %s: FAIL =====\n' "$test_script"
        failed+=("$test_script")
    fi
done

if [[ ${#failed[@]} -gt 0 ]]; then
    printf '\n❌ resilience contracts failed: %s\n' "${failed[*]}" >&2
    exit 1
fi
printf '\n✅ all resilience contracts passed\n'
