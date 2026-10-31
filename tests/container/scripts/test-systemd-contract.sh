#!/bin/bash
set -euo pipefail

TEST_USER=testuser
TEST_HOME=/home/testuser
CONFIG_DIR="$TEST_HOME/.config/mihomo"
CONFIG_FILE="$CONFIG_DIR/config.yaml"
TOKEN_FILE="$CONFIG_DIR/service-token"
SYSTEM_UNIT=/etc/systemd/system/mihomo.service
CORE_BINARY=/usr/local/lib/mihomo/mihomo
INSTALLER_CLI=/tests/mihomo-cli-source
GEN_ROOT=/var/lib/mihomo-cli-generations

fail() {
    echo "FAIL: $*" >&2
    systemctl status mihomo --no-pager >&2 || true
    journalctl -u mihomo --no-pager -n 50 >&2 || true
    exit 1
}

as_user() {
    sudo -u "$TEST_USER" env HOME="$TEST_HOME" USER="$TEST_USER" "$@"
}

json_value() {
    local expression="$1"
    python3 -c "import json,sys; value=$expression; print(str(value).lower() if isinstance(value, bool) else value)"
}

status_json() {
    as_user mihomo-cli --json status --system
}

wait_for_json_value() {
    local expression="$1"
    local expected="$2"
    for _ in $(seq 1 100); do
        local output value
        output=$(status_json 2>/dev/null || true)
        value=$(printf '%s' "$output" | json_value "$expression" 2>/dev/null || true)
        [ "$value" = "$expected" ] && return 0
        sleep 0.1
    done
    return 1
}

echo "=== systemd install and lifecycle contract ==="

[ "$(id -u)" = 0 ] || fail "contract must run as root inside the container"
command -v systemctl >/dev/null || fail "systemctl is unavailable"
[ ! -e "$SYSTEM_UNIT" ] || fail "container did not start from an uninstalled state"
[ -x "$CORE_BINARY" ] || fail "fake Core ELF is missing"
"$CORE_BINARY" -v | grep -q "Mihomo Meta" || fail "fake Core failed the version smoke test"

mkdir -p "$CONFIG_DIR"
[ ! -e "$CONFIG_FILE" ] || fail "container fixture unexpectedly contains config.yaml"
chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME/.config"
as_user mihomo-cli uninstall --all --yes
[ ! -e "$SYSTEM_UNIT" ] || fail "clean reinstall setup left the system unit installed"
[ ! -e "$CONFIG_DIR" ] || fail "clean reinstall setup left the shared config directory"
mkdir -p "$CONFIG_DIR"
cp /tests/fake-mihomo "$CORE_BINARY"
chmod 755 "$CORE_BINARY"
chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME/.config"

# Keep installation offline while satisfying the installer's Geo integrity gates.
truncate -s 8000001 "$CONFIG_DIR/geoip.metadb"
truncate -s 2000001 "$CONFIG_DIR/GeoSite.dat"
chown "$TEST_USER:$TEST_USER" "$CONFIG_DIR/geoip.metadb" "$CONFIG_DIR/GeoSite.dat"

echo "--- install --system from a normal user (no outer sudo) ---"
# 裁决 B 验收：标准安装态主旅程必须由普通用户直接驱动；CLI 自己按业务阶段请求
# 管理员授权（不预先 sudo、不预先加组、不放宽 mode）。
as_user "$INSTALLER_CLI" install --system --yes

[ -f "$SYSTEM_UNIT" ] || fail "systemd unit was not installed without an initial config"
grep -q '^User=mihomo$' "$SYSTEM_UNIT" || fail "unit does not run the daemon as mihomo"
grep -q '^ExecStart=/usr/local/bin/mihomo-cli daemon$' "$SYSTEM_UNIT" || fail "unit ExecStart is incorrect"
systemctl is-enabled --quiet mihomo || fail "systemd unit was not enabled"
systemctl is-active --quiet mihomo || fail "systemd daemon is not active after install"
DAEMON_PID=$(systemctl show mihomo -p MainPID --value)
DAEMON_UID=$(awk '/^Uid:/ { print $2 }' "/proc/$DAEMON_PID/status")
printf 'daemon pid=%s uid=%s expected_uid=%s\n' "$DAEMON_PID" "$DAEMON_UID" "$(id -u mihomo)"
[ "$DAEMON_UID" = "$(id -u mihomo)" ] || \
    fail "system daemon is not running as the mihomo service account"
[ -S /var/run/mihomo/service.sock ] || fail "daemon IPC socket is missing after install"
[ -f "$CONFIG_FILE" ] || fail "install without subscription did not generate direct-only config.yaml"
[ -S /var/run/mihomo/mihomo.sock ] || fail "install without subscription did not start the Core API"
grep -q '^mode: rule$' "$CONFIG_FILE" || fail "direct-only config did not set rule mode"
grep -q '^proxies: \[\]$' "$CONFIG_FILE" || fail "direct-only config did not set empty proxies"
grep -q '^proxy-groups: \[\]$' "$CONFIG_FILE" || fail "direct-only config did not set empty proxy groups"
grep -q 'MATCH,DIRECT' "$CONFIG_FILE" || fail "direct-only config did not set DIRECT fallback"
if grep -q '^tun:' "$CONFIG_FILE" && grep -A1 '^tun:' "$CONFIG_FILE" | grep -q 'enable: true'; then
    fail "direct-only install unexpectedly enabled TUN"
fi
[ -f "$TOKEN_FILE" ] || fail "missing-config install did not provision the client token"
[ -f /var/lib/mihomo-cli/geoip.metadb ] || fail "install did not stage geoip.metadb for TUN runtime"
[ -f /var/lib/mihomo-cli/GeoSite.dat ] || fail "install did not stage GeoSite.dat for TUN runtime"
[ "$(stat -c '%s' /var/lib/mihomo-cli/geoip.metadb)" -gt 8000000 ] || fail "TUN runtime geoip.metadb is incomplete"
[ "$(stat -c '%s' /var/lib/mihomo-cli/GeoSite.dat)" -gt 2000000 ] || fail "TUN runtime GeoSite.dat is incomplete"
[ "$(stat -c '%G' /var/lib/mihomo-cli/geoip.metadb)" = "mihomo" ] || fail "TUN runtime geoip.metadb is not readable by mihomo group"
[ "$(stat -c '%G' /var/lib/mihomo-cli/GeoSite.dat)" = "mihomo" ] || fail "TUN runtime GeoSite.dat is not readable by mihomo group"
wait_for_json_value 'json.load(sys.stdin)["data"]["configuration"]' applied || \
    fail "direct-only install did not attest the running configuration as applied"
[ "$(stat -c '%a' "$TOKEN_FILE")" = "600" ] || fail "client token mode is not 0600"
[ "$(stat -c '%U:%G' "$TOKEN_FILE")" = "$TEST_USER:$TEST_USER" ] || \
    fail "client token owner is not $TEST_USER:$TEST_USER"
CORE_PID_BEFORE_ACCESS=$(status_json | json_value 'json.load(sys.stdin)["data"]["core"]["pid"]')
DOCTOR_OUTPUT=$(as_user mihomo-cli doctor --system)
printf '%s\n' "$DOCTOR_OUTPUT"
printf '%s\n' "$DOCTOR_OUTPUT" | grep -q 'Daemon 授权.*daemon_authenticated=true' || \
    fail "doctor did not report the original user as authorized"
CORE_PID_AFTER_ACCESS=$(status_json | json_value 'json.load(sys.stdin)["data"]["core"]["pid"]')
[ "$CORE_PID_AFTER_ACCESS" = "$CORE_PID_BEFORE_ACCESS" ] || \
    fail "doctor changed the running Core process"

echo "--- config import after install completes the clean reinstall journey ---"
cat > /tmp/mihomo-clean-config.yaml <<'EOF'
mixed-port: 7890
allow-lan: false
mode: rule
proxies: []
proxy-groups: []
rules:
  - MATCH,DIRECT
external-controller-unix: /var/run/mihomo/mihomo.sock
EOF
chown "$TEST_USER:$TEST_USER" /tmp/mihomo-clean-config.yaml
IMPORT_OUTPUT=$(as_user mihomo-cli config --import /tmp/mihomo-clean-config.yaml --yes)
printf '%s\n' "$IMPORT_OUTPUT"
printf '%s\n' "$IMPORT_OUTPUT" | grep -q '^  ✅ system configuration promoted and runtime applied$' || \
    fail "config import did not report successful system promotion"
