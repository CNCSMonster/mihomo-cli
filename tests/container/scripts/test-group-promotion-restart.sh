#!/bin/bash
# Group promotion & selection replay regression (system mode, fake core):
#   A) `group create` with TUN on promotes into the running snapshot (runtime_applied)
#   B) pending group change (daemon down) is applied by `mihomo-cli restart`
#   C) persisted selection is replayed to the core after restart
#   D) override 派生键在 restart 前后都不丢，且 restart 后运行时对账闭环 (Issue #022 B/C)
#   E) doctor 本地解析器探测：绕过可检出、fake-ip 判健康、--no-probe-dns 关闭 (Issue #022 A)
#   F) override 中 TUN 事务管辖键显式告警，且不顶掉 TUN 意图 (Issue #022 B)
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
API_SOCK=/var/run/mihomo/mihomo.sock
IPC_SOCK=/var/run/mihomo/service.sock
ARGV_REC=/var/run/mihomo/fake-core-argv
PUT_REC=/var/run/mihomo/fake-core-selection-puts
PORT=8091

step() { printf '\n########## %s ##########\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$1" >&2; kill "$(cat /tmp/repro-mocksub.pid 2>/dev/null)" 2>/dev/null; pkill -f "$CLI daemon" 2>/dev/null; exit 1; }
launched_config() { awk '/^-f$/{getline; print; exit}' "$ARGV_REC" 2>/dev/null; }
puts_count() { grep -c 'PUT /proxies/' "$PUT_REC" 2>/dev/null || true; }
api_up() { [ -S "$API_SOCK" ]; }

step "0 env"
if ! id -u mihomo >/dev/null 2>&1; then
    useradd --system --no-create-home --shell /usr/sbin/nologin mihomo || true
fi
mkdir -p /dev/net
if [ ! -e /dev/net/tun ]; then
    mknod /dev/net/tun c 10 200 2>/dev/null || touch /dev/net/tun || true
fi
mkdir -p /usr/local/lib/mihomo "$RT" /var/run/mihomo /etc/systemd/system
if [ "$FAKE_CORE" != "$CORE" ] && [ -f "$FAKE_CORE" ]; then
    cp -f "$FAKE_CORE" "$CORE" || fail "cannot install fake core"
fi
"$CORE" -v | grep -q fake || fail "fake core missing"
cat > /etc/systemd/system/mihomo.service <<'EOF'
[Unit]
Description=Mihomo CLI System Daemon
[Service]
Type=simple
ExecStart=/usr/local/bin/mihomo-cli daemon
EOF
head -c 9000000 /dev/zero | tr '\0' '\253' > "$RT/geoip.metadb"
head -c 3000000 /dev/zero | tr '\0' '\012' > "$RT/GeoSite.dat"
rm -rf "$CFG"; mkdir -p "$CFG"
pkill -f "$CLI daemon" 2>/dev/null
pkill -f mock-subscription.py 2>/dev/null
pkill -x "$(basename "$FAKE_CORE")" 2>/dev/null
rm -f "$ARGV_REC" "$PUT_REC" "$API_SOCK"
sleep 1

step "1 daemon up + mock subscription + config --add"
nohup "$CLI" daemon > /tmp/daemon.log 2>&1 &
sleep 2
ls "$IPC_SOCK" >/dev/null || fail "daemon IPC socket missing"
python3 /tests/mock/mock-subscription.py "$PORT" > /tmp/mocksub.log 2>&1 &
echo $! > /tmp/repro-mocksub.pid
for i in 1 2 3 4 5; do curl -sf "http://127.0.0.1:$PORT/sub" >/dev/null && break; sleep 1; done
export HOME=/root
OUT=$($CLI config --add "http://127.0.0.1:$PORT/sub" --yes 2>&1) || { echo "$OUT"; fail "config --add failed"; }
echo "$OUT" | grep -q 'Added subscription' || { echo "$OUT"; fail "subscription not added"; }
if printf '%s' "$OUT" | grep -q 'selection mirror not updated'; then
    fail "selection mirror should be built while daemon is running"
fi
$CLI start --system >/dev/null 2>&1
sleep 2
api_up || fail "core API not up after start"
OUT=$($CLI tun on --yes 2>&1) || { echo "$OUT"; fail "tun on failed"; }
echo "$OUT" | grep -q 'TUN enabled' || fail "tun on did not report enabled"

step "2 scenario A: group create with TUN on applies to runtime"
OUT=$($CLI group create StreamHK --type select --member "Mock Proxy 1" --member "Mock Proxy 2" 2>&1) || { echo "$OUT"; fail "group create failed"; }
echo "$OUT"
printf '%s' "$OUT" | grep -q 'runtime_applied=true' || fail "A: expected runtime_applied=true, got pending"
grep -q 'StreamHK' "$RT/tun-config.yaml" || fail "A: StreamHK missing from tun-config snapshot"
grep -q 'StreamHK' "$CFG/config.yaml" || fail "A: StreamHK missing from intent config.yaml"
echo "A PASS: group create under TUN-on promoted to snapshot + runtime"

step "3 scenario B: pending change (daemon down) is applied by restart"
$CLI group delete StreamHK >/dev/null 2>&1
pkill -f "$CLI daemon"; sleep 1
OUT=$($CLI group create StreamB --type select --member "Mock Proxy 1" 2>&1) || { echo "$OUT"; fail "group create (offline) failed"; }
echo "$OUT"
printf '%s' "$OUT" | grep -qi 'restart' || fail "B: expected pending hint pointing at restart"
grep -q 'StreamB' "$CFG/config.yaml" || fail "B: StreamB missing from intent config.yaml"
nohup "$CLI" daemon >> /tmp/daemon.log 2>&1 &
sleep 2
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "restart failed"; }
echo "$OUT"
sleep 2
printf '%s' "$OUT" | grep -q 'Selections not replayed' && fail "B: misleading selection replay warning without persisted intent (Issue #022 C)"
LCFG=$(launched_config)
[ -n "$LCFG" ] || fail "B: no launched config recorded"
grep -q 'StreamB' "$LCFG" || fail "B: restart did not apply pending StreamB (launched $LCFG)"
echo "B PASS: restart applied pending group to runtime config ($LCFG)"

