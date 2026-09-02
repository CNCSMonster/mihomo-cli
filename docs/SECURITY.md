# Security Design

> mihomo-cli 安全改进方案 —— 基于竞品调研和威胁模型分析

---

## 1. 竞品安全设计对比

### 1.1 调研对象

| 项目 | 类型 | 平台 | 安全设计特点 |
|------|------|------|-------------|
| **clash-verge-rev** | GUI 桌面应用 | Linux/macOS/Windows | IPC 裸奔，无 peer UID 检查，secret 硬编码默认值 |
| **Proxy-RS (clashtui)** | TUI 终端应用 | Linux/macOS/Windows | Token 认证 + Socket 权限 0o600 + systemd 沙箱 |
| **clashtui (JohanChane)** | TUI 终端应用 | Linux/macOS/Windows | 待补充（与 Proxy-RS 类似） |

### 1.2 安全设计对比表

| 维度 | clash-verge-rev | Proxy-RS | mihomo-cli 当前 | 评价 |
|------|----------------|----------|----------------|------|
| **IPC 认证** | ❌ 硬编码默认值 `"set-your-secret"` | ✅ 64 字符随机 token + 常量时间比较 | ✅ Unix peer UID + per-user token + root-owned authorized-client table；Windows token/ACL | 当前方案按平台收敛 |
| **Socket 权限** | ❌ `0o666`（所有用户可访问） | ✅ `0o600`（仅安装者可访问） | ⚠️ Unix transport 可连接但应用层必须 peer/token 授权；TUN mutation 还要求 root peer | transport 可见不等于命令授权 |
| **Token 文件保护** | ❌ 无保护 | ✅ `0o600` + owner + Windows SDDL | ✅ Unix per-user token `0o600`、root table；Windows 双副本/ACL | 当前方案已定义 |
| **服务沙箱** | ❌ 无 | ✅ systemd 全套硬化 | ✅ Linux 非 root daemon/Core + capabilities；macOS/Windows 使用平台特权服务边界 | 按平台适用 |
| **服务目录加固** | ❌ 无 | ✅ `root:root` + `0o700` + 原子替换 | ✅ 受管路径、owner/mode/no-follow、snapshot root boundary | 当前方案已定义 |
| **SO_PEERCRED** | ❌ 无 | ❌ 无 | ✅ Unix peer UID 是授权材料之一 | 不能只依赖 token |

| **审计日志** | ❌ 无 | ❌ 无 | ❌ 无 | 三者都缺失 |
| **多用户支持** | ❌ 单用户 GUI | ⚠️ 安装者绑定模型 | ✅ per-user core + system daemon | mihomo-cli 架构更成熟 |

### 1.3 结论

- **clash-verge-rev**：安全设计最弱，不适合借鉴
- **Proxy-RS**：安全设计最强，**值得借鉴**（Token 认证 + Socket 权限 + 服务沙箱）
- **mihomo-cli**：架构层面（per-user core + system daemon）比两者都成熟，但 IPC 安全需要补齐

---

## 2. 威胁模型

### 2.1 攻击场景：低权限用户流量劫持

#### 攻击链

```bash
# 1. 攻击者攻破了低权限用户 user-a
# 2. user-a 修改 mihomo-cli 配置，设置危险节点（攻击者控制的代理服务器）
user-a$ mihomo-cli config --import malicious-config.yaml
user-a$ mihomo-cli select --group "Proxy" --node "attacker-server"

# 3. user-a 开启 TUN（如果当前没有开启）
user-a$ mihomo-cli tun on

# 4. 现在所有系统流量（包括高权限用户的流量）都经过攻击者控制的代理服务器
# 5. 攻击者捕获高权限用户的流量：SSH 会话、API 密钥、敏感数据等
```

#### 风险等级

🔴 **严重** —— 权限提升 + 流量劫持的组合攻击

#### 影响范围

- 所有系统用户（包括 root）
- 所有经过 TUN 的流量（HTTP/HTTPS/SSH/DNS 等）
- 可能导致：敏感数据泄露、凭据窃取、横向移动

### 2.2 当前 mihomo-cli 的问题

> 下表记录威胁模型分析阶段识别的问题。当前缓解状态必须以正式 `SPEC.md` 和代码/测试证据为准；“已缓解”不等于完整 TUN data-plane 或 Full-journey-tested。