[ -f "$CONFIG_FILE" ] || fail "config import did not create config.yaml after install"
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
    fail "config import did not preserve a running Core after reload"
[ -S /var/run/mihomo/mihomo.sock ] || fail "config import reload left the Core API unavailable"
wait_for_json_value 'json.load(sys.stdin)["data"]["configuration"]' applied || \
    fail "config import did not attest the imported configuration as applied"

# The target journey ends with an explicit restart and a usable Core service.
as_user mihomo-cli restart --system
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
    fail "restart did not start Core after config import"
[ -S /var/run/mihomo/mihomo.sock ] || fail "Core API socket is missing after clean reinstall journey"
wait_for_json_value 'json.load(sys.stdin)["data"]["configuration"]' applied || \
    fail "restart did not attest the imported configuration as applied"

# R2 test-first：验收 DoD「System 非 TUN 配置变更 pid 不变」。
# 当前实现 promote 一刀切（daemon 无条件 stop_core+start_core）→ 本段预期在 R2 落地前 FAIL；
# R2 热重载落地后必须 PASS。
# R2 落地时需同步修订上方 "import 必须输出 promoted 文案" 断言（改热重载文案），
# 本段只负责 pid 不变；新旧两段在 R2 前允许一红一绿或同红。
echo "--- System non-TUN config change must keep Core pid (R2 invariant) ---"
PID_BEFORE_MUTATION=$(status_json | json_value 'json.load(sys.stdin)["data"]["core"]["pid"]')
wait_for_json_value 'json.load(sys.stdin)["data"]["configuration"]' applied || \
    fail "pre-mutation baseline configuration is not applied"
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
    fail "pre-mutation Core is not running"

cat > /tmp/mihomo-r2-non-tun-config.yaml <<'EOF'
mixed-port: 7890
allow-lan: false
mode: rule
log-level: warning
proxies: []
proxy-groups: []
rules:
  - MATCH,DIRECT
external-controller-unix: /var/run/mihomo/mihomo.sock
EOF
chown "$TEST_USER:$TEST_USER" /tmp/mihomo-r2-non-tun-config.yaml
MUTATION_OUTPUT=$(as_user mihomo-cli config --import /tmp/mihomo-r2-non-tun-config.yaml --activate --yes 2>&1) || {
    printf '%s\n' "$MUTATION_OUTPUT"
    fail "non-TUN config mutation command failed: $MUTATION_OUTPUT"
}
printf '%s\n' "$MUTATION_OUTPUT"
wait_for_json_value 'json.load(sys.stdin)["data"]["configuration"]' applied || \
    fail "non-TUN config mutation did not reach configuration=applied"
PID_AFTER_MUTATION=$(status_json | json_value 'json.load(sys.stdin)["data"]["core"]["pid"]')
[ "$PID_AFTER_MUTATION" = "$PID_BEFORE_MUTATION" ] || \
    fail "R2 invariant: System non-TUN change restarted Core (old=$PID_BEFORE_MUTATION new=$PID_AFTER_MUTATION)"

echo "--- install remains idempotent after the clean reinstall journey ---"
as_user "$INSTALLER_CLI" install --system --skip-config --yes
[ -S /var/run/mihomo/mihomo.sock ] || fail "idempotent install unexpectedly stopped Core"

echo "--- resolved paths and running state ---"
STATUS=$(status_json)
printf '%s\n' "$STATUS"
CONFIG_PATH=$(printf '%s' "$STATUS" | json_value 'json.load(sys.stdin)["data"]["config"]["path"]')
MODE=$(printf '%s' "$STATUS" | json_value 'json.load(sys.stdin)["data"]["mode"]')
CORE_RUNNING=$(printf '%s' "$STATUS" | json_value 'json.load(sys.stdin)["data"]["core"]["running"]')
[ "$CONFIG_PATH" = "$CONFIG_FILE" ] || fail "resolved config path is $CONFIG_PATH, expected $CONFIG_FILE"
[ "$MODE" = "system service" ] || fail "resolved mode is $MODE, expected system service"
[ "$CORE_RUNNING" = "true" ] || fail "Core is not reported running after install"

echo "--- stop -> explicit start -> restart lifecycle ---"
PID_BEFORE=$(printf '%s' "$STATUS" | json_value 'json.load(sys.stdin)["data"]["core"]["pid"]')
as_user mihomo-cli stop
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' false || fail "Core did not stop"
systemctl is-active --quiet mihomo || fail "stop incorrectly stopped the daemon service"

as_user mihomo-cli start
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || fail "explicit start did not restore Core running state"
PID_AFTER_START=$(status_json | json_value 'json.load(sys.stdin)["data"]["core"]["pid"]')
[ "$PID_AFTER_START" != "$PID_BEFORE" ] || fail "start reused the stopped Core PID"

chown root:mihomo "$CONFIG_FILE"
as_user mihomo-cli restart
[ "$(stat -c '%u' "$CONFIG_FILE")" = "$(id -u "$TEST_USER")" ] || \
    fail "restart did not restore configuration ownership"
[ "$(stat -c '%g' "$CONFIG_FILE")" = "$(getent group mihomo | cut -d: -f3)" ] || \
    fail "restart did not preserve the service group"
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || fail "Core did not restart"
PID_AFTER_RESTART=$(status_json | json_value 'json.load(sys.stdin)["data"]["core"]["pid"]')
[ "$PID_AFTER_RESTART" != "$PID_AFTER_START" ] || fail "restart did not replace the Core process"

as_user mihomo-cli stop
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' false || fail "Core did not stop before explicit restart"
as_user mihomo-cli restart
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || fail "explicit restart did not restore Core running state"
PID_AFTER_SECOND_RESTART=$(status_json | json_value 'json.load(sys.stdin)["data"]["core"]["pid"]')
[ "$PID_AFTER_SECOND_RESTART" != "$PID_AFTER_RESTART" ] || fail "second restart reused the stopped Core PID"

echo "--- terminal proxy output uses the installed instance ---"
if as_user python3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.connect("/var/run/mihomo/mihomo.sock")' 2>/dev/null; then
    fail "ordinary user bypassed daemon authorization and connected directly to Core API"
fi
PROXY_ON=$(as_user mihomo-cli proxy on)
printf '%s\n' "$PROXY_ON" | grep -q 'export http_proxy=http://127.0.0.1:7890' || fail "proxy on omitted http_proxy"
printf '%s\n' "$PROXY_ON" | grep -q 'export https_proxy=http://127.0.0.1:7890' || fail "proxy on omitted https_proxy"
PROXY_OFF=$(as_user mihomo-cli proxy off)
printf '%s\n' "$PROXY_OFF" | grep -q 'unset http_proxy' || fail "proxy off omitted unset command"
PROXY_LIST=$(as_user mihomo-cli list)
printf '%s\n' "$PROXY_LIST" | grep -q 'Test' || fail "list did not use the daemon-backed Core API"
SELECT_OUTPUT=$(as_user mihomo-cli select --group Test --node DIRECT)
printf '%s\n' "$SELECT_OUTPUT" | grep -q 'Switched Test' || fail "select did not use the daemon-backed Core API"
if sudo -u untrusted env HOME=/home/untrusted USER=untrusted mihomo-cli list >/tmp/untrusted.out 2>&1; then
    fail "unauthorized user accessed the daemon-backed Core API"
fi
grep -q 'invalid or missing auth token' /tmp/untrusted.out || fail "unauthorized user did not receive an auth rejection"

sudo -u untrusted env HOME=/home/untrusted USER=untrusted \
    mihomo-cli doctor --system >/tmp/untrusted-doctor.out 2>&1 || true
