# macOS socket 目录修复方案

> 状态: 已确认
> 日期: 2026-07-22

## 1. 现状与问题

### 当前实现

socket 目录按平台硬编码（src/utils.rs:129-146）：

| 平台 | socket 路径 |
|------|-------------|
| Linux | `$XDG_RUNTIME_DIR/mihomo/mihomo.sock`（per-user，正确） |
| macOS | `/tmp/mihomo/mihomo.sock`（全局共享，有缺陷） |
| Windows | named pipe（不涉及） |

该路径由 mihomo-cli 注入 config.yaml 的 `external-controller-unix` 字段（src/config.rs:672），
mihomo core 启动时 bind，CLI 侧按同样逻辑算路径去连接。

### 问题

| 问题 | 说明 |
|------|------|
| **root/user 模式冲突** | `/tmp` 是全局 1777 目录。root 模式（LaunchDaemon）先创建 `/tmp/mihomo`（root 所有，700）后，user 模式 start.sh 的 `chmod 700` 和 mihomo bind 全部 EACCES，且需人工 `sudo chown` 修复（src/service.rs:1451-1467） |
| **多用户冲突** | 同一台 Mac 上两个用户都装 user 模式会互相抢占同一路径 |
| **预占攻击面** | 任何本地用户可抢先创建 `/tmp/mihomo`，DoS 他人首次安装 |
| **清理策略错位** | macOS periodic 脚本清理 `/tmp` 中 3 天未访问的文件，存在"进程在跑但 socket 被清"的边角情况 |
| **语义偏差** | user 级服务的运行时文件不应放全局目录；Linux 侧已遵循 XDG per-user 语义，macOS 侧未对齐 |

## 2. 修复方向

### 2.1 决策：macOS 使用 `{config_dir}/run/`

```
~/.config/mihomo/run/mihomo.sock
```

### 2.2 候选方案对比

| 方案 | 确定性 | 隔离性 | 语义纯度 | 结论 |
|------|--------|--------|----------|------|
| `$TMPDIR/mihomo/`（macOS per-user temp） | ⚠️ 依赖 shell 与 launchd agent 环境一致 | ✅ 天然 per-user | ✅ 标准 runtime 语义 | 备选 |
| `{config_dir}/run/` | ✅ 两进程共享 `$HOME`，任何上下文一致 | ✅ per-user（HOME 隔离） | ⚠️ 持久目录放运行时文件 | **采用** |
| `/tmp/mihomo-$UID/` | ✅ | ⚠️ 仅解决多用户，root/user 冲突仍在 | ✅ | 否决 |

### 2.3 选择理由

1. **跨进程确定性优先**：socket 路径由两个独立进程各自计算——launchd 加载的 mihomo core（读 config.yaml）和 shell 里的 mihomo-cli。`$TMPDIR` 依赖两边环境变量一致，一旦不一致 CLI 无法连接且难以排查；`$HOME` 在任何上下文下确定。
2. **消掉 root/user 冲突**：root 模式的 config dir 与 user 模式不同（`/root/.config` vs `~/.config`），两种模式天然隔离，不再需要 `/tmp` 权限修复逻辑。
3. **实践先例**：Docker Desktop（`~/.docker/run/docker.sock`）等 macOS 应用同样将 socket 放用户目录。
4. **路径长度安全**：macOS unix socket 路径上限 104 字节，新路径远短于此。

### 2.4 各平台最终行为

| 平台 | 修复后 socket 路径 | 变化 |
|------|-------------------|------|
| Linux | `$XDG_RUNTIME_DIR/mihomo/mihomo.sock` | 不变 |
| macOS | `~/.config/mihomo/run/mihomo.sock` | **变更** |
| Windows | `\\.\pipe\mihomo` | 不变 |

## 3. 改动点

| 位置 | 改动 |
|------|------|
| `src/utils.rs` `socket_dir()` | macOS 分支改为 `format!("{}/run", config_dir())` |
| `src/mihomo_api.rs` `socket_path()` | macOS 分支同步修改（该函数独立实现了同样逻辑，应改为复用 `utils::socket_dir()`，消除重复） |
| `src/config.rs` 注入 `external-controller-unix` | 无需改（已使用 `socket_dir()` 拼接），确认所有注入点走统一入口 |
| `src/service.rs` `write_start_script` / `ensure_socket_dir_writable` | start.sh 改为创建 config_dir/run；删除针对 `/tmp/mihomo` 的 sudo chown 修复提示 |
| 迁移逻辑 | install/start 时若检测到旧 `/tmp/mihomo` 且 config.yaml 中注入的是旧路径，重新生成 config.yaml 并提示 restart |

## 4. 兼容性

- **新装用户**：无感，直接生效。
- **存量 macOS 用户**：socket 路径变化后，运行中的 mihomo 仍监听旧路径，CLI 会连不上 → 需要一次 `mihomo-cli restart`（或 stop + start）使 core 加载新 config。升级提示中说明。
- **Linux / Windows**：零影响。

## 5. 后续可选优化（不在本次范围）

- macOS user 模式 restart 路径误用 privileged `launchctl stop/start <Label>`（src/service.rs:388-391），应改为 user 域的 unload/load 或 `kickstart gui/$UID`
- 重复 `start` 幂等性：`launchctl load` 对已加载 job 报错，应先查状态
- `proxy on` 端口回退：`mixed-port` 缺失时尝试 `port`/`socks-port`
