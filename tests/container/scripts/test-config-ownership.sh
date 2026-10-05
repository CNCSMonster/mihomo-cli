#!/bin/bash
# 验证 mihomo-cli 在 sudo 写入配置后，文件所有者是原始用户而非 root
# 回归测试（commit dcabf5e）
set -e

TEST_USER=testuser
TEST_HOME="/home/$TEST_USER"
TEST_CONFIG_DIR="$TEST_HOME/.config/mihomo"
TEST_TMP_DIR=$(mktemp -d)

cleanup() {
    rm -rf "$TEST_CONFIG_DIR"
    if [ -e "$TEST_TMP_DIR/config-backup" ]; then
        mkdir -p "$(dirname "$TEST_CONFIG_DIR")"
        mv "$TEST_TMP_DIR/config-backup" "$TEST_CONFIG_DIR"
    fi
    rm -rf "$TEST_TMP_DIR"
}

echo "=== 配置文件所有权测试 ==="

if ! command -v mihomo-cli &>/dev/null; then
    echo "FAIL: mihomo-cli 未安装"
    rm -rf "$TEST_TMP_DIR"
    exit 1
fi

# 隔离已有 testuser 状态：测试结束时恢复，不删除用户或任何非测试资产。
if [ -e "$TEST_CONFIG_DIR" ]; then
    mv "$TEST_CONFIG_DIR" "$TEST_TMP_DIR/config-backup"
fi
trap cleanup EXIT

if [ -f /usr/local/lib/mihomo/mihomo ]; then
    sudo mkdir -p /root/.local/bin
    sudo ln -sf /usr/local/lib/mihomo/mihomo /root/.local/bin/mihomo
    mkdir -p "$TEST_HOME/.local/bin"
    ln -sf /usr/local/lib/mihomo/mihomo "$TEST_HOME/.local/bin/mihomo"
fi

cat > "$TEST_TMP_DIR/test-config.yaml" << 'EOF'
mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
proxies:
  - name: Mock-SS
    type: ss
    server: 127.0.0.1
    port: 8388
    cipher: aes-256-gcm
    password: test-password
proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - Mock-SS
rules:
  - MATCH,Proxy
EOF

EXPECTED_UID=$(id -u "$TEST_USER")
EXPECTED_GID=$(id -g "$TEST_USER")
assert_original_owner() {
    local path="$1"
    local actual_uid actual_gid
    actual_uid=$(stat -c '%u' "$path")
    actual_gid=$(stat -c '%g' "$path")
    echo "文件所有者: $path -> uid=$actual_uid gid=$actual_gid"
    if [ "$actual_uid" != "$EXPECTED_UID" ] || [ "$actual_gid" != "$EXPECTED_GID" ]; then
        echo "FAIL: 文件所有者错误：应该是 $TEST_USER ($EXPECTED_UID:$EXPECTED_GID)"
        exit 1
    fi
}

echo "以 $TEST_USER 身份通过 sudo 运行 mihomo-cli config --import..."
sudo -u "$TEST_USER" sudo mihomo-cli config --import "$TEST_TMP_DIR/test-config.yaml" --yes

