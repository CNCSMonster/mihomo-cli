#!/bin/bash
# SPEC-selection-mirror-resilience: add→select→switch→remove→add 同 URL→restart，
# 断言 ID 不变、选点恢复（A1' revive 身份连续性）。
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
PORT=$((20000 + RANDOM % 20000))
MIRROR_DIR="$RT/selections"

step() { printf '\n########## %s ##########\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; printf '%s\n' '---- daemon log ----' >&2; tail -n 40 /tmp/daemon-revive.log >&2 || true; cleanup; exit 1; }
cleanup() {
    kill "$(cat /tmp/revive-mocksub.pid 2>/dev/null)" 2>/dev/null
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
rm -f /var/run/mihomo/service.sock /var/run/mihomo/mihomo.sock
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
nohup "$CLI" daemon > /tmp/daemon-revive.log 2>&1 &
sleep 2
ls "$IPC_SOCK" >/dev/null || fail "daemon IPC socket missing"
python3 /tests/mock/mock-subscription.py "$PORT" > /tmp/revive-mocksub.log 2>&1 &
MOCKPID=$!
echo $MOCKPID > /tmp/revive-mocksub.pid
for i in 1 2 3 4 5; do curl -sf "http://127.0.0.1:$PORT/sub" >/dev/null && break; sleep 1; done
kill -0 $MOCKPID 2>/dev/null || { cat /tmp/revive-mocksub.log >&2; fail "mock subscription server died"; }

SUB_URL="http://127.0.0.1:$PORT/sub"

step "1 add + select: 建立选点"
OUT=$($CLI config --add "$SUB_URL" --yes 2>&1) || { echo "$OUT"; fail "config --add failed"; }
$CLI start --system >/dev/null 2>&1; sleep 2
SUB_ID=$(tr -d '[:space:]' < "$MIRROR_DIR/active") || fail "mirror active pointer missing"
echo "active subscription: $SUB_ID"
OUT=$($CLI select --system --group Test --node DIRECT 2>&1) || { echo "$OUT"; fail "select failed"; }
[ -f "$CFG/selections/$SUB_ID.yaml" ] || fail "selection file missing"
echo "1 PASS"

step "2 switch + remove: 切换到另一订阅并删除原订阅"
# 添加第二个订阅
OUT=$($CLI config --add "$SUB_URL" --yes 2>&1) || { echo "$OUT"; fail "second config --add failed"; }
# 记录原始 ID
ORIG_ID="$SUB_ID"
# 删除原订阅（retire）
OUT=$($CLI config --remove "$ORIG_ID" --yes 2>&1) || { echo "$OUT"; fail "config --remove failed"; }
echo "2 PASS"

step "3 add 同 URL: 复活同一 ID"
OUT=$($CLI config --add "$SUB_URL" --yes 2>&1) || { echo "$OUT"; fail "revive config --add failed"; }
REVIVED_ID=$(tr -d '[:space:]' < "$MIRROR_DIR/active") || fail "revived mirror active pointer missing"
echo "revived subscription: $REVIVED_ID"
[ "$REVIVED_ID" = "$ORIG_ID" ] || fail "revive did not restore the same subscription ID (expected $ORIG_ID, got $REVIVED_ID)"
echo "3 PASS"

step "4 restart: 选点恢复"
$CLI restart --system --yes >/dev/null 2>&1; sleep 2
[ -f "$CFG/selections/$REVIVED_ID.yaml" ] || fail "selection file missing after restart"
# 选点内容应延续
grep -q "Test" "$CFG/selections/$REVIVED_ID.yaml" || fail "selection content lost after revive + restart"
echo "4 PASS"

step "ALL PASS"
cleanup
exit 0
