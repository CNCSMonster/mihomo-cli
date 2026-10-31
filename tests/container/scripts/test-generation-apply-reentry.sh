#!/bin/bash
# test-generation-apply-reentry (，裁决 D)
# 确定性孤儿现场（daemon 消失 + PID 记录缺失 + 受管 Core 存活）→ 真进程退出后由新
# 进程续跑管线：重入必须证明旧进程退出（pidfd）并让新进程绑定目标 artifact/revision；
# 中断残留 pending → 重入收敛提交；提交后重入不得假成功。
set -euo pipefail
source /tests/scripts/lib-generation-e2e.sh

echo "=== generation apply reentry ==="
require_root_container
reset_instance
install_system

# 裁决 D 故障现场：daemon 消失 + PID 记录缺失 + 受管 Core 存活并响应 API。
spawn_managed_orphan_core
OLD_CORE_PID="$ORPHAN_PID"

# 只读分类：status/doctor 必须认出孤儿但不停止、不清理（裁决 D-3）。
as_user sudo -n "$CLI_SOURCE" status --system >/tmp/orphan-status.log 2>&1 || true
grep -q 'Orphan Core:.*managed pid' /tmp/orphan-status.log || {
    cat /tmp/orphan-status.log >&2
    fail "status did not classify the managed orphan read-only"
}
as_user sudo -n "$CLI_SOURCE" doctor --system >/tmp/orphan-doctor.log 2>&1 || true
grep -q '孤儿 Core' /tmp/orphan-doctor.log || {
    cat /tmp/orphan-doctor.log >&2
    fail "doctor did not classify the orphan read-only"
}
kill -0 "$ORPHAN_PID" 2>/dev/null || fail "status/doctor must not stop the orphan"
[ -S "$API_SOCKET" ] || fail "status/doctor must not remove the orphan socket"

# 制造 skew，让 restart 自行暂存一个 pending 代际。
inject_skew_only

# 后台启动管线，观察到 pending 落盘后立刻 SIGKILL（真进程退出，不走清理路径）。
# 直接以 root 调用 CLI，避免 sudo 分层导致 kill 打偏。
"$CLI_SOURCE" restart --system --yes >/tmp/reentry-first.log 2>&1 &
PIPELINE_PID=$!

OBSERVED_PENDING=""
for _ in $(seq 1 600); do
    OBSERVED_PENDING=$(state_value pending)
    if [ -n "$OBSERVED_PENDING" ]; then
        break
    fi
    if ! kill -0 "$PIPELINE_PID" 2>/dev/null; then
        break
    fi
    sleep 0.01
done

BRANCH=""
if [ -n "$OBSERVED_PENDING" ] && kill -0 "$PIPELINE_PID" 2>/dev/null; then
    kill -9 "$PIPELINE_PID" 2>/dev/null || true
    wait "$PIPELINE_PID" 2>/dev/null || true
    BRANCH="crash-window"
    echo "killed pipeline mid-apply with pending=$OBSERVED_PENDING"
else
    wait "$PIPELINE_PID" 2>/dev/null || true
    BRANCH="post-commit"
    echo "pipeline finished before the kill window; exercising the committed reentry path"
fi

REENTRY_START=$(date +%s)
# 新进程重入：必须收敛（不得静默假成功）。
as_user sudo -n "$CLI_SOURCE" restart --system --yes >/tmp/reentry-second.log 2>&1 || {
    cat /tmp/reentry-second.log >&2
    fail "reentry restart failed"
}
cat /tmp/reentry-second.log

PENDING_AFTER=$(state_value pending)
ACTIVE_AFTER=$(state_value active)
echo "after reentry: active=$ACTIVE_AFTER pending='$PENDING_AFTER' branch=$BRANCH"
[ -z "$PENDING_AFTER" ] || fail "pending was not consumed by the reentry"
[ -n "$ACTIVE_AFTER" ] || fail "active generation missing after reentry"

if [ "$BRANCH" = "crash-window" ]; then
    # 中断的 pending 必须由新进程续跑并提交。
    [ "$ACTIVE_AFTER" = "$OBSERVED_PENDING" ] || \
        fail "reentry did not commit the interrupted pending ($OBSERVED_PENDING → $ACTIVE_AFTER)"
    grep -Eq 'Successfully upgraded|已从绑定基线|补偿已收敛' /tmp/reentry-second.log || \
        fail "reentry produced no explicit outcome line"
else
    # 提交后崩溃重入：必须显式复验（verify_committed_generation），不得静默假成功。
    grep -q '已提交代际' /tmp/reentry-second.log || \
        fail "post-commit reentry did not verify/report the committed generation"
fi

assert_service_running
# 重入证据：旧进程退出（pidfd）、新进程绑定目标 artifact/revision、启动因果。
assert_exit_evidence "$OLD_CORE_PID" /tmp/reentry-second.log /tmp/reentry-first.log
NEW_CORE_PID=$(wait_for_new_core "$OLD_CORE_PID") || fail "reentry left no new managed core"
assert_live_core_identity "$NEW_CORE_PID"
assert_start_causality "$NEW_CORE_PID" "$REENTRY_START"
assert_target_artifact "$ACTIVE_AFTER"

echo "✅ PASS: reentry convergence (branch=$BRANCH, orphan exit proven, pending cleared, service running)"
