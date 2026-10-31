#!/bin/bash
# 迁移后首次显式采纳 E2E（systemd + 真实 Core，契约 B 冻结版）。
#
# 现场：初装并运行服务 → 用「字节不同于已安装 CLI」的 CLI 暂存/应用出一个可信代际
# → 把 state.json/generations 放回旧 runtime 布局并删除新信任根 → 普通 restart 完成
# 迁移后因全 untrusted 拒绝准入。随后验证冻结协议：
#   1. 普通 install/restart 不隐式采纳，指引 install --system --adopt-current；
#   2. adopt 不再要求旧 manifest 匹配：二进制部分替换、Geo 变化/缺失均接受，
#      但必须警告不证明历史/兼容/健康；可信槽位/根不安全/symlink 仍拒绝；
#   3. pending 全 U 场景只能用 --adopt-current --discard-pending=P 精确同提交处理，
#      pending-only discard / 错误 P / 缺少 P 均零变化；
#   4. 成功 state active=∅、pending=∅、previous=B；B 含 CLI/Core/3 Geo（缺失显式记录）
#      与可审计 approval metadata；旧迁移 live/withdrawn 证据不变；重复采纳同 B 幂等，
#      采纳后漂移拒绝再次采纳；
#   5. 已采纳恢复态只允许显式 install 绑定 B；restart skew 不得自动备候选；
#   6. dry-run 不迁移/init/建锁/staging/提权，JSON stdout 纯 JSON；
#   7. doctor --system --privileged 为非 root 的显式只读授权检查，默认诊断提示该命令。
set -euo pipefail
source /tests/scripts/lib-generation-e2e.sh

REAL_CORE=/tests/real-mihomo
LEGACY_ROOT=/var/lib/mihomo-cli
LEGACY_STATE="$LEGACY_ROOT/state.json"
LEGACY_GENERATIONS="$LEGACY_ROOT/generations"
SELECT_GROUP=节点选择
SELECT_NODE=N2
# 旧 CLI 留下的墓碑必须使用独立合法 ID：与 live 候选同 ID 会在定向撤销时撞名。
TOMBSTONE_LEGACY_ID=gen-20240101000000-0550beef
WORK=/tmp/055-migration-adoption
MARKED_CLI="$WORK/marked-mihomo-cli"

rm -rf "$WORK"
mkdir -p "$WORK"

require_fixture() {
    [ "$(id -u)" = 0 ] || fail "must run as root inside the container"
    command -v systemctl >/dev/null || fail "systemctl unavailable"
    [ -x "$CLI_SOURCE" ] || fail "installer CLI missing at $CLI_SOURCE"
    [ -x "$REAL_CORE" ] || fail "real Core missing at $REAL_CORE"
    "$REAL_CORE" -v | grep -q '^Mihomo Meta' || fail "$REAL_CORE is not a real Mihomo Core"
}

check_frozen_surface() {
    as_user "$CLI_SOURCE" install --help 2>&1 | grep -qF -- '--adopt-current' || \
        fail "install does not expose --adopt-current"
    as_user "$CLI_SOURCE" install --help 2>&1 | grep -qF -- '--dry-run' || \
        fail "install does not expose --dry-run"
    as_user "$CLI_SOURCE" doctor --help 2>&1 | grep -qF -- '--privileged' || \
        fail "doctor does not expose --privileged"
}

core_pid_value() {
    python3 - "$CORE_PID_FILE" <<'PY'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        print(json.load(handle).get("pid", ""))
except Exception:
    print("")
PY
}

