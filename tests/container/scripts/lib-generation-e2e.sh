#!/bin/bash
# 共享夹具：代际升级应用 E2E。仅在 systemd 容器内运行（root 执行脚本，
# 命令以 testuser + sudo 复现真实用户旅程，与 test-systemd-contract.sh 同构）。
set -euo pipefail

CLI_SOURCE=/tests/mihomo-cli-source
INSTALLED_CLI=/usr/local/bin/mihomo-cli
CORE_BINARY=/usr/local/lib/mihomo/mihomo
FAKE_CORE=/tests/fake-mihomo
GEN_ROOT=/var/lib/mihomo-cli-generations
STATE="$GEN_ROOT/state.json"
TEST_USER=testuser
TEST_HOME=/home/testuser
CONFIG_DIR="$TEST_HOME/.config/mihomo"
SYSTEM_UNIT=/etc/systemd/system/mihomo.service
API_SOCKET=/var/run/mihomo/mihomo.sock
CORE_PID_FILE=/var/run/mihomo/core.pid
INJECT_FAIL_MARKER=/var/log/mihomo/inject-fail-first
MANAGED_STATE_DIR=/var/lib/mihomo-cli
MANAGED_CONFIG="$MANAGED_STATE_DIR/active-config.yaml"

fail() {
    echo "FAIL: $*" >&2
    systemctl status mihomo --no-pager >&2 || true
    journalctl -u mihomo --no-pager -n 80 >&2 || true
    echo "--- generation state ---" >&2
    cat "$STATE" >&2 2>/dev/null || true
    ls -la "$GEN_ROOT/generations" >&2 2>/dev/null || true
    exit 1
}

require_root_container() {
    [ "$(id -u)" = 0 ] || fail "must run as root inside the container"
    command -v systemctl >/dev/null || fail "systemctl unavailable"
    [ -x "$CLI_SOURCE" ] || fail "installer CLI missing at $CLI_SOURCE"
    [ -x "$FAKE_CORE" ] || fail "fake core missing at $FAKE_CORE"
}

as_user() {
    sudo -n -u "$TEST_USER" env HOME="$TEST_HOME" USER="$TEST_USER" PATH=/usr/local/bin:/usr/bin:/bin "$@"
}

# 原子替换二进制（tmp + rename）：运行中的可执行文件不能就地覆盖（ETXTBSY），
# 但同目录 rename 会换 inode，服务可继续运行旧 inode 或稍后重启。
replace_binary() {
    local source="$1"
    local target="$2"
    local staged
    staged="$(dirname "$target")/.$(basename "$target").staged.$$"
    cp "$source" "$staged"
    chmod 755 "$staged"
    mv -f "$staged" "$target"
}

# 干净起点：先停服务（避免覆盖运行中的可执行文件）、卸装、清代际存储，
# 再恢复 fake core 与 pristine CLI。
reset_instance() {
    systemctl stop mihomo >/dev/null 2>&1 || true
    as_user "$CLI_SOURCE" uninstall --all --yes >/dev/null 2>&1 || true
    systemctl stop mihomo >/dev/null 2>&1 || true
    rm -rf "$GEN_ROOT" /var/lib/mihomo-cli /var/run/mihomo
    rm -f "$INJECT_FAIL_MARKER"
    rm -f "$SYSTEM_UNIT"
    systemctl daemon-reload >/dev/null 2>&1 || true
    replace_binary "$CLI_SOURCE" "$INSTALLED_CLI"
    replace_binary "$FAKE_CORE" "$CORE_BINARY"
    mkdir -p "$CONFIG_DIR"
    # 离线满足 installer 的 Geo 完整性门槛（与 systemd contract 同口径）。
    truncate -s 8000001 "$CONFIG_DIR/geoip.metadb"
    truncate -s 2000001 "$CONFIG_DIR/GeoSite.dat"
    chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME/.config"
}

install_system() {
    as_user sudo -n "$CLI_SOURCE" install --system --yes >/tmp/install.log 2>&1 || {
        cat /tmp/install.log >&2
        fail "install --system failed"
    }
    systemctl is-active --quiet mihomo || fail "daemon service is not active after install"
    [ -f "$CONFIG_DIR/config.yaml" ] || fail "install did not generate the direct-only config"
}

# 注入「每次启动都失败」的 core：用于验证失败面与显式恢复（非回滚成功路径）。
# 注入「每次启动都失败」：mock core 保持**合法 ELF**（installer 的 -v 校验通过，
# 因此离线环境绝不会触发重新下载），仅在实际启动时失败。夹具标记文件位于
# /var/log/mihomo（unit 的 PrivateTmp 会隐藏 /tmp，daemon 派生的 core 读不到），
# 只由本测试控制，不属于安装文件。
inject_always_failing_start() {
    install -d -o mihomo -g mihomo -m 0755 /var/log/mihomo
    printf '99' >"$INJECT_FAIL_MARKER"
    replace_binary "$FAKE_CORE" "$CORE_BINARY"
    inject_skew_only
}

