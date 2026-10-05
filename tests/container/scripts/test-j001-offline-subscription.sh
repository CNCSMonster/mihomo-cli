#!/bin/bash
set -euo pipefail

TEST_DIR="$(mktemp -d)"
CONFIG_DIR="$TEST_DIR/config"
FETCHED_FILE="$TEST_DIR/fetched.yaml"
ACCESS_LOG="$TEST_DIR/access.log"
MOCK_PID=""

cleanup() {
    if [ -n "$MOCK_PID" ]; then
        kill "$MOCK_PID" 2>/dev/null || true
        wait "$MOCK_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_file_contains() {
    local path="$1"
    local pattern="$2"
    local description="$3"
    grep -q -- "$pattern" "$path" || fail "$description"
}

mkdir -p "$CONFIG_DIR"
touch "$ACCESS_LOG"
export MIHOMO_CLI_CONFIG_DIR="$CONFIG_DIR"

python3 - "$TEST_DIR" "$ACCESS_LOG" <<'PY' &
import http.server
import os
import socketserver
import sys

root, access_log = sys.argv[1:]
subscription = """mixed-port: 7890
allow-lan: false
mode: rule
proxies:
  - name: Mock-SS-1
    type: ss
    server: 127.0.0.1
    port: 8388
    cipher: aes-256-gcm
    password: testpassword
proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - Mock-SS-1
rules:
  - MATCH,DIRECT
"""

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        with open(access_log, "a", encoding="utf-8") as handle:
            handle.write(f"{self.command} {self.path}\n")
        self.send_response(200)
        self.send_header("Content-Type", "text/yaml")
        self.end_headers()
        self.wfile.write(subscription.encode())

    def log_message(self, *args):
        pass

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as server:
    with open(os.path.join(root, "port"), "w", encoding="utf-8") as handle:
        handle.write(str(server.server_address[1]))
    server.serve_forever()
PY
MOCK_PID=$!

for _ in $(seq 1 50); do
    [ -s "$TEST_DIR/port" ] && break
    sleep 0.1
done
[ -s "$TEST_DIR/port" ] || fail "mock subscription server did not start"
kill -0 "$MOCK_PID" 2>/dev/null || fail "mock subscription server exited early"
MOCK_PORT="$(cat "$TEST_DIR/port")"

printf '=== J001: isolated offline subscription journey ===\n'
mihomo-cli config fetch "http://127.0.0.1:${MOCK_PORT}/subscription" -o "$FETCHED_FILE"
[ -s "$FETCHED_FILE" ] || fail "config fetch did not create an output file"
assert_file_contains "$FETCHED_FILE" "Mock-SS-1" "fetched subscription lost its proxy"
if ! grep -Eq '^GET /subscription(\?.*)?$' "$ACCESS_LOG"; then
    printf 'mock access log:\n%s\n' "$(cat "$ACCESS_LOG")" >&2
    fail "config fetch did not reach the local mock subscription endpoint"
fi

REQUESTS_BEFORE_IMPORT="$(wc -l < "$ACCESS_LOG")"
mihomo-cli config --import "$FETCHED_FILE" --yes
[ "$(wc -l < "$ACCESS_LOG")" = "$REQUESTS_BEFORE_IMPORT" ] || fail "offline import unexpectedly fetched a URL"

[ -s "$CONFIG_DIR/config.yaml" ] || fail "import did not generate config.yaml"
[ -s "$CONFIG_DIR/subscriptions.yaml" ] || fail "import did not write subscription metadata"
[ -s "$CONFIG_DIR/subscriptions/active" ] || fail "import did not select an active subscription"
ACTIVE_ID="$(tr -d '[:space:]' < "$CONFIG_DIR/subscriptions/active")"
[ -n "$ACTIVE_ID" ] || fail "active subscription identifier is empty"
[ -s "$CONFIG_DIR/subscriptions/${ACTIVE_ID}.yaml" ] || fail "active subscription cache is missing"
assert_file_contains "$CONFIG_DIR/subscriptions/${ACTIVE_ID}.yaml" "Mock-SS-1" "subscription cache lost imported data"
assert_file_contains "$CONFIG_DIR/subscriptions.yaml" "file://" "offline import metadata is not file-backed"
assert_file_contains "$CONFIG_DIR/config.yaml" "Mock-SS-1" "generated config lost imported proxy"

VALIDATE_OUTPUT="$(mihomo-cli config --validate 2>&1)" || {
    printf '%s\n' "$VALIDATE_OUTPUT" >&2
    fail "config validation failed"
}
printf '%s\n' "$VALIDATE_OUTPUT" | grep -q 'mihomo -t passed' || fail "validation did not run the fake Core"

printf 'PASS: J001 isolated fetch, offline import, and runtime validation journey\n'