wait_api_055() {
    for _ in $(seq 1 300); do
        if api_responding; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

proxy_now() {  # <group>
    python3 - "$API_SOCKET" "$1" <<'PY'
import json, socket, sys, urllib.parse
path, group = sys.argv[1], sys.argv[2]
endpoint = "/proxies/" + urllib.parse.quote(group)
request = f"GET {endpoint} HTTP/1.0\r\nHost: localhost\r\n\r\n".encode()
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
    sock.settimeout(5)
    sock.connect(path)
    sock.sendall(request)
    data = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        data += chunk
print(json.loads(data.split(b"\r\n\r\n", 1)[1]).get("now", ""))
PY
}

wait_proxy_now() {  # <group> <node>
    for _ in $(seq 1 100); do
        if [ "$(proxy_now "$1" 2>/dev/null || true)" = "$2" ]; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

state_of() {  # <root> <key>
    python3 - "$1/state.json" "$2" <<'PY'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        state = json.load(handle)
except FileNotFoundError:
    print("")
    raise SystemExit(0)
value = state.get(sys.argv[2])
print("" if value is None else value)
PY
}

state_slots() {  # <root>; all non-empty slots, one per line
    python3 - "$1/state.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    state = json.load(handle)
for key in ("active", "pending", "previous"):
    value = state.get(key)
    if value:
        print(value)
PY
}

state_write_slots() {  # <root> <active|-> <pending|-> <previous|->
    python3 - "$1/state.json" "$2" "$3" "$4" <<'PY'
import json, sys
path, active, pending, previous = sys.argv[1:5]
with open(path, encoding="utf-8") as handle:
    state = json.load(handle)
state["active"] = None if active in ("-", "") else active
state["pending"] = None if pending in ("-", "") else pending
state["previous"] = None if previous in ("-", "") else previous
with open(path, "w", encoding="utf-8") as handle:
    json.dump(state, handle, indent=2, ensure_ascii=False)
    handle.write("\n")
PY
}

manifest_hashes() {  # <root> <generation>; prints both hashes on one line for `read`
    python3 - "$1/generations/$2/manifest.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    manifest = json.load(handle)
print(manifest["daemon"]["sha256"], manifest["core"]["sha256"])
PY
}

manifest_baseline() {  # <root> <generation>
    python3 - "$1/generations/$2/manifest.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    manifest = json.load(handle)
print(manifest.get("metadata", {}).get("baseline", ""))
PY
}

retarget_manifest_id() {  # <manifest> <new-generation-id>
    python3 - "$1" "$2" <<'PY'
import json, sys
path, new_id = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as handle:
    manifest = json.load(handle)
manifest["generation_id"] = new_id
with open(path, "w", encoding="utf-8") as handle:
    json.dump(manifest, handle, indent=2, ensure_ascii=False)
    handle.write("\n")
PY
}

add_manifest_extra() {  # <root> <generation> <relative_path> <source> <kind>
    local root="$1" id="$2" rel="$3" source="$4" kind="$5"
    local gen="$root/generations/$id"
    cp -a "$source" "$gen/$rel"
    chmod 0640 "$gen/$rel"
    python3 - "$gen/manifest.json" "$rel" "$kind" "$gen/$rel" <<'PY'
import hashlib, json, sys
manifest_path, rel, kind, artifact = sys.argv[1:5]
with open(artifact, "rb") as handle:
    data = handle.read()
entry = {
    "relative_path": rel,
    "kind": kind,
    "sha256": hashlib.sha256(data).hexdigest(),
    "size_bytes": len(data),
    "unix_mode": 0o640,
}
with open(manifest_path, encoding="utf-8") as handle:
    manifest = json.load(handle)
extras = [
    item for item in manifest.get("extra_artifacts", [])
    if item.get("relative_path") != rel
]
extras.append(entry)
manifest["extra_artifacts"] = extras
with open(manifest_path, "w", encoding="utf-8") as handle:
    json.dump(manifest, handle, indent=2, ensure_ascii=False)
    handle.write("\n")
PY
}

tree_fingerprint() {  # <path>; content + structure + modes, mtimes intentionally excluded
    local root="$1"
    if [ ! -e "$root" ]; then
        printf 'absent:%s\n' "$root"
        return 0
    fi
    if [ -f "$root" ]; then
        sha256sum "$root" | cut -d' ' -f1
        return 0
    fi
    {
        find "$root" -mindepth 1 -printf '%P|%y|%m|%s\n' | LC_ALL=C sort
        find "$root" -type f -print0 | LC_ALL=C sort -z | while IFS= read -r -d '' path; do
            printf '%s|' "${path#"$root"/}"
            sha256sum "$path" | cut -d' ' -f1
        done
    } | sha256sum | cut -d' ' -f1
}

evidence_tree_fingerprint() {  # artifact/manifest evidence, ignores untrusted markers
    local root="$1"
    {
        find "$root" -mindepth 1 ! -name '.migrated-untrusted' \
            -printf '%P|%y|%m|%s\n' | LC_ALL=C sort
        find "$root" -type f ! -name '.migrated-untrusted' -print0 | LC_ALL=C sort -z | \
            while IFS= read -r -d '' path; do
                printf '%s|' "${path#"$root"/}"
                sha256sum "$path" | cut -d' ' -f1
            done
    } | sha256sum | cut -d' ' -f1
}

store_fingerprint() {
    {
        tree_fingerprint "$GEN_ROOT/state.json"
        tree_fingerprint "$GEN_ROOT/generations"
    } | sha256sum | cut -d' ' -f1
}

legacy_fingerprint() {
    {
        tree_fingerprint "$LEGACY_STATE"
        tree_fingerprint "$LEGACY_GENERATIONS"
    } | sha256sum | cut -d' ' -f1
}

evidence_fingerprint() {  # <generation-dir-name...>
    local name
    for name in "$@"; do
        printf '%s\n' "$name"
        tree_fingerprint "$GEN_ROOT/generations/$name"
    done | sha256sum | cut -d' ' -f1
}

runtime_fingerprint() {  # install bytes + Core inode; excludes live service identity
    {
        sha256sum "$INSTALLED_CLI" | cut -d' ' -f1
        sha256sum "$CORE_BINARY" | cut -d' ' -f1
        stat -c '%i' "$CORE_BINARY"
        sha256sum "$CONFIG_DIR/config.yaml" | cut -d' ' -f1
    } | sha256sum | cut -d' ' -f1
}

service_fingerprint() {
    {
        systemctl show -p MainPID -p NRestarts -p ActiveEnterTimestampMonotonic --value mihomo 2>/dev/null || true
        core_pid_value
    } | sha256sum | cut -d' ' -f1
}

generation_names() {
    find "$GEN_ROOT/generations" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' 2>/dev/null | LC_ALL=C sort
}

assert_all_slots_untrusted() {  # <root>
    local id slots
    slots=$(state_slots "$1")
    [ -n "$slots" ] || fail "migrated state has no non-empty slots"
    for id in $slots; do
        [ -e "$1/generations/$id/.migrated-untrusted" ] || \
            fail "migrated slot $id is missing the untrusted marker"
    done
}

run_expect_success() {  # <log> <cmd...>
    local log="$1"
    shift
    if ! "$@" >"$log" 2>&1; then
        cat "$log" >&2
        fail "command failed: $*"
    fi
}

capture_status() {  # <log> <cmd...>; echoes exit status, never aborts
    local log="$1"
    shift
    set +e
    "$@" >"$log" 2>&1
    local status=$?
    set -e
    printf '%s\n' "$status"
}

adopt() {
    as_user sudo -n "$CLI_SOURCE" install --system --adopt-current "$@"
}

adopt_discard() {  # <pending-id> [extra args...]
    local pending="$1"
    shift
    adopt --discard-pending="$pending" "$@"
}

adopt_without_confirmation() {
    as_user sudo -n "$CLI_SOURCE" install --system --adopt-current </dev/null
}

reset_fixture() {
    systemctl stop mihomo >/dev/null 2>&1 || true
    as_user "$CLI_SOURCE" uninstall --all --yes >/dev/null 2>&1 || true
    systemctl stop mihomo >/dev/null 2>&1 || true
    rm -rf "$GEN_ROOT" "$LEGACY_ROOT" /var/run/mihomo /etc/systemd/system/mihomo.service
    systemctl daemon-reload >/dev/null 2>&1 || true
    rm -rf "$CONFIG_DIR"
    mkdir -p "$CONFIG_DIR"
    replace_binary "$CLI_SOURCE" "$INSTALLED_CLI"
    replace_binary "$REAL_CORE" "$CORE_BINARY"
    truncate -s 8000001 "$CONFIG_DIR/geoip.metadb"
    truncate -s 2000001 "$CONFIG_DIR/GeoSite.dat"
    chown -R "$TEST_USER:$TEST_USER" "$TEST_HOME/.config"
}

install_real_system() {
    run_expect_success "$WORK/install.log" as_user sudo -n "$CLI_SOURCE" install --system --yes
    [ -S "$API_SOCKET" ] || fail "real Core API socket missing after install"
    [ "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" = "$(sha256sum "$CLI_SOURCE" | cut -d' ' -f1)" ] || \
        fail "initial install did not install the invoking CLI"
    wait_service_active || fail "daemon service is not active after install"
    wait_api_055 || fail "real Core API did not become ready after install"
}

install_selection_fixture() {
    cat >"$WORK/selection-fixture.yaml" <<'YAML'
proxies:
  - {name: N1, type: direct}
  - {name: N2, type: direct}
proxy-groups:
  - {name: 节点选择, type: select, proxies: [N1, N2]}
rules:
  - MATCH,节点选择
YAML
    run_expect_success "$WORK/import.log" \
        as_user sudo -n "$CLI_SOURCE" config --system --import "$WORK/selection-fixture.yaml" --activate --yes
    wait_api_055 || fail "real Core API not ready after config import"
    run_expect_success "$WORK/select.log" \
        as_user sudo -n "$CLI_SOURCE" select --system --group "$SELECT_GROUP" --node "$SELECT_NODE"
    wait_proxy_now "$SELECT_GROUP" "$SELECT_NODE" || fail "selection fixture was not applied"
}

# 可信且与安装态一致的代际：字节不同于已安装 CLI 的新 CLI 暂存候选，再由它自己应用。
build_trusted_generation() {
    cp "$CLI_SOURCE" "$MARKED_CLI"
    chmod 755 "$MARKED_CLI"
    printf '\n# 055 fixture: daemon bytes diverge from the pristine CLI\n' >>"$MARKED_CLI"
    local pid_before
    pid_before=$(core_pid_value)
    [ -n "$pid_before" ] || fail "no running Core before staging"
    run_expect_success "$WORK/stage.log" as_user sudo -n "$MARKED_CLI" install --system --yes
    grep -q 'Upgrade prepared in pending generation' "$WORK/stage.log" || {
        cat "$WORK/stage.log" >&2
        fail "staged install did not prepare a pending generation"
    }
    systemctl is-active --quiet mihomo || fail "staging interrupted the daemon"
    [ "$(core_pid_value)" = "$pid_before" ] || fail "staging replaced the running Core"
    cp "$GEN_ROOT/state.json" "$WORK/staged-pending-state.json"
    run_expect_success "$WORK/apply.log" as_user sudo -n "$MARKED_CLI" restart --system --yes
    [ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "staged generation was not applied"
    [ -n "$(state_of "$GEN_ROOT" active)" ] || fail "no active generation after apply"
    [ "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" = "$(sha256sum "$MARKED_CLI" | cut -d' ' -f1)" ] || \
        fail "apply did not install the staged daemon bytes"
    wait_api_055 || fail "real Core API did not recover after apply"
    wait_proxy_now "$SELECT_GROUP" "$SELECT_NODE" || fail "selection was not replayed after apply"
}

# 「迁移前」现场：旧 store 根 = runtime 根，包含旧 CLI 遗留的未标记 .withdrawn。
build_legacy_store() {
    local active previous
    active=$(state_of "$GEN_ROOT" active)
    previous=$(state_of "$GEN_ROOT" previous)
    [ -n "$active" ] || fail "trusted fixture has no active generation"
    [ -f "$LEGACY_ROOT/geoip.metadb" ] || fail "managed geoip.metadb is missing"

    rm -f "$LEGACY_STATE"
    rm -rf "$LEGACY_GENERATIONS"
    cp -a "$GEN_ROOT/state.json" "$LEGACY_STATE"
    cp -a "$GEN_ROOT/generations" "$LEGACY_GENERATIONS"
    add_manifest_extra "$LEGACY_ROOT" "$active" "geoip.metadb" "$LEGACY_ROOT/geoip.metadb" "geoip"
    # 旧 CLI 写入的墓碑：迁移前没有 untrusted 标记；新迁移必须补标。
    # 使用独立 ID，避免与 live 候选同 ID 在定向撤销时发生墓碑撞名。
    cp -a "$LEGACY_GENERATIONS/$active" "$LEGACY_GENERATIONS/${TOMBSTONE_LEGACY_ID}.withdrawn"
    retarget_manifest_id \
        "$LEGACY_GENERATIONS/${TOMBSTONE_LEGACY_ID}.withdrawn/manifest.json" "$TOMBSTONE_LEGACY_ID"
    rm -rf "$GEN_ROOT"

    [ -f "$LEGACY_STATE" ] || fail "legacy state.json was not staged"
    [ -d "$LEGACY_GENERATIONS/$active" ] || fail "legacy active generation was not staged"
    [ -d "$LEGACY_GENERATIONS/${TOMBSTONE_LEGACY_ID}.withdrawn" ] || \
        fail "legacy withdrawn record was not staged"
    [ ! -e "$GEN_ROOT" ] || fail "new generation root still exists before migration"
    printf '%s\n%s\n' "$active" "$previous"
}

# dry-run 必须完全不迁移/不建根/不建锁/不 staging/不提权。非 root 读不到旧根时应诚实报 unknown。
assert_non_root_dry_run_never_migrates() {
    mkdir -p "$WORK/nosudo"
    cat >"$WORK/nosudo/sudo" <<'SH'
#!/bin/sh
echo "sudo must not be invoked by --dry-run" >&2
exit 97
SH
    chmod 755 "$WORK/nosudo/sudo"
    local out="$WORK/dry-run-nonroot.json" err="$WORK/dry-run-nonroot.err"
    local legacy_before
    legacy_before=$(legacy_fingerprint)
    set +e
    sudo -n -u "$TEST_USER" env HOME="$TEST_HOME" USER="$TEST_USER" \
        PATH="$WORK/nosudo:/usr/local/bin:/usr/bin:/bin" \
        "$CLI_SOURCE" --json install --system --adopt-current --dry-run >"$out" 2>"$err"
    local status=$?
    set -e
    cat "$err" >&2
    [ "$status" -eq 0 ] || fail "non-root dry-run must report unknown without failing (status=$status)"
    ! grep -q 'sudo must not be invoked' "$err" || fail "dry-run attempted privilege escalation"
    python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$out" || {
        cat "$out" >&2
        fail "non-root dry-run stdout is not pure JSON"
    }
    grep -Eqi 'unknown|无法判定|不可读|unreadable|permission|权限|EACCES' "$out" "$err" || {
        cat "$out" >&2
        cat "$err" >&2
        fail "non-root dry-run did not honestly report unknown"
    }
    [ ! -e "$GEN_ROOT" ] || fail "dry-run migrated/created the trust root"
    [ ! -e "$GEN_ROOT/.generation.lock" ] || fail "dry-run created a generation lock"
    [ -z "$(find /var/lib -maxdepth 1 -name '.mihomo-cli-generations.migrating-*' -print -quit)" ] || \
        fail "dry-run created a migration staging tree"
    [ "$legacy_before" = "$(legacy_fingerprint)" ] || fail "dry-run modified the legacy evidence"
}

assert_root_dry_run_preview() {
    local before after lock_before lock_after legacy_before root_mode_before
    local out="$WORK/dry-run-root.json" err="$WORK/dry-run-root.err"
    before=$(store_fingerprint)
    legacy_before=$(legacy_fingerprint)
    root_mode_before=$(stat -c '%a' "$GEN_ROOT")
    lock_before=$(stat -c '%i|%Y' "$GEN_ROOT/.generation.lock" 2>/dev/null || echo absent)
    set +e
    as_user sudo -n "$CLI_SOURCE" --json install --system --adopt-current --dry-run >"$out" 2>"$err"
    local status=$?
    set -e
    cat "$err" >&2
    [ "$status" -eq 0 ] || fail "root dry-run failed (status=$status)"
    wait_api_055 || fail "Core API stopped responding during dry-run"
    python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$out" || {
        cat "$out" >&2
        fail "dry-run stdout is not pure JSON"
    }
    local preview
    preview=$(cat "$out")
    grep -Fq -- "$INSTALLED_CLI" <<<"$preview" || fail "dry-run preview lacks the CLI path"
    grep -Fq -- "$CORE_BINARY" <<<"$preview" || fail "dry-run preview lacks the Core path"
    for geo in geoip.metadb GeoSite.dat Country.mmdb; do
        grep -Fq -- "$geo" <<<"$preview" || fail "dry-run preview lacks $geo"
    done
    grep -Fq -- "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" <<<"$preview" || \
        fail "dry-run preview lacks the full CLI hash"
    grep -Fq -- "$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)" <<<"$preview" || \
        fail "dry-run preview lacks the full Core hash"
    grep -Eqi 'missing|缺失' <<<"$preview" || fail "dry-run preview does not mark missing artifacts"
    after=$(store_fingerprint)
    [ "$before" = "$after" ] || fail "dry-run modified generation state"
    [ "$legacy_before" = "$(legacy_fingerprint)" ] || fail "dry-run modified legacy evidence"
    [ "$root_mode_before" = "$(stat -c '%a' "$GEN_ROOT")" ] || fail "dry-run changed the trust-root mode"
    lock_after=$(stat -c '%i|%Y' "$GEN_ROOT/.generation.lock" 2>/dev/null || echo absent)
    [ "$lock_before" = "$lock_after" ] || fail "dry-run touched the generation lock"
    [ -z "$(find /var/lib -maxdepth 1 -name '.mihomo-cli-generations.migrating-*' -print -quit)" ] || \
        fail "dry-run created a migration staging tree"
}

assert_readonly_doctor() {
    local store_before legacy_before runtime_before service_before out
    local root_meta_before
    store_before=$(store_fingerprint)
    legacy_before=$(legacy_fingerprint)
    runtime_before=$(runtime_fingerprint)
    service_before=$(service_fingerprint)
    root_meta_before=$(stat -c '%a|%u|%g' "$GEN_ROOT")
    local legacy_meta_before
    legacy_meta_before=$(stat -c '%a|%u|%g' "$LEGACY_ROOT")

    for command in status doctor; do
        out="$WORK/${command}-nonroot.log"
        set +e
        sudo -n -u "$TEST_USER" env HOME="$TEST_HOME" USER="$TEST_USER" \
            PATH="$WORK/nosudo:/usr/local/bin:/usr/bin:/bin" \
            "$CLI_SOURCE" "$command" --system >"$out" 2>&1
        set -e
        cat "$out" >&2
        ! grep -q 'sudo must not be invoked' "$out" || \
            fail "default $command attempted privilege escalation"
        grep -qF -- '--privileged' "$out" || \
            fail "default $command did not point at doctor --system --privileged"
    done
    [ "$store_before" = "$(store_fingerprint)" ] || fail "default status/doctor modified generation state"
    [ "$legacy_before" = "$(legacy_fingerprint)" ] || fail "default status/doctor modified legacy evidence"
    [ "$runtime_before" = "$(runtime_fingerprint)" ] || fail "default status/doctor touched install bytes"
    [ "$service_before" = "$(service_fingerprint)" ] || fail "default status/doctor stopped or restarted the runtime"
    [ "$root_meta_before" = "$(stat -c '%a|%u|%g' "$GEN_ROOT")" ] || \
        fail "default status/doctor repaired the generation trust-root metadata"
    [ "$legacy_meta_before" = "$(stat -c '%a|%u|%g' "$LEGACY_ROOT")" ] || \
        fail "default status/doctor repaired the runtime-state directory metadata"

    local privileged_status
    privileged_status=$(capture_status "$WORK/doctor-privileged.log" \
        as_user "$CLI_SOURCE" doctor --system --privileged)
    cat "$WORK/doctor-privileged.log" >&2
    [ -s "$WORK/doctor-privileged.log" ] || fail "privileged doctor produced no diagnostics"
    ! grep -Eqi 'a password is required|a terminal is required|no tty present|not allowed' \
        "$WORK/doctor-privileged.log" || \
        fail "privileged doctor did not obtain the read-only authorization (status=$privileged_status)"
    [ "$store_before" = "$(store_fingerprint)" ] || fail "privileged doctor modified generation state"
    [ "$legacy_before" = "$(legacy_fingerprint)" ] || fail "privileged doctor modified legacy evidence"
    [ "$runtime_before" = "$(runtime_fingerprint)" ] || fail "privileged doctor touched install bytes"
    [ "$service_before" = "$(service_fingerprint)" ] || fail "privileged doctor stopped or restarted the runtime"
    [ "$root_meta_before" = "$(stat -c '%a|%u|%g' "$GEN_ROOT")" ] || \
        fail "privileged doctor repaired the generation trust-root metadata"
    [ "$legacy_meta_before" = "$(stat -c '%a|%u|%g' "$LEGACY_ROOT")" ] || \
        fail "privileged doctor repaired the runtime-state directory metadata"
    systemctl is-active --quiet mihomo || fail "privileged doctor left the daemon inactive"
    api_responding || fail "privileged doctor left the Core API unresponsive"
}

# B 的固定快照 + approval 元数据。expected_* 为采纳时刻的确切字节；missing_* 必须显式记录。
assert_adopted_baseline() {  # <root> <B> <cli-sha> <core-sha> <geoip-sha|-> <discarded-p|-> <missing-csv> <original...>
    local root="$1" baseline="$2" cli_sha="$3" core_sha="$4" geoip_sha="$5" discarded="$6" expected_missing="$7"
    shift 7
    python3 - "$root/generations/$baseline/manifest.json" \
        "$cli_sha" "$core_sha" "$geoip_sha" "$discarded" "$expected_missing" "$@" <<'PY'
import json, re, sys
(
    manifest_path, cli_sha, core_sha, geoip_sha, discarded, expected_missing, *originals
) = sys.argv[1:]
with open(manifest_path, encoding="utf-8") as handle:
    manifest = json.load(handle)
meta = manifest.get("metadata") or {}
assert meta.get("kind") == "baseline", meta
assert manifest["daemon"]["sha256"] == cli_sha, (manifest["daemon"], cli_sha)
assert manifest["core"]["sha256"] == core_sha, (manifest["core"], core_sha)

entries = [manifest["daemon"], manifest["core"], *manifest.get("extra_artifacts", [])]
by_base = {}
for entry in entries:
    by_base.setdefault(entry["relative_path"].rsplit("/", 1)[-1], []).append(entry)

missing_value = meta.get("missing", "")
if isinstance(missing_value, list):
    missing = {str(item).rsplit("/", 1)[-1] for item in missing_value}
else:
    missing = {
        item.strip().rsplit("/", 1)[-1]
        for item in str(missing_value).split(",")
        if item.strip()
    }
manifest_text = json.dumps(manifest, ensure_ascii=False)
for name in ("geoip.metadb", "GeoSite.dat", "Country.mmdb"):
    assert name in manifest_text, f"fixed snapshot does not mention {name}"
for name in [item for item in expected_missing.split(",") if item]:
    assert name in missing, f"{name} is not recorded as missing: {missing}"
if geoip_sha != "-":
    assert "geoip.metadb" in by_base, by_base.keys()
    assert any(item["sha256"] == geoip_sha for item in by_base["geoip.metadb"]), \
        by_base["geoip.metadb"]
else:
    assert "geoip.metadb" in missing, f"geoip.metadb is neither present nor missing: {missing}"

text = json.dumps(meta, ensure_ascii=False).lower()
assert re.search(r"[0-9a-f]{64}", text), "approval metadata lacks a confirmation digest"
assert any(word in text for word in (
    "adopt", "approval", "approve", "confirm", "source", "migration",
    "采纳", "审批", "确认", "来源", "迁移"
)), text
assert any(word in text for word in (
    "subject", "operator", "principal", "user", "uid", "root", "admin",
    "主体", "操作者", "管理员"
)), text
assert any(word in text for word in (
    "method", "mode", "yes", "non-interactive", "interactive", "--yes", "方式",
    "confirm"
)), text
for original in originals:
    assert original in text, f"original slot {original} is not recorded: {text}"
if discarded != "-":
    assert discarded in text, f"discarded pending {discarded} is not recorded: {text}"
    assert "discard" in text or "撤销" in text, text
print("adopted baseline verified:", meta)
PY
}

echo "=== generation migration adoption (, frozen contract B) ==="
require_fixture
check_frozen_surface
reset_fixture
install_real_system
install_selection_fixture
build_trusted_generation
SLOT_OUTPUT=$(build_legacy_store)
mapfile -t SLOTS <<<"$SLOT_OUTPUT"
ACTIVE_OLD=${SLOTS[0]}
PREVIOUS_OLD=${SLOTS[1]:-}
TOMBSTONE_OLD="${TOMBSTONE_LEGACY_ID}.withdrawn"
[ -n "$PREVIOUS_OLD" ] || fail "trusted fixture has no previous baseline"
echo "migrated fixture: active=$ACTIVE_OLD previous=$PREVIOUS_OLD"

assert_non_root_dry_run_never_migrates

LEGACY_BEFORE=$(legacy_fingerprint)
RUNTIME_BEFORE=$(runtime_fingerprint)
SERVICE_BEFORE=$(service_fingerprint)
CONFIG_SHA=$(sha256sum "$CONFIG_DIR/config.yaml" | cut -d' ' -f1)

echo "--- 0. install-first 迁移（SOP 真实路径，）---"
STATUS=$(capture_status "$WORK/install-first-migration.log" \
    as_user sudo -n "$CLI_SOURCE" install --system --yes)
cat "$WORK/install-first-migration.log"
[ "$STATUS" -ne 0 ] || fail "install-first migration admitted the store without adoption"
grep -q '代际存储已从旧路径迁移到新信任根' "$WORK/install-first-migration.log" || \
    fail "install-first did not migrate the legacy store"
grep -qF -- '--adopt-current' "$WORK/install-first-migration.log" || \
    fail "install-first refusal does not point at --adopt-current"
[ -f "$GEN_ROOT/state.json" ] || fail "install-first migration did not publish state.json"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "install-first refusal left a pending generation"
assert_all_slots_untrusted "$GEN_ROOT"
[ "$LEGACY_BEFORE" = "$(legacy_fingerprint)" ] || fail "install-first migration modified legacy evidence"
[ "$RUNTIME_BEFORE" = "$(runtime_fingerprint)" ] || fail "install-first migration touched install bytes"
# 回到迁移前现场，继续既有 restart-first 旅程（两入口都必须覆盖）。
rm -rf "$GEN_ROOT"
[ ! -e "$GEN_ROOT" ] || fail "fixture reset left the generation root behind"

echo "--- 1. 普通 restart 完成迁移但拒绝准入，指引显式采纳 ---"
STATUS=$(capture_status "$WORK/restart-refuse.log" \
    as_user sudo -n "$CLI_SOURCE" restart --system --yes)
cat "$WORK/restart-refuse.log"
[ "$STATUS" -ne 0 ] || fail "restart admitted the migrated store without adoption"
grep -q '代际存储已从旧路径迁移到新信任根' "$WORK/restart-refuse.log" || \
    fail "restart did not report the legacy store migration"
grep -qF -- '--adopt-current' "$WORK/restart-refuse.log" || \
    fail "refusal guidance does not point at --adopt-current"
[ -f "$GEN_ROOT/state.json" ] || fail "migration did not publish the new trust root"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "refusal left an implicit pending generation"
assert_all_slots_untrusted "$GEN_ROOT"
[ -e "$GEN_ROOT/generations/$TOMBSTONE_OLD/.migrated-untrusted" ] || \
    fail "new migration did not mark the withdrawn record as untrusted"
[ "$LEGACY_BEFORE" = "$(legacy_fingerprint)" ] || fail "migration modified the legacy originals"
[ "$RUNTIME_BEFORE" = "$(runtime_fingerprint)" ] || fail "refusal replaced install bytes/config/inode"
[ "$SERVICE_BEFORE" = "$(service_fingerprint)" ] || fail "refusal stopped or restarted the runtime"
systemctl is-active --quiet mihomo || fail "refusal left the daemon inactive"
api_responding || fail "refusal left the Core API unresponsive"
STORE_BEFORE=$(store_fingerprint)
cp -a "$GEN_ROOT" "$WORK/store-post-migration"
echo "store after migration: $STORE_BEFORE"

echo "--- 2. 普通 install 同样不隐式采纳 ---"
STATUS=$(capture_status "$WORK/install-refuse.log" \
    as_user sudo -n "$CLI_SOURCE" install --system --yes)
cat "$WORK/install-refuse.log"
[ "$STATUS" -ne 0 ] || fail "install implicitly admitted the migrated store"
grep -qF -- '--adopt-current' "$WORK/install-refuse.log" || \
    fail "install refusal does not point at --adopt-current"
[ "$STORE_BEFORE" = "$(store_fingerprint)" ] || fail "install refusal modified the store"
[ "$LEGACY_BEFORE" = "$(legacy_fingerprint)" ] || fail "install refusal modified legacy evidence"
[ "$RUNTIME_BEFORE" = "$(runtime_fingerprint)" ] || fail "install refusal touched install bytes"
[ "$SERVICE_BEFORE" = "$(service_fingerprint)" ] || fail "install refusal stopped or restarted the runtime"

echo "--- 3. 非 TTY 且无 --yes 必须失败，零写入 ---"
STATUS=$(capture_status "$WORK/adopt-no-yes.log" adopt_without_confirmation)
cat "$WORK/adopt-no-yes.log"
[ "$STATUS" -ne 0 ] || fail "non-interactive adoption without --yes succeeded"
[ "$STORE_BEFORE" = "$(store_fingerprint)" ] || fail "unconfirmed adoption modified the store"
[ "$LEGACY_BEFORE" = "$(legacy_fingerprint)" ] || fail "unconfirmed adoption modified legacy evidence"
[ "$RUNTIME_BEFORE" = "$(runtime_fingerprint)" ] || fail "unconfirmed adoption touched install bytes/config"
[ "$SERVICE_BEFORE" = "$(service_fingerprint)" ] || fail "unconfirmed adoption stopped or restarted the runtime"

echo "--- 4. dry-run：零迁移/零提权/纯 JSON；root 预览完整路径与哈希 ---"
assert_root_dry_run_preview
assert_readonly_doctor

echo "--- 5. pending 全 U：只允许 --adopt-current --discard-pending=P 同一提交 ---"
cp -a "$WORK/staged-pending-state.json" "$GEN_ROOT/state.json"
PENDING_MIGRATED=$(state_of "$GEN_ROOT" pending)
[ "$PENDING_MIGRATED" = "$ACTIVE_OLD" ] || fail "pending fixture does not reference the migrated candidate"

# 唯一引用是迁移 pending 时，单独 discard 会把槽位清空成「首装」：必须拒绝。
cp -a "$GEN_ROOT/state.json" "$WORK/state-with-previous.bak"
state_write_slots "$GEN_ROOT" "-" "$PENDING_MIGRATED" "-"
ONLY_PENDING_BEFORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/pending-only-empty.log" \
    as_user sudo -n "$CLI_SOURCE" install --system --discard-pending="$PENDING_MIGRATED" --yes)
cat "$WORK/pending-only-empty.log"
[ "$STATUS" -ne 0 ] || fail "pending-only discard emptied the slots into a first-install lookalike"
grep -qF -- '--adopt-current' "$WORK/pending-only-empty.log" || \
    fail "pending-only empty refusal did not point at the combined adoption command"
[ "$ONLY_PENDING_BEFORE" = "$(store_fingerprint)" ] || fail "pending-only empty refusal modified the store"
cp -a "$WORK/state-with-previous.bak" "$GEN_ROOT/state.json"
GENS_BEFORE=$(generation_names)
G_EVIDENCE_BEFORE=$(evidence_tree_fingerprint "$GEN_ROOT/generations/$PENDING_MIGRATED")
P_MANIFEST_SHA_BEFORE=$(sha256sum "$GEN_ROOT/generations/$PENDING_MIGRATED/manifest.json" | cut -d' ' -f1)
EVIDENCE_PENDING_BEFORE=$(evidence_fingerprint "$PREVIOUS_OLD" "$TOMBSTONE_OLD")
NEG_BEFORE=$(store_fingerprint)

STATUS=$(capture_status "$WORK/adopt-pending-missing-discard.log" adopt --yes)
cat "$WORK/adopt-pending-missing-discard.log"
[ "$STATUS" -ne 0 ] || fail "adoption accepted a migrated pending without an explicit discard target"
grep -qF -- '--discard-pending' "$WORK/adopt-pending-missing-discard.log" || \
    fail "pending refusal does not name --discard-pending"
grep -qF -- "$PENDING_MIGRATED" "$WORK/adopt-pending-missing-discard.log" || \
    fail "pending refusal does not name the pending generation"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "pending refusal modified the store"

STATUS=$(capture_status "$WORK/pending-only-discard.log" \
    as_user sudo -n "$CLI_SOURCE" install --system --discard-pending="$PENDING_MIGRATED" --yes)
cat "$WORK/pending-only-discard.log"
[ "$STATUS" -ne 0 ] || fail "pending-only discard emptied the slots and masqueraded as first install"
grep -qF -- '--adopt-current' "$WORK/pending-only-discard.log" || \
    fail "pending-only discard did not point at the combined adoption command"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "pending-only discard modified the store"

WRONG_PENDING=gen-00000000000000-deadbeef
STATUS=$(capture_status "$WORK/adopt-wrong-pending.log" \
    adopt_discard "$WRONG_PENDING" --yes)
cat "$WORK/adopt-wrong-pending.log"
[ "$STATUS" -ne 0 ] || fail "adoption accepted a mismatched discard target"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "mismatched discard modified the store"

run_expect_success "$WORK/adopt-with-pending.log" adopt_discard "$PENDING_MIGRATED" --yes
cat "$WORK/adopt-with-pending.log"
[ -z "$(state_of "$GEN_ROOT" active)" ] || fail "adoption with pending left an active slot"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "adoption with pending did not clear the pending slot"
B_PENDING=$(state_of "$GEN_ROOT" previous)
[ -n "$B_PENDING" ] || fail "adoption with pending established no previous baseline"
[ "$B_PENDING" != "$PENDING_MIGRATED" ] || fail "discarded pending was reused as the trusted baseline"
read -r P_CLI P_CORE <<<"$(manifest_hashes "$GEN_ROOT" "$B_PENDING")"
[ "$P_CLI" = "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" ] || \
    fail "pending adoption baseline daemon != current CLI bytes"
[ "$P_CORE" = "$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)" ] || \
    fail "pending adoption baseline Core != current Core bytes"
assert_adopted_baseline "$GEN_ROOT" "$B_PENDING" "$P_CLI" "$P_CORE" \
    "$(sha256sum "$LEGACY_ROOT/geoip.metadb" | cut -d' ' -f1)" "$PENDING_MIGRATED" \
    "Country.mmdb" "$PREVIOUS_OLD"
CANDIDATE_DIR=""
for candidate in \
    "$GEN_ROOT/generations/$PENDING_MIGRATED" \
    "$GEN_ROOT/generations/${PENDING_MIGRATED}.withdrawn"; do
    if [ -f "$candidate/manifest.json" ]; then
        CANDIDATE_DIR="$candidate"
        break
    fi
done
[ -n "$CANDIDATE_DIR" ] || fail "adoption-with-pending deleted the discarded candidate evidence"
[ "$G_EVIDENCE_BEFORE" = "$(evidence_tree_fingerprint "$CANDIDATE_DIR")" ] || \
    fail "adoption-with-pending modified the discarded candidate evidence"
[ "$P_MANIFEST_SHA_BEFORE" = "$(sha256sum "$CANDIDATE_DIR/manifest.json" | cut -d' ' -f1)" ] || \
    fail "adoption-with-pending rewrote the discarded candidate manifest"
[ "$EVIDENCE_PENDING_BEFORE" = \
    "$(evidence_fingerprint "$PREVIOUS_OLD" "$TOMBSTONE_OLD")" ] || \
    fail "adoption-with-pending modified migrated live/withdrawn evidence"
[ "$LEGACY_BEFORE" = "$(legacy_fingerprint)" ] || fail "adoption-with-pending modified legacy evidence"
[ "$RUNTIME_BEFORE" = "$(runtime_fingerprint)" ] || fail "adoption-with-pending touched install bytes/config/inode"
[ "$SERVICE_BEFORE" = "$(service_fingerprint)" ] || fail "adoption-with-pending stopped or restarted the runtime"
[ -n "$(comm -13 <(printf '%s\n' "$GENS_BEFORE") <(generation_names))" ] || \
    fail "adoption-with-pending did not create a baseline generation"

echo "--- restore post-migration store for the no-pending journey ---"
rm -rf "$GEN_ROOT"
cp -a "$WORK/store-post-migration" "$GEN_ROOT"
[ "$(state_of "$GEN_ROOT" active)" = "$ACTIVE_OLD" ] || fail "post-migration store restore failed"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "post-migration store restore kept a pending slot"

echo "--- 6. 负例：可信槽位 / 不安全根 / symlink 仍拒绝 ---"
ACTIVE_NOW=$(state_of "$GEN_ROOT" active)
MARKER="$GEN_ROOT/generations/$ACTIVE_NOW/.migrated-untrusted"
cp -a "$MARKER" "$WORK/untrusted-marker.bak"
rm -f "$MARKER"
NEG_BEFORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/adopt-trusted-slot.log" adopt --yes)
cat "$WORK/adopt-trusted-slot.log"
[ "$STATUS" -ne 0 ] || fail "adoption accepted a store that still has a trusted slot"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "trusted-slot refusal modified the store"
cp -a "$WORK/untrusted-marker.bak" "$MARKER"

ROOT_MODE=$(stat -c '%a' "$GEN_ROOT")
chmod 0777 "$GEN_ROOT"
NEG_BEFORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/adopt-unsafe-root.log" adopt --yes)
cat "$WORK/adopt-unsafe-root.log"
[ "$STATUS" -ne 0 ] || fail "adoption accepted an unsafe generation trust root"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "unsafe-root refusal modified the store"
chmod "$ROOT_MODE" "$GEN_ROOT"

cp -a "$CORE_BINARY" "$WORK/core.saved"
ln -sfn "$REAL_CORE" "$CORE_BINARY"
[ -L "$CORE_BINARY" ] || fail "symlink fixture was not created"
NEG_BEFORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/adopt-symlink-core.log" adopt --yes)
cat "$WORK/adopt-symlink-core.log"
[ "$STATUS" -ne 0 ] || fail "adoption accepted a symlinked canonical artifact"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "symlink refusal modified the store"
replace_binary "$WORK/core.saved" "$CORE_BINARY"

mv "$CORE_BINARY" "$WORK/core.missing"
NEG_BEFORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/adopt-missing-core.log" adopt --yes)
cat "$WORK/adopt-missing-core.log"
[ "$STATUS" -ne 0 ] || fail "adoption accepted a missing mandatory Core binary"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "missing-Core refusal modified the store"
mv "$WORK/core.missing" "$CORE_BINARY"

mv "$INSTALLED_CLI" "$WORK/cli.missing"
NEG_BEFORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/adopt-missing-cli.log" adopt --yes)
cat "$WORK/adopt-missing-cli.log"
[ "$STATUS" -ne 0 ] || fail "adoption accepted a missing mandatory CLI binary"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "missing-CLI refusal modified the store"
mv "$WORK/cli.missing" "$INSTALLED_CLI"

echo "--- 7. 负例：匹配的未认证墓碑不得为普通暂存认证安装态 ---"
cp -a "$GEN_ROOT/state.json" "$WORK/state.bak"
rm -f "$GEN_ROOT/generations/$TOMBSTONE_OLD/.migrated-untrusted"
# 模拟旧版本已迁移、墓碑缺标记的存量现场：live 槽位不匹配，只有墓碑字节匹配。
state_write_slots "$GEN_ROOT" "$PREVIOUS_OLD" "-" "-"
rm -rf "$WORK/live-active-hold"
mv "$GEN_ROOT/generations/$ACTIVE_NOW" "$WORK/live-active-hold"
NEG_BEFORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/restage-tombstone.log" \
    as_user sudo -n "$CLI_SOURCE" install --system --yes)
cat "$WORK/restage-tombstone.log"
[ "$STATUS" -ne 0 ] || fail "restage admitted the install through an unauthenticated tombstone"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "tombstone restage modified the store"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "tombstone restage created a pending generation"
STATUS=$(capture_status "$WORK/restart-tombstone.log" \
    as_user sudo -n "$CLI_SOURCE" restart --system --yes)
cat "$WORK/restart-tombstone.log"
[ "$STATUS" -ne 0 ] || fail "restart skew admitted the install through an unauthenticated tombstone"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "tombstone restart modified the store"
[ "$SERVICE_BEFORE" = "$(service_fingerprint)" ] || fail "tombstone restart stopped or restarted the runtime"
mv "$WORK/live-active-hold" "$GEN_ROOT/generations/$ACTIVE_NOW"
cp -a "$WORK/state.bak" "$GEN_ROOT/state.json"
# 墓碑保持未标记：它不能成为采纳拒绝理由，下一步的成功采纳必须容忍它。

echo "--- 8. 显式采纳：无旧 manifest 匹配、二进制部分替换、Geo 变化/缺失也接受 ---"
cp "$CORE_BINARY" "$WORK/core.drift"
printf '\0' >>"$WORK/core.drift"
chmod 755 "$WORK/core.drift"
replace_binary "$WORK/core.drift" "$CORE_BINARY"
cp -a "$LEGACY_ROOT/geoip.metadb" "$WORK/geoip.saved"
cp -a "$LEGACY_ROOT/GeoSite.dat" "$WORK/gesite.saved"
cp -a "$CONFIG_DIR/geoip.metadb" "$WORK/config-geoip.saved"
cp -a "$CONFIG_DIR/GeoSite.dat" "$WORK/config-gesite.saved"
printf '\0' >>"$LEGACY_ROOT/geoip.metadb"
printf '\0' >>"$CONFIG_DIR/geoip.metadb"
rm -f "$LEGACY_ROOT/GeoSite.dat" "$CONFIG_DIR/GeoSite.dat"
[ ! -e "$LEGACY_ROOT/Country.mmdb" ] || fail "Country.mmdb fixture unexpectedly exists"

CLI_CURRENT_SHA=$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)
CORE_CURRENT_SHA=$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)
GEOIP_CURRENT_SHA=$(sha256sum "$LEGACY_ROOT/geoip.metadb" | cut -d' ' -f1)
EVIDENCE_BEFORE=$(evidence_fingerprint "$ACTIVE_NOW" "$PREVIOUS_OLD" "$TOMBSTONE_OLD")
STORE_BEFORE=$(store_fingerprint)
INSTALLED_CLI_INODE=$(stat -c '%i' "$INSTALLED_CLI")
CORE_INODE=$(stat -c '%i' "$CORE_BINARY")

