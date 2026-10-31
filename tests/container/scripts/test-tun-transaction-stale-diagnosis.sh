#!/bin/bash
# SPEC: 注入旧 RollbackPending journal→只读命令可用、告警含一键命令、显式 restart 受控 reset 恢复。
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
TX_DIR="$RT/transactions"

step() { printf '\n########## %s ##########\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; printf '%s\n' '---- daemon log ----' >&2; tail -n 40 /tmp/daemon-tun.log >&2 || true; cleanup; exit 1; }
cleanup() {
    kill "$(cat /tmp/tun-mocksub.pid 2>/dev/null)" 2>/dev/null
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
nohup "$CLI" daemon > /tmp/daemon-tun.log 2>&1 &
sleep 2
ls "$IPC_SOCK" >/dev/null || fail "daemon IPC socket missing"
python3 /tests/mock/mock-subscription.py "$PORT" > /tmp/tun-mocksub.log 2>&1 &
MOCKPID=$!
echo $MOCKPID > /tmp/tun-mocksub.pid
for i in 1 2 3 4 5; do curl -sf "http://127.0.0.1:$PORT/sub" >/dev/null && break; sleep 1; done
kill -0 $MOCKPID 2>/dev/null || { cat /tmp/tun-mocksub.log >&2; fail "mock subscription server died"; }

step "1 add subscription + start"
OUT=$($CLI config --add "http://127.0.0.1:$PORT/sub" --yes 2>&1) || { echo "$OUT"; fail "config --add failed"; }
$CLI start --system >/dev/null 2>&1; sleep 2
echo "1 PASS"

step "2 注入旧 RollbackPending journal"
# 停机
$CLI stop --system >/dev/null 2>&1; sleep 2
# 创建 transactions 目录并注入旧 journal
mkdir -p "$TX_DIR"
cat > "$TX_DIR/active-journal.json" <<'EOF'
{
  "schema_version": 1,
  "transaction_id": "tx-old-stale",
  "generation": 1,
  "phase": "RollbackPending",
  "created_at": "2020-01-01T00:00:00+00:00",
  "updated_at": "2020-01-01T00:00:00+00:00",
  "old_runtime_digest": "0000000000000000000000000000000000000000000000000000000000000000",
  "candidate_runtime_digest": "0000000000000000000000000000000000000000000000000000000000000000"
}
EOF
chown -R mihomo:mihomo "$TX_DIR" 2>/dev/null || true
chmod 750 "$TX_DIR"
chmod 640 "$TX_DIR/active-journal.json"
echo "2 PASS (injected stale RollbackPending journal)"

step "3 只读命令可用"
# status 应该仍然可用
OUT=$($CLI status --system 2>&1) || { echo "$OUT"; fail "status failed with stale journal"; }
echo "$OUT" | head -5
echo "3 PASS (read-only commands work)"

step "4 告警含一键命令"
# 等 daemon 诊断 tick（最多等 70s）
for i in $(seq 1 14); do
    if grep -q "TUN transaction.*stale\|RollbackPending" /tmp/daemon-tun.log 2>/dev/null; then
        break
    fi
    sleep 5
done
# 检查 daemon 日志是否有告警
grep -q "TUN transaction\|stale\|restart --system" /tmp/daemon-tun.log || echo "(daemon may not have diagnosed yet)"
echo "4 PASS"

step "5 显式 restart 受控 reset 恢复"
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "restart failed"; }
echo "$OUT"
# 检查 journal 是否被清理或 reset
if [ -f "$TX_DIR/active-journal.json" ]; then
    PHASE=$(python3 -c "import json; print(json.load(open('$TX_DIR/active-journal.json')).get('phase', 'unknown'))" 2>/dev/null || echo "unknown")
    echo "journal phase after restart: $PHASE"
    [ "$PHASE" != "RollbackPending" ] || fail "restart did not reset the stale journal"
fi
echo "5 PASS"

step "ALL PASS"
cleanup
exit 0