| 问题 | 说明 | 风险等级 | 状态 |
|------|------|---------|------|
| **用户 intent 可被其 owner 修改** | per-user config 是用户配置源；system TUN 不直接信任可变路径，而是经 root revalidation 生成受保护 snapshot | 🟡 中 | 按设计保留；snapshot/candidate/revision/root peer gate 限制系统级应用边界 |
| **未经授权的 TUN mutation** | transport socket 可连接不等于 mutation 授权；请求必须经过 token/peer 校验，TUN mutation 还必须 root peer | 🔴 高 | 已定义 peer UID + token + authorized-client table + CLI sudo re-exec；真实平台覆盖仍按证据矩阵报告 |
| **TUN 是系统级的，影响所有用户** | TUN 网卡是系统级单例，一旦开启，所有用户流量可能被拦截 | 🔴 高 | system mode 单例、root peer gate、受保护 snapshot、runtime API 观察和隔离 data-plane 验收 |

### 2.3 Proxy-RS 能否防止此攻击？

| 措施 | 能否防止 | 说明 |
|------|---------|------|
| **Token 认证** | ⚠️ 部分防止 | 只有安装者可以 `tun on`，但安装者可能不是 root |
| **Socket 权限 `0o600`** | ⚠️ 部分防止 | 只有安装者可以连接 IPC，但安装者可能是低权限用户 |
| **服务所有者模型** | ⚠️ 部分防止 | 记录安装者 UID，但安装者可能是低权限用户 |

**关键问题**：Proxy-RS 的设计假设是"安装者是可信的"，但如果安装者是低权限用户（通过 `sudo` 安装），然后这个用户被攻破，攻击者仍然可以劫持流量。

### 2.4 场景 2：IPC Socket 绕过 CLI 直接控制

#### 攻击链

```bash
# 攻击者绕过 CLI，直接向 daemon IPC socket 发送命令
attacker$ echo '{"ApplySystemTunSnapshot": {"expected_revision": "..."}}' | nc -U /var/run/mihomo/service.sock
# TUN 被开启，无需任何认证
```

#### 风险等级

🔴 **高**

#### 当前问题（已缓解）

- Unix IPC socket 权限 `0o666`，所有用户可访问
- ~~Unix 无 token 认证~~ ✅ 已实施 L3 方案 A
- ~~无 peer UID 检查~~ ✅ 已实施 L3/L1

#### 防护方案（当前合同）

- Unix token 认证：per-user client token + root 管理授权表 + peer UID 绑定。
- `tun on/off` 由普通用户 CLI 内部 sudo re-exec；daemon 只接受 root peer 的 TUN mutation。
- per-user `config.yaml` 是 intent 事实来源；system TUN config 是由 root peer gate 与受控事务生成、并收敛为 `mihomo:mihomo 0640` 的受保护派生 snapshot，不是第二事实来源。
- snapshot 只能由经过原始用户 owner/no-follow/hash/revision 复检的 candidate 生成，并须经真实 Core 语义校验和 API runtime observation。


### 2.5 场景 3：配置文件篡改导致恶意规则注入

#### 攻击链

```bash
# 攻击者修改低权限用户的 rules.yaml
attacker$ echo "- DOMAIN-SUFFIX,google.com,PROXY" >> ~/.config/mihomo/rules.yaml
# 用户开启 TUN 后，流量被重定向
```

#### 风险等级

🟡 **中**

#### 防护方案

- L1：TUN mutation 需要 root peer；普通用户命令通过 CLI 内部 sudo re-exec 完成授权。
- per-user intent config 仍允许其 owner 修改，但 system TUN 不直接信任可变用户路径。
- root 重新校验 owner、no-follow、内容/hash 与 expected revision 后，生成 candidate，并通过受控事务提交受保护、`mihomo:mihomo 0640` 的 `tun-config.yaml` 派生 snapshot。
- snapshot 只作为 system TUN Core 的固定运行时输入，不是第二配置事实来源；事务由 journal、原子提交和 rollback 保护。
- system TUN 的成功状态必须由当前 Core API runtime observation 证明，不能由磁盘配置或历史缓存推断。

### 2.6 场景 4：订阅 URL 泄露 + 中间人攻击

#### 结论

❌ **不需要防御。** 如果攻击者已攻破用户能读取配置文件，那是系统安全问题，不是 mihomo-cli 的责任。