UNTRUSTED_AUTH_BLOCK=$(awk '
    /Daemon 授权:/ { capture = 1 }
    capture && /^  (✅|❌)/ && !/Daemon 授权:/ { exit }
    capture { print }
' /tmp/untrusted-doctor.out)
printf '%s\n' "$UNTRUSTED_AUTH_BLOCK"
[ -n "$UNTRUSTED_AUTH_BLOCK" ] || fail "untrusted doctor omitted the daemon authorization block"
printf '%s\n' "$UNTRUSTED_AUTH_BLOCK" | grep -q 'sudo mihomo-cli install --system' || \
    fail "untrusted doctor auth block did not point to install --system"
if printf '%s\n' "$UNTRUSTED_AUTH_BLOCK" | grep -q 'mihomo-cli restart --system'; then
    fail "untrusted doctor auth block incorrectly recommends restart --system"
fi

echo "--- OS system proxy on/off dispatches exact GNOME settings ---"
GSETTINGS_LOG=/tmp/mihomo-fake-gsettings.log
rm -f "$GSETTINGS_LOG"
as_user env MIHOMO_FAKE_GSETTINGS_LOG="$GSETTINGS_LOG" mihomo-cli system-proxy on
grep -Fxq 'set org.gnome.system.proxy mode manual' "$GSETTINGS_LOG" || fail "system-proxy on did not select manual mode"
grep -Fxq 'set org.gnome.system.proxy.http port 7890' "$GSETTINGS_LOG" || fail "system-proxy on did not set the resolved HTTP port"
grep -Fxq 'set org.gnome.system.proxy.https port 7890' "$GSETTINGS_LOG" || fail "system-proxy on did not set the resolved HTTPS port"
grep -Fxq 'set org.gnome.system.proxy.socks port 7890' "$GSETTINGS_LOG" || fail "system-proxy on did not set the resolved SOCKS port"
as_user env MIHOMO_FAKE_GSETTINGS_LOG="$GSETTINGS_LOG" mihomo-cli system-proxy off
[ "$(tail -n 1 "$GSETTINGS_LOG")" = 'set org.gnome.system.proxy mode none' ] || fail "system-proxy off did not restore GNOME proxy mode to none"

echo "--- core autostart on/status/off preserves daemon infrastructure ---"
# Bug #11 修复后：daemon 拥有自启 marker，写入/读取都在固定 runtime；
# CLI 经 SetAutostart / GetStatus IPC 与 daemon 交互。
AUTOSTART_MARKER="/var/lib/mihomo-cli/autostart"
as_user mihomo-cli autostart on --system
[ -f "$AUTOSTART_MARKER" ] || fail "autostart on did not create the daemon-owned system-core marker"
as_user mihomo-cli autostart status --system | grep -Fxq 'Autostart: enabled (system core)' || fail "autostart status did not report enabled"
systemctl is-enabled --quiet mihomo || fail "core autostart on changed daemon unit enablement"
# daemon 重启后 Core 必须随 autostart marker 自动拉起（Bug #11 端到端回归）
systemctl restart mihomo
for _ in $(seq 1 50); do
    if as_user mihomo-cli --json status --system 2>/dev/null \
        | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['core']['running'])" 2>/dev/null \
        | grep -Fxq True; then
        break
    fi
    sleep 0.2
done
as_user mihomo-cli --json status --system \
    | python3 -c "import json,sys; print(json.load(sys.stdin)['data']['core']['running'])" 2>/dev/null \
    | grep -Fxq True || fail "daemon restart did not auto-start the Core via autostart marker"
as_user mihomo-cli autostart off --system
[ ! -e "$AUTOSTART_MARKER" ] || fail "autostart off did not remove the daemon-owned system-core marker"
as_user mihomo-cli autostart status --system | grep -Fxq 'Autostart: disabled (system core)' || fail "autostart status did not report disabled"
systemctl is-enabled --quiet mihomo || fail "core autostart off disabled daemon infrastructure"

echo "--- TUN preflight rejects missing runtime Geo before creating a transaction ---"
rm /var/lib/mihomo-cli/GeoSite.dat
if as_user mihomo-cli tun on --yes > /tmp/tun-geo-preflight.out 2>&1; then
    cat /tmp/tun-geo-preflight.out >&2
    fail "tun on unexpectedly continued without TUN runtime GeoSite.dat"
fi
[ ! -e "/var/lib/mihomo-cli/transactions/tun-journal.json" ] || \
    fail "tun on created a journal before runtime Geo preflight passed"
cp "$CONFIG_DIR/GeoSite.dat" /var/lib/mihomo-cli/GeoSite.dat
chown mihomo:mihomo /var/lib/mihomo-cli/GeoSite.dat
chmod 0640 /var/lib/mihomo-cli/GeoSite.dat

echo "--- TUN on/status/off through sudo re-exec ---"
mkdir -p /var/lib/mihomo-cli
cat > /var/lib/mihomo-cli/tun-config.yaml <<'EOF'
mode: rule
proxies: []
proxy-groups:
  - name: Test
    type: select
    proxies:
      - DIRECT
rules:
  - MATCH,DIRECT
EOF
chown root:root /var/lib/mihomo-cli /var/lib/mihomo-cli/geoip.metadb /var/lib/mihomo-cli/tun-config.yaml
chmod 0750 /var/lib/mihomo-cli
chmod 0600 /var/lib/mihomo-cli/tun-config.yaml
python3 - <<'PY'
import json
from pathlib import Path

config = Path("/home/testuser/.config/mihomo/config.yaml")
transactions = Path("/var/lib/mihomo-cli/transactions")
transactions.mkdir(parents=True, exist_ok=True)
candidate = transactions / "tun-candidate.yaml"
candidate.write_bytes(b"prepared-but-unpromoted\n")

