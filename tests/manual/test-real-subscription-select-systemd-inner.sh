#!/usr/bin/env bash
set -euo pipefail

TEST_USER=testuser
CONFIG_DIR=/home/testuser/.config/mihomo
CLI=/usr/local/bin/mihomo-cli
SOCKET=/var/run/mihomo/mihomo.sock
WORK=/tmp/real-subscription-select
mkdir -p "$WORK"
chown -R "$TEST_USER:$TEST_USER" "$WORK"

fail() {
  echo "FAIL: $*" >&2
  stat -c '%A %a %U:%G %n' /home/testuser /home/testuser/.config "$CONFIG_DIR" "$CONFIG_DIR/.selection-state.lock" 2>&1 || true
  journalctl -u mihomo --no-pager -n 80 >&2 || true
  exit 1
}
as_user() { sudo -u "$TEST_USER" env HOME=/home/testuser USER=$TEST_USER CLASH_CONFIG_URL="$CLASH_CONFIG_URL" "$@"; }
api_get() {
  python3 - "$@" <<'PY'
import socket, sys
path, endpoint = sys.argv[1:]
request = f"GET {endpoint} HTTP/1.0\r\nHost: localhost\r\n\r\n".encode()
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
    sock.settimeout(5); sock.connect(path); sock.sendall(request)
    data = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk: break
        data += chunk
print(data.split(b"\r\n\r\n", 1)[1].decode())
PY
}
wait_api() {
  for _ in $(seq 1 150); do
    api_get "$SOCKET" /configs >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}

[ "$(id -u)" = 0 ] || fail "test must run as root"
[ -n "${CLASH_CONFIG_URL:-}" ] || fail "CLASH_CONFIG_URL is missing"
[ -x /usr/local/lib/mihomo/mihomo ] || fail "real Core is missing"
mkdir -p "$CONFIG_DIR"
chown -R "$TEST_USER:$TEST_USER" /home/testuser/.config

as_user "$CLI" install --system --yes >/dev/null 2>"$WORK/install.err" || fail "system install failed"
wait_api || fail "real Core API did not become ready after install"

as_user "$CLI" config fetch "$CLASH_CONFIG_URL" --output "$WORK/subscription.yaml" >/dev/null 2>"$WORK/fetch.err" || {
  python3 - "$WORK/fetch.err" <<'PY'
import os, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
url = os.environ.get("CLASH_CONFIG_URL", "")
print(text.replace(url, "<redacted-subscription-url>"), end="", file=sys.stderr)
PY
  fail "subscription fetch failed"
}
as_user "$CLI" config --import "$WORK/subscription.yaml" --activate --yes >/dev/null 2>"$WORK/import.err" || fail "subscription import failed"
wait_api || fail "real Core API did not become ready after subscription import"

api_get "$SOCKET" /proxies >"$WORK/proxies.json" || fail "cannot read real Core proxy groups"
python3 - "$WORK/proxies.json" "$WORK/selection.env" <<'PY'
import json, sys
proxies = json.load(open(sys.argv[1]))["proxies"]
for name, value in proxies.items():
    if value.get("type", "").lower() == "selector" and len(value.get("all", [])) >= 2:
        with open(sys.argv[2], "w") as f:
            f.write(f"GROUP={name}\nFIRST={value['all'][0]}\nSECOND={value['all'][1]}\n")
        break
else:
    raise SystemExit("FAIL: subscription has no selector group with two members")
PY
. "$WORK/selection.env"

as_user "$CLI" select --system --group "$GROUP" --node "$SECOND" >/dev/null 2>"$WORK/select.err" || {
  python3 - "$WORK/select.err" <<'PY'
import os, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
print(text.replace(os.environ.get("CLASH_CONFIG_URL", ""), "<redacted-subscription-url>"), end="", file=sys.stderr)
PY
  fail "real subscription node selection failed"
}
[ -s "$CONFIG_DIR/selection-state.yaml" ] || fail "selection-state.yaml was not created"
python3 - "$CONFIG_DIR/selection-state.yaml" "$GROUP" "$SECOND" <<'PY'
import sys, yaml
state = yaml.safe_load(open(sys.argv[1]))
if state.get("selections", {}).get(sys.argv[2]) != sys.argv[3]:
    raise SystemExit("FAIL: persisted selection does not match selected node")
PY

current=$(api_get "$SOCKET" "/proxies/$GROUP" | python3 -c "import json,sys; print(json.load(sys.stdin)['now'])")
[ "$current" = "$SECOND" ] || fail "real Core did not apply selected node"

as_user "$CLI" restart --system >/dev/null 2>"$WORK/restart.err" || fail "system restart failed"
wait_api || fail "real Core API did not become ready after restart"
as_user "$CLI" select --replay --system >/dev/null 2>"$WORK/replay.err" || fail "selection replay command failed"
current=$(api_get "$SOCKET" "/proxies/$GROUP" | python3 -c "import json,sys; print(json.load(sys.stdin)['now'])")
[ "$current" = "$SECOND" ] || fail "persisted selection was not replayed by real Core"

as_user "$CLI" select --unpin --system --group "$GROUP" >/dev/null 2>"$WORK/unpin.err" || fail "unpin failed"
python3 - "$CONFIG_DIR/selection-state.yaml" "$GROUP" <<'PY'
import sys, yaml
state = yaml.safe_load(open(sys.argv[1]))
if sys.argv[2] in state.get("selections", {}):
    raise SystemExit("FAIL: --unpin did not remove persisted selection")
PY

echo "PASS: real subscription system select, persistence, restart replay, and unpin"
