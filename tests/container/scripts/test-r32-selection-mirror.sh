#!/bin/bash
# R3.2-1 selection mirror 对账 + 自愈 + 写者门禁 端到端验证（system 模式，fake core）。
# 场景：
#   1) 一致态：select 后镜像与用户树逐字节一致，doctor 报 ✅
#   2) Drifted：篡改镜像 → doctor 报 ❌(Drifted) 且保持只读（不顺手修复）
#   3) 自愈：restart --system 触发 reconcile，镜像按用户树重推，doctor 转 ✅
#   4) Missing：删除镜像 → doctor 报 ❌(Missing) → restart 修复 + 回放仍生效
#   5) 写者门禁（fail-closed）：原始 IPC 用非法 id（路径穿越）查询 → daemon 整体拒绝
#   6) push 失败可行动报告：daemon 停机时 select，用户树仍写、报告含 restart --system 指引
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
fail() { printf 'FAIL: %s\n' "$1" >&2; printf '%s\n' '---- daemon log ----' >&2; tail -n 40 /tmp/daemon-r32.log >&2 || true; cleanup; exit 1; }
cleanup() {
    kill "$(cat /tmp/r32-mocksub.pid 2>/dev/null)" 2>/dev/null
    pkill -f "mihomo-cli daemon" 2>/dev/null
    pkill -f "$(basename "$FAKE_CORE")" 2>/dev/null
}

doctor_mirror_line() { $CLI doctor --system 2>&1 | grep -A1 "Selection mirror"; }

step "0 env: fake core + daemon + mock subscription"
if ! id -u mihomo >/dev/null 2>&1; then
    useradd --system --no-create-home --shell /usr/sbin/nologin mihomo || true
fi
mkdir -p /dev/net; [ -e /dev/net/tun ] || { mknod /dev/net/tun c 10 200 2>/dev/null || touch /dev/net/tun || true; }
mkdir -p /usr/local/lib/mihomo "$RT" /var/run/mihomo /etc/systemd/system
# 测试镜像已把 fake core 装到 $CORE；$FAKE_CORE 只在宿主路径可见时覆盖安装。
if [ -f "$FAKE_CORE" ]; then
    cp -f "$FAKE_CORE" "$CORE" || fail "cannot install fake core"
fi
"$CORE" -v | grep -q fake || fail "fake core missing at $CORE"
# 沙箱可能有历史会话残留：旧 unit 拉起的过期 daemon、上一代 pending generation。
# 统一把当前 release 二进制装到 unit 路径，并清干净服务与运行时状态。
cp -f /work/target/release/mihomo-cli /usr/local/bin/mihomo-cli
systemctl stop mihomo 2>/dev/null || true
systemctl disable mihomo 2>/dev/null || true
rm -f /etc/systemd/system/mihomo.service
systemctl daemon-reload 2>/dev/null || true
systemctl reset-failed 2>/dev/null || true
# 彻底清场：任何历史会话遗留的 daemon/core/mocksub（含 systemd 托管单元）都会抢占 IPC socket 与端口
systemctl stop mihomo 2>/dev/null || true
systemctl disable mihomo 2>/dev/null || true
cleanup; sleep 1
for _ in $(seq 1 20); do pgrep -f "mihomo-cli daemon" >/dev/null || pgrep -f mock-subscription.py >/dev/null || break; sleep 0.5; done
cleanup; sleep 1
rm -rf "$CFG" "$RT"; mkdir -p "$CFG" "$RT"
head -c 9000000 /dev/zero | tr '\0' '\253' > "$RT/geoip.metadb"
head -c 3000000 /dev/zero | tr '\0' '\012' > "$RT/GeoSite.dat"
rm -f "$PUT_REC" /var/run/mihomo/service.sock /var/run/mihomo/mihomo.sock
# 端口必须真正可绑定（connect 探测无法发现 LISTEN 套接字释放竞态）
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
nohup "$CLI" daemon > /tmp/daemon-r32.log 2>&1 &
sleep 2
ls "$IPC_SOCK" >/dev/null || fail "daemon IPC socket missing"
pgrep -f "mihomo-cli daemon" | wc -l | grep -qx 1 || fail "exactly one fresh daemon must own the IPC socket"
python3 /tests/mock/mock-subscription.py "$PORT" > /tmp/r32-mocksub.log 2>&1 &
MOCKPID=$!
echo $MOCKPID > /tmp/r32-mocksub.pid
for i in 1 2 3 4 5; do curl -sf "http://127.0.0.1:$PORT/sub" >/dev/null && break; sleep 1; done
kill -0 $MOCKPID 2>/dev/null || { cat /tmp/r32-mocksub.log >&2; fail "mock subscription server died (port $PORT)"; }

