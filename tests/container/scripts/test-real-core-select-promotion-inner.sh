#!/usr/bin/env bash
# Real-Core 终审：#007 按订阅选择持久化 + #004 TUN active promotion dispatcher。
# 在 privileged systemd 容器内由 root 执行；通过 TEST_IDENTITY（testuser|root）
# 分别以普通用户与 root 两种身份完整跑两遍旅程。
set -euo pipefail

TEST_IDENTITY="${TEST_IDENTITY:?set TEST_IDENTITY to testuser or root}"
SOCKET=/var/run/mihomo/mihomo.sock
# uninstall --all 会移除系统路径上的 CLI/Core 二进制本身，install 也以自身为安装来源；
# 因此全部 CLI 调用统一走 /tests 下的受管副本（与 test-real-core-systemd.sh 同模式）。
CLI=/tests/mihomo-cli-persist
cp /usr/local/bin/mihomo-cli "$CLI"
chmod 755 "$CLI"
WORK=/tmp/real-select-promotion
mkdir -p "$WORK"

if [ "$TEST_IDENTITY" = root ]; then
    TEST_HOME=/root
else
    TEST_HOME=/home/testuser
fi
CONFIG_DIR="$TEST_HOME/.config/mihomo"

as_id() {
    if [ "$TEST_IDENTITY" = root ]; then
        env HOME="$TEST_HOME" USER="$TEST_IDENTITY" "$@"
    else
        sudo -u "$TEST_IDENTITY" env HOME="$TEST_HOME" USER="$TEST_IDENTITY" "$@"
    fi
}

fail() {
    echo "FAIL [$TEST_IDENTITY]: $*" >&2
    journalctl -u mihomo --no-pager -n 60 >&2 || true
    stat -c '%A %a %U:%G %n' "$CONFIG_DIR" "$CONFIG_DIR/selections" \
        /var/lib/mihomo-cli/transactions 2>&1 || true
    exit 1
}

api_get() {
    python3 - "$SOCKET" "$1" <<'PY'
import socket, sys
path, endpoint = sys.argv[1:2][0], sys.argv[2]
request = f"GET {endpoint} HTTP/1.0\r\nHost: localhost\r\n\r\n".encode()
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
    sock.settimeout(5); sock.connect(path); sock.sendall(request)
    data = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        data += chunk
print(data.split(b"\r\n\r\n", 1)[1].decode())
PY
}

proxy_now() { # $1 = raw group name
    python3 - "$SOCKET" "$1" <<'PY'
import json, socket, sys, urllib.parse
path, group = sys.argv[1], sys.argv[2]
endpoint = "/proxies/" + urllib.parse.quote(group)
request = f"GET {endpoint} HTTP/1.0\r\nHost: localhost\r\n\r\n".encode()
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
    sock.settimeout(5); sock.connect(path); sock.sendall(request)
    data = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        data += chunk
body = json.loads(data.split(b"\r\n\r\n", 1)[1])
print(body.get("now", ""))
PY
}

proxy_all() { # $1 = raw group name → space-separated members
    python3 - "$SOCKET" "$1" <<'PY'
import json, socket, sys, urllib.parse
path, group = sys.argv[1], sys.argv[2]
endpoint = "/proxies/" + urllib.parse.quote(group)
request = f"GET {endpoint} HTTP/1.0\r\nHost: localhost\r\n\r\n".encode()
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
    sock.settimeout(5); sock.connect(path); sock.sendall(request)
    data = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        data += chunk
body = json.loads(data.split(b"\r\n\r\n", 1)[1])
print(" ".join(body.get("all", [])))
PY
}

wait_status() { # $1 = python expr on status json, $2 = expected
    local expr="$1" expected="$2"
    for _ in $(seq 1 150); do
        local value
        value=$(as_id "$CLI" --json status --system 2>/dev/null \
            | python3 -c "import json,sys; print($expr)" 2>/dev/null || true)
        [ "$value" = "$expected" ] && return 0
        sleep 0.2
    done
    return 1
}