step "4 scenario C: selection replayed after restart"
OUT=$($CLI select --group Test --node DIRECT 2>&1) || { echo "$OUT"; fail "select failed"; }
echo "$OUT"
grep -q 'PUT /proxies/Test' "$PUT_REC" || fail "C: initial selection PUT not observed"
BASE=$(puts_count)
pkill -f "$CLI daemon"; sleep 1
pkill -f "$(basename "$FAKE_CORE")"; sleep 1
rm -f "$API_SOCK"
nohup "$CLI" daemon >> /tmp/daemon.log 2>&1 &
sleep 2
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "restart (C) failed"; }
echo "$OUT"
printf '%s' "$OUT" | grep -q 'Selections not replayed' && fail "C: replay reported unavailable"
sleep 2
AFTER=$(puts_count)
[ "${AFTER:-0}" -gt "${BASE:-0}" ] || fail "C: no selection PUT recorded after restart (base=$BASE after=$AFTER)"
grep -q 'PUT /proxies/Test' "$PUT_REC" || fail "C: selection for Test not replayed after restart"
echo "C PASS: selection replayed to core after restart ($BASE -> $AFTER PUTs)"

step "5 scenario D: override keys survive restart (Issue #022 B)"
cat > /tmp/override-issue022.yaml <<'OEOF'
sniffer:
  enable: true
  override-destination: true
dns:
  enhanced-mode: fake-ip
  fake-ip-range: 198.18.0.1/16
