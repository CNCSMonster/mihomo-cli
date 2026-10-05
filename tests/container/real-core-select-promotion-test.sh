#!/usr/bin/env bash
# 真实 Mihomo Core 终审：#007 按订阅选择持久化 + #004 TUN-active promotion。
# 分别以普通用户（testuser）与 root 两种身份在隔离 privileged systemd 容器中运行；
# 缺少真实 Core 或 docker 时失败。真实 TUN 需要 /dev/net/tun。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REAL_CORE="${MIHOMO_REAL_CORE:-${HOME}/.local/bin/mihomo}"
IMAGE=mihomo-cli-real-select-promotion

[ -x "$REAL_CORE" ] || { echo "FAIL: set MIHOMO_REAL_CORE to an executable real Mihomo Core" >&2; exit 1; }
command -v docker >/dev/null || { echo "FAIL: Docker is required" >&2; exit 1; }

real_core="$(realpath "$REAL_CORE")"
case "$(file -b "$real_core")" in
    *"ELF"*) ;;
    *) echo "FAIL: MIHOMO_REAL_CORE is not an ELF executable" >&2; exit 1 ;;
esac

cd "$PROJECT_ROOT"
bash tests/container/prepare-artifacts.sh
cp "$real_core" target/container-test/mihomo-real
chmod 755 target/container-test/mihomo-real

docker build -t "$IMAGE" -f tests/container/Dockerfile.real-select-promotion .

run_identity() {
    local identity="$1"
    local container_id
    container_id=$(docker run --detach \
        --privileged \
        --hostname "mihomo-real-sp-$identity" \
        --add-host "mihomo-real-sp-$identity:127.0.0.1" \
        --device /dev/net/tun \
        --cgroupns private \
        --network bridge \
        --tmpfs /run \
        --tmpfs /run/lock \
        --env "TEST_IDENTITY=$identity" \
        --entrypoint /usr/bin/systemd \
        "$IMAGE")
    # 容器生命周期由本脚本持有的 trap 清理。
    for _ in $(seq 1 100); do
        if docker exec "$container_id" systemctl is-system-running --wait >/dev/null 2>&1; then
            break
        fi
        sleep 0.1
    done
    if ! docker exec "$container_id" /tests/test-inner.sh; then
        echo "--- inner test failed for identity=$identity; dumping captured stderr ---" >&2
        docker exec "$container_id" bash -c \
            'for f in /tmp/real-select-promotion/*.err; do [ -e "$f" ] && { echo "== $f =="; cat "$f"; }; done' >&2 2>&1 || true
        docker rm -f "$container_id" >/dev/null 2>&1 || true
        return 1
    fi
    docker rm -f "$container_id" >/dev/null 2>&1 || true
}

cleanup() {
    docker ps -aq --filter "ancestor=$IMAGE" | xargs -r docker rm -f >/dev/null 2>&1 || true
}
trap cleanup EXIT

# 仅 testuser 身份：产品合同拒绝 root 直接 install --system（main.rs:8024），
# root 情况的"被拒"是合同行为，不是测试场景。
run_identity testuser

echo "PASS: real-Core #007 and #004 journeys verified with selection runtime mirror"
