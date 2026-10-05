#!/usr/bin/env bash
set -euo pipefail

TEST_USER=testuser
TEST_HOME=/home/testuser
CONFIG_DIR="$TEST_HOME/.config/mihomo"
SYSTEM_UNIT=/etc/systemd/system/mihomo.service
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

cp /usr/local/bin/mihomo-cli /tmp/real-core-pending-generation-mihomo-cli
chmod 0755 /tmp/real-core-pending-generation-mihomo-cli
printf '\n# real Core pending generation marker\n' >> /tmp/real-core-pending-generation-mihomo-cli
UPGRADE_OUT=$(as_user sudo -n /tmp/real-core-pending-generation-mihomo-cli install --system --yes)
printf '%s\n' "$UPGRADE_OUT"
printf '%s\n' "$UPGRADE_OUT" | grep -q 'Upgrade prepared in pending generation' || \
    fail "install did not stage the real-Core pending generation"
systemctl is-active --quiet mihomo || fail "staged install interrupted the daemon"
python3 - <<'PY' || exit 1
import json
from pathlib import Path

state = json.loads(Path("/var/lib/mihomo-cli/state.json").read_text())
if not state.get("pending"):
    raise SystemExit("FAIL: staged install did not persist a pending generation")
PY

as_user mihomo-cli restart --system || fail "restart did not apply the real-Core pending generation"
wait_for 'json.load(sys.stdin)["data"]["core"]["running"]' True || \
    fail "real Core did not become running after pending generation recovery"
assert_core_launch_paths
[ -S /var/run/mihomo/mihomo.sock ] || \
    fail "real Core API socket missing after pending generation recovery"
[ ! -e /var/lib/mihomo-cli/transactions/active ] || \
    fail "pending generation restart did not finalize the migrated legacy transaction"
[ ! -e /var/lib/mihomo-cli/transactions/tun-journal.json ] || \
    fail "pending generation restart left the legacy TUN journal"
[ ! -e /var/lib/mihomo-cli/transactions/tun-candidate.yaml ] || \
    fail "pending generation restart left the legacy TUN candidate"
cmp -s /tmp/real-core-pending-generation-mihomo-cli /usr/local/bin/mihomo-cli || \
    fail "pending generation did not install the marked CLI binary"
python3 - <<'PY' || exit 1
import json
from pathlib import Path

state = json.loads(Path("/var/lib/mihomo-cli/state.json").read_text())
if state.get("pending"):
    raise SystemExit("FAIL: pending generation remained after restart")
PY

as_user mihomo-cli tun on --yes || fail "tun on with real Core failed"
wait_for 'json.load(sys.stdin)["data"]["tun"]' enabled || fail "status did not attest live TUN enabled"
device=$(wait_for_tun_device) || fail "real Core did not expose a live TUN device"
ip link show "$device" >/dev/null || fail "TUN device disappeared immediately"

as_user mihomo-cli tun off || fail "tun off with real Core failed"
wait_for 'json.load(sys.stdin)["data"]["tun"]' disabled || fail "status did not attest live TUN disabled"
for _ in $(seq 1 100); do
    if ! ip link show "$device" >/dev/null 2>&1; then
        echo "PASS: real Core install, restart, TUN on/off, runtime attestation, and interface cleanup"
        exit 0
    fi
    sleep 0.1
done
fail "real Core left TUN device $device after tun off"