OEOF
OUT=$($CLI override import /tmp/override-issue022.yaml 2>&1) || { echo "$OUT"; fail "D: override import failed"; }
echo "$OUT"
sleep 2
grep -q '^sniffer:' "$CFG/config.yaml" || fail "D: intent config.yaml missing override keys after import"
LCFG=$(launched_config)
[ -n "$LCFG" ] || fail "D: no launched config recorded after import"
grep -q '^sniffer:' "$LCFG" || fail "D: running config ($LCFG) missing override keys after import"
grep -q 'fake-ip' "$LCFG" || fail "D: running config ($LCFG) missing override dns keys after import"
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "D: restart failed"; }
echo "$OUT"
sleep 2
LCFG=$(launched_config)
[ -n "$LCFG" ] || fail "D: no launched config recorded after restart"
grep -q '^sniffer:' "$CFG/config.yaml" || fail "D: intent config.yaml lost override keys after restart"
grep -q '^sniffer:' "$LCFG" || fail "D: launched config ($LCFG) lost override keys after restart"
grep -q 'fake-ip' "$LCFG" || fail "D: launched config ($LCFG) lost override dns keys after restart"
# Issue #022 C：restart 之后运行时对账必须闭环，不得留下 unknown/degraded
# Issue #022 C：三份证据（intent / 启动配置 / 受管 TUN 快照）必须收敛到同一 revision，
# 否则 attestation 无法闭环，status 会长期显示 unknown / degraded。
I_REV=$(sha256sum "$CFG/config.yaml" | cut -d' ' -f1)
L_REV=$(sha256sum "$LCFG" | cut -d' ' -f1)
T_REV=$(sha256sum "$RT/tun-config.yaml" | cut -d' ' -f1)
[ "$I_REV" = "$L_REV" ] || fail "D: launched revision != intent revision ($L_REV != $I_REV)"
[ "$I_REV" = "$T_REV" ] || fail "D: TUN snapshot revision != intent revision (Issue #022 C)"
ST=$($CLI status 2>&1)
echo "$ST"
printf '%s' "$ST" | grep -qE 'Configuration: +unknown' && fail "D: status Configuration unknown after restart (Issue #022 C)"
printf '%s' "$ST" | grep -qE 'TUN: +unknown' && fail "D: status TUN unknown after restart (Issue #022 C)"
printf '%s' "$ST" | grep -qE 'Health: +degraded' && fail "D: status Health degraded after restart (Issue #022 C)"
echo "D PASS: override keys survived restart (intent + launched)"

step "6 scenario E: doctor local DNS probe (Issue #022 A)"

# 用本机 UDP DNS 模拟器扮演系统解析器：
#   mode=real 返回真实地址（#022 的绕过签名），mode=fake 返回 fake-ip（正常状态）。
cat > /tmp/mock-dns.py <<'PYEOF'
import socket, struct, sys

mode = sys.argv[1]
ip = "93.184.216.34" if mode == "real" else "198.18.0.10"
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("127.0.0.1", 53))
while True:
    data, addr = sock.recvfrom(512)
    if len(data) < 12:
        continue
    tid = data[:2]
    i = 12
    while i < len(data) and data[i] != 0:
        i += 1 + data[i]
    question = data[12:i + 5]
    header = tid + struct.pack(">HHHHH", 0x8180, 1, 1, 0, 0)
    answer = b"\xc0\x0c" + struct.pack(">HHIH", 1, 1, 30, 4) + socket.inet_aton(ip)
    sock.sendto(header + question + answer, addr)
    print("query", flush=True)
PYEOF

start_mock_dns() {
    pkill -f /tmp/mock-dns.py 2>/dev/null || true
    python3 /tmp/mock-dns.py "$1" >/tmp/mock-dns.log 2>&1 &
    echo $! > /tmp/mock-dns.pid
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        grep -q '0100007F:0035 ' /proc/net/udp 2>/dev/null && return 0
        sleep 0.3
    done
    cat /tmp/mock-dns.log 2>/dev/null
    fail "E: mock DNS resolver did not bind 127.0.0.1:53"
}

cp /etc/resolv.conf /tmp/resolv.conf.bak || fail "E: cannot backup /etc/resolv.conf"
printf 'nameserver 127.0.0.1\n' > /etc/resolv.conf || fail "E: cannot rewrite /etc/resolv.conf"
# MINOR-6（review）：任何 fail/提前退出都必须还原 /etc/resolv.conf 并停掉 mock，
# 否则容器后续步骤会带着被改写的 resolv.conf 继续跑（污染测试环境）。
restore_e_env() {
    pkill -f /tmp/mock-dns.py 2>/dev/null || true
    [ -f /tmp/resolv.conf.bak ] && cp /tmp/resolv.conf.bak /etc/resolv.conf 2>/dev/null || true
}
trap restore_e_env EXIT
query_count() { grep -c '^query$' /tmp/mock-dns.log 2>/dev/null || echo 0; }