def revision(data):
    value = 0xcbf29ce484222325
    for byte in data:
        value ^= byte
        value = (value * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"

journal = {
    "transaction_id": "stale-prepared",
    "base_revision": revision(config.read_bytes()),
    "candidate_revision": revision(candidate.read_bytes()),
    "state": "Prepared",
    "candidate_path": str(candidate),
    "snapshot_path": "/var/lib/mihomo-cli/tun-config.yaml",
}
(transactions / "tun-journal.json").write_text(json.dumps(journal))
PY
chown -R root:root /var/lib/mihomo-cli/transactions
chmod 0755 /var/lib/mihomo-cli/transactions
chmod 0644 /var/lib/mihomo-cli/transactions/tun-candidate.yaml /var/lib/mihomo-cli/transactions/tun-journal.json
# the legacy fixture must observe the current contract — early
# refusal with the explicit controlled-reset next step, zero runtime side
# effects. `tun on --yes` has no runtime-reset authority (SPEC §12.10);
# the migrated RecoveryRequired journal with incomplete legacy evidence
# refuses under Resume + Base intent (src/tun_transaction.rs plan_recovery).
LEGACY_JOURNAL=/var/lib/mihomo-cli/transactions/tun-journal.json
LEGACY_CANDIDATE=/var/lib/mihomo-cli/transactions/tun-candidate.yaml
SNAPSHOT_FILE=/var/lib/mihomo-cli/tun-config.yaml
# the daemon runs as user mihomo, and the root:root 0750 fixture
# applied above blinds its state-dir reads — daemon-reported core.pid is None
# for the whole window even though the Core process keeps running untouched.
# The process table is the ground truth for the "refusal must not replace the
# running Core" invariant.
CORE_PID_BEFORE_LEGACY=$(pgrep -x mihomo | head -n1)
[ -n "$CORE_PID_BEFORE_LEGACY" ] || fail "legacy fixture baseline: no running Core process"
SNAPSHOT_SHA_BEFORE=$(sha256sum "$SNAPSHOT_FILE" | cut -d' ' -f1)
INTENT_SHA_BEFORE=$(sha256sum "$CONFIG_FILE" | cut -d' ' -f1)
LEGACY_JOURNAL_SHA_BEFORE=$(sha256sum "$LEGACY_JOURNAL" | cut -d' ' -f1)
LEGACY_CANDIDATE_SHA_BEFORE=$(sha256sum "$LEGACY_CANDIDATE" | cut -d' ' -f1)
if as_user mihomo-cli tun on --yes > /tmp/tun-legacy-recovery.out 2>&1; then
    cat /tmp/tun-legacy-recovery.out >&2
    fail "tun on accepted a legacy transaction without attested old runtime evidence"
fi
cat /tmp/tun-legacy-recovery.out
# plan the refusal before any runtime attestation, so the contract is
# "refused at RecoveryRequired + actionable Fix", not the retired attestation text.
# additionally requires the user-visible managed recovery message. Both texts
# come from the same refusal (planner Fix line + managed_recovery_required_message),
# so both are asserted.
grep -q 'RecoveryRequired' /tmp/tun-legacy-recovery.out || {
    cat /tmp/tun-legacy-recovery.out >&2
    fail "legacy recovery refusal did not report the RecoveryRequired phase"
}
grep -q '暂时无法自动恢复' /tmp/tun-legacy-recovery.out || {
    cat /tmp/tun-legacy-recovery.out >&2
    fail "legacy recovery did not refuse with the managed recovery message"
}
grep -q 'mihomo-cli restart --yes' /tmp/tun-legacy-recovery.out || {
    cat /tmp/tun-legacy-recovery.out >&2
    fail "legacy recovery refusal did not offer the restart --yes next step"
}
if grep -qE '已保留用户配置并重置|正在从用户配置重新建立运行状态|受控重置未执行' /tmp/tun-legacy-recovery.out; then
    cat /tmp/tun-legacy-recovery.out >&2
    fail "legacy recovery refusal performed or claimed a controlled runtime reset"
fi
# Ownership drift converges to the managed owners: the sudo re-exec'd `tun on`
# calls ensure_mihomo_system_state_dir (src/main.rs tun entry + legacy-migration
# admission) which repairs the state root to root:mihomo 770 and the migrated
# active journal to mihomo:mihomo 640. This is permission convergence only,
# not a runtime reset.
[ "$(stat -c '%U:%G %a' /var/lib/mihomo-cli)" = "root:mihomo 770" ] || \
    fail "legacy recovery did not restore the group-writable system state root"
[ "$(stat -c '%U:%G %a' /var/lib/mihomo-cli/transactions/active/journal.json)" = "mihomo:mihomo 640" ] || \
    fail "legacy recovery did not restore the active journal ownership"
# State invariants: the refusal must not touch the user config, the managed
# snapshot, the legacy evidence, or the running Core.
[ "$(sha256sum "$SNAPSHOT_FILE" | cut -d' ' -f1)" = "$SNAPSHOT_SHA_BEFORE" ] || \
    fail "legacy recovery modified the managed TUN snapshot content"
[ "$(sha256sum "$CONFIG_FILE" | cut -d' ' -f1)" = "$INTENT_SHA_BEFORE" ] || \
    fail "legacy recovery modified the user intent configuration"
[ "$(sha256sum "$LEGACY_JOURNAL" | cut -d' ' -f1)" = "$LEGACY_JOURNAL_SHA_BEFORE" ] || \
    fail "legacy recovery modified the legacy journal evidence"
[ "$(sha256sum "$LEGACY_CANDIDATE" | cut -d' ' -f1)" = "$LEGACY_CANDIDATE_SHA_BEFORE" ] || \
    fail "legacy recovery modified the legacy candidate evidence"
CORE_PID_AFTER_LEGACY=$(pgrep -x mihomo | head -n1)
[ "$CORE_PID_AFTER_LEGACY" = "$CORE_PID_BEFORE_LEGACY" ] || \
    fail "legacy recovery replaced the running Core (old=$CORE_PID_BEFORE_LEGACY new=$CORE_PID_AFTER_LEGACY)"
# The refusal's sudo re-exec already repaired the state-dir ownership, so the
# daemon's own view is trustworthy again and must report the Core running.
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
    fail "legacy recovery stopped the running Core"
python3 - <<'PY' || fail "legacy recovery did not leave a parseable RecoveryRequired journal"
import json

journal = json.load(open("/var/lib/mihomo-cli/transactions/active/journal.json"))
assert journal["phase"] == "RecoveryRequired", journal["phase"]
assert journal["legacy_source"] is True
assert journal["rollback_evidence_complete"] is False
# None-valued candidate identity fields are skipped by the serializer
# (skip_serializing_if), so absence — not an explicit null — is the proof.
assert journal.get("candidate_core_identity") is None
assert journal.get("candidate_core_pid") is None
assert journal["legacy_transaction_id"] == "stale-prepared"
PY
[ -d /var/lib/mihomo-cli/transactions/gc ] || \
    fail "state repair did not create the managed gc directory"
[ -z "$(ls -A /var/lib/mihomo-cli/transactions/gc)" ] || \
    fail "legacy recovery quarantined artifacts into gc without an explicit reset"
if find /var/lib/mihomo-cli -maxdepth 1 -name 'cache.db.corrupt-*' | grep -q .; then
    fail "legacy recovery isolated cache.db without an explicit reset"
fi
# the controlled reset must now converge on exactly this fixture.
# The migrated placeholder answers "which old runtime was this?" (it does not
# claim the current Core is stopped); the running Core is independently proven
# to be the current managed instance, stopped with an observed exit, and only
# then are the artifacts reset. No pre-stop, no evidence deletion, no faked
# fingerprint is involved — the fixture above is still the untouched v9 shape.
CONFIG_SHA_BEFORE_LEGACY_RESET=$(sha256sum "$CONFIG_FILE" | cut -d' ' -f1)
CORE_PID_BEFORE_LEGACY_RESET=$(pgrep -x mihomo | head -n1)
[ -n "$CORE_PID_BEFORE_LEGACY_RESET" ] || fail "legacy reset fixture: no running Core"
LEGACY_RESET_GENERATION=$(python3 -c 'import json; print(json.load(open("/var/lib/mihomo-cli/transactions/active/journal.json"))["generation"])')
# Boundary negative controls: break one admission condition at a time and prove
# the destructive cleanup count stays 0 while the unknown process keeps running.
ACTIVE_BACKUP=/tmp/legacy-active-backup
rm -rf "$ACTIVE_BACKUP"
cp -a /var/lib/mihomo-cli/transactions/active "$ACTIVE_BACKUP"
cp -a /var/run/mihomo/core.pid /tmp/legacy-core-pid-backup
GC_RESET_COUNT_BEFORE=$(find /var/lib/mihomo-cli/transactions/gc -maxdepth 1 -name 'reset-*' | wc -l)

# (a) placeholder evidence digest mismatch
printf '\n' >> /var/lib/mihomo-cli/transactions/active/old-runtime.json
if as_user mihomo-cli restart --system --yes >/tmp/legacy-reset-bad-evidence.out 2>&1; then
    cat /tmp/legacy-reset-bad-evidence.out >&2
    fail "controlled reset accepted a mismatched old-runtime digest"
fi
[ "$(pgrep -x mihomo | head -n1)" = "$CORE_PID_BEFORE_LEGACY_RESET" ] || \
    fail "a refused reset replaced the unknown Core"
[ -d /var/lib/mihomo-cli/transactions/active ] || \
    fail "a refused reset deleted the transaction evidence"
rm -rf /var/lib/mihomo-cli/transactions/active
cp -a "$ACTIVE_BACKUP" /var/lib/mihomo-cli/transactions/active

# (b) current-instance ownership cannot be established (no launch record)
rm -f /var/run/mihomo/core.pid
if as_user mihomo-cli restart --system --yes >/tmp/legacy-reset-bad-owner.out 2>&1; then
    cat /tmp/legacy-reset-bad-owner.out >&2
    fail "controlled reset accepted an unprovable Core ownership"
fi
[ "$(pgrep -x mihomo | head -n1)" = "$CORE_PID_BEFORE_LEGACY_RESET" ] || \
    fail "the ownership refusal terminated the unknown Core"
[ -d /var/lib/mihomo-cli/transactions/active ] || \
    fail "the ownership refusal deleted the transaction evidence"
cp -a /tmp/legacy-core-pid-backup /var/run/mihomo/core.pid
[ "$(find /var/lib/mihomo-cli/transactions/gc -maxdepth 1 -name 'reset-*' | wc -l)" = "$GC_RESET_COUNT_BEFORE" ] || \
    fail "a refused admission quarantined the transaction"

if ! as_user mihomo-cli restart --system --yes > /tmp/legacy-reset.out 2>&1; then
    cat /tmp/legacy-reset.out >&2
    echo "--- legacy reset diagnostics ---" >&2
    find /var/lib/mihomo-cli/transactions -printf '%y %M %u:%g %p\n' >&2 || true
    journalctl -u mihomo.service --no-pager -n 100 >&2 || true
    fail "restart --yes did not converge the migrated legacy transaction while the Core ran"
fi
cat /tmp/legacy-reset.out
grep -q '检测到旧版本运行状态' /tmp/legacy-reset.out || \
    fail "controlled reset did not state its impact to the user"
grep -q '已保留用户配置并重置' /tmp/legacy-reset.out || \
    fail "controlled reset did not report the reset"
[ ! -e /var/lib/mihomo-cli/transactions/active ] || \
    fail "legacy reset left the active transaction"
[ ! -e "$LEGACY_JOURNAL" ] || fail "legacy reset left the legacy journal"
[ ! -e "$LEGACY_CANDIDATE" ] || fail "legacy reset left the legacy candidate"
find /var/lib/mihomo-cli/transactions/gc -maxdepth 1 \
    -name "reset-${LEGACY_RESET_GENERATION}-*" | grep -q . || \
    fail "legacy reset did not quarantine the transaction evidence"
[ "$(sha256sum "$CONFIG_FILE" | cut -d' ' -f1)" = "$CONFIG_SHA_BEFORE_LEGACY_RESET" ] || \
    fail "legacy reset changed the user configuration"
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
    fail "legacy reset did not restore the Core"
CORE_PID_AFTER_LEGACY_RESET=$(pgrep -x mihomo | head -n1)
[ -n "$CORE_PID_AFTER_LEGACY_RESET" ] && \
    [ "$CORE_PID_AFTER_LEGACY_RESET" != "$CORE_PID_BEFORE_LEGACY_RESET" ] || \
    fail "legacy reset did not replace the Core process"
if ! as_user mihomo-cli tun on --yes > /tmp/tun-on.out 2>&1; then
    cat /tmp/tun-on.out >&2
    echo "--- TUN transaction diagnostics ---" >&2
    find /var/lib/mihomo-cli/transactions -printf '%y %M %u:%g %p\n' >&2 || true
    journalctl -u mihomo.service --no-pager -n 100 >&2 || true
    fail "tun on failed after legacy fixture cleanup"
fi
[ "$(stat -c '%U:%G' /var/lib/mihomo-cli/tun-config.yaml)" = "mihomo:mihomo" ] || \
    fail "tun on did not restore the managed TUN snapshot owner"
[ "$(stat -c '%a' /var/lib/mihomo-cli/tun-config.yaml)" = "640" ] || \
    fail "tun on did not restore the managed TUN snapshot mode"
if ! wait_for_json_value 'json.load(sys.stdin)["data"]["tun"]' enabled; then
    cat /tmp/tun-on.out >&2
    as_user mihomo-cli tun status --system >&2 || true
    find /var/lib/mihomo-cli/transactions -printf '%y %M %u:%g %p\n' >&2 || true
    fail "TUN did not become enabled"
fi
as_user mihomo-cli restart --system
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || fail "Core did not restart after TUN on"
wait_for_json_value 'json.load(sys.stdin)["data"]["tun"]' enabled || fail "TUN state was not preserved after restart"
SELECT_TUN_OUTPUT=$(as_user mihomo-cli select --group Test --node DIRECT)
printf '%s\n' "$SELECT_TUN_OUTPUT"
printf '%s\n' "$SELECT_TUN_OUTPUT" | grep -q 'Switched Test' || \
    fail "select --node failed while system TUN was active"
TUN_STATUS=$(as_user mihomo-cli tun status)
printf '%s\n' "$TUN_STATUS"
printf '%s\n' "$TUN_STATUS" | grep -q '^TUN enabled: enabled$' || {
    echo "--- TUN status diagnostics ---" >&2
    status_json >&2 || true
    stat /var/lib/mihomo-cli/transactions /var/lib/mihomo-cli/transactions/tun-journal.json >&2 || true
    sudo -u "$TEST_USER" test -r /var/lib/mihomo-cli/transactions/tun-journal.json || echo "testuser cannot read journal" >&2
    sudo -u "$TEST_USER" cat /var/lib/mihomo-cli/transactions/tun-journal.json >&2 || true
    python3 -c 'import json; print(json.dumps(json.load(open("/var/lib/mihomo-cli/transactions/tun-journal.json")), indent=2))' >&2 || true
    python3 - <<'PY' >&2
import pathlib

def revision(path):
    value = 0xcbf29ce484222325
    for byte in pathlib.Path(path).read_bytes():
        value ^= byte
        value = (value * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"

for path in [
    "/var/lib/mihomo-cli/tun-config.yaml",
    "/home/testuser/.config/mihomo/config.yaml",
]:
    print(path, revision(path))
PY
    fail "tun status did not report enabled"
}
if ! as_user mihomo-cli tun off > /tmp/tun-off.out 2>&1; then
    cat /tmp/tun-off.out >&2
    find /var/lib/mihomo-cli/transactions -printf '%y %M %u:%g %p\n' >&2 || true
    fail "tun off failed"
fi
wait_for_json_value 'json.load(sys.stdin)["data"]["tun"]' disabled || fail "TUN did not become disabled"
TUN_STATUS=$(as_user mihomo-cli tun status)
printf '%s\n' "$TUN_STATUS" | grep -q '^TUN enabled: disabled$' || fail "tun status did not report disabled"

as_user mihomo-cli restart --system
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || fail "Core did not restart after TUN off"
wait_for_json_value 'json.load(sys.stdin)["data"]["tun"]' disabled || fail "TUN state changed after restart"
as_user mihomo-cli tun off
wait_for_json_value 'json.load(sys.stdin)["data"]["tun"]' disabled || fail "repeated TUN off after restart did not remain disabled"
TUN_STATUS=$(as_user mihomo-cli tun status)
printf '%s\n' "$TUN_STATUS" | grep -q '^TUN enabled: disabled$' || fail "repeated tun off did not report disabled"

echo "--- runtime validation invokes the fake Core ---"
VALIDATION_LOG="$CONFIG_DIR/validation.log"
rm -f "$VALIDATION_LOG"
as_user env MIHOMO_FAKE_VALIDATION_LOG="$VALIDATION_LOG" mihomo-cli config --validate
[ -s "$VALIDATION_LOG" ] || fail "config --validate did not invoke mihomo -t"
grep -q -- '-t' "$VALIDATION_LOG" || fail "validation log does not contain -t"

echo "--- complete uninstall -> install -> restart -> tun on -> status journey ---"
as_user mihomo-cli uninstall --system --yes
[ ! -e "$SYSTEM_UNIT" ] || fail "uninstall --system left the system unit installed"
[ ! -S /var/run/mihomo/service.sock ] || fail "uninstall --system left daemon IPC socket"
as_user mihomo-cli install --system --yes
[ -e "$SYSTEM_UNIT" ] || fail "install --system did not restore the system unit"
systemctl is-active --quiet mihomo || fail "system daemon is not active after reinstall"
RESET_STATUS=$(as_user mihomo-cli status --system)
printf '%s\n' "$RESET_STATUS"
printf '%s\n' "$RESET_STATUS" | grep -q '^Core:.*running$' || \
    fail "clean install status did not report a running Core"
printf '%s\n' "$RESET_STATUS" | grep -q '^TUN:.*disabled$\|^TUN:.*unknown$' || \
    fail "clean install status did not report TUN disabled or unknown"
RESET_TUN_STATUS=$(as_user mihomo-cli tun status --system)
printf '%s\n' "$RESET_TUN_STATUS"
printf '%s\n' "$RESET_TUN_STATUS" | grep -q '^TUN enabled: disabled$\|^TUN enabled: unknown$' || \
    fail "clean install tun status did not report TUN disabled or unknown"
as_user mihomo-cli tun on --yes >/tmp/complete-tun-on.out
wait_for_json_value 'json.load(sys.stdin)["data"]["tun"]' enabled || fail "TUN did not become enabled in complete journey"
STATUS_TEXT=$(as_user mihomo-cli status --system)
printf '%s\n' "$STATUS_TEXT"
printf '%s\n' "$STATUS_TEXT" | grep -q '^Health:' || fail "default status omitted Health"
printf '%s\n' "$STATUS_TEXT" | grep -q '^Instance:' || fail "default status omitted Instance"
printf '%s\n' "$STATUS_TEXT" | grep -q '^Core:' || fail "default status omitted Core"
printf '%s\n' "$STATUS_TEXT" | grep -q '^API:' || fail "default status omitted API"
printf '%s\n' "$STATUS_TEXT" | grep -q '^TUN:.*enabled$' || fail "default status did not report live TUN enabled"
printf '%s\n' "$STATUS_TEXT" | grep -q '^Rule mode:' || fail "default status omitted Rule mode"
printf '%s\n' "$STATUS_TEXT" | grep -q '^Default route:' || fail "default status omitted Default route"
printf '%s\n' "$STATUS_TEXT" | grep -q '^Configuration:' || fail "default status omitted Configuration"
if printf '%s\n' "$STATUS_TEXT" | grep -qE 'Config:|PID:|socket|Logs:|system-proxy off|proxy off'; then
    fail "default status still contains diagnostic or operation-prompt fields"
fi
STATUS_VERBOSE=$(as_user mihomo-cli status --system --verbose)
printf '%s\n' "$STATUS_VERBOSE" | grep -q '^Listening ports:' || fail "verbose status omitted listening ports"
printf '%s\n' "$STATUS_VERBOSE" | grep -q 'mixed-port=7890' || fail "verbose status did not read live Core port"

echo "--- staged upgrade while system daemon is running ---"
# A legacy install may leave the generation directories root-owned inside the
# trust root. A normal user must still be able to prepare a pending update
# through the CLI's controlled privilege boundary.
mkdir -p "$GEN_ROOT/generations"
chown root:root "$GEN_ROOT/generations"
chmod 0755 "$GEN_ROOT/generations"

cp /usr/local/bin/mihomo-cli /tmp/new-mihomo-cli
chmod 0755 /tmp/new-mihomo-cli
echo "# staged upgrade marker" >> /tmp/new-mihomo-cli

UPGRADE_OUT=$(as_user /tmp/new-mihomo-cli install --system --yes)
printf '%s
' "$UPGRADE_OUT"
printf '%s
' "$UPGRADE_OUT" | grep -q 'Upgrade prepared in pending generation' ||     fail "install --system did not prepare pending generation for running service"
printf '%s
' "$UPGRADE_OUT" | grep -q 'To apply this update, run: mihomo-cli restart' ||     fail "install --system did not tell the normal user how to apply the pending update"

systemctl is-active --quiet mihomo || fail "daemon was interrupted during staged upgrade"

# moved generation state into the root:root 0750 trust root, which the normal
# user cannot read, so pending detection is asserted from the privileged context.
# 刻意以 root 身份读取代际状态：这是受保护的观测面，用于验证 pending 检测本身。
DOCTOR_OUT=$(sudo -n /usr/local/bin/mihomo-cli doctor)
printf '%s
' "$DOCTOR_OUT" | grep -q '待应用更新' || fail "doctor did not detect pending generation"

chown root:mihomo /var/lib/mihomo-cli
chmod 0755 /var/lib/mihomo-cli
chown -R root:root "$GEN_ROOT/generations"
chown root:root "$GEN_ROOT/state.json"
rm -f "$GEN_ROOT/.generation.lock"
# A root rewrite can leave a valid terminal TUN journal inaccessible to the
# daemon. Restart must converge this managed state before daemon StartCore.
mkdir -p /var/lib/mihomo-cli/transactions/active
cat > /var/lib/mihomo-cli/transactions/active/journal.json <<'EOF'
{
  "schema_version": 1,
  "transaction_id": "root-owned-terminal",
  "generation": 1,
  "phase": "IntentCommitted",
  "original_uid": 1001,
  "target_runtime_tun": false,
  "candidate_revision": "test-candidate",
  "old_runtime_digest": "test-runtime"
}
EOF
chown root:root /var/lib/mihomo-cli/transactions/active \
    /var/lib/mihomo-cli/transactions/active/journal.json
chmod 0750 /var/lib/mihomo-cli/transactions/active
chmod 0640 /var/lib/mihomo-cli/transactions/active/journal.json
sudo -u "$TEST_USER" test ! -w /var/lib/mihomo-cli || \
    fail "system state root unexpectedly became writable by the normal user"

as_user /tmp/new-mihomo-cli restart --system
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || fail "Core not running after staged upgrade apply"
[ ! -e /var/lib/mihomo-cli/transactions/active ] || \
    fail "restart did not converge the terminal active TUN transaction"
[[ "$(stat -c '%U:%G %a' /var/lib/mihomo-cli/transactions)" =~ ^mihomo:mihomo\ (2750|750)$ ]] || \
    fail "restart did not preserve the transaction directory ownership, was: $(stat -c '%U:%G %a' /var/lib/mihomo-cli/transactions)"
[ "$(stat -c '%U:%G %a' /var/lib/mihomo-cli)" = "root:mihomo 770" ] || \
    fail "staged upgrade changed system state root ownership or mode"
[ "$(stat -c '%U:%G %a' "$GEN_ROOT/state.json")" = "root:root 644" ] || \
    fail "staged upgrade did not preserve root-owned generation state"

cmp -s /tmp/new-mihomo-cli /usr/local/bin/mihomo-cli || fail "installed CLI binary was not upgraded to pending generation"

echo "--- staged upgrade recovers a migrated legacy TUN transaction ---"
cp /usr/local/bin/mihomo-cli /tmp/legacy-recovery-mihomo-cli
chmod 0755 /tmp/legacy-recovery-mihomo-cli
echo "# legacy recovery staged upgrade marker" >> /tmp/legacy-recovery-mihomo-cli

LEGACY_UPGRADE_OUT=$(as_user /tmp/legacy-recovery-mihomo-cli install --system --yes)
printf '%s\n' "$LEGACY_UPGRADE_OUT"
printf '%s\n' "$LEGACY_UPGRADE_OUT" | grep -q 'Upgrade prepared in pending generation' || \
    fail "install --system did not prepare a pending generation for legacy recovery"

# plant the fixture against a **running** Core — that is the real
# v9 upgrade shape. No pre-stop and no journal deletion: the migrated
# placeholder plus the independently proven instance ownership are the whole
# admission.
CORE_PID_BEFORE_MIGRATED=$(pgrep -x mihomo | head -n1)
[ -n "$CORE_PID_BEFORE_MIGRATED" ] || fail "staged upgrade fixture: no running Core"
rm -rf /var/lib/mihomo-cli/transactions/active
python3 - <<'PY'
import json
from pathlib import Path

state_root = Path("/var/lib/mihomo-cli")
transactions = state_root / "transactions"
intent = Path("/home/testuser/.config/mihomo/config.yaml")
candidate = transactions / "tun-candidate.yaml"
candidate_bytes = intent.read_bytes() + b"# legacy candidate differs from current intent\n"
candidate.write_bytes(candidate_bytes)
# Deliberate fixture correspondence (issue -tail): the legacy journal
# names this file as `snapshot_path`, and the legacy site being modelled is one
# where the snapshot already carries the candidate while the journal stayed
# Prepared. Keeping snapshot == candidate is what makes the migrated site
# recoverable (classification Candidate -> converge); rewriting the snapshot to
# the pre-candidate intent would instead be "no durable old-snapshot evidence"
# and the controlled reset would refuse. Do not "fix" this to old intent bytes.
(state_root / "tun-config.yaml").write_bytes(candidate_bytes)

def revision(data):
    value = 0xcbf29ce484222325
    for byte in data:
        value ^= byte
        value = (value * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"

journal = {
    "transaction_id": "legacy-pending-generation",
    "base_revision": "0000000000000000",
    "candidate_revision": revision(candidate.read_bytes()),
    "state": "Prepared",
    "candidate_path": str(candidate),
    "snapshot_path": str(state_root / "tun-config.yaml"),
}
(transactions / "tun-journal.json").write_text(json.dumps(journal))
PY
chown mihomo:mihomo /var/lib/mihomo-cli/transactions/tun-journal.json \
    /var/lib/mihomo-cli/transactions/tun-candidate.yaml \
    /var/lib/mihomo-cli/tun-config.yaml
chmod 0640 /var/lib/mihomo-cli/transactions/tun-journal.json \
    /var/lib/mihomo-cli/transactions/tun-candidate.yaml \
    /var/lib/mihomo-cli/tun-config.yaml

# The recovery runs before the pending generation is applied (main.rs recovery
# entry precedes apply_pending_generation), so this exercises the same ordering
# a real upgrade has.
MIGRATED_RESET_OUT=$(as_user /tmp/legacy-recovery-mihomo-cli restart --system --yes 2>&1) || {
    printf '%s\n' "$MIGRATED_RESET_OUT" >&2
    fail "restart --yes did not converge the migrated transaction while the Core ran"
}
printf '%s\n' "$MIGRATED_RESET_OUT"
printf '%s\n' "$MIGRATED_RESET_OUT" | grep -q '已保留用户配置并重置' || \
    fail "migrated legacy reset did not report the reset"
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
    fail "Core not running after migrated legacy transaction recovery"
[ ! -e /var/lib/mihomo-cli/transactions/active ] || \
    fail "restart did not finalize the migrated legacy TUN transaction"
[ ! -e /var/lib/mihomo-cli/transactions/tun-journal.json ] || \
    fail "restart did not remove the legacy TUN journal"
CORE_PID_AFTER_MIGRATED=$(pgrep -x mihomo | head -n1)
[ -n "$CORE_PID_AFTER_MIGRATED" ] && \
    [ "$CORE_PID_AFTER_MIGRATED" != "$CORE_PID_BEFORE_MIGRATED" ] || \
    fail "migrated legacy reset did not replace the Core process"
cmp -s /tmp/legacy-recovery-mihomo-cli /usr/local/bin/mihomo-cli || \
    fail "installed CLI binary was not upgraded after migrated legacy recovery"

echo "--- idempotent install converges a migrated legacy TUN transaction ---"
as_user mihomo-cli stop
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' false || \
    fail "fixture could not stop the Core before managed reset"
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
candidate = intent.read_bytes() + b"# migrated legacy candidate blocks install\n"
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
# Deliberate fixture correspondence (issue -tail): this fixture models a
# migrated legacy transaction whose snapshot/old-snapshot already equal the
# candidate and whose journal records that same revision. The controlled reset
# only converges such a site (RecoveryRequired + snapshot == candidate); writing
# the pre-candidate intent here would turn it into an unattestable "other"
# snapshot and the fixture would stop exercising the migration path.
(state_root / "tun-config.yaml").write_bytes(candidate)

journal = {
    "schema_version": 1,
    "transaction_id": "tun-migrated-idempotent-install",
    "generation": 99,
    "phase": "RecoveryRequired",
    "original_uid": 1001,
    "target_runtime_tun": False,
    "candidate_revision": hashlib.sha256(candidate).hexdigest(),
    "old_snapshot_revision": hashlib.sha256(candidate).hexdigest(),
    "old_runtime_digest": hashlib.sha256((active / "old-runtime.json").read_bytes()).hexdigest(),
    "legacy_source": True,
    "legacy_transaction_id": "legacy-idempotent-install",
    "legacy_base_revision": "0000000000000000",
    "rollback_evidence_complete": False,
}
(active / "journal.json").write_text(json.dumps(journal))
PY
chown -R mihomo:mihomo /var/lib/mihomo-cli/transactions
chmod 0750 /var/lib/mihomo-cli/transactions \
    /var/lib/mihomo-cli/transactions/active
chmod 0640 /var/lib/mihomo-cli/transactions/active/* \
    /var/lib/mihomo-cli/tun-config.yaml

# 普通用户重入：授权边界必须自行覆盖迁移事务的恢复与后续重启。
as_user /usr/local/bin/mihomo-cli install --system --yes
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
    fail "idempotent install did not recover and restart the Core"
[ ! -e /var/lib/mihomo-cli/transactions/active ] || \
    fail "idempotent install did not finalize the migrated legacy TUN transaction"

 echo "--- restart --yes resets a managed legacy RecoveryRequired transaction ---"
as_user mihomo-cli stop
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' false || \
    fail "fixture could not stop the Core before managed reset"
CONFIG_DIGEST_BEFORE=$(sha256sum "$CONFIG_FILE" | cut -d' ' -f1)
cp "$CONFIG_FILE" /tmp/managed-reset-config-before.yaml
EXTERNAL_FILE=/var/lib/mihomo-cli/external-runtime-residue
printf 'must survive managed reset\n' > "$EXTERNAL_FILE"
rm -f /var/lib/mihomo-cli/transactions/tun-journal.json \
    /var/lib/mihomo-cli/transactions/tun-candidate.yaml
rm -rf /var/lib/mihomo-cli/transactions/active
mkdir -p /var/lib/mihomo-cli/transactions/active
python3 - <<'PY'
import hashlib
import json
from pathlib import Path

state_root = Path("/var/lib/mihomo-cli")
transactions = state_root / "transactions"
active = transactions / "active"
intent = Path("/home/testuser/.config/mihomo/config.yaml")
candidate = intent.read_bytes() + b"# managed reset candidate\n"
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
(state_root / "tun-config.yaml").write_bytes(b"managed snapshot with an unknown revision\n")
(active / "journal.json").write_text(json.dumps({
    "schema_version": 1,
    "transaction_id": "managed-reset-recovery-required",
    "generation": 101,
    "phase": "RecoveryRequired",
    "original_uid": 1001,
    "target_runtime_tun": False,
    "candidate_revision": hashlib.sha256(candidate).hexdigest(),
    "old_snapshot_revision": hashlib.sha256(candidate).hexdigest(),
    "old_runtime_digest": hashlib.sha256((active / "old-runtime.json").read_bytes()).hexdigest(),
    "legacy_source": True,
    "legacy_transaction_id": "legacy-reset-source",
    "legacy_base_revision": "0000000000000000",
    "rollback_evidence_complete": False,
}))
PY
chown -R mihomo:mihomo /var/lib/mihomo-cli/transactions /var/lib/mihomo-cli/tun-config.yaml
chmod 0750 /var/lib/mihomo-cli/transactions /var/lib/mihomo-cli/transactions/active
chmod 0640 /var/lib/mihomo-cli/transactions/active/* \
    /var/lib/mihomo-cli/tun-config.yaml
if as_user mihomo-cli restart --system >/tmp/managed-reset-no-yes.out 2>&1; then
    cat /tmp/managed-reset-no-yes.out >&2
    fail "non-interactive restart reset managed runtime without --yes"
fi
cmp -s "$CONFIG_FILE" /tmp/managed-reset-config-before.yaml || \
    fail "config changed during rejected managed reset"
printf 'legacy candidate to quarantine\n' > /var/lib/mihomo-cli/transactions/tun-candidate.yaml
chown mihomo:mihomo /var/lib/mihomo-cli/transactions/tun-candidate.yaml
chmod 0640 /var/lib/mihomo-cli/transactions/tun-candidate.yaml
as_user mihomo-cli restart --system --yes
wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
    fail "restart --yes did not restore the Core after managed reset"
CONFIG_DIGEST_AFTER=$(sha256sum "$CONFIG_FILE" | cut -d' ' -f1)
[ "$CONFIG_DIGEST_AFTER" = "$CONFIG_DIGEST_BEFORE" ] || \
    fail "managed reset changed user configuration"
[ ! -e /var/lib/mihomo-cli/transactions/active ] || \
    fail "managed reset left the active transaction"
[ ! -e /var/lib/mihomo-cli/transactions/tun-journal.json ] || \
    fail "managed reset left the legacy journal"
[ ! -e /var/lib/mihomo-cli/transactions/tun-candidate.yaml ] || \
    fail "managed reset left the legacy candidate"
[ ! -e /var/lib/mihomo-cli/tun-config.yaml ] || \
    fail "managed reset did not rebuild the managed runtime snapshot"
[ "$(cat "$EXTERNAL_FILE")" = "must survive managed reset" ] || \
    fail "managed reset touched an external runtime residue"

# hard gate: the recovery entry runs BEFORE the pending generation is
# applied, so a real upgrade always meets the daemon that was already running —
# which cannot know the new admission. This section downgrades only the daemon
# binary to the pre- baseline and proves the whole sequence still completes:
# the privileged CLI performs the stop (identity + exit observation), the reset
# is done, and only then is the daemon generation applied.
if [ -x /tests/legacy/mihomo-cli ]; then
    echo "--- real upgrade combination: pre- daemon + current CLI ---"
    install -m 0755 /tests/mihomo-cli-source /tmp/new-cli-049
    NEW_CLI_SHA=$(sha256sum /tmp/new-cli-049 | cut -d' ' -f1)
    OLD_DAEMON_SHA=$(sha256sum /tests/legacy/mihomo-cli | cut -d' ' -f1)
    [ "$NEW_CLI_SHA" != "$OLD_DAEMON_SHA" ] || fail "legacy daemon artifact is not a distinct build"

    # Downgrade the whole installation through the pre- CLI itself: stage it
    # as a pending generation and apply it. A hand-swapped binary would be a
    # "mixed install" the generation guard (correctly) refuses, so the fixture
    # must be a real old install state.
    LEGACY_DOWNGRADE_OUT=$(as_user /tests/legacy/mihomo-cli install --system --yes 2>&1) || {
        printf '%s\n' "$LEGACY_DOWNGRADE_OUT" >&2
        fail "the pre- CLI could not stage its own install"
    }
    printf '%s\n' "$LEGACY_DOWNGRADE_OUT"
    printf '%s\n' "$LEGACY_DOWNGRADE_OUT" | grep -q 'Upgrade prepared in pending generation' || \
        fail "the pre- CLI did not prepare a pending generation"
    as_user /tests/legacy/mihomo-cli restart --system
    [ "$(sha256sum /usr/local/bin/mihomo-cli | cut -d' ' -f1)" = "$OLD_DAEMON_SHA" ] || \
        fail "the pre- install did not take effect"
    OLD_DAEMON_PID=$(systemctl show mihomo -p MainPID --value)
    [ "$(sha256sum "/proc/$OLD_DAEMON_PID/exe" | cut -d' ' -f1)" = "$OLD_DAEMON_SHA" ] || \
        fail "running daemon is not the pre- binary"
    # The installed CLI is the old one, so this restart never stages an upgrade:
    # it only starts the Core under the pre- daemon.
    as_user mihomo-cli restart --system
    wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
        fail "pre- daemon could not start the Core"
    CORE_PID_BEFORE_OLD_DAEMON=$(pgrep -x mihomo | head -n1)
    [ -n "$CORE_PID_BEFORE_OLD_DAEMON" ] || fail "old-daemon fixture: no running Core"
    CONFIG_SHA_BEFORE_OLD_DAEMON=$(sha256sum "$CONFIG_FILE" | cut -d' ' -f1)
    # The current CLI is newer than the installed daemon, so this is the upgrade
    # direction the admission has to survive.
    [ "$(sha256sum /usr/local/bin/mihomo-cli | cut -d' ' -f1)" != "$NEW_CLI_SHA" ] || \
        fail "daemon downgrade did not take effect"

    rm -rf /var/lib/mihomo-cli/transactions/active
    python3 - <<'PY'
import json
from pathlib import Path

state_root = Path("/var/lib/mihomo-cli")
transactions = state_root / "transactions"
intent = Path("/home/testuser/.config/mihomo/config.yaml")
candidate = transactions / "tun-candidate.yaml"
candidate_bytes = intent.read_bytes() + b"# legacy candidate for the old-daemon upgrade\n"
candidate.write_bytes(candidate_bytes)
(state_root / "tun-config.yaml").write_bytes(candidate_bytes)

def revision(data):
    value = 0xcbf29ce484222325
    for byte in data:
        value ^= byte
        value = (value * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"

journal = {
    "transaction_id": "legacy-old-daemon-upgrade",
    "base_revision": "0000000000000000",
    "candidate_revision": revision(candidate.read_bytes()),
    "state": "Prepared",
    "candidate_path": str(candidate),
    "snapshot_path": str(state_root / "tun-config.yaml"),
}
(transactions / "tun-journal.json").write_text(json.dumps(journal))
PY
    chown mihomo:mihomo /var/lib/mihomo-cli/transactions/tun-journal.json \
        /var/lib/mihomo-cli/transactions/tun-candidate.yaml \
        /var/lib/mihomo-cli/tun-config.yaml
    chmod 0640 /var/lib/mihomo-cli/transactions/tun-journal.json \
        /var/lib/mihomo-cli/transactions/tun-candidate.yaml \
        /var/lib/mihomo-cli/tun-config.yaml

    if ! as_user /tmp/new-cli-049 restart --system --yes > /tmp/old-daemon-upgrade.out 2>&1; then
        cat /tmp/old-daemon-upgrade.out >&2
        echo "--- old daemon upgrade diagnostics ---" >&2
        find /var/lib/mihomo-cli/transactions -printf '%y %M %u:%g %p\n' >&2 || true
        journalctl -u mihomo.service --no-pager -n 100 >&2 || true
        fail "the pre- daemon + current CLI upgrade path did not execute the reset"
    fi
    cat /tmp/old-daemon-upgrade.out
    # The pre- daemon rejected the new request; the privileged CLI had to be
    # the stop executor, and it reports the observed exit before any cleanup.
    grep -q "旧 daemon 不识别本次准入" /tmp/old-daemon-upgrade.out || \
        fail "the compatibility stop executor was not used against the pre- daemon"
    grep -q "pid=$CORE_PID_BEFORE_OLD_DAEMON，退出已确认" /tmp/old-daemon-upgrade.out || \
        fail "the compatibility stop did not report the observed exit of the original Core"
    COMPAT_LINE=$(grep -n '旧 daemon 不识别本次准入' /tmp/old-daemon-upgrade.out | cut -d: -f1)
    RESET_LINE=$(grep -n '已保留用户配置并重置' /tmp/old-daemon-upgrade.out | cut -d: -f1)
    [ -n "$COMPAT_LINE" ] && [ -n "$RESET_LINE" ] && [ "$COMPAT_LINE" -lt "$RESET_LINE" ] || \
        fail "the reset was reported before the stop proof"
    # Recovery must precede the generation apply (main.rs recovery entry runs
    # before apply_pending_generation): the old daemon is still running the old
    # binary while the compat stop happens.
    SKEW_LINE=$(grep -n '检测到当前 CLI 与系统 Daemon 二进制不一致' /tmp/old-daemon-upgrade.out | cut -d: -f1)
    [ -n "$SKEW_LINE" ] && [ "$RESET_LINE" -lt "$SKEW_LINE" ] || \
        fail "the daemon generation was applied before the legacy reset"
    # Dynamic proof: the original process is gone and a different Core serves now.
    for _ in $(seq 1 50); do [ ! -e "/proc/$CORE_PID_BEFORE_OLD_DAEMON" ] && break; sleep 0.1; done
    [ ! -e "/proc/$CORE_PID_BEFORE_OLD_DAEMON" ] || \
        fail "the original Core process $CORE_PID_BEFORE_OLD_DAEMON is still alive"
    CORE_PID_AFTER_OLD_DAEMON=$(pgrep -x mihomo | head -n1)
    [ -n "$CORE_PID_AFTER_OLD_DAEMON" ] && \
        [ "$CORE_PID_AFTER_OLD_DAEMON" != "$CORE_PID_BEFORE_OLD_DAEMON" ] || \
        fail "the upgrade did not start a new Core process"
    wait_for_json_value 'json.load(sys.stdin)["data"]["core"]["running"]' true || \
        fail "Core not running after the old-daemon upgrade"
    grep -q 'API ready at /var/run/mihomo/mihomo.sock' /tmp/old-daemon-upgrade.out || \
        fail "the upgrade did not reach API readiness"
    grep -q 'Successfully upgraded to generation' /tmp/old-daemon-upgrade.out || \
        fail "the upgrade did not report a successful generation apply"
    # Evidence: transaction quarantined, legacy artifacts gone, user config intact.
    [ ! -e /var/lib/mihomo-cli/transactions/active ] || \
        fail "old-daemon upgrade left the active transaction"
    [ ! -e /var/lib/mihomo-cli/transactions/tun-journal.json ] || \
        fail "old-daemon upgrade left the legacy journal"
    [ ! -e /var/lib/mihomo-cli/transactions/tun-candidate.yaml ] || \
        fail "old-daemon upgrade left the legacy candidate"
    find /var/lib/mihomo-cli/transactions/gc -maxdepth 1 -name 'reset-*' | grep -q . || \
        fail "old-daemon upgrade did not quarantine the transaction evidence"
    [ "$(sha256sum "$CONFIG_FILE" | cut -d' ' -f1)" = "$CONFIG_SHA_BEFORE_OLD_DAEMON" ] || \
        fail "old-daemon upgrade changed the user configuration"
    # The daemon generation was applied only after the reset.
    [ "$(sha256sum /usr/local/bin/mihomo-cli | cut -d' ' -f1)" = "$NEW_CLI_SHA" ] || \
        fail "old-daemon upgrade did not apply the pending daemon generation"
    NEW_DAEMON_PID=$(systemctl show mihomo -p MainPID --value)
    [ "$(sha256sum "/proc/$NEW_DAEMON_PID/exe" | cut -d' ' -f1)" = "$NEW_CLI_SHA" ] || \
        fail "the running daemon was not upgraded to the current binary"
else
    echo "SKIP: /tests/legacy/mihomo-cli is missing; the pre- daemon upgrade combination was not exercised"
fi

 echo "--- all-uninstall is non-interactive and removes both modes ---"
as_user mihomo-cli uninstall --all --yes
[ ! -e "$SYSTEM_UNIT" ] || fail "uninstall --all --yes left the system unit installed"
[ ! -S /var/run/mihomo/service.sock ] || fail "uninstall --all --yes left daemon IPC socket"
[ ! -e "$CONFIG_DIR" ] || fail "uninstall --all --yes left the shared config directory"

printf 'PASS: install, paths, lifecycle, shell/system proxy, autostart, TUN, runtime validation, and complete status journey contracts passed\n'