run_expect_success "$WORK/adopt.log" adopt --yes
cat "$WORK/adopt.log"
for pattern in "$INSTALLED_CLI" "$CORE_BINARY" \
    geoip.metadb GeoSite.dat Country.mmdb \
    "$CLI_CURRENT_SHA" "$CORE_CURRENT_SHA" "$GEOIP_CURRENT_SHA"; do
    grep -Fq -- "$pattern" "$WORK/adopt.log" || fail "adoption output lacks $pattern"
done
grep -Eqi 'missing|缺失' "$WORK/adopt.log" || fail "adoption output does not mark missing artifacts"
grep -Eqi '⚠|warning|警告' "$WORK/adopt.log" || fail "adoption output carries no warning"
grep -Eqi '不证明|无法证明|not.*prove|does not prove' "$WORK/adopt.log" || \
    fail "adoption warning does not state that history/provenance is unproven"
grep -Eqi '历史|兼容|健康|history|compat|health' "$WORK/adopt.log" || \
    fail "adoption warning does not mention history/compatibility/health"

[ "$STORE_BEFORE" != "$(store_fingerprint)" ] || fail "adoption did not update generation state"
[ -z "$(state_of "$GEN_ROOT" active)" ] || fail "adoption left active non-empty"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "adoption left pending non-empty"
B_NEW=$(state_of "$GEN_ROOT" previous)
[ -n "$B_NEW" ] || fail "adoption established no previous baseline"
[ "$B_NEW" != "$ACTIVE_NOW" ] || fail "adoption reused the migrated live candidate as B"
assert_adopted_baseline "$GEN_ROOT" "$B_NEW" "$CLI_CURRENT_SHA" "$CORE_CURRENT_SHA" \
    "$GEOIP_CURRENT_SHA" "-" "GeoSite.dat,Country.mmdb" "$ACTIVE_NOW" "$PREVIOUS_OLD"
