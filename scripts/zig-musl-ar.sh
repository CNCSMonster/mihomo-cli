#!/usr/bin/env bash
# 以 zig 充当 musl 目标的静态库打包器（供 scripts/lint.sh 与 zig-musl-cc.sh 配套使用）。
set -euo pipefail

if ! command -v zig >/dev/null 2>&1; then
    echo "zig-musl-ar: zig not found in PATH (install: https://ziglang.org)" >&2
    exit 1
fi

exec zig ar "$@"