for path in \
    "$TEST_HOME/.config" \
    "$TEST_CONFIG_DIR" \
    "$TEST_CONFIG_DIR/subscriptions" \
    "$TEST_CONFIG_DIR/config.yaml" \
    "$TEST_CONFIG_DIR/subscriptions.yaml" \
    "$TEST_CONFIG_DIR/subscriptions/active" \
    "$TEST_CONFIG_DIR"/subscriptions/*.yaml; do
    if [ ! -e "$path" ]; then
        echo "FAIL: 配置状态路径未被创建: $path"
        exit 1
    fi
    assert_original_owner "$path"
done

echo "PASS: 所有配置状态文件均归原始用户 $TEST_USER 所有"

echo "验证已有 active 时非交互裸导入只保存订阅..."
cat > "$TEST_TMP_DIR/inactive-config.yaml" << 'EOF'
mixed-port: 7891
allow-lan: false
mode: direct
log-level: debug
proxies: []
proxy-groups: []
rules:
  - MATCH,DIRECT
EOF
cp "$TEST_CONFIG_DIR/config.yaml" "$TEST_TMP_DIR/config-before-inactive-import.yaml"
cp "$TEST_CONFIG_DIR/subscriptions/active" "$TEST_TMP_DIR/active-before-inactive-import"
find "$TEST_CONFIG_DIR/subscriptions" -maxdepth 1 -type f -printf '%f\n' | sort > "$TEST_TMP_DIR/subscription-files-before-inactive-import"

sudo -u "$TEST_USER" sudo mihomo-cli config --import "$TEST_TMP_DIR/inactive-config.yaml" --yes

if ! cmp -s "$TEST_TMP_DIR/config-before-inactive-import.yaml" "$TEST_CONFIG_DIR/config.yaml" || \
    ! cmp -s "$TEST_TMP_DIR/active-before-inactive-import" "$TEST_CONFIG_DIR/subscriptions/active"; then
    echo "FAIL: 非交互裸导入意外替换了 active 配置"
    exit 1
fi

find "$TEST_CONFIG_DIR/subscriptions" -maxdepth 1 -type f -printf '%f\n' | sort > "$TEST_TMP_DIR/subscription-files-after-inactive-import"
if [ "$(comm -13 "$TEST_TMP_DIR/subscription-files-before-inactive-import" "$TEST_TMP_DIR/subscription-files-after-inactive-import" | wc -l)" -ne 1 ]; then
    echo "FAIL: 非交互裸导入未恰好新增一个订阅缓存"
    exit 1
fi
INACTIVE_SUBSCRIPTION_FILE=$(comm -13 "$TEST_TMP_DIR/subscription-files-before-inactive-import" "$TEST_TMP_DIR/subscription-files-after-inactive-import")
if ! grep -q 'mixed-port: 7891' "$TEST_CONFIG_DIR/subscriptions/$INACTIVE_SUBSCRIPTION_FILE"; then
    echo "FAIL: 非交互裸导入未保存导入的订阅内容"
    exit 1
fi
assert_original_owner "$TEST_CONFIG_DIR/subscriptions/$INACTIVE_SUBSCRIPTION_FILE"
echo "PASS: 非交互裸导入仅保存订阅，active 配置保持不变"

echo "验证失败回滚后配置文件所有权..."
# system 已安装时解析 core_binary 用 planned 路径、不读 MIHOMO_CLI_MIHOMO_PATH，
# 故不靠 env 注入假 mihomo，改用内容级校验失败（合法 YAML + proxies 键 + 真 mihomo -t 必挂字段），
# 对有/无 system 实例的环境都有效。
cat > "$TEST_TMP_DIR/failing-config.yaml" << 'EOF'
mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
proxies:
  - name: Bad-Proxy
    type: not-a-real-proxy-type
    server: 127.0.0.1
    port: 8388
proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - Bad-Proxy
rules:
  - MATCH,Proxy
EOF
sudo -u "$TEST_USER" sh -c "printf 'mixed-port: 7890\\n' > '$TEST_CONFIG_DIR/config.yaml'"
cp "$TEST_CONFIG_DIR/config.yaml" "$TEST_TMP_DIR/config-before-rollback.yaml"
cp "$TEST_CONFIG_DIR/subscriptions.yaml" "$TEST_TMP_DIR/subscriptions-before-rollback.yaml"
cp "$TEST_CONFIG_DIR/subscriptions/active" "$TEST_TMP_DIR/active-before-rollback"
find "$TEST_CONFIG_DIR/subscriptions" -maxdepth 1 -type f -printf '%f\n' | sort > "$TEST_TMP_DIR/subscription-files-before-rollback"
for path in "$TEST_CONFIG_DIR"/subscriptions/*.yaml; do
    cp "$path" "$TEST_TMP_DIR/${path##*/}.before-rollback"
done

sudo -u "$TEST_USER" sudo mihomo-cli config --import "$TEST_TMP_DIR/failing-config.yaml" --activate --yes >"$TEST_TMP_DIR/config-rollback.out" 2>&1 && {
    cat "$TEST_TMP_DIR/config-rollback.out"
    echo "FAIL: 验证失败的导入意外成功"
    exit 1
}