[ "$EVIDENCE_BEFORE" = "$(evidence_fingerprint "$ACTIVE_NOW" "$PREVIOUS_OLD" "$TOMBSTONE_OLD")" ] || \
    fail "adoption rewrote migrated live/withdrawn evidence"
[ "$LEGACY_BEFORE" = "$(legacy_fingerprint)" ] || fail "adoption modified the legacy originals"
[ "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" = "$CLI_CURRENT_SHA" ] || fail "adoption replaced the installed CLI"
[ "$(stat -c '%i' "$INSTALLED_CLI")" = "$INSTALLED_CLI_INODE" ] || fail "adoption rewrote the installed CLI inode"
[ "$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)" = "$CORE_CURRENT_SHA" ] || fail "adoption replaced the installed Core"
[ "$(stat -c '%i' "$CORE_BINARY")" = "$CORE_INODE" ] || fail "adoption rewrote the installed Core inode"
[ "$(sha256sum "$CONFIG_DIR/config.yaml" | cut -d' ' -f1)" = "$CONFIG_SHA" ] || fail "adoption modified config.yaml"
[ "$SERVICE_BEFORE" = "$(service_fingerprint)" ] || fail "adoption stopped or restarted the runtime"
systemctl is-active --quiet mihomo || fail "adoption left the daemon inactive"
api_responding || fail "adoption left the Core API unresponsive"

