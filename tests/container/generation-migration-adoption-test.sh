#!/usr/bin/env bash
# legacy 迁移后显式采纳的容器入口：真实 Core + systemd + 离线。
# 镜像与容器名带 055 前缀并以 PID 区分，避免与并发会话的容器测试互相污染。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REAL_CORE="${MIHOMO_REAL_CORE:-${HOME}/.local/bin/mihomo}"
RUN_ID="$$"
IMAGE="mihomo-cli-055-migration-adoption-${RUN_ID}"
CONTAINER="mihomo-055-migration-adoption-${RUN_ID}"

[ -x "$REAL_CORE" ] || {
    echo "FAIL: set MIHOMO_REAL_CORE to an executable real Mihomo Core" >&2
    exit 1
}
real_core="$(realpath "$REAL_CORE")"
case "$(file -b "$real_core")" in
    *"ELF"*) ;;
    *) echo "FAIL: MIHOMO_REAL_CORE is not an ELF executable" >&2; exit 1 ;;
esac

source "$SCRIPT_DIR/container-runtime.sh"
container_runtime_detect || {
    container_runtime_diagnostic
    exit 1
}

cd "$PROJECT_ROOT"
bash tests/container/prepare-artifacts.sh
cp "$real_core" target/container-test/mihomo-real-055
chmod 755 target/container-test/mihomo-real-055

container_runtime build -t "$IMAGE" -f tests/container/Dockerfile.055-migration-adoption .

container_id=""
cleanup() {
    if [ -n "$container_id" ]; then
        container_runtime rm -f "$container_id" >/dev/null 2>&1 || true
    fi
    if [ -z "${MIHOMO_055_KEEP_IMAGE:-}" ]; then
        container_runtime rmi -f "$IMAGE" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

container_id=$(container_runtime run \
    --detach \
    --name "$CONTAINER" \
    --privileged \
    --hostname "mihomo-055" \
    --add-host "mihomo-055:127.0.0.1" \
    --cgroupns private \
    --network none \
    --tmpfs /run \
    --tmpfs /run/lock \
    --entrypoint /usr/bin/systemd \
    "$IMAGE")

for _ in $(seq 1 200); do
    if container_runtime exec "$container_id" systemctl is-system-running --wait >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

container_runtime exec "$container_id" /tests/scripts/test-generation-migration-adoption.sh
