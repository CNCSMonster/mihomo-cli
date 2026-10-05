#!/bin/bash
set -euo pipefail

fail() {
    echo "FAIL: $*" >&2
    systemctl status mihomo --no-pager || true
    cat /var/lib/mihomo-cli/fake-core-argv 2>/dev/null || true
    exit 1
}

wait_for_path() {
    local path="$1"
    for _ in $(seq 1 100); do
        [ -e "$path" ] && return 0
        sleep 0.1
    done
    return 1
}

cat > /home/testuser/.config/mihomo/config.yaml <<'EOF'
proxies: []
proxy-groups: []
rules:
  - MATCH,DIRECT
external-controller-unix: /var/run/mihomo/mihomo.sock
EOF
chown testuser:testuser /home/testuser/.config/mihomo/config.yaml

cat > /var/lib/mihomo-cli/tun-config.yaml <<'EOF'
proxies: []
proxy-groups: []
rules:
  - MATCH,DIRECT
external-controller-unix: /var/run/mihomo/mihomo.sock
EOF

cat > /etc/systemd/system/mihomo.service <<'EOF'
[Unit]
Description=Mihomo CLI daemon contract test

[Service]
Type=simple
ExecStart=/usr/local/bin/mihomo-cli daemon

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl start mihomo
wait_for_path /var/run/mihomo/service.sock || fail "daemon IPC socket did not appear"

python3 - <<'PY'
import json
import socket
import struct

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect("/var/run/mihomo/service.sock")
payload = json.dumps({
    "type": "StartCore",
    "config_path": "/home/testuser/.config/mihomo/config.yaml",
}).encode()
sock.sendall(struct.pack("<I", len(payload)) + payload)
length = struct.unpack("<I", sock.recv(4))[0]
response = b""
while len(response) < length:
    response += sock.recv(length - len(response))
sock.close()
response = json.loads(response)
if response.get("type") != "Success":
    raise SystemExit(f"StartCore failed: {response}")
PY

chown root:root /home/testuser/.config/mihomo/config.yaml
sudo -u testuser env HOME=/home/testuser USER=testuser \
    mihomo-cli restart || fail "testuser restart did not repair configuration ownership"
[ "$(stat -c '%u:%g' /home/testuser/.config/mihomo/config.yaml)" = "$(id -u testuser):$(id -g testuser)" ] || \
    fail "restart did not restore testuser configuration ownership"

echo "PASS: testuser restart repaired managed configuration ownership and continued"

sudo -u testuser env HOME=/home/testuser USER=testuser \
    mihomo-cli tun on --yes || fail "testuser tun on did not complete through sudo reexec"

[ -S /var/run/mihomo/mihomo.sock ] || fail "fake Core did not create the snapshot API socket"
[ -f /var/lib/mihomo-cli/fake-core-argv ] || fail "fake Core did not record its argv"

mapfile -t argv < /var/lib/mihomo-cli/fake-core-argv
expected=("-d" "/var/lib/mihomo-cli" "-f" "/var/lib/mihomo-cli/tun-config.yaml")
[ "${#argv[@]}" -eq "${#expected[@]}" ] || fail "unexpected fake Core argv length: ${argv[*]}"
for index in "${!expected[@]}"; do
    [ "${argv[$index]}" = "${expected[$index]}" ] || fail "argv mismatch: ${argv[*]}"
done

systemctl stop mihomo
printf 'PASS: testuser tun on reexeced through sudo and daemon restarted Core with explicit tun-config.yaml\n'
