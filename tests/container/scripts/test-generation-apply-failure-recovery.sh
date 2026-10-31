#!/bin/bash
# test-generation-apply-failure-recovery (，裁决 A1 + D)
# 确定性孤儿现场（daemon 消失 + PID 记录缺失 + 旧 Core 持续响应 API）→ 毒化 pending
# （B/P 同源）→ 授权停机必须给出 pidfd 退出证明、失败面不假成功（不借旧 API）→
# 显式 --discard-pending 撤销元数据（保留基线 B、零停启、零替换）→ 恢复态 install
# 准备候选 C → restart 提交并留存目标 artifact/启动因果证据。
set -euo pipefail
source /tests/scripts/lib-generation-e2e.sh

echo "=== generation apply failure recovery (explicit discard) ==="
require_root_container
reset_instance
install_system

# 裁决 D 故障现场：daemon 消失 + PID 记录缺失 + 受管 Core 持续响应 API。
# 旧 API 就在端点上应答——新 core 启动失败时不得借它报成功。
spawn_managed_orphan_core
OLD_CORE_PID="$ORPHAN_PID"

# 注入必然失败的启动（B 与 P 的 core 同源：补偿也无法起动）。
inject_always_failing_start

set +e
as_user sudo -n "$CLI_SOURCE" restart --system --yes >/tmp/restart.log 2>&1
RESTART_STATUS=$?
set -e
cat /tmp/restart.log
[ "$RESTART_STATUS" -ne 0 ] || fail "a failed upgrade must not report success"
grep -q 'upgrade failed and rollback failed' /tmp/restart.log || \
    fail "failure surface missing 'upgrade failed and rollback failed'"
grep -q 'Pending .* retained; runtime has not been restored' /tmp/restart.log || \
    fail "failure surface missing the retained-pending statement"
grep -q -- '--discard-pending=' /tmp/restart.log || \
    fail "failure surface missing the discard Fix guidance"
grep -q 'Successfully upgraded' /tmp/restart.log && \
    fail "the old orphan API was borrowed for a false success"
# 停机证明：旧孤儿只有在 pidfd 观察到退出后才允许替换/启动。
assert_exit_evidence "$OLD_CORE_PID" /tmp/restart.log

PENDING=$(state_value pending)
BASELINE=$(state_value previous)
echo "after failed apply: pending=$PENDING previous=$BASELINE"
[ -n "$PENDING" ] || fail "failed apply must not consume the pending slot"
[ -n "$BASELINE" ] || fail "baseline binding missing after failed apply"
[ "$(withdrawn_count)" -eq 0 ] || fail "failed rollback must not publish a tombstone"

# 默认 install 绝不隐式撤销。
CLI_SHA_BEFORE=$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)
CORE_SHA_BEFORE=$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)
set +e
as_user sudo -n "$CLI_SOURCE" install --system --yes >/tmp/default-install.log 2>&1
DEFAULT_STATUS=$?
set -e
[ "$DEFAULT_STATUS" -ne 0 ] || fail "default install must not silently discard a pending generation"
[ "$(state_value pending)" = "$PENDING" ] || fail "default install changed the pending slot"

# 显式撤销：仅元数据（零停止、零启动、零替换）。
DISCARD_SNAPSHOT=$(runtime_snapshot)
as_user sudo -n "$CLI_SOURCE" install --system --discard-pending="$PENDING" --yes >/tmp/discard.log 2>&1 || {
    cat /tmp/discard.log >&2
    fail "explicit --discard-pending failed"
}
cat /tmp/discard.log
[ -z "$(state_value pending)" ] || fail "discard did not clear pending"
[ -z "$(state_value active)" ] || fail "discard must not claim an active runtime"
[ "$(state_value previous)" = "$BASELINE" ] || fail "discard must keep the authenticated baseline as previous"
[ "$(withdrawn_count)" -ge 1 ] || fail "discard must publish a <P>.withdrawn tombstone"
[ "$CLI_SHA_BEFORE" = "$(sha256sum "$INSTALLED_CLI" | cut -d' ' -f1)" ] || fail "discard replaced the installed daemon"
[ "$CORE_SHA_BEFORE" = "$(sha256sum "$CORE_BINARY" | cut -d' ' -f1)" ] || fail "discard replaced the installed core"
[ "$DISCARD_SNAPSHOT" = "$(runtime_snapshot)" ] || fail "discard stopped, started or replaced runtime state"

# 撤销后重入不得再应用 P、不得声称运行态已恢复。
as_user sudo -n "$CLI_SOURCE" restart --system --yes >/tmp/reentry-after-discard.log 2>&1 || true
[ -z "$(state_value pending)" ] || fail "post-discard restart re-applied the revoked generation"
grep -q '已从绑定基线' /tmp/reentry-after-discard.log && \
    fail "post-discard restart falsely claimed the runtime was restored"

# 恢复态：install 准备候选 C（绑定已认证基线 B，不捕获混合安装态）。
# 注意：不做任何人工文件替换（安装态仍是失败候选的现场）；只解除夹具自身的
# 启动失败注入（/tmp 标记文件，不属于安装文件）。
clear_failure_injection
as_user sudo -n "$CLI_SOURCE" install --system --yes >/tmp/prepare-c.log 2>&1 || {
    cat /tmp/prepare-c.log >&2
    fail "recovery-mode install (candidate C) failed"
}
cat /tmp/prepare-c.log
CANDIDATE=$(state_value pending)
[ -n "$CANDIDATE" ] || fail "recovery install did not prepare a candidate"
[ "$(state_value previous)" = "$BASELINE" ] || fail "candidate C did not bind the authenticated baseline B"

APPLY_C_START=$(date +%s)
as_user sudo -n "$CLI_SOURCE" restart --system --yes >/tmp/apply-c.log 2>&1 || {
    cat /tmp/apply-c.log >&2
    fail "applying candidate C failed"
}
cat /tmp/apply-c.log
[ -z "$(state_value pending)" ] || fail "candidate C was not committed"
[ "$(state_value active)" = "$CANDIDATE" ] || fail "candidate C did not become active"
assert_service_running
# 目标启动证明 + 证据留存：新进程身份、目标 artifact 哈希、启动因果。
NEW_CORE_PID=$(wait_for_new_core "$OLD_CORE_PID") || fail "no new managed core after candidate C"
assert_live_core_identity "$NEW_CORE_PID"
assert_start_causality "$NEW_CORE_PID" "$APPLY_C_START"
assert_target_artifact "$CANDIDATE"

echo "✅ PASS: orphan takeover with exit proof → poisoned pending → explicit discard (metadata only) → candidate C → applied"