echo "--- 9. 重复采纳同 B 幂等；采纳后漂移拒绝再次采纳 ---"
GENS_BEFORE=$(generation_names)
STATE_BEFORE=$(state_of "$GEN_ROOT" previous)
run_expect_success "$WORK/adopt-repeat.log" adopt --yes
cat "$WORK/adopt-repeat.log"
[ "$(state_of "$GEN_ROOT" previous)" = "$B_NEW" ] || fail "repeat adoption changed the baseline"
[ "$(state_of "$GEN_ROOT" previous)" = "$STATE_BEFORE" ] || fail "repeat adoption moved the baseline"
[ "$GENS_BEFORE" = "$(generation_names)" ] || fail "repeat adoption created another baseline"
[ -z "$(state_of "$GEN_ROOT" active)" ] || fail "repeat adoption set active"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "repeat adoption set pending"

cp -a "$INSTALLED_CLI" "$WORK/cli.saved"
cp "$INSTALLED_CLI" "$WORK/cli.drift"
printf '\0' >>"$WORK/cli.drift"
chmod 755 "$WORK/cli.drift"
replace_binary "$WORK/cli.drift" "$INSTALLED_CLI"
NEG_BEFORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/adopt-after-drift.log" adopt --yes)
cat "$WORK/adopt-after-drift.log"
[ "$STATUS" -ne 0 ] || fail "adoption accepted bytes that drifted after the trusted baseline"
[ "$NEG_BEFORE" = "$(store_fingerprint)" ] || fail "post-adoption drift refusal modified the store"
replace_binary "$WORK/cli.saved" "$INSTALLED_CLI"