if ! cmp -s "$TEST_TMP_DIR/config-before-rollback.yaml" "$TEST_CONFIG_DIR/config.yaml" || \
    ! cmp -s "$TEST_TMP_DIR/subscriptions-before-rollback.yaml" "$TEST_CONFIG_DIR/subscriptions.yaml" || \
    ! cmp -s "$TEST_TMP_DIR/active-before-rollback" "$TEST_CONFIG_DIR/subscriptions/active"; then
    cat "$TEST_TMP_DIR/config-rollback.out"
    echo "FAIL: 验证失败后未恢复配置或订阅元数据内容"
    exit 1
fi

find "$TEST_CONFIG_DIR/subscriptions" -maxdepth 1 -type f -printf '%f\n' | sort > "$TEST_TMP_DIR/subscription-files-after-rollback"
if ! cmp -s "$TEST_TMP_DIR/subscription-files-before-rollback" "$TEST_TMP_DIR/subscription-files-after-rollback"; then
    cat "$TEST_TMP_DIR/config-rollback.out"
    echo "FAIL: 验证失败后 subscriptions 文件集合未恢复"
    exit 1
fi

for path in \
    "$TEST_CONFIG_DIR/config.yaml" \
    "$TEST_CONFIG_DIR/subscriptions.yaml" \
    "$TEST_CONFIG_DIR/subscriptions/active" \
    "$TEST_CONFIG_DIR"/subscriptions/*.yaml; do
    assert_original_owner "$path"
    if [[ "$path" == *.yaml && "$path" == */subscriptions/* ]] && \
        ! cmp -s "$TEST_TMP_DIR/${path##*/}.before-rollback" "$path"; then
        cat "$TEST_TMP_DIR/config-rollback.out"
        echo "FAIL: 回滚后的订阅内容错误: $path"
        exit 1
    fi
done

echo "PASS: 验证失败回滚后配置状态内容和所有权均已恢复"

echo "验证配置目录符号链接逃逸被 fail-closed 拒绝..."
rm -rf "$TEST_CONFIG_DIR"
sudo -u "$TEST_USER" ln -s /etc "$TEST_CONFIG_DIR"
sudo -u "$TEST_USER" sudo mihomo-cli config --import "$TEST_TMP_DIR/test-config.yaml" --yes >"$TEST_TMP_DIR/symlink-attack.out" 2>&1 && {
    cat "$TEST_TMP_DIR/symlink-attack.out"
    echo "FAIL: 指向 /etc 的配置目录符号链接未被拒绝"
    exit 1
}
for leaked in \
    /etc/config.yaml \
    /etc/config.yaml.tmp \
    /etc/subscriptions.yaml \
    /etc/subscriptions.yaml.tmp; do
    if [ -e "$leaked" ]; then
        cat "$TEST_TMP_DIR/symlink-attack.out"
        echo "FAIL: 符号链接攻击导致 root 写入 $leaked"
        exit 1
    fi
done
if [ -d /etc/subscriptions ]; then
    cat "$TEST_TMP_DIR/symlink-attack.out"
    echo "FAIL: 符号链接攻击导致 root 在 /etc 下创建目录"
    exit 1
fi
rm -f "$TEST_CONFIG_DIR"
echo "PASS: 符号链接逃逸写入被拒绝且 /etc 未被触及"

echo "验证伪造的 _MIHOMO_CLI_ORIGINAL_* 环境变量被忽略..."
sudo -u "$TEST_USER" sudo env _MIHOMO_CLI_ORIGINAL_HOME=/root _MIHOMO_CLI_ORIGINAL_UID=0 \
    mihomo-cli config --import "$TEST_TMP_DIR/test-config.yaml" --yes
if [ -e /root/.config/mihomo ]; then
    echo "FAIL: 伪造的 env home 导致 root 写入 /root/.config/mihomo"
    exit 1
fi
assert_original_owner "$TEST_CONFIG_DIR/config.yaml"
echo "PASS: 身份解析以 SUDO_UID + passwd 为准，伪造 env 未引导写入"