# 撤销成功后解除失败注入（夹具动作，不触碰任何安装文件）。
clear_failure_injection() {
    rm -f "$INJECT_FAIL_MARKER"
}

# pending 代际的 daemon 字节必须等于「当前运行的 CLI」，因此 skew 通过给
# installed CLI 追加字节制造（不影响当前进程）。
inject_skew_only() {
    local skewed=/tmp/skewed-cli-$$
    cp "$CLI_SOURCE" "$skewed"
    printf 'skew' >>"$skewed"
    chmod 755 "$skewed"
    replace_binary "$skewed" "$INSTALLED_CLI"
    rm -f "$skewed"
}

# 只恢复 CLI（= 失败候选的 daemon 字节），保留 core 现场：恢复态下不得人工替换
# 安装文件，否则安装态就不再是「已知现场」。
restore_pristine_cli_only() {
    replace_binary "$CLI_SOURCE" "$INSTALLED_CLI"
}

# rename 换 inode，运行中的服务不受影响；不停止服务（显式重试依赖「服务在运行」
# 才会走 staged upgrade 路径）。
restore_pristine_binaries() {
    replace_binary "$CLI_SOURCE" "$INSTALLED_CLI"
    replace_binary "$FAKE_CORE" "$CORE_BINARY"
}

# ── 证据采集（裁决 D 验收证据：旧/新身份、退出证据、目标哈希、启动因果） ──
proc_exe() { readlink "/proc/$1/exe" 2>/dev/null || true; }
proc_argv() { tr '\0' ' ' <"/proc/$1/cmdline" 2>/dev/null || true; }

api_responding() {
    python3 - "$API_SOCKET" <<'PYEOF'
import socket, sys
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.settimeout(2)
sock.connect(sys.argv[1])
sock.sendall(b"GET /configs HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
sys.exit(0 if sock.recv(64).startswith(b"HTTP/1.1 200") else 1)
PYEOF
}

# 确定性故障现场（裁决 D）：daemon 消失 + PID 记录缺失 + 受管 Core 存活并持续
# 响应 API。这里**制造**故障本体——不预杀孤儿、不删 socket（socket 由孤儿自己
# 建立），因此后续的归属证明/退出证明是在真实现场上验收的。
spawn_managed_orphan_core() {
    systemctl stop mihomo >/dev/null 2>&1 || true
    for _ in $(seq 1 100); do
        systemctl is-active --quiet mihomo || break
        sleep 0.1
    done
    install -d -o mihomo -g mihomo -m 0711 /var/run/mihomo
    rm -f "$CORE_PID_FILE"                # PID 记录缺失
    [ -f "$MANAGED_CONFIG" ] || fail "managed active config missing at $MANAGED_CONFIG"
    rm -f /tmp/orphan-core.pid
    setsid runuser -u mihomo -- sh -c 'echo $$ > /tmp/orphan-core.pid; exec "$@"' sh \
        "$CORE_BINARY" -d "$MANAGED_STATE_DIR" -f "$MANAGED_CONFIG" \
        >/tmp/orphan-core.log 2>&1 &
    ORPHAN_PID=""
    for _ in $(seq 1 100); do
        ORPHAN_PID=$(cat /tmp/orphan-core.pid 2>/dev/null || true)
        if [ -n "$ORPHAN_PID" ] && [ -S "$API_SOCKET" ] && api_responding; then
            break
        fi
        sleep 0.1
    done
    [ -n "$ORPHAN_PID" ] || fail "managed orphan core did not start"
    kill -0 "$ORPHAN_PID" 2>/dev/null || fail "managed orphan core is not alive"
    case "$(proc_exe "$ORPHAN_PID")" in
        "$CORE_BINARY" | "$CORE_BINARY (deleted)") ;;
        *) fail "orphan core identity is not the managed artifact: $(proc_exe "$ORPHAN_PID")" ;;
    esac
    systemctl is-active --quiet mihomo && fail "daemon service must be gone in the orphan scenario"
    [ ! -f "$CORE_PID_FILE" ] || fail "PID record must be missing in the orphan scenario"
    echo "orphan scenario: daemon=gone pid-record=missing core=$ORPHAN_PID exe=$(proc_exe "$ORPHAN_PID") argv=$(proc_argv "$ORPHAN_PID")"
}

