#!/usr/bin/env bash
set -euo pipefail

TEST_USER=testuser
TEST_UID="$(id -u "$TEST_USER")"
TEST_HOME="/home/$TEST_USER"
CONFIG_DIR="$TEST_HOME/.config/mihomo"
RUNTIME_DIR="/run/user/$TEST_UID/mihomo"
UNIT_FILE="$CONFIG_DIR/../systemd/user/mihomo.service"
CORE_BINARY="$TEST_HOME/.local/bin/mihomo"
SERVER_DIR="/tmp/mihomo-user-mirror"
SERVER_PID=""
LAST_STATUS=""

as_user() {
    sudo -u "$TEST_USER" env \
        HOME="$TEST_HOME" USER="$TEST_USER" \
        XDG_RUNTIME_DIR="/run/user/$TEST_UID" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$TEST_UID/bus" \
        "$@"
}

fail() {
    echo "FAIL: $*" >&2
    if [ -n "$LAST_STATUS" ]; then
        printf '%s\n' '--- Last status response ---' >&2
        printf '%s\n' "$LAST_STATUS" >&2
    fi
    as_user systemctl --user is-system-running >&2 2>/dev/null || true
    as_user systemctl --user status mihomo --no-pager >&2 2>/dev/null || true
    as_user journalctl --user -u mihomo --no-pager -n 50 >&2 2>/dev/null || true
    if [ -f "$TEST_HOME/.local/state/mihomo/mihomo.log" ]; then
        printf '%s\n' '--- Core log ---' >&2
        cat "$TEST_HOME/.local/state/mihomo/mihomo.log" >&2 || true
    fi
    exit 1
}

cleanup() {
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

wait_for_user_manager() {
    for _ in $(seq 1 100); do
        if as_user systemctl --user is-system-running >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    fail "user systemd manager did not become ready"
}

wait_for_status_ready() {
    LAST_STATUS=""
    for _ in $(seq 1 100); do
        LAST_STATUS="$(as_user /usr/local/bin/mihomo-cli --json status 2>/dev/null || true)"
        if printf '%s\n' "$LAST_STATUS" | \
            python3 -c 'import json,sys; data=json.load(sys.stdin)["data"]; raise SystemExit(0 if data["core"]["running"] is True and data["api"] == "reachable" else 1)' 2>/dev/null; then
            return 0
        fi
        if ! as_user systemctl --user is-active --quiet mihomo 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    return 1
}

[ "$(id -u)" = 0 ] || fail "user-mode container setup must run as root"

rm -rf "$CONFIG_DIR" "$TEST_HOME/.local" "$TEST_HOME/.cache" "$SERVER_DIR"
mkdir -p "$CONFIG_DIR" "$TEST_HOME/.config/systemd/user" "$TEST_HOME/.local/state/mihomo" \
    "$TEST_HOME/.local/bin" "/run/user/$TEST_UID" "$SERVER_DIR"
ln -sf /usr/local/bin/mihomo-cli "$TEST_HOME/.local/bin/mihomo-cli"
chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME" "/run/user/$TEST_UID"
chmod 700 "/run/user/$TEST_UID"

# Let the system manager own the per-user systemd instance, as on a real login host.
mkdir -p "/run/user/$TEST_UID"
chown "$TEST_USER:$TEST_USER" "/run/user/$TEST_UID"
chmod 700 "/run/user/$TEST_UID"
systemctl start "user@${TEST_UID}.service"
wait_for_user_manager

# Build a local release asset from the test Core. The archive is large enough to
# exercise the installer's binary-size gate, while the executable remains fake.
cp /tests/fake-mihomo "$SERVER_DIR/mihomo"
truncate -s 5000001 "$SERVER_DIR/mihomo"
chmod 755 "$SERVER_DIR/mihomo"
gzip -c "$SERVER_DIR/mihomo" > "$SERVER_DIR/mihomo-linux-amd64-v1.19.27.gz"
truncate -s 8000001 "$SERVER_DIR/geoip.metadb"
truncate -s 2000001 "$SERVER_DIR/GeoSite.dat"

python3 - "$SERVER_DIR" > /tmp/mihomo-user-mirror-port <<'PY' &
import http.server
import pathlib
import sys

root = pathlib.Path(sys.argv[1])

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path
        if "geoip.metadb" in path:
            name = "geoip.metadb"
        elif "GeoSite.dat" in path:
            name = "GeoSite.dat"
        else:
            name = "mihomo-linux-amd64-v1.19.27.gz"
        payload = (root / name).read_bytes()
        self.send_response(200)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_args):
        pass

server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open("/tmp/mihomo-user-mirror-port", "w", encoding="ascii") as handle:
    handle.write(str(server.server_address[1]))
server.serve_forever()
PY
SERVER_PID=$!
for _ in $(seq 1 100); do
    [ -s /tmp/mihomo-user-mirror-port ] && break
    sleep 0.1
done
[ -s /tmp/mihomo-user-mirror-port ] || fail "local Core mirror did not start"
MIRROR="http://127.0.0.1:$(cat /tmp/mihomo-user-mirror-port)"

printf '%s\n' '=== user-mode install downloads Core and bootstraps direct-only config ==='
as_user /usr/local/bin/mihomo-cli install --user --skip-config --yes --github-mirror "$MIRROR"
[ -x "$CORE_BINARY" ] || fail "user-mode install did not install Core at $CORE_BINARY"
[ -f "$UNIT_FILE" ] || fail "user-mode install did not write the systemd user unit"
[ -f "$CONFIG_DIR/config.yaml" ] || fail "user-mode install did not generate direct-only config.yaml"
grep -q '^mode: rule$' "$CONFIG_DIR/config.yaml" || fail "user-mode install did not set rule mode"
grep -q '^proxies: \[\]$' "$CONFIG_DIR/config.yaml" || fail "user-mode install did not set empty proxies"
grep -q '^proxy-groups: \[\]$' "$CONFIG_DIR/config.yaml" || fail "user-mode install did not set empty proxy groups"
grep -q 'MATCH,DIRECT' "$CONFIG_DIR/config.yaml" || fail "user-mode install did not set DIRECT fallback"

printf '%s\n' '=== user-mode config and lifecycle ==='
wait_for_status_ready || fail "user-mode install did not converge to running/reachable while service was active"
STATUS="$LAST_STATUS"
printf '%s\n' "$STATUS"
printf '%s\n' "$STATUS" | python3 -c 'import json,sys; data=json.load(sys.stdin)["data"]; raise SystemExit(0 if data["core"]["running"] is True and data["api"] == "reachable" and data["mode"] == "per-user" else 1)' || fail "user-mode status response failed final readiness validation"

as_user /usr/local/bin/mihomo-cli stop
as_user systemctl --user is-active --quiet mihomo && fail "user-mode stop left the Core service active"
as_user /usr/local/bin/mihomo-cli restart
as_user systemctl --user is-active --quiet mihomo || fail "user-mode restart did not reactivate the Core service"

printf '%s\n' '=== user-mode uninstall cleans service and user artifacts ==='
as_user /usr/local/bin/mihomo-cli uninstall --user --all --yes
[ ! -e "$UNIT_FILE" ] || fail "user-mode uninstall left the systemd user unit"
[ ! -e "$CORE_BINARY" ] || fail "user-mode uninstall left the Core binary"
[ ! -e "$CONFIG_DIR" ] || fail "user-mode uninstall left the config directory"

printf '%s\n' 'PASS: user-mode install, Core readiness, status, stop/restart, and uninstall journey'
