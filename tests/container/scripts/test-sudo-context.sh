#!/bin/bash
# 验证 sudo 上下文保留
# 测试场景：sudo 提权后，应能还原原始用户信息
set -e

echo "测试 sudo 上下文保留..."

# 该脚本由容器 runner 调度；缺少 runner 承诺的资源必须失败，而非跳过。
if [ ! -f /.dockerenv ] && [ "$(cat /proc/1/cgroup 2>/dev/null | grep -c docker)" = "0" ]; then
    echo "❌ sudo 上下文测试必须在容器中运行" >&2
    exit 1
fi

if ! command -v sudo &>/dev/null; then
    echo "❌ 测试镜像缺少 sudo" >&2
    exit 1
fi

# 测试 1: 验证 sudo 后的环境变量
echo ""
echo "--- 测试 1: sudo 环境变量 ---"

# 以当前用户运行
echo "当前用户: $(whoami)"
echo "当前 HOME: $HOME"

# 以 root 运行，检查 SUDO_UID 和 SUDO_USER
SUDO_RESULT=$(sudo bash -c 'echo "SUDO_UID=$SUDO_UID SUDO_USER=$SUDO_USER HOME=$HOME"')
echo "sudo 后: $SUDO_RESULT"

# 验证 SUDO_UID/SUDO_USER 精确指向调用者
EXPECTED_UID=$(id -u)
EXPECTED_USER=$(id -un)
if echo "$SUDO_RESULT" | grep -q "SUDO_UID=$EXPECTED_UID"; then
    echo "✅ SUDO_UID 精确指向调用用户"
else
    echo "❌ SUDO_UID 未指向调用用户"
    exit 1
fi
if echo "$SUDO_RESULT" | grep -q "SUDO_USER=$EXPECTED_USER"; then
    echo "✅ SUDO_USER 精确指向调用用户"
else
    echo "❌ SUDO_USER 未指向调用用户"
    exit 1
fi

# 测试 2: 验证 mihomo-cli 的私有环境变量传递
echo ""
echo "--- 测试 2: 私有环境变量传递 ---"

if command -v mihomo-cli &>/dev/null; then
    # 模拟 mihomo-cli 的 sudo reexec 行为
    ORIGINAL_HOME="$HOME"
    ORIGINAL_UID=$(id -u)
    
    SUDO_ENV=$(sudo _MIHOMO_CLI_ORIGINAL_HOME="$ORIGINAL_HOME" \
                    _MIHOMO_CLI_ORIGINAL_UID="$ORIGINAL_UID" \
                    bash -c 'echo "ORIGINAL_HOME=$_MIHOMO_CLI_ORIGINAL_HOME ORIGINAL_UID=$_MIHOMO_CLI_ORIGINAL_UID"')
    echo "传递结果: $SUDO_ENV"
    
    if echo "$SUDO_ENV" | grep -q "ORIGINAL_HOME=$ORIGINAL_HOME"; then
        echo "✅ _MIHOMO_CLI_ORIGINAL_HOME 传递成功"
    else
        echo "❌ _MIHOMO_CLI_ORIGINAL_HOME 传递失败"
        exit 1
    fi
    
    if echo "$SUDO_ENV" | grep -q "ORIGINAL_UID=$ORIGINAL_UID"; then
        echo "✅ _MIHOMO_CLI_ORIGINAL_UID 传递成功"
    else
        echo "❌ _MIHOMO_CLI_ORIGINAL_UID 传递失败"
        exit 1
    fi
else
    echo "mihomo-cli 未安装，跳过私有环境变量测试"
fi

echo ""
echo "✅ sudo 上下文测试通过"