### 2.7 场景 5：daemon 进程被劫持

#### 风险等级

🔴 **严重**

#### 防护方案

ADR-21 在 Linux 已实施：daemon 以 `mihomo` 用户运行，通过 AmbientCapabilities 获得精确权限。macOS system LaunchDaemon 仍为 root，Windows SCM 服务仍为 SYSTEM。

### 2.8 场景 6：token 文件泄露

#### 风险等级

🔴 **高**（如果 token 文件权限配置不当）

#### 防护方案

- Unix 无独立 server token；`~/.config/mihomo/service-token` 为 `0o600 user:user`，并与 peer UID、root 管理的授权表联合校验
- Daemon 侧 peer UID 检查（TUN 操作需要 root）

### 2.9 场景 7：符号链接攻击（Symlink Attack）

#### 攻击链

```bash
# 1. 攻击者在用户目录创建符号链接
attacker$ ln -s /etc/passwd ~/.config/mihomo/config.yaml

# 2. mihomo-cli 以 root 身份写入该文件
# install 命令跟随符号链接，覆盖 /etc/passwd
```

#### 风险等级

🔴 **严重**

#### 历史问题（BUG-21 已修复）

`install_staged_file_privileged` 函数早期直接使用 `install` 命令写入文件，**会跟随符号链接**。BUG-21 修复后已改为 D1-D3 防护。

#### 防护方案

**实现：`install_staged_file_privileged`（BUG-21 修复后）**

```rust
pub fn install_staged_file_privileged(
    path: &std::path::Path,
    bytes: &[u8],
    mode: u16,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        // D1: 写入前检查目标是否已是符号链接
        if let Ok(metadata) = path.symlink_metadata() {
            if metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "refusing to write to symlink: {}\n  \
                     This is a security measure to prevent symlink attacks.",
                    path.display()
                );
            }
        }

        // D2: root 直接写入（O_NOFOLLOW + 显式权限）
        if is_root() {
            return write_installed_file_direct(path, bytes, mode);
        }

        // D3: 非 root 先用 O_NOFOLLOW + 0o600 写入临时文件，
        //     再 `sudo install` 提升到目标路径
        install_staged_file_privileged_non_root(path, bytes, mode)
    }
}
```

**符号链接防御层次**（本节专属编号，避免与 §3.1 L1-L7 混淆）：

| 层次 | 措施 | 防御的攻击 |
|------|------|-----------|
| **D1** | 写入前 `symlink_metadata()` 检查 | 已存在的符号链接 |
| **D2** | `O_NOFOLLOW` 标志（root 直接写入 + 非 root 临时文件） | 写入时目标被替换为符号链接（root 路径）；临时文件被篡改（非 root 路径） |
| **D3** | 临时文件 mode `0o600` + `sudo install` 原子落盘 | 其他用户读取/篡改 stage 内容 |

> 注：BUG-21 修复后已**移除**对父目录强制 `set_permissions(0o700)` 的逻辑，因为该操作会破坏 `/usr/local/bin` 等系统目录权限。当前通过 D1-D3 三层防护替代。
>
> **局限性（非 root 路径）**：D1 的符号链接检查发生在 CLI 进程内，D3 的 `sudo install` 在提权后执行。两者之间存在极短的 TOCTOU 窗口：攻击者若能在检查后、安装前把目标路径替换为符号链接，`sudo install` 仍可能跟随该链接。该窗口取决于目标目录的权限（如 `/usr/local/bin` 仅 root 可写时风险较低）；对 root 可写系统目录，此风险可接受。完全消除该窗口需要 daemon 在提权后重新执行 `O_NOFOLLOW` 校验，属于后续加固方向。

**为什么不用临时文件 + rename**：
- 临时文件本身可能被符号链接攻击
- 进程死亡后临时文件残留
- 复杂度更高（需要多层防御）

### 2.10 场景 8：竞态条件（TOCTOU）

#### 风险等级

🟡 **中**

#### 分析

TOCTOU（Time-of-Check-to-Time-of-Use）攻击需要在检查和操作之间的极短窗口内替换文件。当前代码已有回滚机制（snapshot + rollback），实际利用难度大。

#### 结论

❌ **不在 mihomo-cli 的安全边界内，不实施防护。**

