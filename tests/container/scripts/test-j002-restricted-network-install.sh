#!/bin/bash
# J002 受限网络安装测试
# 验证 install --github-mirror, --skip-config 等命令行为
# L2 层级：验证命令存在性和 help 输出，不需要真实网络
set -e

echo "=== J002: 受限网络安装测试 ==="
echo ""

# ─── 测试 1: install --github-mirror 参数存在 ───
echo "--- 测试 1: install --github-mirror 参数存在 ---"
if mihomo-cli install --help 2>&1 | grep -q "\-\-github-mirror"; then
    echo "✅ install --github-mirror 参数存在"
else
    echo "❌ install --github-mirror 参数不存在"
    exit 1
fi
echo ""

# ─── 测试 2: install --skip-config 参数存在 ───
echo "--- 测试 2: install --skip-config 参数存在 ---"
if mihomo-cli install --help 2>&1 | grep -q "\-\-skip-config"; then
    echo "✅ install --skip-config 参数存在"
else
    echo "❌ install --skip-config 参数不存在"
    exit 1
fi
echo ""

# ─── 测试 3: install --help 显示完整安装选项 ───
echo "--- 测试 3: install --help 显示完整安装选项 ---"
INSTALL_HELP=$(mihomo-cli install --help 2>&1)

# 验证关键参数存在
MISSING_PARAMS=()
for param in "--github-mirror" "--skip-config"; do
    if ! echo "$INSTALL_HELP" | grep -q -- "$param"; then
        MISSING_PARAMS+=("$param")
    fi
done

if [[ ${#MISSING_PARAMS[@]} -eq 0 ]]; then
    echo "✅ install 命令包含所有预期参数"
else
    echo "❌ install 命令缺少参数: ${MISSING_PARAMS[*]}"
    exit 1
fi
echo ""

# ─── 测试 4: install 子命令存在 ───
echo "--- 测试 4: install 子命令存在 ---"
if mihomo-cli --help 2>&1 | grep -q "install"; then
    echo "✅ install 子命令存在"
else
    echo "❌ install 子命令不存在"
    exit 1
fi
echo ""

echo "=== J002 受限网络安装测试完成 ==="
echo "✅ 所有关键命令行为验证通过"
