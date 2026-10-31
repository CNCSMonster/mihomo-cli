# 升级 mihomo-cli

> 读者：使用者
> 回答：如何把用户工具与服务二进制一起升级、失败如何回退、什么时候需要 sudo
> 权威：本文为用户手册一部分

标准升级流程（Linux system 模式）：

```bash
# 1. 更新用户工具（标准位置，无需 sudo）
cargo install --path .          # 或直接覆盖 ~/.cargo/bin/mihomo-cli

# 2. 升级服务（一条命令，会提示输入一次 sudo 密码）
mihomo-cli restart --system
```

第 2 步是关键：systemd 服务运行的是 `/usr/local/bin` 下的服务二进制，仅更新用户工具不会影响正在运行的 daemon。`mihomo-cli restart --system` 检测到「当前 CLI 与服务二进制不一致」时会自动：

1. 将新 CLI 与现有 Core 打包为待应用版本（原子暂存）；
2. 安全停机 → 替换服务二进制 → 重启 daemon 服务；
3. 恢复运行状态并回放你的选点（selection）等持久化意图。

验证升级结果：

```bash
mihomo-cli version   # 应显示新版本号
mihomo-cli status    # Health 应恢复正常，不再提示 degraded
```

升级失败时的行为（system 模式）：若停机后的替换或启动证明失败，`mihomo-cli` 会按升级前捕获的**基线代际**把 daemon/Core/资源文件逐项换回并重启到基线，然后报告「已回退、升级未生效」，同时把这次待应用版本标记为 `<代际>.withdrawn`（墓碑）——**不会**自动重试，重试必须显式执行 `mihomo-cli install --system`。

若回退本身也失败（例如新候选与基线的文件同源、两者都起不来），输出：

```
⚠ upgrade failed and rollback failed
Pending <generation> retained; runtime has not been restored.
Fix: mihomo-cli install --system --discard-pending=<generation>
```

此时运行态未被恢复、待应用代际仍然保留。按提示执行撤销：

```bash
mihomo-cli install --system --discard-pending=<generation> --yes
```

撤销**只处理元数据**：不停启服务、不替换任何安装文件，把该候选标记为 `.withdrawn`（字节保留为证据），并把槽位收敛为「无 active、无 pending、回退基线仍为 previous」。撤销后处于**恢复态**：自动同步（skew helper）不会重新暂存；准备新候选请显式执行 `mihomo-cli install --system`——新候选继续绑定的仍是**已认证的旧基线**（不会把当前安装态当成新基线）。候选准备完成后按提示执行 `mihomo-cli restart --system`；候选无法准备时保持恢复态并非零退出，不会把撤销报告成"服务已恢复"。

升级结束时打印的 `core 启动/重启请求 …` 行是本轮实际启动请求的绑定 ID，可用于对照 daemon 报告的选点回放结论。

只有在安装态与已知代际（已应用版本、待应用版本或升级基线）逐字节一致时才允许重新暂存升级；出现「混合态」提示说明文件被部分替换，需要先人工核对安装文件。

停机证明：`mihomo-cli` 只在**观察到受管 Core 进程确实退出**（子进程 wait，或非子进程的内核句柄退出观察）后才继续替换/清理；信号发送成功不算证明。若无法观察（权限不足、身份不可证、平台不支持），升级会中止并保留全部运行态证据，不会执行任何破坏性操作。当 Core API 端点仍有进程响应但无法证明其归属时，同样拒绝继续，也不建议手工 kill 进程或删除 socket。

旧版本遗留的运行状态（v9 TUN 事务迁移后留下的合成证据）会在同一条 `restart --system` 中自动收敛，**无需预先手工停止** Core。执行前会明确说明影响：将停止当前受管 Core、保留你的配置并重建运行状态，代理连接会暂时中断，TUN 可能关闭；交互终端先确认，`--yes` 只跳过询问，管理员授权仍单独要求。若当前受管 Core 的归属或它的退出无法证明（例如 pid 记录与进程身份不一致、退出不可观察），升级会中止并保留全部运行态证据，不做任何破坏性清理。

`mihomo-cli status` / `mihomo-cli doctor` 会发现「daemon 已停止但仍有受管 Core 在响应 API」的孤儿现场，并只读报告其归属分类（可证：身份摘要；不可证：原因）。两者都不停止进程、不清理文件，也不提供手工 kill 指令；下一次显式 `restart`/`install` 会在授权范围内接管孤儿，并以进程退出证明为前提继续。

## sudo 什么时候需要？

日常操作（选点、查状态、改配置、订阅管理、TUN 开关等）**不需要** sudo——mihomo-cli 在需要特权时会自行提权并提示你确认。真正需要 root 的只有系统服务层的三件事：

- 写入 `/usr/local/bin/mihomo-cli`（服务二进制）；
- 写入 `/etc/systemd/system/mihomo.service`（服务单元）；
- systemd 服务管理（安装/卸载/重启服务本身）。

这些均由 `mihomo-cli install --system`、`mihomo-cli restart --system` 等命令内部完成，无需手动执行 `sudo systemctl ...`。

**业务操作授权与权限判定是两件事**：只读命令（`status`、`doctor`、`tun status`）不判断权限、不提权、不写入、不触恢复；`restart`/`start`/`stop`/`tun on`/`tun off`/`uninstall`/`install` 在需要检查受保护的运行状态时，会为**你请求的这次操作**申请一次管理员授权，提示形如“为执行你请求的 restart --system，需要管理员授权检查受保护的运行状态，并按原命令范围恢复事务或应用更新。当前尚未判定权限异常；重启可能暂时中断连接。”——它**不表示**检测到权限问题，也不会顺带修改任何文件属主或权限。授权按原命令参数重入且每命令至多一次；已是管理员却仍读不到受保护状态时直接结构化失败（不会递归 sudo）。无终端时不会等待交互输入，OS 认证不成立即非零退出——`--yes` 只确认本命令已声明的业务步骤，不能替代管理员授权，也不会在未确认时触发运行态重置。

系统状态目录（Linux：`/var/lib/mihomo-cli`，按约定为 `0770 root:mihomo`）与用户配置树（`~/.config/mihomo` 及其子目录）只在进入生命周期命令前做只读探测。**探测不到不等于需要修复**：当前用户不在 `mihomo` 组或目录不可读时，CLI 此时不会提权，只提示"无法以当前身份校验系统状态目录"或"无法校验配置目录属主"后继续原命令；只有确证的属主或 mode 漂移才会走上述 sudo 修复流程。`cache.db` 同理：权限判据采信 `0600` 与 `0640`（对齐 Linux 下真实 Core 上游 bbolt 落盘默认值）白名单，合格状态不触发无谓的 chmod；读不到属于"无法判定"，不会被误判为权限漂移，更不会因此提权。

代际信任根（`/var/lib/mihomo-cli-generations`，约定 `0750 root:root`）同理：非特权用户对 system 实例运行 `status`/`doctor` 读不动它是正常权限边界，CLI 既不会把它当成「没有待应用更新」，也不会因此提权——`status` 在 stderr 提示无法判定、`doctor` 显示 ❓、`status --json` 报 `pending_generation.state = "unknown"`。需要确认或应用更新时执行 `mihomo-cli restart --system`：它会为这次操作申请管理员授权，以特权身份检查并应用。