wait_api() {
    for _ in $(seq 1 150); do
        api_get /configs >/dev/null 2>&1 && return 0
        sleep 0.2
    done
    return 1
}

active_id() { tr -d '[:space:]' < "$CONFIG_DIR/subscriptions/active"; }

selection_of() { # $1 = subscription id, $2 = group → node or empty
    python3 - "$CONFIG_DIR/selections/$1.yaml" "$2" <<'PY'
import sys, yaml
try:
    state = yaml.safe_load(open(sys.argv[1]))
except FileNotFoundError:
    print(""); raise SystemExit(0)
if not state:
    print(""); raise SystemExit(0)
print(state.get("selections", {}).get(sys.argv[2], ""))
PY
}

reset_state() {
    as_id "$CLI" uninstall --all --yes >/dev/null 2>&1 || true
    # uninstall --all 移除 Core 二进制；显式复制真实 Core 到目标路径，避免 install 尝试网络下载
    cp /tests/real-mihomo /usr/local/lib/mihomo/mihomo
    chmod 755 /usr/local/lib/mihomo/mihomo
    rm -rf "$CONFIG_DIR"
    mkdir -p "$CONFIG_DIR"
    truncate -s 8000001 "$CONFIG_DIR/geoip.metadb"
    truncate -s 2000001 "$CONFIG_DIR/GeoSite.dat"
    if [ "$TEST_IDENTITY" = root ]; then
        chown -R root:root "$CONFIG_DIR"
        chown root:root "$TEST_HOME"
        chmod 0750 "$TEST_HOME"
    else
        chown -R "$TEST_IDENTITY:$TEST_IDENTITY" "$TEST_HOME/.config"
        chown "$TEST_IDENTITY:$TEST_IDENTITY" "$TEST_HOME"
        chmod 0750 "$TEST_HOME"
    fi
}

write_fixture() { # $1 = path, $2 = first node, $3 = second node, $4 = extra node or empty
    local extra=""
    if [ -n "$4" ]; then
        extra="
  - name: $4
    type: ss
    server: 127.0.0.1
    port: 8390
    cipher: aes-256-gcm
    password: pw"
    fi
    cat > "$1" <<EOF
mixed-port: 7890
allow-lan: false
mode: rule
proxies:
  - name: $2
    type: ss
    server: 127.0.0.1
    port: 8388
    cipher: aes-256-gcm
    password: pw
  - name: $3
    type: ss
    server: 127.0.0.1
    port: 8389
    cipher: aes-256-gcm
    password: pw$extra
proxy-groups:
  - name: 自动选择
    type: select
    proxies:
      - $2
      - $3$( [ -n "$4" ] && printf '\n      - %s' "$4" )
rules:
  - MATCH,自动选择
EOF
}

[ "$(id -u)" = 0 ] || fail "inner test must run as root"
[ -x /usr/local/lib/mihomo/mihomo ] || fail "real Core is missing"
/usr/local/lib/mihomo/mihomo -v | grep -q '^Mihomo Meta v' || fail "not a real Mihomo Core"
[ -c /dev/net/tun ] || fail "/dev/net/tun is unavailable"

write_fixture "$WORK/sub-a.yaml" AA-01 AA-02 ""
write_fixture "$WORK/sub-b.yaml" BB-01 BB-02 ""
write_fixture "$WORK/sub-a2.yaml" AA-01 AA-02 AA-03
printf 'mixed-port: [broken\n' > "$WORK/broken.yaml"

# ---------------------------------------------------------------- Journey A (#007)
printf '=== [%s] Journey A: per-subscription select persistence and replay ===\n' "$TEST_IDENTITY"
reset_state

as_id "$CLI" install --system --yes >/dev/null 2>"$WORK/install.err" \
    || fail "system install failed"
wait_api || fail "real Core API not ready after install"

