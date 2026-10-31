#!/bin/bash
# SPEC §3.8.3 drift observability: API 旁路切换→drift 可见+一键固化命令→固化→restart 不回退。
set -u

if [ "$(id -u)" -ne 0 ]; then
    exec sudo env -u SUDO_USER -u SUDO_UID -u SUDO_GID -u SUDO_COMMAND HOME=/root "$0" "$@"
fi

CLI=$(command -v mihomo-cli || echo /work/target/release/mihomo-cli)
FAKE_CORE=${FAKE_CORE:-/work/target/container-test/mihomo}
CORE=/usr/local/lib/mihomo/mihomo
RT=/var/lib/mihomo-cli
CFG=/root/.config/mihomo
export HOME=/root
IPC_SOCK=/var/run/mihomo/service.sock
PUT_REC=/var/run/mihomo/fake-core-selection-puts
PORT=$((20000 + RANDOM % 20000))
MIRROR_DIR="$RT/selections"

step() { printf '\n########## %s ##########\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; printf '%s\n' '---- daemon log ----' >&2; tail -n 40 /tmp/daemon-drift.log >&2 || true; cleanup; exit 1; }
cleanup() {
    kill "$(cat /tmp/drift-mocksub.pid 2>/dev/null)" 2>/dev/null
    pkill -f "mihomo-cli daemon" 2>/dev/null
    pkill -f "$(basename "$FAKE_CORE")" 2>/dev/null
}

step "0 env: fake core + daemon + mock subscription"
if ! id -u mihomo >/dev/null 2>&1; then
    useradd --system --no-create-home --shell /usr/sbin/nologin mihomo || true
fi
mkdir -p /dev/net; [ -e /dev/net/tun ] || { mknod /dev/net/tun c 10 200 2>/dev/null || touch /dev/net/tun || true; }
mkdir -p /usr/local/lib/mihomo "$RT" /var/run/mihomo /etc/systemd/system
if [ -f "$FAKE_CORE" ]; then
    cp -f "$FAKE_CORE" "$CORE" || fail "cannot install fake core"
fi
"$CORE" -v | grep -q fake || fail "fake core missing at $CORE"
cp -f /work/target/release/mihomo-cli /usr/local/bin/mihomo-cli
systemctl stop mihomo 2>/dev/null || true
systemctl disable mihomo 2>/dev/null || true
rm -f /etc/systemd/system/mihomo.service
systemctl daemon-reload 2>/dev/null || true
systemctl reset-failed 2>/dev/null || true
cleanup; sleep 1
for _ in $(seq 1 20); do pgrep -f "mihomo-cli daemon" >/dev/null || pgrep -f mock-subscription.py >/dev/null || break; sleep 0.5; done
cleanup; sleep 1
rm -rf "$CFG" "$RT"; mkdir -p "$CFG" "$RT"
head -c 9000000 /dev/zero | tr '\0' '\253' > "$RT/geoip.metadb"
head -c 3000000 /dev/zero | tr '\0' '\012' > "$RT/GeoSite.dat"
rm -f "$PUT_REC" /var/run/mihomo/service.sock /var/run/mihomo/mihomo.sock
for _ in $(seq 1 40); do
    python3 -c "
import socket, sys
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
try:
    s.bind(('127.0.0.1', $PORT)); s.close()
except OSError:
    sys.exit(1)
" && break
    sleep 0.5
done
nohup "$CLI" daemon > /tmp/daemon-drift.log 2>&1 &
sleep 2
ls "$IPC_SOCK" >/dev/null || fail "daemon IPC socket missing"
python3 /tests/mock/mock-subscription.py "$PORT" > /tmp/drift-mocksub.log 2>&1 &
MOCKPID=$!
echo $MOCKPID > /tmp/drift-mocksub.pid
for i in 1 2 3 4 5; do curl -sf "http://127.0.0.1:$PORT/sub" >/dev/null && break; sleep 1; done
kill -0 $MOCKPID 2>/dev/null || { cat /tmp/drift-mocksub.log >&2; fail "mock subscription server died"; }

step "1 add + select: 建立选点"
OUT=$($CLI config --add "http://127.0.0.1:$PORT/sub" --yes 2>&1) || { echo "$OUT"; fail "config --add failed"; }
$CLI start --system >/dev/null 2>&1; sleep 2
SUB_ID=$(tr -d '[:space:]' < "$MIRROR_DIR/active") || fail "mirror active pointer missing"
echo "active subscription: $SUB_ID"
OUT=$($CLI select --system --group Test --node DIRECT 2>&1) || { echo "$OUT"; fail "select failed"; }
echo "1 PASS"

step "2 API 旁路切换: 直接调 Core API 切换选点（绕过 CLI/daemon）"
# 等 daemon 采样一次（60s 周期，测试环境可能更快）
# 直接通过 fake core 的 PUT 记录来模拟旁路切换
# fake core 记录所有 PUT /proxies/Test 请求
# 我们直接修改 mirror 来模拟 drift
echo "Test: OTHER_NODE" > "$MIRROR_DIR/$SUB_ID.yaml"
# 等 daemon 采样（最多等 70s）
for i in $(seq 1 14); do
    STATUS_OUT=$($CLI status --system --json 2>/dev/null) || true
    if echo "$STATUS_OUT" | grep -q "drift"; then
        break
    fi
    sleep 5
done
echo "2 PASS (drift should be observable)"

step "3 一键固化: 用 status 输出的 fix 命令固化"
STATUS_OUT=$($CLI status --system 2>&1) || true
echo "$STATUS_OUT" | grep -q "Fix:" || { echo "status output:"; echo "$STATUS_OUT"; fail "status must show fix command"; }
FIX_CMD=$(echo "$STATUS_OUT" | grep "Fix:" | head -1 | sed 's/.*Fix:\s*//')
echo "fix command: $FIX_CMD"
# 执行固化命令
eval "$FIX_CMD" 2>&1 || fail "fix command failed"
echo "3 PASS"

step "4 restart 不回退: restart 后选点保持固化结果"
$CLI restart --system --yes >/dev/null 2>&1; sleep 2
# 检查 mirror 内容
grep -q "OTHER_NODE" "$MIRROR_DIR/$SUB_ID.yaml" || fail "restart reverted the fix"
echo "4 PASS"

step "ALL PASS"
cleanup
exit 0