# E-1：系统解析器返回真实地址 → doctor 必须报出绕过（#022 当初漏检的正是这一状态）
start_mock_dns real
OUT=$($CLI doctor 2>&1)
echo "$OUT"
printf '%s' "$OUT" | grep -q '系统 DNS 路径' || fail "E: doctor did not run the local resolver probe"
printf '%s' "$OUT" | grep -q '系统解析未经过 mihomo' || fail "E: doctor missed the bypassed system DNS"

# E-2：系统解析器返回 fake-ip → doctor 判定为经过 mihomo
start_mock_dns fake
OUT=$($CLI doctor 2>&1)
echo "$OUT"
printf '%s' "$OUT" | grep -q '系统解析经过 mihomo' || fail "E: doctor did not recognise the fake-ip answer"

# E-3：显式关闭时不得发起任何探测（MINOR-6：断言 mock 收到 0 次查询，而不只是看输出）
BEFORE_Q=$(query_count)
OUT=$($CLI doctor --no-probe-dns 2>&1)
echo "$OUT"
printf '%s' "$OUT" | grep -q '系统 DNS 路径' && fail "E: --no-probe-dns must suppress the probe"
AFTER_Q=$(query_count)
[ "$AFTER_Q" = "$BEFORE_Q" ] || fail "E: --no-probe-dns still sent DNS queries ($BEFORE_Q -> $AFTER_Q)"

# E-4（review 回归-1）：非 root 调用 doctor 不得因读不到 /var/lib 下的运行配置而永远 ❓。
# 仅在探测确实执行（输出含"系统 DNS 路径"）时断言确定态判定，避免 daemon/socket 权限导致的空跑误报。
if command -v runuser >/dev/null 2>&1; then
    id -u testuser >/dev/null 2>&1 || useradd -m testuser
    OUT_E4=$(runuser -u testuser -- "$CLI" doctor --system 2>&1 || true)
    echo "E-4: doctor --system as testuser:"
    printf '%s\n' "$OUT_E4"
    if printf '%s' "$OUT_E4" | grep -q "系统 DNS 路径"; then
        printf '%s' "$OUT_E4" | grep "系统 DNS 路径" | grep -Eq "系统解析经过 mihomo|系统解析未经过 mihomo" \
            || fail "E-4: 非 root doctor 的探测必须给出确定判定（回归-1：attested 时回退读 intent）"
        printf '%s' "$OUT_E4" | grep "系统 DNS 路径" | grep -q "无法判定" \
            && fail "E-4: 非 root doctor 不得因读不到运行配置而退化成 ❓（回归-1）"
        echo "  PASS E-4: 非 root doctor 仍给出确定判定"
    else
        echo "  SKIP E-4: 非 root 未执行探测（daemon/socket 权限），代码路径由单测覆盖"
    fi
else
    echo "  SKIP E-4: runuser 不可用"
fi

restore_e_env
trap - EXIT
echo "E PASS: probe detects bypass, recognises fake-ip, and honours --no-probe-dns"

step "7 scenario F: override tun.* explicitly rejected (Issue #022 §4.7)"
# 控制键（enable/stack/dns-hijack/strict-route/auto-route/auto-detect-interface）被拒绝；
# 非控制键（route-exclude-address 等）按 ADR-22 照常生效——后者正是 #023 的推荐手段。
cat > /tmp/override-tun-owned.yaml <<'OEOF'
tun:
  enable: false
  stack: system
  route-exclude-address:
  - 169.254.0.0/16
sniffer:
  override-destination: true