as_id "$CLI" config --import "$WORK/sub-a.yaml" --activate --yes \
    >/dev/null 2>"$WORK/import-a.err" || fail "import sub-a failed"
wait_api || fail "real Core API not ready after sub-a import"
A_ID="$(active_id)" || fail "no active subscription after sub-a import"
[ -s "$CONFIG_DIR/subscriptions/$A_ID.yaml" ] || fail "sub-a cache missing"

as_id "$CLI" select --system --group 自动选择 --node AA-02 \
    >/dev/null 2>"$WORK/select.err" || fail "select AA-02 failed"
[ "$(proxy_now 自动选择)" = AA-02 ] || fail "real Core did not apply AA-02"
[ "$(selection_of "$A_ID" 自动选择)" = AA-02 ] \
    || fail "selections/$A_ID.yaml does not record AA-02"
[ ! -e "$CONFIG_DIR/selection-state.yaml" ] \
    || fail "legacy global selection-state.yaml must not be created"

as_id "$CLI" config --import "$WORK/sub-b.yaml" --activate --yes \
    >/dev/null 2>"$WORK/import-b.err" || fail "import/switch to sub-b failed"
wait_api || fail "real Core API not ready after sub-b import"
B_ID="$(active_id)"
[ "$B_ID" != "$A_ID" ] || fail "active subscription did not switch to sub-b"
[ "$(proxy_all 自动选择)" = "BB-01 BB-02" ] \
    || fail "sub-b groups not live after switch"
[ "$(proxy_now 自动选择)" = BB-01 ] \
    || fail "sub-a selection leaked into sub-b (expected default BB-01)"
[ "$(selection_of "$A_ID" 自动选择)" = AA-02 ] \
    || fail "sub-a selection file lost after switch"
[ -z "$(selection_of "$B_ID" 自动选择)" ] \
    || fail "sub-b selection file must start empty"

as_id "$CLI" config --switch "$A_ID" >/dev/null 2>"$WORK/switch-back.err" \
    || fail "switch back to sub-a failed"
wait_api || fail "real Core API not ready after switch back"
replayed=""
for _ in $(seq 1 50); do
    if [ "$(proxy_now 自动选择)" = AA-02 ]; then replayed=1; break; fi
    sleep 0.2
done
[ -n "$replayed" ] || fail "sub-a selection AA-02 was not replayed after switch back"

# --- 架构债务收敛断言：selection 镜像在固定 runtime，daemon replay 不依赖用户树 ---
[ -d /var/lib/mihomo-cli/selections ] || fail "selection mirror dir missing"
[ "$(tr -d '[:space:]' < /var/lib/mihomo-cli/selections/active)" = "$A_ID" ] \
    || fail "mirror active pointer mismatch"
MIRROR_NODE="$(python3 - "/var/lib/mihomo-cli/selections/$A_ID.yaml" 自动选择 <<'PY'
import sys, yaml
print(yaml.safe_load(open(sys.argv[1])).get("selections", {}).get(sys.argv[2], ""))
PY
)"
[ "$MIRROR_NODE" = AA-02 ] || fail "mirror selection mismatch: $MIRROR_NODE"
[ ! -e /var/lib/mihomo-cli/selection-intent-dir ] \
    || fail "selection-intent-dir record must not be recreated"

# 收回 daemon 对用户树 selections 的读取能力（0700，owner only）
chmod 0700 "$CONFIG_DIR/selections"
as_id "$CLI" autostart on --system >/dev/null 2>"$WORK/autostart.err" \
    || fail "autostart on failed"
systemctl restart mihomo
wait_status 'json.load(sys.stdin)["data"]["core"]["running"]' True \
    || fail "core not running after daemon restart"
wait_api || fail "real Core API not ready after daemon restart"
replayed_mirror=""
for _ in $(seq 1 50); do
    if [ "$(proxy_now 自动选择)" = AA-02 ]; then replayed_mirror=1; break; fi
    sleep 0.2
