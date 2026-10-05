#!/bin/bash
# J006 固定代理节点测试
# 验证 select/list 命令行为（L1 层级：无真实代理节点）
set -e

echo "=== J006: 固定代理节点命令行为测试 ==="
echo ""

# ─── 测试 1: select --help 命令存在 ───
echo "--- 测试 1: select 子命令存在 ---"
SELECT_HELP=$(mihomo-cli select --help 2>&1)
if echo "$SELECT_HELP" | grep -qi "select\|node"; then
    echo "✅ select 子命令存在且帮助信息正确"
else
    echo "❌ select 子命令不存在或帮助信息异常"
    exit 1
fi
echo ""

# ─── 测试 2: select --help 显示 --group 选项 ───
echo "--- 测试 2: select --group 选项存在 ---"
if echo "$SELECT_HELP" | grep -q "\-\-group"; then
    echo "✅ select --group 选项存在"
else
    echo "❌ select --group 选项不存在"
    exit 1
fi
echo ""

# ─── 测试 3: select --help 显示 --node 选项 ───
echo "--- 测试 3: select --node 选项存在 ---"
if echo "$SELECT_HELP" | grep -q "\-\-node"; then
    echo "✅ select --node 选项存在（非交互式固定节点）"
else
    echo "❌ select --node 选项不存在"
    exit 1
fi
echo ""

# ─── 测试 4: list --help 命令存在 ───
echo "--- 测试 4: list 子命令存在 ---"
LIST_HELP=$(mihomo-cli list --help 2>&1)
if echo "$LIST_HELP" | grep -qi "list\|proxy\|group"; then
    echo "✅ list 子命令存在且帮助信息正确"
else
    echo "❌ list 子命令不存在或帮助信息异常"
    exit 1
fi
echo ""

# ─── 测试 5: 无运行实例时 list 必须明确失败并给出恢复指引 ───
echo "--- 测试 5: list 缺少运行实例时返回可操作错误 ---"
set +e
LIST_OUTPUT=$(mihomo-cli list 2>&1)
LIST_STATUS=$?
set -e
if [ "$LIST_STATUS" -eq 0 ]; then
    echo "$LIST_OUTPUT"
    echo "❌ 无运行实例时 list 意外成功"
    exit 1
fi
echo "$LIST_OUTPUT" | grep -Eqi 'not running|start|service|install|daemon' || {
    echo "$LIST_OUTPUT"
    echo "❌ list 错误缺少 start/service/install 恢复指引"
    exit 1
}
if echo "$LIST_OUTPUT" | grep -Eqi 'panic|thread.*panicked|backtrace'; then
    echo "$LIST_OUTPUT"
    echo "❌ list 发生 panic"
    exit 1
fi
echo "✅ list 返回非零并给出恢复指引"
echo ""

echo "=== J006 固定代理节点命令行为测试完成 ==="
echo "✅ 所有关键命令行为验证通过"
