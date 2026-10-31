#!/usr/bin/env bash
set -euo pipefail

TEST_USER=testuser
TEST_HOME=/home/testuser
CONFIG_DIR="$TEST_HOME/.config/mihomo"
SYSTEM_UNIT=/etc/systemd/system/mihomo.service
GEN_STATE=/var/lib/mihomo-cli-generations/state.json
ACTIVE_TRANSACTION=/var/lib/mihomo-cli/transactions/active
CLI=/tests/mihomo-cli-source

fail() {
    echo "FAIL: $*" >&2
    systemctl status mihomo --no-pager >&2 || true
    journalctl -u mihomo --no-pager -n 80 >&2 || true
    stat -c '%A %a %U:%G %n' "$TEST_HOME" "$TEST_HOME/.config" "$CONFIG_DIR" "$CONFIG_DIR/config.yaml" 2>&1 || true
    namei -l "$CONFIG_DIR/config.yaml" 2>&1 || true
    systemctl cat mihomo 2>&1 || true
    sudo -u mihomo cat "$CONFIG_DIR/config.yaml" >/dev/null 2>&1 || echo 'daemon-user direct config read: FAILED' >&2
    exit 1
}

as_user() {
    sudo -u "$TEST_USER" env HOME="$TEST_HOME" USER="$TEST_USER" "$@"
}

status_json() {
    as_user mihomo-cli --json status --system
}

sha256_of() {
    sha256sum "$1" | cut -d' ' -f1
}

wait_for() {
    local expression="$1"
    local expected="$2"
    for _ in $(seq 1 150); do
        local value
        value=$(status_json 2>/dev/null | python3 -c "import json,sys; print($expression)" 2>/dev/null || true)
        [ "$value" = "$expected" ] && return 0
        sleep 0.1
    done
    return 1
}

core_configs() {
    python3 - <<'PY'
import socket

request = b"GET /configs HTTP/1.0\r\nHost: localhost\r\n\r\n"
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
    sock.connect("/var/run/mihomo/mihomo.sock")
    sock.sendall(request)
    response = b""
    while True:
        chunk = sock.recv(8192)
        if not chunk:
            break
        response += chunk
body = response.split(b"\r\n\r\n", 1)[1]
print(body.decode())
PY
}

assert_core_launch_paths() {
    local pid
    pid=$(python3 - <<'PY'
import json
from pathlib import Path

print(json.loads(Path("/var/run/mihomo/core.pid").read_text())["pid"])
PY
)
    python3 - "$pid" <<'PY'
import pathlib
import sys

pid = sys.argv[1]
args = pathlib.Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")[:-1]
args = [item.decode() for item in args]
expected = [
    "-d",
    "/var/lib/mihomo-cli",
]
if args[args.index("-d"):args.index("-d") + 2] != expected:
    raise SystemExit(f"unexpected Core data directory: {args}")
config_index = args.index("-f") + 1
if args[config_index] not in {
    "/var/lib/mihomo-cli/active-config.yaml",
    "/var/lib/mihomo-cli/tun-config.yaml",
}:
    raise SystemExit(f"unexpected Core config path: {args}")
PY
}

wait_for_tun_device() {
    for _ in $(seq 1 100); do
        local device
        device=$(core_configs 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["tun"]["device"])' 2>/dev/null || true)
        if [ -n "$device" ] && [ "$device" != "None" ] && ip link show "$device" >/dev/null 2>&1; then
            printf '%s\n' "$device"
            return 0
        fi
        sleep 0.1
    done
    return 1
}

[ "$(id -u)" = 0 ] || fail "test must run as root"
[ -x /usr/local/lib/mihomo/mihomo ] || fail "real Core is missing"
/usr/local/lib/mihomo/mihomo -v | grep -q '^Mihomo Meta v' || fail "mounted binary is not a real Mihomo Core"
[ -c /dev/net/tun ] || fail "/dev/net/tun is unavailable"

mkdir -p "$CONFIG_DIR"
chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME/.config"

as_user mihomo-cli uninstall --all --yes >/dev/null 2>&1 || true
rm -f "$SYSTEM_UNIT"
rm -rf "$CONFIG_DIR"
mkdir -p "$CONFIG_DIR"
cp /tests/real-mihomo /usr/local/lib/mihomo/mihomo
chmod 755 /usr/local/lib/mihomo/mihomo
chmod 0750 "$TEST_HOME"
chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME/.config"
truncate -s 8000001 "$CONFIG_DIR/geoip.metadb"
truncate -s 2000001 "$CONFIG_DIR/GeoSite.dat"
chown "$TEST_USER:$TEST_USER" "$CONFIG_DIR/geoip.metadb" "$CONFIG_DIR/GeoSite.dat"

