#!/usr/bin/env bash
set -euo pipefail

: "${CLASH_CONFIG_URL:?set CLASH_CONFIG_URL to run this external test}"
: "${MIHOMO_REAL_CORE:?set MIHOMO_REAL_CORE to a real Mihomo executable}"

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CLI_BINARY="${MIHOMO_CLI_BINARY:-$PROJECT_ROOT/target/release/mihomo-cli}"

[ -x "$CLI_BINARY" ] || {
    echo "FAIL: current mihomo-cli binary is not executable: $CLI_BINARY" >&2
    exit 1
}
[ -x "$MIHOMO_REAL_CORE" ] || {
    echo "FAIL: real Mihomo binary is not executable: $MIHOMO_REAL_CORE" >&2
    exit 1
}
command -v docker >/dev/null || {
    echo "FAIL: Docker is required" >&2
    exit 1
}
[ -c /dev/net/tun ] || {
    echo "FAIL: host /dev/net/tun is required" >&2
    exit 1
}

cli_binary="$(realpath "$CLI_BINARY")"
real_core="$(realpath "$MIHOMO_REAL_CORE")"

exec docker run --rm --privileged --network bridge \
    --env CLASH_CONFIG_URL \
    --mount "type=bind,src=$cli_binary,dst=/usr/local/bin/mihomo-cli,readonly" \
    --mount "type=bind,src=$real_core,dst=/usr/local/bin/mihomo,readonly" \
    ubuntu:24.04 bash -ceu '
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends ca-certificates curl iproute2 python3 python3-yaml >/dev/null
work=$(mktemp -d)
core_pid=""
probe_pid=""
cleanup() {
  if [ -n "$probe_pid" ]; then kill "$probe_pid" 2>/dev/null || true; wait "$probe_pid" 2>/dev/null || true; fi
  if [ -n "$core_pid" ]; then kill "$core_pid" 2>/dev/null || true; wait "$core_pid" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
mihomo -v | grep -q "Mihomo Meta"
mkdir -p "$work/state"
mihomo-cli config fetch "$CLASH_CONFIG_URL" --output "$work/subscription.yaml" >/dev/null 2>"$work/fetch.err" || {
  echo "FAIL: current CLI could not fetch or convert the subscription" >&2
  exit 1
}
python3 - "$work/subscription.yaml" "$work/runtime.yaml" "$work/tun.yaml" <<"PY"
import sys
import yaml
source, runtime, tun_target = sys.argv[1:]
with open(source, encoding="utf-8") as f:
    config = yaml.safe_load(f)
if not isinstance(config, dict):
    raise SystemExit("FAIL: converted subscription is not a mapping")
for target, tun_enabled in ((runtime, False), (tun_target, True)):
    value = dict(config)
    value["external-controller"] = "127.0.0.1:19090"
    value.pop("external-controller-unix", None)
    value.pop("secret", None)
    value["mixed-port"] = 17890
    tun = value.get("tun")
    if not isinstance(tun, dict):
        tun = {}
        value["tun"] = tun
    tun.update({"enable": tun_enabled, "stack": "system", "auto-route": True,
                "auto-detect-interface": True, "strict-route": False})
    tun.pop("dns-hijack", None)
    with open(target, "w", encoding="utf-8") as f:
        yaml.safe_dump(value, f, allow_unicode=True, sort_keys=False)
PY
mihomo -d "$work/state" -f "$work/runtime.yaml" -t >/dev/null
mihomo -d "$work/state" -f "$work/runtime.yaml" >"$work/core.log" 2>&1 &
core_pid=$!
for _ in $(seq 1 150); do
  curl --fail --silent http://127.0.0.1:19090/version >/dev/null && break
  kill -0 "$core_pid" 2>/dev/null || { echo "FAIL: Core exited before non-TUN API readiness" >&2; exit 1; }
  sleep 0.1
done
curl --fail --silent --show-error --max-time 30 --proxy http://127.0.0.1:17890 https://www.gstatic.com/generate_204 -o /dev/null
kill "$core_pid"
wait "$core_pid" || true
core_pid=""
mihomo -d "$work/state" -f "$work/tun.yaml" -t >/dev/null
mihomo -d "$work/state" -f "$work/tun.yaml" >"$work/core.log" 2>&1 &
core_pid=$!
for _ in $(seq 1 150); do
  curl --fail --silent http://127.0.0.1:19090/configs >"$work/configs.json" && break
  kill -0 "$core_pid" 2>/dev/null || { echo "FAIL: Core exited before TUN API readiness" >&2; exit 1; }
  sleep 0.1
done
python3 - "$work/configs.json" "$work/tun-device" <<"PY"
import json
import sys
with open(sys.argv[1]) as f:
    config = json.load(f)
tun = config.get("tun", {})
if tun.get("enable") is not True:
    raise SystemExit("FAIL: Core API did not report tun.enable=true")
device = tun.get("device")
if not isinstance(device, str) or not device:
    raise SystemExit("FAIL: Core API did not report a TUN device name")
open(sys.argv[2], "w").write(device)
PY
device=$(cat "$work/tun-device")
ip link show "$device" >/dev/null
curl --fail --silent --show-error --max-time 90 --interface "$device" --noproxy "*" --limit-rate 64k https://speed.hetzner.de/100MB.bin -o /dev/null >"$work/probe.out" 2>"$work/probe.err" &
probe_pid=$!
for _ in $(seq 1 100); do
  kill -0 "$probe_pid" 2>/dev/null || break
  if curl --fail --silent http://127.0.0.1:19090/connections >"$work/connections.json" && python3 - "$work/connections.json" <<"PY"
import json
import sys
with open(sys.argv[1]) as f:
    data = json.load(f)
raise SystemExit(0 if data.get("connections") else 1)
PY
  then
    echo "PASS: real subscription, current CLI, real Core, non-TUN proxy, and TUN data path"
    exit 0
  fi
  sleep 0.1
done
echo "FAIL: Core recorded no live connection for the bound TUN data-path probe" >&2
exit 1
'