assert_exit_evidence() {  # <old-pid> <log...>
    local pid="$1"
    shift
    for _ in $(seq 1 100); do
        [ ! -e "/proc/$pid" ] && break
        sleep 0.05
    done
    [ ! -e "/proc/$pid" ] || fail "old managed core pid $pid is still alive after the authorized takeover"
    local log
    for log in "$@"; do
        if grep -q "已观察到 pid $pid 退出" "$log" 2>/dev/null; then
            echo "exit evidence: pidfd observed pid $pid exit ($(basename "$log"))"
            return 0
        fi
    done
    fail "no pidfd exit observation for old core pid $pid in: $*"
}

wait_for_new_core() {  # <previous-pid>; echoes the new pid
    local previous="$1"
    local pid=""
    for _ in $(seq 1 200); do
        pid=$(python3 - "$CORE_PID_FILE" 2>/dev/null <<'PYEOF'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        print(json.load(handle).get("pid", ""))
except Exception:
    print("")
PYEOF
)
        if [ -n "$pid" ] && [ "$pid" != "$previous" ] && kill -0 "$pid" 2>/dev/null; then
            echo "$pid"
            return 0
        fi
        sleep 0.05
    done
    return 1
}

assert_live_core_identity() {  # <pid>
    local pid="$1"
    kill -0 "$pid" 2>/dev/null || fail "new core pid $pid is not running"
    case "$(proc_exe "$pid")" in
        "$CORE_BINARY" | "$CORE_BINARY (deleted)") ;;
        *) fail "new core exe is not the managed artifact: $(proc_exe "$pid")" ;;
    esac
    local argv
    argv=$(proc_argv "$pid")
    case "$argv" in
        *"-d $MANAGED_STATE_DIR "* | "-d $MANAGED_STATE_DIR "*) ;;
        *) fail "new core lacks the managed -d argument: $argv" ;;
    esac
    case "$argv" in
        *"-f $MANAGED_CONFIG "*) ;;
        *) fail "new core lacks the managed -f argument: $argv" ;;
    esac
    echo "new core identity: pid=$pid exe=$(proc_exe "$pid") argv=$argv"
}

assert_start_causality() {  # <pid> <since-epoch-seconds>
    local pid="$1"
    local since="$2"
    [ "$(stat -c %Y "$CORE_PID_FILE")" -ge "$since" ] || \
        fail "core.pid receipt predates this start: no start causality"
    python3 - "$CORE_PID_FILE" "$pid" <<'PYEOF'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    receipt = json.load(handle)
assert receipt.get("pid") == int(sys.argv[2]), receipt
assert receipt.get("config_revision"), "start receipt carries no config revision"
assert receipt.get("api_endpoint"), "start receipt carries no endpoint"
print("start causality: receipt config_revision=%s endpoint=%s"
      % (receipt["config_revision"][:16], receipt["api_endpoint"]))
PYEOF
}

assert_target_artifact() {  # <generation-id>
    local id="$1"
    [ -n "$id" ] || fail "no active generation id"
    python3 - "$GEN_ROOT/generations/$id/manifest.json" "$CORE_BINARY" "$INSTALLED_CLI" <<'PYEOF'
import hashlib, json, sys
manifest = json.load(open(sys.argv[1], encoding="utf-8"))
core = hashlib.sha256(open(sys.argv[2], "rb").read()).hexdigest()
daemon = hashlib.sha256(open(sys.argv[3], "rb").read()).hexdigest()
assert manifest["core"]["sha256"] == core, (manifest["core"]["sha256"], core)
assert manifest["daemon"]["sha256"] == daemon, (manifest["daemon"]["sha256"], daemon)
print("target artifact verified: core=%s daemon=%s" % (core[:12], daemon[:12]))
PYEOF
}

# 运行态快照：用于断言「零停止/零启动/零替换」。
runtime_snapshot() {
    printf '%s|%s|%s|%s' \
        "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" \
        "$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)" \
        "$(systemctl show -p NRestarts --value mihomo 2>/dev/null || true)" \
        "$(systemctl show -p ActiveEnterTimestampMonotonic --value mihomo 2>/dev/null || true)"
}

state_value() {
    python3 - "$1" <<'PY'
import json, sys
key = sys.argv[1]
try:
    with open("/var/lib/mihomo-cli-generations/state.json", encoding="utf-8") as handle:
        state = json.load(handle)
except FileNotFoundError:
    print("")
    raise SystemExit(0)
value = state.get(key)
print("" if value is None else value)
PY
}

withdrawn_count() {
    find "$GEN_ROOT/generations" -maxdepth 1 -type d -name '*.withdrawn' 2>/dev/null | wc -l | tr -d ' '
}

wait_service_active() {
    for _ in $(seq 1 150); do
        if systemctl is-active --quiet mihomo; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

assert_service_running() {
    wait_service_active || fail "daemon service is not running"
    as_user "$CLI_SOURCE" status --system >/tmp/status.log 2>&1 || {
        cat /tmp/status.log >&2
        fail "status --system failed (runtime not healthy)"
    }
}