# 测试分支主动恢复健康字节，相对已批准 B 属漂移，产品必须继续拒绝；
# 不能为凑升级成功而把这次人工替换洗白。之后仅在隔离夹具中重建全 U
# 起点，再采纳健康快照，分别验证风险采纳与正常升级两条旅程。
replace_binary "$WORK/core.saved" "$CORE_BINARY"
cp -a "$WORK/geoip.saved" "$LEGACY_ROOT/geoip.metadb"
cp -a "$WORK/gesite.saved" "$LEGACY_ROOT/GeoSite.dat"
cp -a "$WORK/config-geoip.saved" "$CONFIG_DIR/geoip.metadb"
cp -a "$WORK/config-gesite.saved" "$CONFIG_DIR/GeoSite.dat"
CORE_HEALTHY_SHA=$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)
[ "$CORE_HEALTHY_SHA" != "$CORE_CURRENT_SHA" ] || fail "fixture did not restore the healthy Core"
DRIFTED_STORE=$(store_fingerprint)
STATUS=$(capture_status "$WORK/install-after-drift.log" \
    as_user sudo -n "$CLI_SOURCE" install --system --yes)
[ "$STATUS" -ne 0 ] || fail "ordinary install recertified a drifted adopted snapshot"
[ "$DRIFTED_STORE" = "$(store_fingerprint)" ] || fail "drifted install modified generation slots"
rm -rf "$GEN_ROOT"
cp -a "$WORK/store-post-migration" "$GEN_ROOT"
run_expect_success "$WORK/adopt-healthy.log" adopt --yes
B_NEW=$(state_of "$GEN_ROOT" previous)
assert_adopted_baseline "$GEN_ROOT" "$B_NEW" "$CLI_CURRENT_SHA" "$CORE_HEALTHY_SHA" \
    "$(sha256sum "$LEGACY_ROOT/geoip.metadb" | cut -d' ' -f1)" "-" "Country.mmdb" "$ACTIVE_NOW" "$PREVIOUS_OLD"