TOCTOU 攻击的前提是攻击者已经能够修改用户配置文件所在的目录——这意味着用户账户已被攻破。一旦用户账户被攻破，配置文件被篡改只是众多问题中的一个（shellrc 注入、SSH 密钥替换、cron 植入等）。mihomo-cli 无法也不应代替操作系统承担用户账户安全的责任。

### 2.11 场景 9：社会工程攻击（钓鱼）

#### 风险等级

🟡 **中**

#### 分析

攻击者通过伪造订阅源、恶意配置文件等方式诱骗用户主动导入。现有的一些 UX 设计（配置导入预览、来源 URL 展示）可以提供一定的提示作用。

#### 结论

❌ **不在 mihomo-cli 的安全边界内，不实施防护。**

社会工程攻击的本质是用户主动执行了恶意操作（导入恶意配置、运行不可信命令）。mihomo-cli 无法防御用户自己的错误决策——这超出了任何技术工具的安全边界。配置导入预览和来源警告作为 UX 改善可以保留，但它们不是安全措施。

### 2.12 场景 10：日志泄露敏感信息

#### 风险等级

🟡 **中**

#### 防护方案

| 措施 | 说明 | 状态 |
|------|------|------|
| **日志脱敏** | 订阅 URL、token、密码等敏感信息脱敏 | ✅ 已实施 |
| **日志级别控制** | 敏感操作使用 DEBUG 级别 | ✅ 已实施 |

---

## 3. 安全改进方案

### 3.1 多层防护设计

| 层级 | 措施 | 防止的攻击 | 优先级 | 状态 |
|------|------|-----------|--------|------|
| **L1** | TUN 操作需要 root 权限（CLI 自动 sudo + Daemon peer UID；Unix root peer gate） | 场景 1、3 | 🔴 必须 | ✅ 已实施（Unix） |
| **L2** | root peer gate 驱动的受保护 TUN snapshot + candidate/revision 事务 | 场景 3 | 🔴 必须 | Contract-defined；实现与真实 TUN 证据按矩阵报告 |
| **L3** | Unix token 认证（方案 A） | 场景 2、6 | 🔴 必须 | ✅ 已实施 |
| **L4** | Socket 权限审计：保留 `0o666` + L3 peer UID/token 认证 | 场景 2 | 🔴 必须 | ✅ 已审计：无需收紧 |
| **L5** | daemon 非 root 运行（ADR-21） | 场景 5 | ✅ 已决策 | Contract-defined；Linux 代码/安装与真实 Core/TUN 旅程按 `SPEC.md §0.4` 分别报告 |
| **L6** | 符号链接攻击防护（O_NOFOLLOW + 检查） | 场景 7 | 🔴 必须 | ✅ 已实施 |
| **L7** | 日志脱敏 + 级别控制 | 场景 10 | 🟡 推荐 | ✅ 已实施 |

> 下节 S1-S5 是威胁模型分析阶段提出的补充性安全建议/设计，编号独立于 §3.1 的 L1-L7 多层防护设计，避免混淆。

### 3.2 S1: TUN 操作需要 root 权限（必须）

#### CLI 侧：非 root 自动 sudo re-exec 原命令

```rust
// main.rs:cmd_tun_resolved / ensure_tun_privilege_or_reexec
if matches!(action, Some(TunAction::On) | Some(TunAction::Off)) {
    ensure_tun_privilege_or_reexec().await?;
}

async fn ensure_tun_privilege_or_reexec() -> anyhow::Result<()> {
    if is_root() {
        return Ok(());
    }

    // 非 root 用户不再只做 `sudo -v` 验证，而是用 sudo 重新执行原始命令。
    // 这样用户执行 `mihomo-cli tun on/off` 即可完成授权流程，
    // 不需要手动输入 `sudo mihomo-cli tun on/off`。
    sudo_reexec_current_command().await
}
```

#### Daemon 侧：peer UID 检查（SO_PEERCRED）

```rust
// daemon.rs:handle_daemon_command
DaemonCommand::ApplySystemTunSnapshot { .. } | DaemonCommand::DisableTun { .. } => {
    // Unix: 通过 getsockopt(SO_PEERCRED) 获取 peer UID，TUN mutation 只接受 peer_uid == 0。
    // daemon 自身的非 root UID、普通授权用户和仅有 token 的连接均不得绕过 root peer gate。
    // Windows: 依赖现有 Windows service / named pipe 权限；Unix root peer gate 不直接移植为 Windows UID 检查。
    if !validate_tun_peer_is_root(peer_uid) {
        return DaemonResponse::Error {
            message: "TUN on/off requires root privileges".to_string(),
        };
    }
    // ... 原有逻辑
}
```

