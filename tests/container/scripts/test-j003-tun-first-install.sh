#!/bin/bash
# J003 TUN 优先安装测试
# 验证 install --system, tun on/off 等命令行为
# L2 层级：验证命令存在性和 help 输出，不需要真实 TUN 设备
set -e

echo "=== J003: TUN 优先安装测试 ==="
echo ""

# ─── 测试 1: install --system 参数存在 ───
echo "--- 测试 1: install --system 参数存在 ---"
if mihomo-cli install --help 2>&1 | grep -q "\-\-system"; then
    echo "✅ install --system 参数存在"
else
    echo "❌ install --system 参数不存在"
    exit 1
fi
echo ""

# ─── 测试 2: tun 子命令存在 ───
echo "--- 测试 2: tun 子命令存在 ---"
if mihomo-cli --help 2>&1 | grep -q "tun"; then
    echo "✅ tun 子命令存在"
else
    echo "❌ tun 子命令不存在"
    exit 1
fi
echo ""

# ─── 测试 3: tun on/off 命令存在 ───
echo "--- 测试 3: tun on/off 命令存在 ---"
TUN_HELP=$(mihomo-cli tun --help 2>&1)

if echo "$TUN_HELP" | grep -q "on"; then
    echo "✅ tun on 命令存在"
else
    echo "❌ tun on 命令不存在"
    exit 1
fi

if echo "$TUN_HELP" | grep -q "off"; then
    echo "✅ tun off 命令存在"
else
    echo "❌ tun off 命令不存在"
    exit 1
fi
echo ""

# ─── 测试 4: install --yes 参数存在 ───
echo "--- 测试 4: install --yes 参数存在 ---"
if mihomo-cli install --help 2>&1 | grep -q "\-\-yes"; then
    echo "✅ install --yes 参数存在"
else
    echo "❌ install --yes 参数不存在"
    exit 1
fi
echo ""

# ─── 测试 5: install 不默认开启 TUN ───
echo "--- 测试 5: install --help 不包含 --tun 参数 ---"
# 根据 J003 产品要求：不新增 install --tun
if mihomo-cli install --help 2>&1 | grep -q "\-\-tun"; then
    echo "❌ install --tun 参数存在（违反 J003 产品要求：install 不默认开启 TUN）"
    exit 1
else
    echo "✅ install 不包含 --tun 参数（符合 J003 产品要求）"
fi
echo ""

echo "=== J003 TUN 优先安装测试完成 ==="
echo "✅ 所有关键命令行为验证通过"
