#!/usr/bin/env bash
# 以 zig 充当 musl 目标的 C 编译器（免 root 的 musl-gcc 替代，供 scripts/lint.sh 使用）。
#
# 背景：musl target 下 ring 等 crate 需要 C 编译器；宿主机通常没有 musl-gcc，
# 而安装 musl-tools 需要 root。zig 自带 musl libc 与 clang，可直接交叉编译。
#
# cc-rs 会传 GNU 风格 --target=<rust triple>（如 x86_64-unknown-linux-musl），
# zig 只认 --target=<arch>-<os>-<abi>（如 x86_64-linux-musl），故过滤后改写。
set -euo pipefail

if ! command -v zig >/dev/null 2>&1; then
    echo "zig-musl-cc: zig not found in PATH (install: https://ziglang.org)" >&2
    exit 1
fi

args=()
for a in "$@"; do
    case "$a" in
        --target=*) ;;
        *) args+=("$a") ;;
    esac
done

exec zig cc -target x86_64-linux-musl "${args[@]}"