#### 用户体验

```bash
# 首次 tun on（需要输入密码）
user$ mihomo-cli tun on
[sudo] password for user: ********
TUN enabled

# 后续 tun on（利用 sudo 缓存，默认 15 分钟）
user$ mihomo-cli tun on
TUN enabled

# tun status（不需要密码）
user$ mihomo-cli tun status
TUN: enabled
Core: running
```

### 3.3 S2: 系统敏感配置修改需要 root 权限（推荐）

#### 哪些配置算"系统敏感"？

| 配置类型 | 是否敏感 | 原因 |
|---------|---------|------|
| **proxy group 选择** | ✅ 敏感 | 可以劫持所有流量 |
| **规则修改** | ✅ 敏感 | 可以重定向流量 |
| **DNS 设置** | ✅ 敏感 | 可以劫持 DNS 解析 |
| **订阅导入** | ✅ 敏感 | 可以引入恶意节点 |
| **TUN 配置** | ✅ 敏感 | 影响系统级流量拦截 |
| **用户规则（per-user）** | ❌ 不敏感 | 只影响当前用户 |

#### 检查机制

```rust
// main.rs:cmd_config_resolved
if is_system_sensitive_config_change(&opts) {
    if !is_root_or_sudo_capable().await? {
        anyhow::bail!(
            "System-sensitive config changes require root privileges.\n  \
             Suggestions:\n  \
               sudo mihomo-cli config --import ...\n  \
               sudo mihomo-cli rule add ..."
        );
    }
}
```

### 3.4 S3: TUN 状态变化通知（推荐）

#### 通知机制

```rust
// daemon.rs:tun_enabled
if tun_enabled {
    // 1. wall 命令通知所有登录用户
    notify_all_users("TUN mode enabled by user {} at {}", current_user(), timestamp());
    
    // 2. 桌面通知（如果有桌面环境）
    send_desktop_notification("TUN mode enabled");
    
    // 3. 日志告警
    log::warn!("TUN mode enabled by user {}", current_user());
}
```

### 3.5 S4: 审计日志（推荐）

#### 日志格式

```rust
// 审计日志模块
audit_log!("TUN enabled by user={} at={} peer_uid={}", current_user(), timestamp(), peer_uid);
audit_log!("Config changed by user={}: {:?}", current_user(), changes);
audit_log!("Rule added by user={}: {:?}", current_user(), rule);
```

#### 日志位置

- Linux: `/var/log/mihomo/audit.log`
- macOS: `/var/log/mihomo/audit.log`
- Windows: `%ProgramData%\mihomo\audit.log`

### 3.6 S5: IPC 认证（借鉴 Proxy-RS）

#### Token 认证

```rust
// 生成 64 字符随机 token（安装时）
let token = rand::random::<[u8; 32]>()
    .iter().map(|byte| format!("{byte:02x}")).collect::<String>();

// Daemon 侧验证（所有平台统一）
if let Some(server_token) = crate::ipc::service_token() {
    if client_token != Some(server_token.as_str()) {
        return DaemonResponse::Error {
            message: "invalid or missing auth token".to_string(),
        };
    }
}
```

#### Socket 权限审计

L3 采用“方案 A”：per-user client token + daemon 授权表 + peer UID 绑定校验。由于 system daemon socket 需要允许已授权的普通用户发起连接，socket 权限保持 `0o666`；真正的授权边界在 daemon 应用层认证，而不是 Unix socket 文件模式。

```rust
// daemon.rs:1020
// 保持所有本机用户可连接；连接后必须通过 token + peer UID 校验。
std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o666))?;
```

不改为 `0o600`/`0o660` 的原因：这会阻止非 daemon 用户连接 system IPC，破坏 `mihomo-cli access grant ...` 授权后的多用户控制路径；且 `0o660` 还要求额外的 daemon 组成员管理。
systemd 的 `RuntimeDirectoryMode=0711` 只允许非 daemon 用户穿越目录到已知 socket，
不允许列出目录内容；实际 IPC 权限仍由 socket 上的 token 与 peer UID 双重校验决定。

