#!/bin/bash
# 本地快速验证容器测试
# 用法: bash tests/container/quick-test.sh
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/container-runtime.sh"

echo "=========================================="
echo "mihomo-cli 容器测试 - 快速验证"
echo "=========================================="

cd "$PROJECT_ROOT"

echo ""
echo "--- Runner 元数据和运行时探测自检 ---"
bash tests/container/test-runner.sh --self-test

echo ""
echo "--- 全部离线、非 systemd 容器测试（skip 即失败）---"
bash tests/container/test-runner.sh --required \
    config-ownership \
    config-regen-intent \
    daemon-unavailable-guidance \
    group-promotion-restart \
    j001-offline-subscription \
    j002-restricted-network-install \
    j003-tun-first-install \
    j005-company-intranet-direct \
    j006-pin-proxy-node \
    j009-subscription-drift-warning \
    sudo-context

# 运行需要特权的测试
echo ""
echo "--- user-mode 安装、Core 生命周期与卸载合约 ---"
bash tests/container/user-mode-test.sh

echo ""
echo "--- system service 安装、路径、生命周期、proxy、TUN 与验证合约 ---"
bash tests/container/systemd-test.sh

echo ""
echo "=========================================="
echo "✅ 快速验证完成"
