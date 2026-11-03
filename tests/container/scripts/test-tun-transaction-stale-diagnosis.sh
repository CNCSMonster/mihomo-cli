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
ACTIVE_TX_DIR="$TX_DIR/active"

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
# journal 真实位置是 transactions/active/journal.json（active_dir + journal.json），
# 且必须是完整 schema——缺必填字段会让 read_active_journal 反序列化失败、诊断与 reset 全部静默。
mkdir -p "$ACTIVE_TX_DIR"
cat > "$ACTIVE_TX_DIR/journal.json" <<'EOF'
{
  "schema_version": 1,
  "transaction_id": "tx-old-stale",
  "generation": 1,
  "phase": "RollbackPending",
  "original_uid": 0,
  "target_runtime_tun": true,
  "candidate_revision": "1111111111111111111111111111111111111111111111111111111111111111",
  "old_runtime_digest": "0000000000000000000000000000000000000000000000000000000000000000",
  "updated_at": "2020-01-01T00:00:00+00:00"
}
EOF
chown -R mihomo:mihomo "$TX_DIR" 2>/dev/null || true
chmod 750 "$TX_DIR" "$ACTIVE_TX_DIR"
chmod 640 "$ACTIVE_TX_DIR/journal.json"
# 注入必须被产品真正读到（否则后续断言全部空转）
python3 - "$ACTIVE_TX_DIR/journal.json" <<'PY' || fail "injected journal is not readable/parseable by the expected schema"
import json, sys
data = json.load(open(sys.argv[1]))
required = {"schema_version", "transaction_id", "generation", "phase",
            "original_uid", "target_runtime_tun", "candidate_revision", "old_runtime_digest"}
missing = required - set(data)
if missing:
    raise SystemExit(f"fixture missing required journal fields: {sorted(missing)}")
PY
# 产品必须真正读到这个 journal：可变更路径的运行时应用必须如实报出在途事务（不得静默）
OUT=$($CLI config --refresh --yes 2>&1) || { echo "$OUT"; fail "config --refresh failed hard under an in-flight journal"; }
echo "$OUT" | grep -q "tx-old-stale" || { echo "$OUT"; fail "mutation path did not surface the in-flight transaction"; }
echo "2 PASS (injected stale RollbackPending journal)"

step "3 只读命令可用"
# 只读命令必须仍在非终态 journal 下工作（B4 豁免）
OUT=$($CLI status --system 2>&1) || { echo "$OUT"; fail "status failed with stale journal"; }
echo "$OUT" | head -5
OUT=$($CLI tun status 2>&1) || { echo "$OUT"; fail "tun status failed with stale journal"; }
echo "$OUT" | head -4
OUT=$($CLI config --list 2>&1) || { echo "$OUT"; fail "config --list failed with stale journal"; }
echo "3 PASS (read-only commands work)"

step "4 告警含一键命令"
# 等 daemon 诊断 tick（60s 周期，留 80s 余量）
ALERT_SEEN=0
for i in $(seq 1 16); do
    if grep -q "TUN transaction tx-old-stale" /tmp/daemon-tun.log 2>/dev/null; then
        ALERT_SEEN=1
        break
    fi
    sleep 5
done
[ "$ALERT_SEEN" = 1 ] || { tail -20 /tmp/daemon-tun.log; fail "daemon never diagnosed the stale journal on its 60s tick"; }
grep "TUN transaction tx-old-stale" /tmp/daemon-tun.log | grep -q "stale" \
    || fail "diagnosis did not classify the journal as stale"
grep "TUN transaction tx-old-stale" /tmp/daemon-tun.log | grep -q "mihomo-cli restart --system" \
    || fail "diagnosis alert is missing the one-click fix command"
echo "4 PASS"

step "5 显式 restart 受控 reset 恢复"
# SPEC B2：RollbackPending 执行超时后的 durable 后继相位是 RecoveryRequired，
# 其唯一后继是显式 restart 受控 reset（不再规划 RetryApply）。把注入态推进到该相位，
# 并让 intent 与 base 同源，从而命中 RefuseNeedsEvidence → 受控 reset 分支。
BASE_REV=$(sha256sum "$CFG/config.yaml" | cut -d' ' -f1)
python3 - "$ACTIVE_TX_DIR/journal.json" "$BASE_REV" <<'PY' || fail "failed to advance the fixture to RecoveryRequired"
import json, sys
path, base = sys.argv[1], sys.argv[2]
data = json.load(open(path))
data["phase"] = "RecoveryRequired"
data["base_intent_revision"] = base
json.dump(data, open(path, "w"))
PY
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "restart failed"; }
echo "$OUT"
echo "$OUT" | grep -q "重置 mihomo 运行状态\|reset" || { echo "$OUT"; fail "restart did not run the controlled reset"; }
[ ! -e "$ACTIVE_TX_DIR/journal.json" ] || fail "controlled reset left the stale journal in place"
ls -d "$TX_DIR"/gc/reset-1-tx-old-stale >/dev/null 2>&1 || fail "controlled reset did not quarantine the transaction under gc/"
STATUS_OUT=$($CLI status --system 2>&1) || true
echo "$STATUS_OUT" | grep -q "API: *reachable" || { echo "$STATUS_OUT"; fail "runtime not re-established after the controlled reset"; }
echo "5 PASS"

step "ALL PASS"
cleanup
exit 0