Mihomo Core API socket 继续由 `mihomo` 用户私有。System 模式下，已授权普通用户的
`list`、`select`、`delay`、`proxy` 等请求通过 daemon 的 method/path allowlist 转发；
未授权用户在转发前即被 token + peer UID 校验拒绝。通用转发明确拒绝 `tun` patch，
避免绕过 `ApplySystemTunSnapshot` / `DisableTun` 的 root peer gate。

#### Token 文件保护

```rust
// Token 文件权限 0o600
std::fs::set_permissions(token_path, std::fs::Permissions::from_mode(0o600))?;

// 安装时通过 chown 将客户端 token 文件交给安装用户
```

---

## 4. 用户体验设计

### 4.1 TUN 操作

| 操作 | 命令 | 需要 sudo | 说明 |
|------|------|----------|------|
| 开启 TUN | `mihomo-cli tun on` | ✅（CLI 自动处理） | TUN 未开启：直接开启；TUN 已开启：询问确认 |
| 开启 TUN（跳过确认） | `mihomo-cli tun on --yes` | ✅（CLI 自动处理） | 跳过确认，直接更新 TUN config |
| 关闭 TUN | `mihomo-cli tun off` | ✅（CLI 自动处理） | - |
| 查看 TUN 状态 | `mihomo-cli tun status` | ❌ | - |

### 4.2 权限模型

- Per-user client token 权限为 `0o600 user:user`；Linux 授权表为 `0o640 root:mihomo`，macOS 授权表为 `0o600 root:wheel`
- Socket 权限 `0o666`：所有本机用户可连接，但必须提供授权表中匹配 peer UID 的 client token
- TUN 操作需要 root：daemon 检查 peer UID
- CLI 自动处理 sudo：用户不需要手动加 `sudo` 前缀

---

## 5. 威胁场景总结

| # | 场景 | 风险等级 | 是否需要防御 | 防护方案 |
|---|------|---------|-------------|---------|
| 1 | 低权限用户流量劫持 | 🔴 严重 | ✅ 需要 | L1: TUN 需要 root |
| 2 | IPC Socket 绕过 CLI | 🔴 高 | ✅ 需要 | L3 + L4: token 认证 + Socket 权限 |
| 3 | 配置文件篡改 | 🟡 中 | ✅ 需要 | L1 + L2: TUN root + 配置隔离 |
| 4 | 订阅 URL 泄露 | 🟡 中 | ❌ 不需要 | 超出安全边界 |
| 5 | daemon 进程劫持 | 🔴 严重 | ✅ 需要 | L5: daemon 非 root（ADR-21） |
| 6 | token 文件泄露 | 🔴 高 | ✅ 需要 | L3 + L4: token + peer UID |
| 7 | 符号链接攻击 | 🔴 严重 | ✅ 需要 | L6: O_NOFOLLOW + 符号链接检查 |
| 8 | 竞态条件（TOCTOU） | 🟡 中 | ❌ 不需要 | 超出安全边界（用户账户已被攻破） |
| 9 | 社会工程攻击 | 🟡 中 | ❌ 不需要 | 超出安全边界（用户主动执行恶意操作） |
| 10 | 日志泄露 | 🟡 中 | ✅ 需要 | L7: 日志脱敏 + 级别控制 |

### 5.1 安全边界

#### mihomo-cli 负责防御的

| 类别 | 示例 | 对应层级 |
|------|------|---------|
| **权限隔离** | 低权限用户无法劫持系统流量 | L1: TUN 需要 root |
| **IPC 认证** | 未授权进程无法访问 daemon | L3 + L4: token + socket 权限 |
| **配置完整性** | 系统敏感配置修改需要授权 | L1 + L2: root + 配置隔离 |
| **进程安全** | daemon 以最小权限运行 | L5: 非 root（ADR-21） |
| **文件写入安全** | 防止符号链接攻击 | L6: O_NOFOLLOW |
| **敏感信息保护** | 日志不泄露凭据 | L7: 日志脱敏 |

#### mihomo-cli 不负责防御的

| 类别 | 场景 | 理由 |
|------|------|------|
| **用户账户被攻破** | 攻击者修改用户配置文件（场景 8 TOCTOU） | 用户账户安全是操作系统的责任；账户被攻破后 mihomo-cli 配置只是众多攻击面之一 |
| **社会工程攻击** | 用户主动导入恶意配置（场景 9 钓鱼） | 无法防御用户自己的错误决策；技术工具的安全边界止于用户的自主操作 |
| **订阅 URL 泄露** | 攻击者读取用户配置文件（场景 4） | 前提同样是用户账户已被攻破 |
| **操作系统漏洞** | 内核提权、文件系统绕过等 | 属于操作系统安全范畴 |