OEOF
OUT=$($CLI override import /tmp/override-tun-owned.yaml 2>&1) || { echo "$OUT"; fail "F: override import failed"; }
echo "$OUT"
printf '%s' "$OUT" | grep -q 'TUN 事务管辖' || fail "F: tun control keys in override must be explicitly warned"
printf '%s' "$OUT" | grep -q '拒绝' || fail "F: warning must state the keys are rejected"
printf '%s' "$OUT" | grep -q 'tun.enable' || fail "F: warning must name the rejected control keys"
sleep 2
# F-1：TUN 意图不得被 override 翻转（事务提交的 enable: true 必须存活）
grep -q '^tun:' "$CFG/config.yaml" || fail "F: tun block missing from intent"
TUN_HEAD=$(awk '/^tun:/{f=1} f&&/enable:/{print; exit}' "$CFG/config.yaml")
printf '%s' "$TUN_HEAD" | grep -q 'enable: true' || fail "F: tun.enable must stay true after override import (got: $TUN_HEAD)"
# F-2：非控制键正常生效（ADR-22 override 权威不受影响，review 回归-3）
grep -q 'override-destination: true' "$CFG/config.yaml" || fail "F: non-tun override key must merge into intent"
grep -q 'route-exclude-address' "$CFG/config.yaml" || fail "F: non-controlled tun key must merge into intent (issue #023 path)"
grep -q '169.254.0.0/16' "$CFG/config.yaml" || fail "F: route-exclude-address value missing from intent"
# F-3：restart（reconcile 路径）不得把 override 的 tun.enable:false 写回 intent
OUT=$($CLI restart --system --yes 2>&1) || { echo "$OUT"; fail "F: restart failed"; }
echo "$OUT"
sleep 2
TUN_HEAD=$(awk '/^tun:/{f=1} f&&/enable:/{print; exit}' "$CFG/config.yaml")
printf '%s' "$TUN_HEAD" | grep -q 'enable: true' || fail "F: reconcile leaked override tun.enable into intent (got: $TUN_HEAD)"
printf '%s' "$TUN_HEAD" | grep -q 'enable: false' && fail "F: override tun.enable:false must be rejected on reconcile"
LCFG=$(launched_config)
[ -n "$LCFG" ] || fail "F: no launched config recorded"
awk '/^tun:/{f=1} f&&/enable:/{print; exit}' "$LCFG" | grep -q 'enable: true' || fail "F: launched config lost tun.enable"
grep -q 'override-destination: true' "$LCFG" || fail "F: launched config lost non-tun override key"
# TUN 已开启时，运行中的 TUN 快照由 TUN 事务独占（promotion 沿用快照里的 tun 块），
# 因此非控制键写入 intent 后，要等下一次 `tun on --yes` 重建候选才进入运行配置。
# 这里验证 `tun on --yes` 这条真实的生效路径。
if grep -q 'route-exclude-address' "$LCFG"; then
    echo "  note: snapshot already carries route-exclude-address before tun on"
fi
OUT=$($CLI tun on --yes 2>&1) || { echo "$OUT"; fail "F: tun on --yes (re-apply candidate) failed"; }
echo "$OUT"
sleep 2
LCFG=$(launched_config)
[ -n "$LCFG" ] || fail "F: no launched config after tun on --yes"
if ! grep -q 'route-exclude-address' "$LCFG" || ! grep -q '169.254.0.0/16' "$LCFG"; then
    echo "--- launched config: $LCFG"; awk '/^tun:/{f=1} f{print}' "$LCFG" | head -30
    fail "F: non-controlled tun key must reach the launched config after tun on --yes"
fi
awk '/^tun:/{f=1} f&&/enable:/{print; exit}' "$LCFG" | grep -q 'enable: true' || fail "F: tun.enable lost after tun on --yes"
grep -q 'stack: system' "$CFG/config.yaml" && fail "F: override tun.stack must stay rejected after restart"
echo "F PASS: override tun control keys rejected with warning; non-controlled tun keys honoured"

kill "$(cat /tmp/repro-mocksub.pid)" 2>/dev/null
pkill -f "$CLI daemon" 2>/dev/null
pkill -f "$(basename "$FAKE_CORE")" 2>/dev/null
echo
echo "PASS: group promotion + restart application + selection replay (system mode, TUN on)"
exit 0
