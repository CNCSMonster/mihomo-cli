#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ARTIFACT_DIR="$PROJECT_ROOT/target/container-test"

prepare_with_host_rust() {
    mkdir -p "$ARTIFACT_DIR"
    (
        cd "$PROJECT_ROOT"
        cargo build --locked --release
        rustc tests/container/mock/fake-mihomo-core.rs -O -o "$ARTIFACT_DIR/mihomo"
        cp target/release/mihomo-cli "$ARTIFACT_DIR/mihomo-cli"
    )
}

prepare_with_container_rust() {
    local container_cli="${CONTAINER_CLI:-docker}"
    "$container_cli" run --rm \
        -v "$PROJECT_ROOT:/src" \
        -w /src \
        rust:slim-bullseye \
        sh -c 'cargo build --locked --release && mkdir -p target/container-test && rustc tests/container/mock/fake-mihomo-core.rs -O -o target/container-test/mihomo && cp target/release/mihomo-cli target/container-test/mihomo-cli'
}

container_cli="${CONTAINER_CLI:-docker}"
host_os="$(uname -s)"
host_arch="$(rustc -vV | sed -n 's/^host: //p')"
container_arch="$("$container_cli" version --format '{{.Server.Arch}}' 2>/dev/null || true)"

# the "old daemon + new CLI" upgrade combination needs a daemon built
# before the legacy reset admission existed. It is built once from the recorded
# baseline commit and cached; when the commit is unavailable (shallow clone) the
# contract script skips that one scenario with an explicit SKIP line.
LEGACY_DAEMON_COMMIT="${MIHOMO_LEGACY_DAEMON_COMMIT:-5441e84}"
LEGACY_DAEMON_DIR="$ARTIFACT_DIR/legacy"

prepare_legacy_daemon() {
    mkdir -p "$LEGACY_DAEMON_DIR"
    if [[ -x "$LEGACY_DAEMON_DIR/mihomo-cli" ]]; then
        echo "复用现有旧 daemon 基线产物: $LEGACY_DAEMON_DIR/mihomo-cli"
        return 0
    fi
    if ! git -C "$PROJECT_ROOT" cat-file -e "${LEGACY_DAEMON_COMMIT}^{commit}" 2>/dev/null; then
        echo "prepare-artifacts: git 中缺少 $LEGACY_DAEMON_COMMIT；旧 daemon 升级组合测试将跳过" >&2
        return 0
    fi
    local work
    work="$(mktemp -d "${TMPDIR:-/tmp}/mihomo-legacy-daemon.XXXXXX")"
    git -C "$PROJECT_ROOT" archive "$LEGACY_DAEMON_COMMIT" | tar -x -C "$work"
    CARGO_TARGET_DIR="$PROJECT_ROOT/target/legacy-daemon" \
        cargo build --locked --release --manifest-path "$work/Cargo.toml"
    cp "$PROJECT_ROOT/target/legacy-daemon/release/mihomo-cli" \
        "$LEGACY_DAEMON_DIR/mihomo-cli"
    rm -rf "$work"
    echo "已构建旧 daemon 基线产物 ($LEGACY_DAEMON_COMMIT): $LEGACY_DAEMON_DIR/mihomo-cli"
}

if [[ "${MIHOMO_ARTIFACTS:-}" == "existing" ]]; then
    # 复用已就位的产物（如 musl 静态版或上一次构建），不重新编译；缺失则失败。
    if [[ ! -x "$ARTIFACT_DIR/mihomo-cli" ]]; then
        echo "prepare-artifacts: MIHOMO_ARTIFACTS=existing 但 $ARTIFACT_DIR/mihomo-cli 不存在" >&2
        exit 1
    fi
    echo "复用现有测试产物: $ARTIFACT_DIR/mihomo-cli"
elif [[ "${MIHOMO_ARTIFACTS:-}" == "container" ]]; then
    # 矩阵镜像（ubuntu:22.04 / debian:12 等）的 glibc 低于现代宿主机；host-rust 快路径产物
    # 在这些镜像内会报 `GLIBC_x.xx not found`。矩阵模式强制走容器内编译（glibc 更新）。
    prepare_with_container_rust
elif [[ "$host_os" == "Linux" && "$host_arch" == x86_64-* && "$container_arch" == "amd64" ]]; then
    prepare_with_host_rust
else
    prepare_with_container_rust
fi

prepare_legacy_daemon
