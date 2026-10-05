#!/usr/bin/env bash
set -euo pipefail

TEST_USER=testuser
TEST_HOME=/home/testuser
CONFIG_DIR="$TEST_HOME/.config/mihomo"
CONFIG_FILE="$CONFIG_DIR/config.yaml"
SYSTEM_UNIT=/etc/systemd/system/mihomo.service
CORE_BINARY=/usr/local/lib/mihomo/mihomo
INSTALLER_CLI=/tests/mihomo-cli-source
UPSTREAM_PORT=18080
MIXED_PORT=17890

fail() {
    echo "FAIL: $*" >&2
    systemctl status mihomo --no-pager >&2 || true
    journalctl -u mihomo --no-pager -n 80 >&2 || true
    exit 1
}

as_user() {
    sudo -u "$TEST_USER" env HOME="$TEST_HOME" USER="$TEST_USER" "$@"
}

wait_for_socket() {
    for _ in $(seq 1 150); do
        [ -S "$1" ] && return 0
        sleep 0.1
    done
    return 1
}

[ "$(id -u)" = 0 ] || fail "test must run as root inside the container"
[ -x "$CORE_BINARY" ] || fail "real Core is missing"
"$CORE_BINARY" -v | grep -q '^Mihomo Meta v' || fail "mounted binary is not a real Mihomo Core"

as_user mihomo-cli uninstall --all --yes >/dev/null 2>&1 || true
rm -f "$SYSTEM_UNIT"
rm -rf "$CONFIG_DIR"
mkdir -p "$CONFIG_DIR" /var/lib/mihomo-cli
cp /tests/real-mihomo "$CORE_BINARY"
chmod 755 "$CORE_BINARY"
chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME/.config"
truncate -s 8000001 "$CONFIG_DIR/geoip.metadb"
truncate -s 2000001 "$CONFIG_DIR/GeoSite.dat"
chown "$TEST_USER:$TEST_USER" "$CONFIG_DIR/geoip.metadb" "$CONFIG_DIR/GeoSite.dat"

python3 - <<PY >/tmp/mihomo-http-upstream.log 2>&1 &
import http.server
import socketserver

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"local-http-fixture-ok\\n"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass

with socketserver.TCPServer(("127.0.0.1", $UPSTREAM_PORT), Handler) as server:
    server.serve_forever()
PY
UPSTREAM_PID=$!
trap 'kill "$UPSTREAM_PID" 2>/dev/null || true' EXIT

cat > /tmp/mihomo-http-config.yaml <<EOF
mixed-port: $MIXED_PORT
allow-lan: false
mode: rule
proxies: []
proxy-groups: []
rules:
  - MATCH,DIRECT
external-controller-unix: /var/run/mihomo/mihomo.sock
EOF
chown "$TEST_USER:$TEST_USER" /tmp/mihomo-http-config.yaml

as_user sudo -n "$INSTALLER_CLI" install --system --yes || fail "system install failed"
as_user mihomo-cli config --import /tmp/mihomo-http-config.yaml --yes || fail "config import failed"
as_user mihomo-cli restart --system || fail "restart failed"
wait_for_socket /var/run/mihomo/mihomo.sock || fail "Core API socket missing"

for _ in $(seq 1 100); do
    if curl --silent --show-error --fail --max-time 2 \
        --proxy "http://127.0.0.1:$MIXED_PORT" \
        "http://127.0.0.1:$UPSTREAM_PORT/" | grep -Fxq 'local-http-fixture-ok'; then
        echo "PASS: real Core HTTP mixed-port data plane reached local upstream"
        exit 0
    fi
    sleep 0.1
done

fail "curl through real Core did not reach local HTTP upstream"
