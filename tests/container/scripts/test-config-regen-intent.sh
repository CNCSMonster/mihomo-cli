#!/bin/bash
# Bug #14/#15/#17 container regression:
#   - rules targeting override-only groups survive config regeneration (Bug #14)
#   - committed TUN intent survives regeneration and reaches a byte-identical fixed point (Bug #15)
#   - `config validate` subcommand works (Bug #17)
# Verified for both root and an unprivileged user identity.
set -euo pipefail

echo "=== config regeneration intent regression (Bug #14/#15/#17) ==="

MOCK_PORT=8091
MOCK_PID=""
TEST_ROOT=$(mktemp -d)
chmod 777 "$TEST_ROOT"

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
  - name: HK-01
    type: ss
    server: 127.0.0.1
    port: 8388
    cipher: aes-256-gcm
    password: test
proxy-groups:
  - name: NodeSelect
    type: select
    proxies:
      - HK-01
rules:
  - MATCH,NodeSelect
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

fail() {
    echo "FAIL: [$1] $2" >&2
    exit 1
}

run_case() {
    local label="$1"; shift
    local home="$1"; shift
    local config_dir="$1"; shift

    rm -rf "$config_dir"
    mkdir -p "$config_dir"
    if [ "$label" = "testuser" ]; then
        chown -R testuser:testuser "$config_dir"
    fi

    cli() {
        if [ "$label" = "testuser" ]; then
            sudo -u testuser env HOME="$home" MIHOMO_CLI_CONFIG_DIR="$config_dir" mihomo-cli "$@"
        else
            env HOME="$home" MIHOMO_CLI_CONFIG_DIR="$config_dir" mihomo-cli "$@"
        fi
    }

    local output
    output=$(cli config --add "http://127.0.0.1:$MOCK_PORT/subscription" --yes 2>&1) || {
        echo "$output"; fail "$label" "config --add failed"
    }

    # override.yaml defines a group that exists nowhere but the override
    cat > "$config_dir/override.yaml" <<'EOF'
proxy-groups:
  - name: StreamHK
    type: select
    proxies:
      - HK-01
EOF
    if [ "$label" = "testuser" ]; then
        chown testuser:testuser "$config_dir/override.yaml"
    fi

    # Refresh merges override.yaml into the effective config, making StreamHK
    # a valid rule target.
    output=$(cli config --refresh 2>&1) || {
        echo "$output"; fail "$label" "first config --refresh failed"
    }
    grep -Fq 'name: StreamHK' "$config_dir/config.yaml" || {
        echo "--- config.yaml ---"; cat "$config_dir/config.yaml"
        fail "$label" "override-only group missing from merged config"
    }

    output=$(cli rule add DOMAIN-SUFFIX,stream.test,StreamHK 2>&1) || {
        echo "$output"; fail "$label" "rule add with override-only group failed"
    }

    # The regeneration under test: the dangling-rule fallback must see the
    # override-merged view, otherwise StreamHK looks dangling again.
    output=$(cli config --refresh 2>&1) || {
        echo "$output"; fail "$label" "config --refresh failed"
    }
    if printf '%s\n' "$output" | grep -F 'StreamHK' | grep -Fq 'missing in the effective config'; then
        echo "$output"; fail "$label" "override-only group was reported as dangling (Bug #14 regression)"
    fi
    grep -Fq 'DOMAIN-SUFFIX,stream.test,StreamHK' "$config_dir/config.yaml" || {
        echo "--- config.yaml ---"; cat "$config_dir/config.yaml"
        fail "$label" "rule targeting override-only group was downgraded (Bug #14)"
    }
    grep -Fq 'DOMAIN-SUFFIX,stream.test,StreamHK' "$config_dir/rules.yaml" || {
        echo "--- rules.yaml ---"; cat "$config_dir/rules.yaml"
        fail "$label" "rules.yaml was rewritten by the dangling-rule fallback (Bug #14)"
    }
    echo "  [$label] Bug #14: override-only group rule survived regeneration"

    # Simulate the intent commit performed by `tun on`: tun block appended last.
    cat >> "$config_dir/config.yaml" <<'EOF'
tun:
  enable: true
  stack: system
  dns-hijack:
    - any:53
  route-exclude-address: []
EOF

    output=$(cli config --refresh 2>&1) || {
        echo "$output"; fail "$label" "refresh after tun intent injection failed"
    }
    grep -Fq 'tun:' "$config_dir/config.yaml" || {
        echo "--- config.yaml ---"; cat "$config_dir/config.yaml"
        fail "$label" "tun block was dropped by regeneration (Bug #15)"
    }
    last_top_key=$(awk '/^[^ \t#]/ { sub(/:$/, "", $0); k=$0 } END { print k }' "$config_dir/config.yaml")
    [ "$last_top_key" = "tun" ] || {
        echo "--- config.yaml ---"; cat "$config_dir/config.yaml"
        fail "$label" "carried-over tun is not the last top-level key: got '$last_top_key'"
    }
    awk '/^tun:/{intun=1;next} /^[^ \t]/{intun=0} intun && /^  enable: true$/{found=1} END{exit !found}' \
        "$config_dir/config.yaml" || {
        fail "$label" "carried-over tun.enable is not true (Bug #15)"
    }

    # Fixed point: a second regeneration must reproduce the file byte-for-byte,
    # which is what restores launched == active == intent attestation.
    sha_before=$(sha256sum "$config_dir/config.yaml" | cut -d' ' -f1)
    output=$(cli config --refresh 2>&1) || {
        echo "$output"; fail "$label" "second refresh after tun intent failed"
    }
    sha_after=$(sha256sum "$config_dir/config.yaml" | cut -d' ' -f1)
    if [ "$sha_before" != "$sha_after" ]; then
        fail "$label" "config is not a regeneration fixed point with tun (Bug #15): $sha_before != $sha_after"
    fi
    echo "  [$label] Bug #15: tun intent survives regeneration and is a byte-identical fixed point"

    # Bug #17: validate subcommand must parse and run.
    output=$(cli config validate 2>&1) || {
        echo "$output"; fail "$label" "config validate subcommand failed"
    }
    if printf '%s\n' "$output" | grep -Fqi 'unrecognized subcommand'; then
        echo "$output"; fail "$label" "config validate subcommand not recognized (Bug #17)"
    fi
    echo "  [$label] Bug #17: config validate subcommand accepted"

    if [ "$label" = "testuser" ]; then
        stat_owner=$(stat -c '%U' "$config_dir/config.yaml")
        [ "$stat_owner" = "testuser" ] || fail "$label" "config.yaml owned by $stat_owner"
    fi
}

run_case "root" "/root" "$TEST_ROOT/root-config"
run_case "testuser" "/home/testuser" "$TEST_ROOT/user-config"

echo "PASS: regeneration preserves override-group rules and tun intent for root and testuser (Bug #14/#15), config validate works (Bug #17)"