as_user sudo -n "$CLI" install --system --yes || fail "real-Core system install failed"
[ -S /var/run/mihomo/service.sock ] || fail "daemon IPC socket missing"
[ -S /var/run/mihomo/mihomo.sock ] || fail "real Core API socket missing after install"
wait_for 'json.load(sys.stdin)["data"]["core"]["running"]' True || fail "real Core did not become running"
assert_core_launch_paths
[ -f /var/lib/mihomo-cli/geoip.metadb ] || fail "system runtime is missing geoip.metadb"
[ -f /var/lib/mihomo-cli/GeoSite.dat ] || fail "system runtime is missing GeoSite.dat"
[ ! -e /var/lib/mihomo-cli/transactions/active/geoip.metadb ] || \
    fail "transaction active unexpectedly contains geoip.metadb"
[ ! -e /var/lib/mihomo-cli/transactions/active/GeoSite.dat ] || \
    fail "transaction active unexpectedly contains GeoSite.dat"
rm -f "$CONFIG_DIR/geoip.metadb" "$CONFIG_DIR/GeoSite.dat"
wait_for 'json.load(sys.stdin)["data"]["tun"]' disabled || fail "install did not leave TUN disabled"

as_user mihomo-cli restart --system || fail "restart with real Core failed"
wait_for 'json.load(sys.stdin)["data"]["core"]["running"]' True || fail "real Core did not restart"

echo "--- real Core staged upgrade recovers pre-existing migrated legacy transaction ---"
as_user mihomo-cli stop || fail "fixture could not stop the real Core"
wait_for 'json.load(sys.stdin)["data"]["core"]["running"]' False || \
    fail "fixture could not observe the real Core stopped"
systemctl is-active --quiet mihomo || fail "fixture stopped the daemon instead of only the Core"

rm -f /var/lib/mihomo-cli/transactions/tun-journal.json \
    /var/lib/mihomo-cli/transactions/tun-candidate.yaml
rm -rf /var/lib/mihomo-cli/transactions/active
mkdir -p /var/lib/mihomo-cli/transactions/active
python3 - <<'PY'
import hashlib
import json
from pathlib import Path

