#!/bin/bash
# J005 公司内网域名直连测试
# 验证 dns policy add, rule add DIRECT 等命令行为
# L2 层级：验证命令存在性和 help 输出，不需要真实内网环境
set -e

echo "=== J005: 公司内网域名直连测试 ==="
echo ""

# ─── 测试 1: dns policy add 命令存在 ───
echo "--- 测试 1: dns policy add 命令存在 ---"
DNS_HELP=$(mihomo-cli dns --help 2>&1)
if echo "$DNS_HELP" | grep -q "policy"; then
    echo "✅ dns policy 子命令存在"
else
    echo "❌ dns policy 子命令不存在"
    exit 1
fi

DNS_POLICY_HELP=$(mihomo-cli dns policy --help 2>&1)
if echo "$DNS_POLICY_HELP" | grep -q "add"; then
    echo "✅ dns policy add 命令存在"
else
    echo "❌ dns policy add 命令不存在"
    exit 1
fi
echo ""

# ─── 测试 2: dns fake-ip-filter add 命令存在 ───
echo "--- 测试 2: dns fake-ip-filter add 命令存在 ---"
if mihomo-cli dns --help 2>&1 | grep -q "fake-ip-filter"; then
    echo "✅ dns fake-ip-filter 子命令存在"
else
    echo "❌ dns fake-ip-filter 子命令不存在"
    exit 1
fi
echo ""

# ─── 测试 3: rule add 命令支持 DIRECT 策略 ───
echo "--- 测试 3: rule add 命令支持 DIRECT 策略 ---"
RULE_HELP=$(mihomo-cli rule add --help 2>&1)

# 验证 rule add 存在
if mihomo-cli rule --help 2>&1 | grep -q "add"; then
    echo "✅ rule add 命令存在"
else
    echo "❌ rule add 命令不存在"
    exit 1
fi

# 验证 rule add help 输出中提及 DOMAIN-SUFFIX 或规则格式
if echo "$RULE_HELP" | grep -q 'TYPE,PARAMETER,POLICY' && \
   echo "$RULE_HELP" | grep -q 'DOMAIN-SUFFIX,example.com,DIRECT'; then
    echo "✅ rule add 支持规则配置"
else
    echo "❌ rule add --help 缺少规则格式或 DIRECT 示例"
    exit 1
fi
echo ""

# ─── 测试 4: rule test 命令存在 ───
echo "--- 测试 4: rule test 命令存在 ---"
if mihomo-cli rule --help 2>&1 | grep -q "test"; then
    echo "✅ rule test 命令存在"
else
    echo "❌ rule test 命令不存在"
    exit 1
fi
echo ""

# ─── 测试 5: restart 命令存在（用于 DNS 改动生效） ───
echo "--- 测试 5: restart 命令存在 ---"
if mihomo-cli --help 2>&1 | grep -q "restart"; then
    echo "✅ restart 命令存在"
else
    echo "❌ restart 命令不存在"
    exit 1
fi
echo ""

echo "=== J005 公司内网域名直连测试完成 ==="
echo "✅ 所有关键命令行为验证通过"