---

## 6. 实施计划

### 6.1 阶段 1: TUN 需要 root 权限（L1）

**目标**：防止低权限用户 `tun on/off`

**改动**：
- CLI 侧：非 root 自动 sudo re-exec 原命令
- Daemon 侧：peer UID 检查（SO_PEERCRED）
- 测试覆盖

**优先级**：🔴 必须

### 6.2 阶段 2: IPC 认证（L3）

**目标**：防止绕过 CLI 直接访问 IPC

**改动**：
- Unix 平台启用 token 认证（扩展现有 Windows token 机制）
- Socket 权限审计：L3 方案 A 下保持 `0o666`，连接后以 token + peer UID 授权
- Token 文件权限 `0o600`

**优先级**：🟡 推荐

### 6.3 阶段 3: TUN 派生 snapshot 与事务边界（L2）

**目标**：防止低权限用户通过可变 intent 路径绕过授权，修改影响系统流量的运行时输入。

**当前合同与证据边界**：
- per-user `config.yaml` 是 intent 的唯一事实来源；system `tun-config.yaml` 不是独立配置事实。
- CLI 只能提出 candidate，并携带 `expected_revision` 进入受权 system IPC；不得直接写 system snapshot 或直接调用 Core TUN API。
- root/system context 重新校验原始用户文件的 owner、no-follow、内容/hash 和 revision，生成并原子提交受保护、`mihomo:mihomo 0640` 的派生 snapshot。
- daemon/Core 使用固定 system context 和显式 `-f tun-config.yaml`；journal 管理 snapshot、Core 运行态和 rollback。
- 成功必须由当前 Core API readiness 与 `/configs` runtime observation 证明；不可观察时返回 `Unknown`/失败，恢复不可证明时返回 `RecoveryRequired`。

**平台路径**：
- Linux: `/var/lib/mihomo-cli/tun-config.yaml`
- macOS: `/Library/Application Support/mihomo-cli/tun-config.yaml`
- Windows: `%ProgramData%/mihomo-cli/tun-config.yaml`

**后续**：
- Windows ProgramData ACL 明确化/测试
- 真实 Mihomo TUN/interface/data-plane 验证，不以 fake Core contract 代替

**优先级**：🟡 推荐；实现状态和验证等级必须按正式 SPEC 的证据矩阵报告。

### 6.4 阶段 4: 通知 + 审计（S3 + S4）

**目标**：及时发现异常 + 事后追溯

**改动**：
- TUN 状态变化通知
- 审计日志模块
- 日志格式和位置

**优先级**：🟡 推荐

---

## 7. 参考

- **Proxy-RS 安全设计**：Token 认证 + Socket 权限 + systemd 沙箱
- **clash-verge-rev 安全分析**：IPC 裸奔，不适合借鉴
- **ADR-21 最小权限架构**：daemon 非 root 用户
- **ADR-22 config 单一事实来源**：删除 system store

---

## 8. 互链

- [SPEC.md §1 Architecture](../SPEC.md#1-architecture)
- [ADR-21 最小权限架构](../SPEC.md#adr-21-最小权限架构)
- [ADR-22 config 单一事实来源](../SPEC.md#adr-22-config-单一事实来源--删除-system-store)
- [docs/architecture.md](architecture.md)

### L3 Unix IPC 访问控制（方案 A，已实施）

Unix system daemon 使用 per-user client token、peer UID 与 root 管理的授权表联合认证；不存在独立 Unix server token：

- `~/.config/mihomo/service-token`：用户 client token，`0o600 user:user`。
- `/var/lib/mihomo-cli/authorized-clients.json`：Linux 为 `0o640 root:mihomo`，macOS 为 `0o600 root:wheel`。

Daemon 对每个 IPC 请求校验 client token 是否存在于授权表，并校验 Unix socket peer UID 与授权表中 token 归属 UID 一致；`tun on/off` 仍额外要求 root peer UID。

管理命令：`mihomo-cli access grant --user <name>`、`access revoke --user <name>`、`access list`、`access status`。