CORE_INODE=$(stat -c '%i' "$CORE_BINARY")
STORE_AFTER_ADOPT=$(store_fingerprint)
SERVICE_AFTER_ADOPT=$(service_fingerprint)
RUNTIME_AFTER_ADOPT=$(runtime_fingerprint)

echo "--- 10. 已采纳恢复态：restart skew 不得自动备候选，只允许显式 install ---"
STATUS=$(capture_status "$WORK/restart-recovery-refuse.log" \
    as_user sudo -n "$CLI_SOURCE" restart --system --yes)
cat "$WORK/restart-recovery-refuse.log"
[ "$STATUS" -ne 0 ] || fail "restart auto-prepared a candidate in the adopted recovery state"
grep -qF -- 'install --system' "$WORK/restart-recovery-refuse.log" || \
    fail "recovery refusal does not point at explicit install"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "recovery restart staged a pending candidate"
[ "$STORE_AFTER_ADOPT" = "$(store_fingerprint)" ] || fail "recovery restart modified the store"
[ "$SERVICE_AFTER_ADOPT" = "$(service_fingerprint)" ] || fail "recovery restart stopped or restarted the runtime"
[ "$RUNTIME_AFTER_ADOPT" = "$(runtime_fingerprint)" ] || fail "recovery restart touched install bytes/config"

