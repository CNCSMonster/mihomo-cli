#!/usr/bin/env bash
# 开发环境初始化脚本
# 用法: scripts/init.sh [profile]
#   profile: auto | all | windows | macos | linux | tools
#   默认 auto：根据当前平台安装合适的 targets
set -euo pipefail

PROFILE="${1:-auto}"

IS_LINUX=false
IS_MACOS=false
[[ "$(uname)" == "Linux" ]] && IS_LINUX=true
[[ "$(uname)" == "Darwin" ]] && IS_MACOS=true

install_target() {
    local target="$1"
    rustup target add "$target" 2>/dev/null && echo "  ✓ $target" || echo "  ⊘ $target 已存在"
}

case "$PROFILE" in
    auto)
        echo "━━━ 自动检测平台 ━━━"
        if $IS_LINUX; then
            echo "  当前平台: Linux"
            echo ""
            echo "安装 Windows targets:"
            install_target x86_64-pc-windows-gnu
            echo ""
            echo "安装 Linux musl targets:"
            install_target x86_64-unknown-linux-musl
            install_target aarch64-unknown-linux-musl
            echo ""
            echo "安装 macOS targets（需要 cargo-zigbuild）:"
            install_target aarch64-apple-darwin
            install_target x86_64-apple-darwin
        elif $IS_MACOS; then
            echo "  当前平台: macOS"
            echo ""
            echo "安装 Windows targets:"
            install_target x86_64-pc-windows-gnu
            echo ""
            echo "安装 Linux targets:"
            install_target x86_64-unknown-linux-gnu
            install_target x86_64-unknown-linux-musl
            install_target aarch64-unknown-linux-musl
            echo ""
            echo "macOS targets 已内置（native）"
        else
            echo "  当前平台: $(uname)（未知）"
            echo "  使用 'just init all' 安装所有 targets"
        fi
        ;;
    all)
        echo "━━━ 安装所有交叉编译 targets ━━━"
        echo ""
        echo "Windows:"
        install_target x86_64-pc-windows-gnu
        echo ""
        echo "Linux:"
        install_target x86_64-unknown-linux-gnu
        install_target x86_64-unknown-linux-musl
        install_target aarch64-unknown-linux-gnu
        install_target aarch64-unknown-linux-musl
        echo ""
        echo "macOS:"
        install_target aarch64-apple-darwin
        install_target x86_64-apple-darwin
        ;;
    windows)
        echo "━━━ 安装 Windows targets ━━━"
        install_target x86_64-pc-windows-gnu
        ;;
    macos)
        echo "━━━ 安装 macOS targets ━━━"
        install_target aarch64-apple-darwin
        install_target x86_64-apple-darwin
        ;;
    linux)
        echo "━━━ 安装 Linux targets ━━━"
        install_target x86_64-unknown-linux-gnu
        install_target x86_64-unknown-linux-musl
        install_target aarch64-unknown-linux-gnu
        install_target aarch64-unknown-linux-musl
        ;;
    tools)
        # 只安装工具，不装 targets
        ;;
    *)
        echo "未知 profile: $PROFILE"
        echo "可用: auto, all, windows, macos, linux, tools"
        exit 1
        ;;
esac

if [ "$PROFILE" != "tools" ]; then
    echo ""
    echo "━━━ 安装开发工具 ━━━"
fi

# cargo-tarpaulin（覆盖率）
if ! command -v cargo-tarpaulin &>/dev/null; then
    echo "  安装 cargo-tarpaulin..."
    cargo install cargo-tarpaulin --locked
else
    echo "  ✓ cargo-tarpaulin"
fi

# cargo-zigbuild（Linux 上用于 macOS 交叉编译）
if $IS_LINUX && ([ "$PROFILE" = "auto" ] || [ "$PROFILE" = "all" ] || [ "$PROFILE" = "macos" ]); then
    if ! command -v cargo-zigbuild &>/dev/null; then
        echo "  安装 cargo-zigbuild（macOS 交叉编译需要）..."
        cargo install cargo-zigbuild
    else
        echo "  ✓ cargo-zigbuild"
    fi
fi

echo ""
echo "━━━ 系统依赖提示 ━━━"
echo ""

if $IS_LINUX; then
    case "$PROFILE" in
        auto|all|windows)
            echo "  Windows 交叉编译需要 mingw-w64:"
            echo "    sudo apt install mingw-w64"
            echo ""
            ;;
        auto|all|linux)
            echo "  Linux musl 交叉编译需要 musl-tools 或 zigbuild:"
            echo "    sudo apt install musl-tools"
            echo ""
            ;;
        auto|all|macos)
            echo "  macOS 交叉编译需要 cargo-zigbuild:"
            echo "    pip install ziglang && cargo install cargo-zigbuild"
            echo ""
            ;;
    esac
elif $IS_MACOS; then
    case "$PROFILE" in
        auto|all|windows)
            echo "  Windows 交叉编译需要 mingw-w64:"
            echo "    brew install mingw-w64"
            echo ""
            ;;
    esac
fi

echo "✅ 开发环境初始化完成（profile: $PROFILE）"