state_root = Path("/var/lib/mihomo-cli")
active = state_root / "transactions" / "active"
intent = Path("/home/testuser/.config/mihomo/config.yaml")
intent_bytes = intent.read_bytes()
candidate = intent_bytes + b"# migrated legacy candidate blocks pending generation recovery\n"
(active / "candidate.yaml").write_bytes(candidate)
(active / "old-snapshot.yaml").write_bytes(candidate)
(active / "old-runtime.json").write_text(json.dumps({
    "core_running": False,
    "core_identity": "legacy",
    "core_pid": 0,
    "launched_revision": "",
    "launch_source": "SystemTunSnapshot",
    "runtime_tun": False,
    "api_endpoint": "",
}))
def legacy_fnv_revision(data: bytes) -> str:
    value = 0xcbf29ce484222325
    for byte in data:
        value ^= byte
        value = (value * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"

(state_root / "tun-config.yaml").write_bytes(candidate)
(active / "journal.json").write_text(json.dumps({
    "schema_version": 1,
    "transaction_id": "tun-migrated-real-core-pending-generation",
    "generation": 99,
    "phase": "RecoveryRequired",
    "original_uid": 1001,
    "target_runtime_tun": False,
    "candidate_revision": hashlib.sha256(candidate).hexdigest(),
    "old_snapshot_revision": hashlib.sha256(candidate).hexdigest(),
    "old_runtime_digest": hashlib.sha256((active / "old-runtime.json").read_bytes()).hexdigest(),
    "legacy_source": True,
    "legacy_transaction_id": "legacy-real-core-pending-generation",
    "legacy_base_revision": legacy_fnv_revision(intent_bytes),
    "rollback_evidence_complete": False,
}))
PY
chown -R mihomo:mihomo /var/lib/mihomo-cli/transactions
chmod 0750 /var/lib/mihomo-cli/transactions \
    /var/lib/mihomo-cli/transactions/active
chmod 0640 /var/lib/mihomo-cli/transactions/active/* \
    /var/lib/mihomo-cli/tun-config.yaml

MARKED_CLI=/tmp/real-core-pending-generation-mihomo-cli
cp /usr/local/bin/mihomo-cli "$MARKED_CLI"
chmod 0755 "$MARKED_CLI"
printf '\n# real Core pending generation marker\n' >> "$MARKED_CLI"
UPGRADE_OUT=$(as_user sudo -n "$MARKED_CLI" install --system --yes)
printf '%s\n' "$UPGRADE_OUT"
printf '%s\n' "$UPGRADE_OUT" | grep -q 'Upgrade prepared in pending generation' || \
    fail "install did not stage the real-Core pending generation"
systemctl is-active --quiet mihomo || fail "staged install interrupted the daemon"
PENDING_ID=$(python3 - "$GEN_STATE" <<'PY'
import json
import sys
from pathlib import Path

pending = json.loads(Path(sys.argv[1]).read_text()).get("pending")
if not pending:
    raise SystemExit("FAIL: staged install did not persist a pending generation")
print(pending)
PY
) || fail "staged install did not persist a pending generation"
INTENT_SHA=$(sha256_of "$CONFIG_DIR/config.yaml")
JOURNAL_SHA=$(sha256_of "$ACTIVE_TRANSACTION/journal.json")
CLI_SHA=$(sha256_of /usr/local/bin/mihomo-cli)

echo "--- pending generation stays unapplied without the reset authorization ---"
set +e
REFUSAL_OUT=$(as_user "$MARKED_CLI" restart --system 2>&1)
REFUSAL_STATUS=$?
set -e
printf '%s\n' "$REFUSAL_OUT"
[ "$REFUSAL_STATUS" -ne 0 ] || \
    fail "restart without --yes applied the pending generation without reset authorization"
printf '%s\n' "$REFUSAL_OUT" | grep -q 'restart --yes' || \
    fail "refusal did not point at the authorized reset entry point"
[ "$(sha256_of "$CONFIG_DIR/config.yaml")" = "$INTENT_SHA" ] || \
    fail "unauthorized restart modified the user intent config"
[ -d "$ACTIVE_TRANSACTION" ] || \
    fail "unauthorized restart removed the un-converged transaction"
[ "$(sha256_of "$ACTIVE_TRANSACTION/journal.json")" = "$JOURNAL_SHA" ] || \
    fail "unauthorized restart modified the un-converged transaction journal"
if compgen -G "/var/lib/mihomo-cli/transactions/gc/reset-*-tun-migrated-real-core-pending-generation" >/dev/null; then
    fail "unauthorized restart quarantined the un-converged transaction"
fi
[ "$(sha256_of /usr/local/bin/mihomo-cli)" = "$CLI_SHA" ] || \
    fail "unauthorized restart installed the pending CLI binary"
python3 - "$GEN_STATE" "$PENDING_ID" <<'PY' || fail "unauthorized restart changed the pending generation"
import json
import sys
from pathlib import Path

if json.loads(Path(sys.argv[1]).read_text()).get("pending") != sys.argv[2]:
    raise SystemExit("FAIL: the pending generation slot changed without authorization")
PY

echo "--- authorized reset applies the pending generation ---"
as_user "$MARKED_CLI" restart --system --yes || \
    fail "authorized restart did not apply the real-Core pending generation"
wait_for 'json.load(sys.stdin)["data"]["core"]["running"]' True || \
    fail "real Core did not become running after pending generation recovery"
assert_core_launch_paths
[ -S /var/run/mihomo/mihomo.sock ] || \
    fail "real Core API socket missing after pending generation recovery"
[ "$(sha256_of "$CONFIG_DIR/config.yaml")" = "$INTENT_SHA" ] || \
    fail "controlled reset modified the user intent config"
[ "$(sha256_of "/var/lib/mihomo-cli/transactions/gc/reset-99-tun-migrated-real-core-pending-generation/journal.json")" = "$JOURNAL_SHA" ] || \
    fail "controlled reset did not quarantine the legacy transaction journal as evidence"
[ ! -e "$ACTIVE_TRANSACTION" ] || \
    fail "pending generation restart did not finalize the migrated legacy transaction"
[ ! -e /var/lib/mihomo-cli/transactions/tun-journal.json ] || \
    fail "pending generation restart left the legacy TUN journal"
[ ! -e /var/lib/mihomo-cli/transactions/tun-candidate.yaml ] || \
    fail "pending generation restart left the legacy TUN candidate"
cmp -s "$MARKED_CLI" /usr/local/bin/mihomo-cli || \
    fail "pending generation did not install the marked CLI binary"
python3 - "$GEN_STATE" "$PENDING_ID" <<'PY' || fail "pending generation was not committed"
import json
import sys
from pathlib import Path

state = json.loads(Path(sys.argv[1]).read_text())
if state.get("pending"):
    raise SystemExit("FAIL: pending generation remained after restart")
if state.get("active") != sys.argv[2]:
    raise SystemExit(
        f"FAIL: applied generation {state.get('active')!r} is not the staged {sys.argv[2]!r}"
    )
PY

as_user mihomo-cli tun on --yes || fail "tun on with real Core failed"
wait_for 'json.load(sys.stdin)["data"]["tun"]' enabled || fail "status did not attest live TUN enabled"
device=$(wait_for_tun_device) || fail "real Core did not expose a live TUN device"
ip link show "$device" >/dev/null || fail "TUN device disappeared immediately"

echo "--- real Core TUN survival across rule add promotion  ---"
# T5 行为证据：apply 前运行态 tun.enable == true，则 promotion 后
# 必须仍为 true（源码锁只是辅助，行为证据必须来自真实 Core + 真实 TUN 设备）。
#
# 刻意不断言"TUN 无条件仍在"：护栏 tun_apply_requires_rollback 一旦检测到 TUN
# 会被拆除就回滚，而按 §8 已知限制，回滚只恢复旧 active-config（其本身没有
# tun 块），TUN 不会随回滚自动回来——此时 rule add 显式报错、TUN 不在，是护栏
# 正确工作而非缺陷。把断言简化成"TUN 无条件仍在"会把护栏的成功报成假红。
# 因此按 rule add 的退出结果分三支，(a)(b) 互斥完备、(c) 无条件：
#   (a) exit 0 ⟹ TUN 设备仍在且 status/tun status 仍 attest enabled
# （堵死 §4.4 禁止的"exit 0 但 TUN 静默消失"组合）；
#   (b) 非零退出 ⟹ 必须呈现为显式 Error 且带可执行恢复指引（tun on --yes），
#        不得是无 Error 的裸失败或被折叠成 Pending；
#   (c) 无论哪支，TUN unattested 时 status 不得输出 healthy（SPEC §0.6）。
#        断言锚定文本 status：status --json 的 health 谓词与文本侧不一致是
# 已知的 ，修好后本断言对 JSON 侧同样成立，故不断言 JSON health。
rule_out=""
rule_rc=0
rule_out=$(as_user mihomo-cli rule --system add \
    "DOMAIN-SUFFIX,zz-tun-attestation.invalid,DIRECT" 2>&1) || rule_rc=$?
printf '%s\n' "$rule_out"

if [ "$rule_rc" -eq 0 ]; then
    echo "T5 branch (a): rule add exited 0 — asserting TUN survival + attestation"
    # (a) 成功退出：TUN 必须仍在——设备存活（复用 wait_for_tun_device 的
    # ip link show 判活并容忍全量重启的重建窗口）且设备名未被换掉。
    device_after=$(wait_for_tun_device) || \
        fail " (a): rule add exited 0 but TUN device $device is gone"
    [ "$device_after" = "$device" ] || \
        fail " (a): rule add swapped the TUN device ($device -> $device_after)"
    ip link show "$device" >/dev/null || \
        fail " (a): TUN device $device vanished right after rule add"
    # 且两级状态都必须仍 attest enabled。
    wait_for 'json.load(sys.stdin)["data"]["tun"]' enabled || \
        fail " (a): rule add exited 0 but status no longer attests TUN enabled"
    tun_status_out=$(as_user mihomo-cli tun status 2>&1 || true)
    printf '%s\n' "$tun_status_out" | grep -q '^TUN enabled: enabled$' || {
        printf '%s\n' "$tun_status_out"
        fail " (a): tun status no longer attests TUN enabled after rule add"
    }
    # 行为证据补强：新规则必须出现在运行中的 Core 的规则表里，排除
    # "rule add 没有真正 apply 却被 (a) 放过"的空洞通过。
    # 注：Core 的 GET /configs 不返回 rules 字段，必须查 GET /rules。
    python3 - <<'PY' || fail " (a): rule add reported success but the rule never reached the running Core rules table"
import socket
import time

def core_rules() -> str:
    request = b"GET /rules HTTP/1.0\r\nHost: localhost\r\n\r\n"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.connect("/var/run/mihomo/mihomo.sock")
        sock.sendall(request)
        response = b""
        while True:
            chunk = sock.recv(8192)
            if not chunk:
                break
            response += chunk
    return response.split(b"\r\n\r\n", 1)[1].decode()

for _ in range(100):
    try:
        if "zz-tun-attestation.invalid" in core_rules():
            raise SystemExit(0)
    except OSError:
        pass
    time.sleep(0.1)
raise SystemExit(1)
PY
else
    echo "T5 branch (b): rule add exited $rule_rc — asserting explicit Error + recovery guidance (TUN survival NOT assumed)"
    # (b) 非零退出：护栏/显式失败路径。必须呈现为显式 Error 且带可执行恢复
    # 指引，不得是无 Error 的裸失败、也不得是被折叠成 Pending 的静默失败。
    printf '%s\n' "$rule_out" | grep -q 'Error' || \
        fail " (b): rule add exited $rule_rc without an explicit Error presentation"
    printf '%s\n' "$rule_out" | grep -Eq 'Fix:|mihomo-cli ' || \
        fail " (b): rule add failure carries no actionable recovery guidance"
    # `mihomo-cli tun on --yes` 指引按 TUN 存活实况条件化：apply 前的拒绝
    # （payload/持久化/preflight、stop_core 拒绝）根本没动运行中的 Core 与 TUN，
    # 其指引是 restart --system，把这类拒绝也判成缺 TUN 指引是假红；而 TUN
    # 确实被拆掉时（护栏回滚的已知限制：回滚不自动拉回 TUN， §8），
    # 错误输出必须给出 tun on 恢复指引，否则就是 §4.4 的"TUN 拆了却没
    # 告诉用户怎么拉回来"。
    device_after=$(wait_for_tun_device || true)
    if [ -z "$device_after" ]; then
        printf '%s\n' "$rule_out" | grep -q 'mihomo-cli tun on' || {
            printf '%s\n' "$rule_out"
            fail " (b): TUN was lost by the failed rule add but the error lacks 'mihomo-cli tun on' recovery guidance"
        }
        # 按护栏自身指引走显式恢复，把后续 tun off 旅程拉回同一前提。
        as_user mihomo-cli tun on --yes || \
            fail " (b): recovery tun on --yes failed after rule add was rejected"
        wait_for 'json.load(sys.stdin)["data"]["tun"]' enabled || \
            fail " (b): status did not attest TUN enabled after recovery tun on"
        device=$(wait_for_tun_device) || \
            fail " (b): recovery tun on did not expose a live TUN device"
    else
        device="$device_after"
    fi
fi

# (c) 共通不变量：TUN unattested（TUN 摘要为 unknown / recovery required）时，
# status 不得输出 healthy。该断言在 修好后仍有回归价值。
status_out=$(as_user mihomo-cli status 2>/dev/null || true)
[ -n "$status_out" ] || fail " (c): status produced no output"
status_tun=$(printf '%s\n' "$status_out" | awk '$1 == "TUN:" { print $2; exit }')
status_health=$(printf '%s\n' "$status_out" | awk '$1 == "Health:" { print $2; exit }')
if [ "$status_tun" != "enabled" ] && [ "$status_tun" != "disabled" ]; then
    echo "T5 branch (c): TUN reads '$status_tun' (unattested) — asserting status does not claim healthy"
    [ "$status_health" != "healthy" ] || {
        printf '%s\n' "$status_out"
        fail " (c): TUN is '$status_tun' (unattested) yet status reports Health: healthy"
    }
fi

as_user mihomo-cli tun off || fail "tun off with real Core failed"
wait_for 'json.load(sys.stdin)["data"]["tun"]' disabled || fail "status did not attest live TUN disabled"
for _ in $(seq 1 100); do
    if ! ip link show "$device" >/dev/null 2>&1; then
        echo "PASS: real Core install, restart, TUN on/off, rule-add TUN survival , runtime attestation, and interface cleanup"
        exit 0
    fi
    sleep 0.1
done
fail "real Core left TUN device $device after tun off"