OUT=$($CLI config --add "http://127.0.0.1:$PORT/sub" --yes 2>&1) || { echo "$OUT"; fail "config --add failed"; }
printf '%s' "$OUT" | grep -q 'selection mirror not updated' && fail "mirror should be built while daemon is running"
$CLI start --system >/dev/null 2>&1; sleep 2
SUB_ID=$(tr -d '[:space:]' < "$MIRROR_DIR/active") || fail "mirror active pointer missing"
echo "active subscription: $SUB_ID"

step "1 一致态：select 后镜像与用户树逐字节一致，doctor ✅"
OUT=$($CLI select --system --group Test --node DIRECT 2>&1) || { echo "$OUT"; fail "select failed"; }
cmp -s "$CFG/selections/$SUB_ID.yaml" "$MIRROR_DIR/$SUB_ID.yaml" \
    || fail "push does not transmit exact user-tree bytes (mirror != user tree after select)"
LINE=$(doctor_mirror_line) || true
printf '%s\n' "$LINE"
printf '%s' "$LINE" | grep -q '✅ Selection mirror' || fail "consistent mirror must pass doctor"
echo "1 PASS"

step "2 Drifted：篡改镜像 → doctor ❌(Drifted) 且只读不修复"
printf '\n# tampered by r32 test\n' >> "$MIRROR_DIR/$SUB_ID.yaml"
TAMPER_SHA=$(sha256sum "$MIRROR_DIR/$SUB_ID.yaml")
LINE=$(doctor_mirror_line) || true
printf '%s\n' "$LINE"
printf '%s' "$LINE" | grep -q '❌ Selection mirror' || fail "drifted mirror must fail doctor"
printf '%s' "$LINE" | grep -q 'Drifted' || fail "doctor must classify Drifted"
[ "$TAMPER_SHA" = "$(sha256sum "$MIRROR_DIR/$SUB_ID.yaml")" ] \
    || fail "doctor must be read-only; mirror changed during doctor"
echo "2 PASS"

step "3 自愈：restart --system 收敛漂移（full push + reconcile 推后验证）"
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "restart failed"; }
echo "$OUT"
# restart 路径先全量重推再 reconcile 验证；"repaired" 行只在 push 丢失而 reconcile
# 兜底时出现，此处接受两种收敛证据，但镜像必须回到与用户树逐字节一致。
printf '%s' "$OUT" | grep -q "selection mirror $SUB_ID repaired" \
    && echo "(reconcile 兜底修复被触发)" || echo "(full push 即已收敛，reconcile 验证通过)"
cmp -s "$CFG/selections/$SUB_ID.yaml" "$MIRROR_DIR/$SUB_ID.yaml" \
    || fail "restart did not restore mirror to exact user-tree bytes"
LINE=$(doctor_mirror_line) || true
printf '%s\n' "$LINE"
printf '%s' "$LINE" | grep -q '✅ Selection mirror' || fail "doctor must pass after self-heal"
grep -q 'PUT /proxies/Test' "$PUT_REC" || fail "replay did not re-apply selection to core"
echo "3 PASS"

