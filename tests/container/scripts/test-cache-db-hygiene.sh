#!/bin/bash
# SPEC: 注入全零 cache.db + root 属主→restart→断言确认合同、停机窗口隔离、留证 K=3、core 正常起。
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

step() { printf '\n########## %s ##########\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; printf '%s\n' '---- daemon log ----' >&2; tail -n 40 /tmp/daemon-hygiene.log >&2 || true; cleanup; exit 1; }
cleanup() {
    kill "$(cat /tmp/hygiene-mocksub.pid 2>/dev/null)" 2>/dev/null
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
[ -f /work/target/release/mihomo-cli ] && cp -f /work/target/release/mihomo-cli /usr/local/bin/mihomo-cli
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
nohup "$CLI" daemon > /tmp/daemon-hygiene.log 2>&1 &
sleep 2
ls "$IPC_SOCK" >/dev/null || fail "daemon IPC socket missing"
python3 /tests/mock/mock-subscription.py "$PORT" > /tmp/hygiene-mocksub.log 2>&1 &
MOCKPID=$!
echo $MOCKPID > /tmp/hygiene-mocksub.pid
for i in 1 2 3 4 5; do curl -sf "http://127.0.0.1:$PORT/sub" >/dev/null && break; sleep 1; done
kill -0 $MOCKPID 2>/dev/null || { cat /tmp/hygiene-mocksub.log >&2; fail "mock subscription server died"; }

step "1 add subscription + start"
OUT=$($CLI config --add "http://127.0.0.1:$PORT/sub" --yes 2>&1) || { echo "$OUT"; fail "config --add failed"; }
$CLI start --system >/dev/null 2>&1; sleep 2
echo "1 PASS"

step "2 注入全零 cache.db + root 属主"
# 停机
$CLI stop --system >/dev/null 2>&1; sleep 2
# 注入全零 cache.db（16 KiB，模拟 incident shape）
head -c 16384 /dev/zero > "$RT/cache.db"
chown root:root "$RT/cache.db"
chmod 0644 "$RT/cache.db"
echo "2 PASS (injected all-zero cache.db with root ownership)"

step "2.1 等待 daemon 60s 诊断 tick 告警"
# 等 daemon 诊断 tick（60s 周期，留 80s 余量）
ALERT_SEEN=0
for i in $(seq 1 16); do
    if grep -q "cache.db is" /tmp/daemon-hygiene.log 2>/dev/null; then
        ALERT_SEEN=1
        break
    fi
    sleep 5
done
[ "$ALERT_SEEN" = 1 ] || { tail -20 /tmp/daemon-hygiene.log; fail "daemon never diagnosed corrupt/permission-drift cache.db on 60s tick"; }
grep "cache.db is" /tmp/daemon-hygiene.log | grep -q "mihomo-cli restart --system" \
    || fail "cache.db diagnosis alert is missing the one-click fix command"
echo "2.1 PASS (diagnosed cache.db on 60s tick)"

step "3 restart: 断言确认合同、停机窗口隔离、留证 K=3"
# restart 应该检测到异常 cache.db 并隔离
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "restart failed"; }
echo "$OUT"
# C1 修复合同：root 属主的 cache.db 必须先修权限（或明确报告 repair_required），不得静默放行
echo "$OUT" | grep -qi "permission\|repair" || fail "restart did not surface the ownership/permission repair contract"
# A2：内容不合格必须在停机窗口内隔离留证，且路径可解释
echo "$OUT" | grep -qi "quarantine" || fail "restart did not report cache.db quarantine"
QUARANTINE_COUNT=$(ls -1 "$RT"/cache.db.corrupt-* 2>/dev/null | wc -l)
echo "quarantine artifacts: $QUARANTINE_COUNT"
[ "$QUARANTINE_COUNT" -ge 1 ] || fail "no quarantine artifact was produced for the all-zero cache.db"
# K=3 retention: 最多保留 3 个
[ "$QUARANTINE_COUNT" -le 3 ] || fail "quarantine retention exceeded K=3: $QUARANTINE_COUNT"
echo "3 PASS"

step "4 core 正常起"
# 检查 core 是否正常运行（CLI 观测到的 API 可达，而非进程名子串匹配）
STATUS_OUT=$($CLI status --system 2>&1) || { echo "$STATUS_OUT"; fail "status failed after restart"; }
echo "$STATUS_OUT" | grep -q "API: *reachable" || { echo "$STATUS_OUT"; fail "core API is not reachable after quarantine + restart"; }
# 留证必须保留原始字节（16 KiB 全零），不是空壳
QFILE=$(ls -1 "$RT"/cache.db.corrupt-* 2>/dev/null | head -1)
[ -n "$QFILE" ] || fail "quarantine artifact disappeared"
[ "$(stat -c %s "$QFILE")" = 16384 ] || fail "quarantine artifact did not preserve the original bytes ($(stat -c %s "$QFILE") bytes)"
echo "4 PASS"

step "5 留证 K=3：连续再隔离 3 次只保留最新 3 份"
for i in 1 2 3; do
    $CLI stop --system >/dev/null 2>&1; sleep 1
    head -c 16384 /dev/zero > "$RT/cache.db"
    chown root:root "$RT/cache.db"
    chmod 0644 "$RT/cache.db"
    OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "restart $i failed during the K=3 retention run"; }
    sleep 1
done
COUNT=$(ls -1 "$RT"/cache.db.corrupt-* 2>/dev/null | wc -l)
echo "quarantine artifacts after 4 isolations: $COUNT"
[ "$COUNT" -eq 3 ] || fail "K=3 retention violated: expected 3 artifacts after 4 isolations, got $COUNT"
echo "5 PASS"

step "ALL PASS"
cleanup
exit 0
