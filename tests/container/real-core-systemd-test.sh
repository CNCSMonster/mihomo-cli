#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
REAL_CORE="${MIHOMO_REAL_CORE:-${HOME}/.local/bin/mihomo}"

[ -x "$REAL_CORE" ] || {
    echo "FAIL: set MIHOMO_REAL_CORE to an executable real Mihomo Core" >&2
    exit 1
}
command -v docker >/dev/null || {
    echo "FAIL: Docker is required" >&2
    exit 1
}

real_core="$(realpath "$REAL_CORE")"
case "$(file -b "$real_core")" in
    *"ELF"*) ;;
    *) echo "FAIL: MIHOMO_REAL_CORE is not an ELF executable" >&2; exit 1 ;;
esac

cd "$PROJECT_ROOT"
bash tests/container/prepare-artifacts.sh
cp "$real_core" target/container-test/mihomo-real
chmod 755 target/container-test/mihomo-real

docker build -t mihomo-cli-real-core -f tests/container/Dockerfile.real-core .
container_id=$(docker run --detach \
    --privileged \
    --hostname mihomo-real-core-test \
    --add-host mihomo-real-core-test:127.0.0.1 \
    --device /dev/net/tun \
    --cgroupns private \
    --network bridge \
    --tmpfs /run \
    --tmpfs /run/lock \
    --entrypoint /usr/bin/systemd \
    mihomo-cli-real-core)

cleanup() {
    docker rm -f "$container_id" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for _ in $(seq 1 100); do
    if docker exec "$container_id" systemctl is-system-running --wait >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

docker exec "$container_id" /tests/test-real-core-systemd.sh