done
[ -n "$replayed_mirror" ] \
    || fail "daemon startup replay did not restore AA-02 from mirror (user tree unreadable by mihomo)"
as_id "$CLI" autostart off --system >/dev/null 2>&1 || true
chmod 2750 "$CONFIG_DIR/selections" 2>/dev/null || true

echo "PASS [$TEST_IDENTITY]: selection runtime mirror — daemon replay independent of user tree"

as_id "$CLI" select --system --unpin --group 自动选择 \
    >/dev/null 2>"$WORK/unpin.err" || fail "unpin failed"
[ -z "$(selection_of "$A_ID" 自动选择)" ] || fail "unpin did not clear sub-a selection"
[ "$(proxy_now 自动选择)" = AA-02 ] || fail "unpin must not change runtime selection"

echo "PASS [$TEST_IDENTITY]: #007 select persistence, cross-subscription isolation, switch replay, unpin"

# ---------------------------------------------------------------- Journey B (#004)
printf '=== [%s] Journey B: TUN-active promotion dispatcher with real Core ===\n' "$TEST_IDENTITY"
reset_state

as_id "$CLI" install --system --yes >/dev/null 2>"$WORK/install-b.err" \
    || fail "system install failed (journey B)"
wait_api || fail "real Core API not ready after install (journey B)"
as_id "$CLI" config --import "$WORK/sub-a.yaml" --activate --yes \
    >/dev/null 2>"$WORK/import-a2.err" || fail "import sub-a failed (journey B)"
wait_api || fail "real Core API not ready after import (journey B)"

as_id "$CLI" tun on --yes >/dev/null 2>"$WORK/tun-on.err" || fail "tun on failed"
wait_status 'json.load(sys.stdin)["data"]["tun"]' enabled \
    || fail "status did not attest TUN enabled"

as_id "$CLI" config --import "$WORK/sub-a2.yaml" --activate --yes \
    >/dev/null 2>"$WORK/import-active.err" || fail "TUN-active config import failed"
wait_status 'json.load(sys.stdin)["data"]["tun"]' enabled \
    || fail "TUN promotion lost TUN enabled state"
A2_ID="$(active_id)"
[ "$A2_ID" != "$A_ID" ] || fail "active subscription did not advance after TUN-active import"
wait_api || fail "real Core API not ready after TUN-active promotion"
[ "$(proxy_all 自动选择)" = "AA-01 AA-02 AA-03" ] \
    || fail "TUN-active promotion did not apply the new effective config"
[ "$(tr -d '[:space:]' < /var/lib/mihomo-cli/selections/active 2>/dev/null)" = "$A2_ID" ] \
    || fail "mirror active pointer not updated after TUN-active import"
[ -z "$(ls -A /var/lib/mihomo-cli/transactions/active 2>/dev/null)" ] \
    || fail "promotion left residue in transactions/active"

if as_id "$CLI" config --import "$WORK/broken.yaml" --activate --yes \
    >/dev/null 2>"$WORK/broken-import.err"; then
    fail "broken config import while TUN active must fail"
fi
wait_status 'json.load(sys.stdin)["data"]["tun"]' enabled \
    || fail "failed import disturbed the TUN state"
[ "$(proxy_all 自动选择)" = "AA-01 AA-02 AA-03" ] \
    || fail "failed import destroyed last-known-good runtime config"
[ "$(active_id)" = "$A2_ID" ] || fail "failed import moved the active pointer"

as_id "$CLI" tun off >/dev/null 2>"$WORK/tun-off.err" || fail "tun off failed"
wait_status 'json.load(sys.stdin)["data"]["tun"]' disabled \
    || fail "status did not attest TUN disabled"

as_id "$CLI" uninstall --all --yes >/dev/null 2>"$WORK/uninstall.err" \
    || fail "uninstall after journey B failed"

echo "PASS [$TEST_IDENTITY]: #004 TUN-active promotion, last-known-good on failure, tun off"
