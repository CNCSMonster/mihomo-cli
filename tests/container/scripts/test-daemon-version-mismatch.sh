#!/bin/bash
# 验证 daemon 未安装/不可用时的错误提示
set -euo pipefail

echo "测试 daemon 未安装/不可用错误提示..."

# 检查是否在容器中
if [ ! -f /.dockerenv ] && [ "$(cat /proc/1/cgroup 2>/dev/null | grep -c docker)" = "0" ]; then
    echo "FAIL: 该测试必须在容器中运行"
    exit 1
fi

# 检查 mihomo-cli 是否可用
if ! command -v mihomo-cli &>/dev/null; then
    echo "FAIL: mihomo-cli 未安装"
    exit 1
fi

echo "测试 system service/daemon 未安装时的只读错误提示..."

# tun status 是只读命令，不会像 tun on 一样自动进入安装流程。
set +e
OUTPUT=$(mihomo-cli tun status 2>&1)
STATUS=$?
set -e
[ "$STATUS" -ne 0 ] || {
    echo "FAIL: daemon/system service 不存在时 tun status 意外成功"
    exit 1
}

echo "输出: $OUTPUT"

# 验证错误提示包含有用的信息
echo "$OUTPUT" | grep -Eqi 'daemon|service' || {
    echo "FAIL: 错误提示缺少 daemon/service 上下文"
    exit 1
}

# 验证错误包含安装恢复动作。
echo "$OUTPUT" | grep -Eqi 'install' || {
    echo "FAIL: 错误提示缺少 system/install 恢复指引"
    exit 1
}

echo "✅ daemon 未安装/不可用恢复指引测试完成"
