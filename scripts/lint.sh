#!/usr/bin/env bash
# 多目标 lint：检查所有已安装且可编译的 target
# 用法: scripts/lint.sh [scope]
#   scope: auto | default | full | windows | macos | linux | native
#   默认 auto：根据当前平台检查可编译的 targets
# 退出码 0 = 全部通过，非 0 = 有 warning/error
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

SCOPE="${1:-auto}"
# auto 和 default 行为相同
[ "$SCOPE" = "auto" ] && SCOPE="default"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

FAILED=0
IS_LINUX=false
IS_MACOS=false
[[ "$(uname)" == "Linux" ]] && IS_LINUX=true
[[ "$(uname)" == "Darwin" ]] && IS_MACOS=true

run_check() {
    local label="$1"
    shift
    echo -e "\n${YELLOW}━━━ $label ━━━${NC}"
    if "$@" 2>&1; then
        echo -e "${GREEN}✓ $label 通过${NC}"
    else
        echo -e "${RED}✗ $label 失败${NC}"
        FAILED=$((FAILED + 1))
    fi
}

skip_check() {
    local label="$1"
    local reason="$2"
    echo -e "\n${YELLOW}━━━ $label ━━━${NC}"
    echo -e "${YELLOW}⊘ 跳过: $reason${NC}"
}

# 判断 target 是否应该被检查
should_check_target() {
    local target="$1"
    local native
    native=$(rustc -vV | grep host | cut -d' ' -f2)
    
    case "$SCOPE" in
        native)
            [ "$target" = "$native" ]
            ;;
        windows)
            [[ "$target" == *"windows"* ]]
            ;;
        macos)
            [[ "$target" == *"apple-darwin"* ]]
            ;;
        linux)
            [[ "$target" == *"linux"* ]]
            ;;
        default)
            # 检查当前平台可编译的所有 targets
            # Linux 上跳过 macOS（需要 macOS SDK）
            if $IS_LINUX && [[ "$target" == *"apple-darwin"* ]]; then
                return 1
            fi
            # 跳过需要交叉编译器的 Linux ARM gnu targets
            if $IS_LINUX && [[ "$target" == "aarch64-unknown-linux-gnu" ]]; then
                return 1
            fi
            # macOS 上跳过需要交叉编译器的 Linux ARM gnu targets
            if $IS_MACOS && [[ "$target" == "aarch64-unknown-linux-gnu" ]]; then
                return 1
            fi
            return 0
            ;;
        full)
            # 检查所有已安装 targets（包括需要特殊工具链的）
            return 0
            ;;
        *)
            echo "未知 scope: $SCOPE"
            echo "可用: auto, default, full, windows, macos, linux, native"
            exit 1
            ;;
    esac
}

# 判断 target 是否可编译（使用标准 cargo clippy --target）
can_compile_target() {
    local target="$1"
    
    # macOS targets 在 Linux 上需要 macOS SDK
    if $IS_LINUX && [[ "$target" == *"apple-darwin"* ]]; then
        return 1
    fi
    
    # Linux ARM gnu 需要交叉编译器
    if [[ "$target" == "aarch64-unknown-linux-gnu" ]]; then
        if command -v aarch64-linux-gnu-gcc &>/dev/null; then
            return 0
        else
            return 1
        fi
    fi
    
    # musl targets 需要 musl-gcc（cargo-zigbuild 不能直接用于 clippy）
    if [[ "$target" == *"musl"* ]]; then
        if [[ "$target" == "x86_64-unknown-linux-musl" ]] && command -v musl-gcc &>/dev/null; then
            return 0
        else
            return 1
        fi
    fi
    
    # 其他 targets 应该可以编译
    return 0
}

echo "━━━ Lint scope: $SCOPE (platform: $(uname)) ━━━"

# 1. 格式检查
run_check "cargo fmt" cargo fmt --check

# 2. 遍历 targets
NATIVE_TARGET=$(rustc -vV | grep host | cut -d' ' -f2)
INSTALLED_TARGETS=$(rustup target list --installed)

for target in $INSTALLED_TARGETS; do
    if ! should_check_target "$target"; then
        continue
    fi
    
    if [ "$target" = "$NATIVE_TARGET" ]; then
        run_check "clippy ($target)" cargo clippy --all-targets -- -D warnings
        continue
    fi
    
    # 检查是否能编译
    if ! can_compile_target "$target"; then
        if [ "$SCOPE" = "full" ]; then
            # full 模式下，不能编译的标记为失败
            skip_check "clippy ($target)" "缺少工具链（full 模式）"
            FAILED=$((FAILED + 1))
        else
            skip_check "clippy ($target)" "缺少工具链"
        fi
        continue
    fi
    
    run_check "clippy ($target)" cargo clippy --target "$target" -- -D warnings
done

# 3. 测试（只在 default/full/native 模式下运行）
if [ "$SCOPE" = "default" ] || [ "$SCOPE" = "full" ] || [ "$SCOPE" = "native" ]; then
    run_check "cargo test" cargo test
fi

# 结果
echo ""
if [ "$FAILED" -eq 0 ]; then
    echo -e "${GREEN}✅ lint 全部通过 (scope: $SCOPE)${NC}"
    exit 0
else
    echo -e "${RED}❌ $FAILED 项检查失败 (scope: $SCOPE)${NC}"
    exit 1
fi