echo "--- 11. 显式 install 绑定 B（不重新捕获现场）→ restart 应用 → 真 Core/API/选点回放 ---"
run_expect_success "$WORK/recovery-install.log" \
    as_user sudo -n "$CLI_SOURCE" install --system --yes
cat "$WORK/recovery-install.log"
[ -z "$(state_of "$GEN_ROOT" active)" ] || fail "recovery install set active before apply"
C_PENDING=$(state_of "$GEN_ROOT" pending)
[ -n "$C_PENDING" ] || fail "recovery install prepared no candidate"
[ "$(state_of "$GEN_ROOT" previous)" = "$B_NEW" ] || fail "recovery install did not keep B as previous"
[ "$(manifest_baseline "$GEN_ROOT" "$C_PENDING")" = "$B_NEW" ] || \
    fail "recovery candidate is not bound to the adopted baseline B"
[ "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" = "$CLI_CURRENT_SHA" ] || \
    fail "recovery install replaced the installed CLI"
[ "$SERVICE_AFTER_ADOPT" = "$(service_fingerprint)" ] || fail "recovery install stopped or restarted the runtime"
[ "$RUNTIME_AFTER_ADOPT" = "$(runtime_fingerprint)" ] || fail "recovery install touched install bytes/config"

OLD_CORE_PID=$(core_pid_value)
START_EPOCH=$(date +%s)
run_expect_success "$WORK/final-restart.log" \
    as_user sudo -n "$CLI_SOURCE" restart --system --yes
cat "$WORK/final-restart.log"
wait_service_active || fail "daemon service is not active after the final restart"
wait_api_055 || fail "real Core API did not recover after the final restart"
[ -z "$(state_of "$GEN_ROOT" pending)" ] || fail "final restart left a pending generation"
FINAL_ACTIVE=$(state_of "$GEN_ROOT" active)
[ -n "$FINAL_ACTIVE" ] || fail "final restart left no active generation"
[ "$FINAL_ACTIVE" = "$C_PENDING" ] || fail "final restart did not commit the explicit candidate"
[ "$(state_of "$GEN_ROOT" previous)" = "$B_NEW" ] || fail "final restart did not preserve B as previous"
[ ! -e "$GEN_ROOT/generations/$FINAL_ACTIVE/.migrated-untrusted" ] || \
    fail "final active generation is untrusted"
CLI_SOURCE_SHA=$(sha256sum "$CLI_SOURCE" | cut -d' ' -f1)
[ "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" = "$CLI_SOURCE_SHA" ] || \
    fail "final restart did not apply the running CLI bytes"
read -r FINAL_DAEMON FINAL_CORE <<<"$(manifest_hashes "$GEN_ROOT" "$FINAL_ACTIVE")"
[ "$FINAL_DAEMON" = "$CLI_SOURCE_SHA" ] || fail "final active daemon hash != installed/running CLI"
[ "$FINAL_CORE" = "$CORE_HEALTHY_SHA" ] || fail "final active Core hash != restored Core bytes"
[ "$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)" = "$CORE_HEALTHY_SHA" ] || fail "final restart replaced Core bytes"
[ "$(stat -c '%i' "$CORE_BINARY")" = "$CORE_INODE" ] || fail "final restart rewrote the unchanged Core inode"
[ "$(sha256sum "$CONFIG_DIR/config.yaml" | cut -d' ' -f1)" = "$CONFIG_SHA" ] || \
    fail "final restart modified config.yaml"
NEW_CORE_PID=$(wait_for_new_core "$OLD_CORE_PID") || fail "final restart left no new managed Core"
assert_live_core_identity "$NEW_CORE_PID"
assert_start_causality "$NEW_CORE_PID" "$START_EPOCH"
wait_proxy_now "$SELECT_GROUP" "$SELECT_NODE" || fail "selection was not replayed after the final restart"
[ "$LEGACY_BEFORE" = "$(legacy_fingerprint)" ] || fail "final restart modified the legacy originals"
as_user "$CLI_SOURCE" status --system >"$WORK/final-status.log" 2>&1 || {
    cat "$WORK/final-status.log" >&2
    fail "status --system failed after the final restart"
}
api_responding || fail "Core API is not serving after the final restart"
echo "final: active=$FINAL_ACTIVE previous=$B_NEW daemon=$FINAL_DAEMON core=$FINAL_CORE replay=$SELECT_NODE"

echo "✅ PASS: migration adoption (frozen contract B, real Core, evidence preserved)"
