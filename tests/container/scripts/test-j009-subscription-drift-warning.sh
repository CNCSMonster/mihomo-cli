#!/bin/bash
# J009 subscription refresh drift warning contract.
set -euo pipefail

echo "=== J009: subscription refresh drift warning contract ==="

MOCK_PORT=8089
MOCK_PID=""
TEST_ROOT=$(mktemp -d)
export MIHOMO_CLI_CONFIG_DIR="$TEST_ROOT/config"

cleanup() {
    if [ -n "$MOCK_PID" ]; then
        kill "$MOCK_PID" 2>/dev/null || true
        wait "$MOCK_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

python3 -c "
import http.server
import socketserver

SUBSCRIPTION = '''mixed-port: 7890
proxies:
  - name: US-02
    type: ss
    server: 127.0.0.1
    port: 8388
    cipher: aes-256-gcm
    password: test-password
proxy-groups:
  - name: OpenAI
    type: select
    proxies:
      - US-02
rules:
  - MATCH,OpenAI
'''

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Type', 'text/yaml')
        self.end_headers()
        self.wfile.write(SUBSCRIPTION.encode())
    def log_message(self, *args):
        pass

with socketserver.TCPServer(('', $MOCK_PORT), Handler) as httpd:
    httpd.serve_forever()
" &
MOCK_PID=$!

for _ in 1 2 3 4 5; do
    if curl --silent --fail "http://127.0.0.1:$MOCK_PORT/subscription" >/dev/null; then
        break
    fi
    sleep 1
done
curl --silent --fail "http://127.0.0.1:$MOCK_PORT/subscription" >/dev/null || {
    echo "FAIL: mock subscription server did not become ready"
    exit 1
}

ADD_OUTPUT=$(mihomo-cli config --add "http://127.0.0.1:$MOCK_PORT/subscription" --yes 2>&1) || {
    echo "$ADD_OUTPUT"
    echo "FAIL: could not add the local subscription"
    exit 1
}

mkdir -p "$MIHOMO_CLI_CONFIG_DIR"
cat > "$MIHOMO_CLI_CONFIG_DIR/selection-state.yaml" <<'EOF'
selections:
  OpenAI: US-01
EOF

REFRESH_OUTPUT=$(mihomo-cli config --refresh 2>&1) || {
    echo "$REFRESH_OUTPUT"
    echo "FAIL: refreshing the local subscription failed"
    exit 1
}
echo "$REFRESH_OUTPUT"

echo "$REFRESH_OUTPUT" | grep -Fq 'Warning: selected node `US-01` is not available in group `OpenAI`.' || {
    echo "FAIL: exact selected-node drift warning was not emitted"
    exit 1
}
echo "$REFRESH_OUTPUT" | grep -Fq 'Check current policies: mihomo-cli rule policies' || {
    echo "FAIL: drift output omitted the policy remediation command"
    exit 1
}
echo "$REFRESH_OUTPUT" | grep -Fq 'Check current groups/nodes: mihomo-cli list' || {
    echo "FAIL: drift output omitted the group/node remediation command"
    exit 1
}

grep -Fq 'US-02' "$MIHOMO_CLI_CONFIG_DIR/config.yaml" || {
    echo "FAIL: refreshed config does not contain the current subscription node"
    exit 1
}

echo "PASS: refresh emitted the exact stale-selection warning and remediation hints"
