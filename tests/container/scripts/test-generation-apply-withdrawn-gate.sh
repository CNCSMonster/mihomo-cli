#!/bin/bash
# test-generation-apply-withdrawn-gate (，裁决 A1 + D)
# 确定性孤儿现场（daemon 消失 + PID 记录缺失 + 受管 Core 持续响应 API）→ 失败候选
# 在授权停机时给出 pidfd 退出证明 → 显式 --discard-pending 产出墓碑 → 同 sha 墓碑
# 阻断自动重暂存（期间零停止/零启动/零替换）→ 显式 install 重试成功、完成目标启动
# 证明后退休墓碑。
set -euo pipefail
source /tests/scripts/lib-generation-e2e.sh

echo "=== generation apply withdrawn gate ==="
require_root_container
reset_instance
install_system

# 裁决 D 故障现场：daemon 消失 + PID 记录缺失 + 受管 Core 持续响应 API。
spawn_managed_orphan_core
OLD_CORE_PID="$ORPHAN_PID"

# 1) 制造一个失败候选，并显式撤销（墓碑 = 仅撤销证明）。
inject_always_failing_start
set +e
as_user sudo -n "$CLI_SOURCE" restart --system --yes >/tmp/gate-fail.log 2>&1
set -e
PENDING=$(state_value pending)
BASELINE=$(state_value previous)
[ -n "$PENDING" ] || {
    cat /tmp/gate-fail.log >&2
    fail "failed upgrade did not retain a pending generation"
}
grep -q 'Successfully upgraded' /tmp/gate-fail.log && \
    fail "the old orphan API was borrowed for a false success"
assert_exit_evidence "$OLD_CORE_PID" /tmp/gate-fail.log
restore_pristine_binaries
as_user sudo -n "$CLI_SOURCE" install --system --discard-pending="$PENDING" --yes >/tmp/gate-discard.log 2>&1 || {
    cat /tmp/gate-discard.log >&2
    fail "explicit discard failed"
}
TOMBSTONES=$(withdrawn_count)
echo "tombstones after explicit discard: $TOMBSTONES"
[ "$TOMBSTONES" -ge 1 ] || fail "discard did not publish a tombstone"
[ "$(state_value previous)" = "$BASELINE" ] || fail "discard dropped the baseline binding"

# 2) 制造 skew：同 sha 墓碑必须机械阻断自动重暂存。
inject_skew_only
BLOCKED_SNAPSHOT=$(runtime_snapshot)
set +e
as_user sudo -n "$CLI_SOURCE" restart --system --yes >/tmp/gate-block.log 2>&1
BLOCK_STATUS=$?
set -e
cat /tmp/gate-block.log
[ "$BLOCK_STATUS" -ne 0 ] || fail "skew helper auto re-staging was not blocked by the tombstone"
grep -q '墓碑' /tmp/gate-block.log || fail "block message does not mention the tombstone"
grep -q 'install --system' /tmp/gate-block.log || fail "block message lacks the explicit install guidance"
# 墓碑阻断期间：零停止、零启动、零替换。
[ "$BLOCKED_SNAPSHOT" = "$(runtime_snapshot)" ] || \
    fail "the tombstone gate stopped, started or replaced runtime state"

# 3) 显式重试：去掉 skew（恢复 CLI = 失败候选的 daemon 字节，不替换 core 现场）
#    → install 暂存候选 C（绑定 B）→ restart 提交并退休墓碑。
restore_pristine_cli_only
clear_failure_injection
# 恢复态运行时一致性：磁盘已回到 pristine，让运行中的 daemon 也切换到该字节；
# 否则「pending 与磁盘一致 → P3/P4 跳过」时，P5 会因 daemon 自证 revision 陈旧而
# fail-closed（这是产品语义，不是夹具要消除的故障，故夹具补齐一致的运行态）。
systemctl restart mihomo
wait_service_active || fail "daemon service did not restart on the restored binary"
as_user sudo -n "$CLI_SOURCE" install --system --yes >/tmp/gate-install.log 2>&1 || {
    cat /tmp/gate-install.log >&2
    fail "explicit install retry failed"
}
[ "$(state_value previous)" = "$BASELINE" ] || fail "retry candidate did not bind the authenticated baseline"
RETRY_START=$(date +%s)
as_user sudo -n "$CLI_SOURCE" restart --system --yes >/tmp/gate-retry.log 2>&1 || {
    cat /tmp/gate-retry.log >&2
    fail "explicit retry restart failed"
}
cat /tmp/gate-retry.log

[ "$(withdrawn_count)" -eq 0 ] || fail "successful retry did not retire the tombstone"
[ -z "$(state_value pending)" ] || fail "pending not consumed after the successful retry"
assert_service_running
NEW_CORE_PID=$(wait_for_new_core "$OLD_CORE_PID") || fail "explicit retry left no new managed core"
assert_live_core_identity "$NEW_CORE_PID"
assert_start_causality "$NEW_CORE_PID" "$RETRY_START"
assert_target_artifact "$(state_value active)"

echo "✅ PASS: withdrawn gate (blocked + guidance, zero runtime mutation) and tombstone retirement"