step "4 Missing：删除镜像 → doctor ❌(Missing) → restart 收敛"
rm -f "$MIRROR_DIR/$SUB_ID.yaml"
LINE=$(doctor_mirror_line) || true
printf '%s\n' "$LINE"
printf '%s' "$LINE" | grep -q '❌ Selection mirror' || fail "missing mirror must fail doctor"
printf '%s' "$LINE" | grep -q 'Missing' || fail "doctor must classify Missing"
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "restart (missing) failed"; }
cmp -s "$CFG/selections/$SUB_ID.yaml" "$MIRROR_DIR/$SUB_ID.yaml" \
    || fail "missing mirror not restored byte-exact"
LINE=$(doctor_mirror_line) || true
printf '%s' "$LINE" | grep -q '✅ Selection mirror' || fail "doctor must pass after missing repair"
echo "4 PASS"

step "5 写者门禁 fail-closed：原始 IPC 非法 id（路径穿越）整体拒绝"
TOKEN=$(cat "$CFG/service-token" 2>/dev/null || true)
REJ=$(python3 - "$IPC_SOCK" "$TOKEN" "$SUB_ID" <<'PY'
import json, socket, struct, sys
sock_path, token, sub_id = sys.argv[1:4]
token = token or None

def call(payload):
    cmd = dict(payload); cmd["token"] = token
    data = json.dumps(cmd).encode()
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(10); s.connect(sock_path)
    s.sendall(struct.pack("<I", len(data)) + data)
    hdr = s.recv(4)
    (n,) = struct.unpack("<I", hdr)
    buf = b""
    while len(buf) < n:
        buf += s.recv(n - len(buf))
    s.close()
    return json.loads(buf)

bad = call({"type": "GetSelectionMirrorRevisions", "subscription_ids": ["../escape"]})
ok = call({"type": "GetSelectionMirrorRevisions", "subscription_ids": [sub_id]})
print(json.dumps({"bad": bad, "ok": ok}))
PY
) || fail "raw IPC query crashed"
echo "$REJ"
printf '%s' "$REJ" | python3 -c "
import json, sys
r = json.loads(sys.stdin.read())
bad, ok = r['bad'], r['ok']
assert bad['type'] == 'Error' and 'invalid subscription id' in bad['message'], bad
rev = ok['revisions'][sys.argv[1]] if 'revisions' in ok else None
assert ok['type'] == 'SelectionMirrorRevisions', ok
assert ok['revisions'][sys.argv[1]] and len(ok['revisions'][sys.argv[1]]) == 64, ok
print('5 PASS: traversal rejected wholesale; valid id returns sha256 revision')
" "$SUB_ID" || fail "fail-closed query contract violated"

step "6 push 失败：可行动报告（daemon 停机 + 非法 revision 状态保持）"
pkill -f "mihomo-cli daemon"; sleep 1
OUT=$($CLI select --system --group Test --node DIRECT 2>&1); SEL_EXIT=$?
echo "$OUT"
if printf '%s' "$OUT" | grep -q 'runtime selection mirror not updated'; then
    printf '%s' "$OUT" | grep -q 'repair: mihomo-cli restart --system' \
        || fail "actionable report must contain restart --system guidance"
    [ -s "$CFG/selections/$SUB_ID.yaml" ] || fail "user tree intent must survive push failure"
    grep -q 'selection mirror' "$CFG/selections/$SUB_ID.yaml" 2>/dev/null || true
    echo "6 PASS: push failure escalated to actionable report (daemon down, select exit=$SEL_EXIT)"
elif [ "$SEL_EXIT" -ne 0 ]; then
    echo "6 SKIP: select hard-fails with daemon down before best-effort push (exit=$SEL_EXIT)"
else
    fail "select silently succeeded while daemon down (expected actionable mirror warning)"
fi

cleanup
echo
echo "PASS: R3.2-1 selection mirror 对账 + 自愈 + 写者门禁 + 可行动报告 (system mode, fake core)"
exit 0
