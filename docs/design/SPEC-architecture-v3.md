# mihomo-cli 架构设计 SPEC v3（互斥模式）

> 状态: Draft  
> 日期: 2026-07-25  
> 替代: v2 双实例模型（draft-root-mode-privilege-model.md）

---

## 1. 设计目标

### 1.1 核心原则

**两种互斥模式**：
- **Per-user 非 TUN 模式**：每用户独立 core，配置/规则隔离，Windows/Linux 完整支持，macOS 部分支持
- **System Service TUN 模式**：单实例系统服务，TUN 全机共享，配置 per-user（最后操作者生效）

**互斥原因**：TUN 在网络层劫持所有流量，per-user 系统代理被忽略。两者不能有意义地共存。

### 1.2 用户体验目标

**参考 clash-verge-rev 的行为**：
1. **无需 `sudo` 前缀**：CLI 内部处理权限提升（提示输入密码）
2. **服务安装一次性**：只有第一次安装系统服务时需要密码
3. **安装后所有操作无需密码**：TUN 开关、服务管理通过 IPC 与 root 服务通信
4. **配置 per-user**：即使系统服务模式，配置仍然是 `~/.config/mihomo/`

### 1.3 与 clash-verge-rev 的对齐

| 维度 | Clash-Verge-Rev | mihomo-cli (v3) |
|------|-----------------|-----------------|
| **界面** | GUI (Tauri) | CLI |
| **服务安装** | 一次性，需密码 | 一次性，CLI 内部提示密码 |
| **TUN 开关** | 无需密码（IPC 到服务） | 无需密码（IPC 到服务） |
| **配置位置** | `~/.config/clash-verge/` | `~/.config/mihomo/` |
| **配置归属** | Per-user（即使服务模式） | Per-user（即使服务模式） |
| **多用户** | ❌ 不支持 | ✅ Per-user 非 TUN 模式支持 |

---

## 2. 架构总览

### 2.1 系统服务模式（TUN 可用）

```
┌─────────────────────────────────────────────────────────────┐
│  用户会话（普通用户）                                        │
│                                                              │
│  ┌──────────────────────┐                                   │
│  │  mihomo-cli          │  (普通用户运行，无需 sudo)        │
│  │  - 命令行界面        │                                   │
│  │  - 配置管理          │                                   │
│  │  - IPC 客户端        │                                   │
│  └──────────┬───────────┘                                   │
│             │ 读写配置                                       │
│             ↓                                                │
│  ┌──────────────────────┐                                   │
│  │  ~/.config/mihomo/   │  (per-user 配置，无需 root)       │
│  │  - config.yaml       │                                   │
│  │  - rules.yaml        │                                   │
│  └──────────┬───────────┘                                   │
│             │ IPC 命令                                       │
│             ↓                                                │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  System Service (root daemon)                        │  │
│  │  ┌────────────────────┐                             │  │
│  │  │ IPC Server         │  ← 接收 CLI 命令            │  │
│  │  │ /var/run/mihomo/   │                             │  │
│  │  │ service.sock       │                             │  │
│  │  └────────┬───────────┘                             │  │
│  │           │ 以 root 身份执行                         │  │
│  │           ↓                                          │  │
│  │  ┌────────────────────┐                             │  │
│  │  │ mihomo core        │  (由服务启动，root 身份)    │  │
│  │  │ - 代理服务         │                             │  │
│  │  │   :7897 (mixed)    │                             │  │
│  │  │ - API 端点         │                             │  │
│  │  │   /var/run/mihomo/ │  ← CLI 通过这里控制 core   │  │
│  │  │   mihomo.sock      │                             │  │
│  │  │ - TUN 设备         │  (root 权限创建)            │  │
│  │  └────────────────────┘                             │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                              │
│  全机流量 (TUN 劫持)                                        │
│  配置来源：当前控制 CLI 的用户的 ~/.config/mihomo/          │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 Per-user 模式（非 TUN）

```
┌─────────────────────────────────────────────────────────────┐
│  用户会话（普通用户）                                        │
│                                                              │
│  ┌──────────────────────┐                                   │
│  │  mihomo-cli          │  (普通用户运行，无需 sudo)        │
│  │  - 命令行界面        │                                   │
│  │  - 配置管理          │                                   │
│  └──────────┬───────────┘                                   │
│             │ 读写配置                                       │
│             ↓                                                │
│  ┌──────────────────────┐                                   │
│  │  ~/.config/mihomo/   │  (per-user 配置)                 │
│  │  - config.yaml       │                                   │
│  │  - rules.yaml        │                                   │
│  └──────────┬───────────┘                                   │
│             │ 启动/管理                                      │
│             ↓                                                │
│  ┌──────────────────────┐                                   │
│  │  User Service        │  (systemd --user / LaunchAgent)  │
│  │  - 以当前用户身份运行│                                   │
│  │  - 无需 root 权限    │                                   │
│  └──────────┬───────────┘                                   │
│             ↓                                                │
│  ┌──────────────────────┐                                   │
│  │  mihomo core         │  (用户身份运行)                   │
│  │  - 代理服务          │                                   │
│  │    :7897 (mixed)     │                                   │
│  │  - API 端点          │                                   │
│  │    $XDG_RUNTIME_DIR/ │                                   │
│  │    mihomo/mihomo.sock│                                   │
│  └──────────────────────┘                                   │
│                                                              │
│  系统代理 (per-user)                                        │
│  - Windows: HKCU registry                                   │
│  - Linux: gsettings                                         │
│  - macOS: ❌ 系统级，无法 per-user 隔离                    │
└─────────────────────────────────────────────────────────────┘
```

---

## 3. 命令设计

### 3.1 核心原则

1. **无 `sudo` 前缀**：所有命令以普通用户运行
2. **无 `--root` flags**：统一使用 system/user 术语
3. **`--user` 仅限 install/uninstall**：运行时命令自动检测模式，公开只保留 `--system` 作为显式 system service override
4. **配置始终 per-user**：`~/.config/mihomo/`，无需 root

### 3.2 命令列表

#### 服务管理

```bash
# 交互式安装（提示选择模式）
mihomo-cli install              # 交互式选择：(1) per-user 或 (2) system service
mihomo-cli install --system     # 非交互式：直接安装系统服务（CLI 提示输入密码）
mihomo-cli install --user       # 非交互式：直接安装用户服务（用于脚本/CI）

# 卸载
mihomo-cli uninstall            # 卸载当前模式的服务（自动检测）
mihomo-cli uninstall --system   # 卸载系统服务
mihomo-cli uninstall --user     # 卸载用户服务
mihomo-cli uninstall --all      # 删除一切（binary + service + config）

# 启动/停止/重启（自动检测当前模式）
mihomo-cli start                # 启动当前模式的 core
mihomo-cli stop                 # 停止当前模式的 core
mihomo-cli restart              # 重启当前模式的 core

# 显式指定模式（可选，通常自动检测）
mihomo-cli start --system       # 启动系统服务 core（IPC）
mihomo-cli stop --system        # 停止系统服务 core（IPC）
mihomo-cli restart --system     # 重启系统服务 core（IPC）
```

#### TUN 管理

```bash
# Per-user 模式（不可用）
mihomo-cli tun on
# → 错误：TUN 需要系统服务
# → 建议：运行 mihomo-cli install --system 安装系统服务

# System Service 模式（IPC 到服务，无需密码）
mihomo-cli tun on               # 启用 TUN（IPC 通知服务）
mihomo-cli tun off              # 禁用 TUN（IPC 通知服务）
mihomo-cli tun status           # 查看 TUN 状态
```

#### 配置管理

```bash
# 所有模式下都是 per-user 配置
mihomo-cli config ...           # 操作 ~/.config/mihomo/config.yaml
mihomo-cli rule ...             # 操作 ~/.config/mihomo/rules.yaml
mihomo-cli dns ...              # 操作 DNS 配置
mihomo-cli override ...         # 操作 override 配置

# 示例
mihomo-cli config set mixed-port 8080
mihomo-cli rule add DOMAIN,example.com,Proxy
mihomo-cli dns template company
```

#### API 命令

```bash
# 自动检测当前实例（per-user 或 system service）
mihomo-cli select               # 切换节点
mihomo-cli delay                # 测延迟
mihomo-cli list                 # 列出节点
mihomo-cli conn                 # 查看连接
mihomo-cli ip                   # 查看出口 IP

# 显式指定系统服务（可选，通常自动检测）
mihomo-cli select --system      # 切换系统服务实例的节点
```

#### 状态查询

```bash
mihomo-cli status               # 显示当前实例状态
# 自动检测：
# - 如果系统服务运行中 → 显示系统服务状态
# - 如果用户服务运行中 → 显示用户服务状态
# - 如果都未运行 → 提示启动

mihomo-cli status --verbose     # 详细信息（包括服务状态、配置路径等）
```

#### 系统代理

```bash
# Per-user 模式（设置当前用户的系统代理）
mihomo-cli system-proxy on      # 启用系统代理（per-user）
mihomo-cli system-proxy off     # 禁用系统代理（per-user）

# System Service 模式（无意义，TUN 已劫持流量）
mihomo-cli system-proxy on
# → 警告：TUN 模式下系统代理无效，流量已被 TUN 劫持
```

---

## 4. 权限模型

### 4.1 系统服务架构

```
安装阶段（需要 root）：
    mihomo-cli install --system
    → CLI 检测到需要 root 权限
    → 提示用户输入密码（类似 sudo）
    → 使用 sudo -S 或 polkit 等机制
    → 创建系统服务单元（systemd / LaunchDaemon）
    → 服务以 root 身份启动，监听 IPC socket
    
运行阶段（无需 root）：
    mihomo-cli tun on
    → CLI 通过 IPC 发送 "enable TUN" 命令
    → 系统服务（root）接收命令
    → 服务以 root 身份执行 TUN 启用
    → CLI 无需 root 权限
```

### 4.2 IPC 协议

```rust
// IPC 命令（CLI → Service）
enum ServiceCommand {
    // config_path 是当前操作用户的 ~/.config/mihomo/config.yaml。
    // core binary 不由客户端传入；daemon 固定使用 system install path。
    StartCore { config_path: PathBuf },
    StopCore,
    RestartCore { config_path: PathBuf },
    EnableTun {
        config_path: PathBuf,
        stack: Option<String>,
        dns_hijack: Option<String>,
    },
    DisableTun,
    GetStatus,
}

// IPC 响应（Service → CLI）
enum ServiceResponse {
    Success,
    Error { message: String },
    Status { running: bool, tun_enabled: bool, pid: Option<u32> },
}

// IPC 传输
// Unix: /var/run/mihomo/service.sock
// Windows: \\.\pipe\mihomo-service
```

**IPC 安全边界**：
- IPC socket/pipe 允许普通用户连接，以满足“安装后无需 sudo”。
- daemon 不信任客户端传入的可执行文件路径；system core binary 由 daemon 固定解析。
- daemon 启动恢复或停止 orphan core 时，pid metadata 必须完整符合 v3 system core 边界：可信 system core binary、clean per-user config path、v3 system API endpoint；legacy numeric-only pid-file 不再作为可恢复/可终止依据。
- daemon 启动恢复或停止 orphan core 时，pid metadata 中记录的 core binary 必须与进程命令行精确匹配；不能仅按 basename 匹配。
- `config_path` 必须是 clean absolute path，且只能指向当前平台的 per-user mihomo `config.yaml`（Linux `/home/<user>/.config/mihomo/config.yaml`，macOS `/Users/<user>/.config/mihomo/config.yaml`/`/var/root/.config/mihomo/config.yaml`，Windows `%APPDATA%\mihomo\config.yaml`）。
- config 生成/合并/fix 必须写入当前 resolved instance 的 runtime API endpoint；system service 配置必须写入 v3 system endpoint，订阅刷新、legacy config merge、rule/dns/override 更新都不能回退到 per-user endpoint。
- daemon 启动 system core 前必须校验配置中的 API endpoint 等于 v3 系统端点：Unix `/var/run/mihomo/mihomo.sock`，Windows `\\.\pipe\mihomo-core`；缺失、TCP endpoint 或其他 socket/pipe 一律拒绝。
- daemon 执行 restart 前必须先完成新配置/core binary/API endpoint preflight；preflight 失败不得停止当前正在运行的 core。
- Unix 上 daemon 使用 socket peer credentials 校验 `config.yaml` owner uid 必须匹配调用方 uid（root 调用例外）。
- 含 `.` / `..` 成分、非 `config.yaml`、非 mihomo config 目录、owner 不匹配或 system core API endpoint 不匹配的请求会被拒绝。

### 4.3 权限矩阵

| 操作 | Per-user 模式 | System Service 模式 |
|------|--------------|---------------------|
| 安装服务 | 无需 root | 需要 root（一次性，CLI 提示） |
| 启动/停止 core | 无需 root | 无需 root（IPC） |
| TUN on/off | ❌ 不可用 | 无需 root（IPC） |
| 编辑配置 | 无需 root（用户文件） | 无需 root（用户文件） |
| API 命令 | 无需 root | 无需 root（socket 权限 666） |
| 系统代理 | 无需 root（per-user） | 无意义（TUN 劫持） |

---

## 5. 路径规划

### 5.1 Per-user 模式

| 组件 | Linux | macOS | Windows |
|------|-------|-------|---------|
| 配置 | `~/.config/mihomo/` | `~/.config/mihomo/` | `%APPDATA%\mihomo\` |
| Socket | `$XDG_RUNTIME_DIR/mihomo/mihomo.sock` | `/tmp/mihomo-$UID/mihomo.sock` | `\\.\pipe\mihomo-$USERNAME` |
| 日志 | `~/.local/state/mihomo/mihomo.log` | `~/Library/Logs/mihomo/mihomo.log` | `%LOCALAPPDATA%\mihomo\mihomo.log` |
| 服务 | `systemd --user` | LaunchAgent | 无（手动启动或 Scheduled Task） |

### 5.2 System Service 模式

| 组件 | Linux | macOS | Windows |
|------|-------|-------|---------|
| 配置 | `~/.config/mihomo/`（per-user） | `~/.config/mihomo/`（per-user） | `%APPDATA%\mihomo\`（per-user） |
| Service IPC | `/var/run/mihomo/service.sock` | `/var/run/mihomo/service.sock` | `\\.\pipe\mihomo-service` |
| Core API | `/var/run/mihomo/mihomo.sock` | `/var/run/mihomo/mihomo.sock` | `\\.\pipe\mihomo-core` |
| 日志 | `/var/log/mihomo/mihomo.log` | `/var/log/mihomo/mihomo.log` | `%ProgramData%\mihomo\mihomo.log` |
| 服务 | `systemd system` | LaunchDaemon | Windows Service |

**关键点**：即使系统服务模式，配置仍然是 per-user 的。服务启动 core 时，读取当前控制 CLI 的用户的配置。

---

## 6. 模式检测与解析

### 6.1 检测逻辑

```rust
enum ActiveMode {
    PerUser,        // 用户服务运行中
    SystemService,  // 系统服务运行中
    NotRunning,     // 都未运行
    Conflict,       // 两者都在运行（异常）
}

fn detect_active_mode() -> ActiveMode {
    let system_running = check_system_service_running();
    let user_running = check_user_service_running();
    
    match (system_running, user_running) {
        (true, false) => ActiveMode::SystemService,
        (false, true) => ActiveMode::PerUser,
        (false, false) => ActiveMode::NotRunning,
        (true, true) => ActiveMode::Conflict,
    }
}
```

### 6.2 命令解析

```rust
fn resolve_target_instance(system_flag: bool) -> Result<Instance> {
    let active = detect_active_mode();
    
    if system_flag {
        // 显式指定 --system
        if active == ActiveMode::SystemService || active == ActiveMode::NotRunning {
            Ok(Instance::System)
        } else {
            bail!("系统服务未运行，请使用 mihomo-cli install --system")
        }
    } else {
        // 未指定 flag，自动检测
        match active {
            ActiveMode::SystemService => Ok(Instance::System),
            ActiveMode::PerUser => Ok(Instance::PerUser),
            ActiveMode::NotRunning => {
                // 默认启动用户服务
                Ok(Instance::PerUser)
            }
            ActiveMode::Conflict => {
                warn!("检测到冲突：系统服务和用户服务同时运行");
                warn!("建议：mihomo-cli stop --system 或 mihomo-cli stop");
                bail!("模式冲突，请先停止一个实例")
            }
        }
    }
}
```

---

## 7. 多用户场景

### 7.1 Per-user 非 TUN 模式（Windows/Linux）

```
用户 A 登录：
    mihomo-cli install              # 安装用户 A 的服务
    mihomo-cli start                # 启动用户 A 的 core
    mihomo-cli system-proxy on      # 设置用户 A 的系统代理
    
用户 B 登录（fast user switching）：
    mihomo-cli install              # 安装用户 B 的服务
    mihomo-cli start                # 启动用户 B 的 core
    mihomo-cli system-proxy on      # 设置用户 B 的系统代理
    
结果：
    ✅ 用户 A 和 B 各有独立的 core、配置、socket
    ✅ 系统代理 per-user（Windows/Linux）
    ✅ 流量隔离
    ⚠️ 端口冲突：如果都用 7897，第二个启动失败
       → 解决方案：mihomo-cli config set mixed-port 7898
```

### 7.2 System Service TUN 模式

```
用户 A 登录：
    mihomo-cli install --system     # 安装系统服务（需密码，一次性）
    mihomo-cli tun on               # 启用 TUN（服务用 A 的配置）
    
用户 B 登录（fast user switching）：
    mihomo-cli tun off              # 禁用 TUN
    mihomo-cli tun on               # 重新启用 TUN（服务切换到 B 的配置）
    
结果：
    ✅ TUN 设备共享（系统级资源）
    ✅ 配置 per-user（最后操作者生效）
    ❌ 不能同时启用两个 TUN（OS 约束）
```

### 7.3 macOS 限制

**系统代理是系统级的**：
- `networksetup` 设置的是全机代理
- 无法 per-user 隔离
- macOS 用户建议：
  - 使用 TUN 模式（全机共享）
  - 或手动配置应用级代理（不通过系统代理）

---

## 8. 平台支持矩阵

| 功能 | Windows | Linux | macOS |
|------|---------|-------|-------|
| **Per-user 非 TUN** | ✅ | ✅ | ⚠️ 部分 |
| 配置隔离 | ✅ | ✅ | ✅ |
| Socket 隔离 | ✅ | ✅ | ✅ |
| 系统代理隔离 | ✅ (HKCU) | ✅ (gsettings) | ❌ (系统级) |
| **System Service TUN** | ✅ | ✅ | ✅ |
| 服务安装 | Windows Service | systemd | LaunchDaemon |
| TUN 支持 | ✅ | ✅ | ✅ |
| IPC 通信 | Named Pipe | Unix Socket | Unix Socket |

---

## 9. 冲突处理

### 9.1 设计原则

**Fail-fast + 清晰报错**：
- 不主动迁移旧版本
- 用户自行运行 `uninstall --all` 清理旧安装
- 遇到冲突时清晰报错，提供诊断和建议

### 9.2 冲突场景

#### 场景 1：旧版本未卸载，直接运行新版本

```bash
$ mihomo-cli install
错误：检测到旧版本服务残留
  - systemd service: mihomo.service (active)
  - 配置文件: ~/.config/mihomo/config.yaml
  
建议：
  1. 先卸载旧版本：mihomo-cli uninstall --all
  2. 然后重新安装：mihomo-cli install
  
或者忽略此错误，如果旧版本兼容，可能正常工作
```

#### 场景 2：系统服务和用户服务同时存在（异常）

```bash
$ mihomo-cli status
警告：检测到多个实例
  - System service: active (PID 1234)
  - User service: active (PID 5678)
  
这可能导致冲突。建议只保留一个：
  - 停止系统服务：mihomo-cli stop --system
  - 或停止用户服务：mihomo-cli stop
```

#### 场景 3：端口冲突

```bash
$ mihomo-cli start
错误：端口 7897 已被占用 (PID 9999)
可能原因：
  - 另一个 mihomo 实例正在运行
  - 其他程序占用了此端口
  
建议：
  1. 更换端口：mihomo-cli config set mixed-port 7898
  2. 或停止占用端口的程序
  3. 然后重新启动：mihomo-cli start
```

#### 场景 4：TUN 已启用，用户尝试启动 per-user core

```bash
$ mihomo-cli start
错误：检测到系统服务正在运行（TUN 模式）
TUN 会劫持所有流量，per-user core 无法工作
建议：
  1. 使用系统实例：mihomo-cli select
  2. 或停止系统服务：mihomo-cli stop --system
```

#### 场景 5：Per-user core 运行中，用户尝试启用 TUN

```bash
$ mihomo-cli tun on
错误：检测到用户服务正在运行
启用 TUN 会导致用户 core 失效
建议：
  1. 先停止用户服务：mihomo-cli stop
  2. 然后启用 TUN：mihomo-cli tun on
```

---

## 10. 实现优先级

### Phase 1: Per-user 模式（基础）

- [x] 用户服务安装/卸载（systemd --user / LaunchAgent；Windows per-user 为直接进程模式）
- [x] 启动/停止/重启
- [x] 配置管理（config/rule/dns/override，路径解析 runtime-first）
- [x] API 命令（select/delay/list/conn/ip，经 resolved endpoint 访问）
- [x] 系统代理（Windows/Linux；macOS 明确为系统级限制）
- [x] 状态查询
- [x] 冲突检测 + 清晰报错

### Phase 2: System Service 模式（TUN）

- [x] 系统服务安装/卸载（CLI 内部权限提升）
- [x] IPC 协议实现（CLI ↔ Service；Unix socket / Windows named pipe；未知字段/非 v3 命令拒绝）
- [x] 服务启动/停止 core（IPC；daemon 自行选择 system core binary，并强制 v3 system core API endpoint）
- [x] TUN 启用/禁用（IPC；配置路径限制为 per-user mihomo config.yaml）
- [x] 模式检测与冲突处理

### Phase 3: 加固与测试

- [x] 端口冲突检测
- [x] 多用户场景测试（路径/命名空间/互斥解析单元测试；Windows 仍需目标平台实测）
- [x] 文档完善（v3 路径、flags、marker 移除、IPC 权限边界已同步）

### Validation 状态

- [x] Linux host: `cargo fmt`、`cargo test -q`、`cargo clippy --all-targets -- -D warnings` 通过。
- [x] Windows GNU target: `cargo check --target x86_64-pc-windows-gnu` 与 `cargo clippy --target x86_64-pc-windows-gnu --all-targets -- -D warnings` 通过。
- [x] Windows MSVC target: `cargo check --target x86_64-pc-windows-msvc` 与 `cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings` 通过。
- [ ] Windows 实机 E2E: 尚未在真实 Windows service/named pipe 环境运行。


---

## 11. 与 v2 SPEC 的差异

| 维度 | v2 SPEC（废弃） | v3 设计（当前） |
|------|----------------|----------------|
| **实例模型** | 双实例并存（root + user） | 互斥模式（per-user OR system） |
| **install 默认** | 安装 root 服务（需 sudo） | 交互式选择（per-user 或 system） |
| **命令 flags** | `--root/--user` | `install`/`uninstall` 保留 `--user`/`--system`；其他公开命令无 `--user`，可用 `--system` 显式指定 system service |
| **配置位置** | root: `/etc/mihomo/`，user: `~/.config/mihomo/` | 始终 per-user: `~/.config/mihomo/` |
| **权限模型** | 每个命令可能需要 sudo | 仅安装系统服务时需要密码（一次性） |
| **TUN** | root 实例专属 | System service 模式（IPC 控制） |
| **多用户** | 设计目标（two-user E2E） | Per-user 模式支持（Win/Linux） |
| **迁移工具** | 有 migrate 命令 | 无，用户自行 uninstall --all |
| **状态检测** | `.service-mode` marker 文件 | 运行时检测（检查服务是否运行） |
| **复杂度** | 高（双实例 resolution） | 低（互斥，无歧义） |

---

## 12. 下一步

1. **Grill with doc**：用现有文档审视此 spec，确保与已有系统一致
2. **Plan**：拆分任务，确认实现顺序
3. **TDD 实现**：测试驱动开发
4. **Review + E2E**：代码审查 + 端到端测试验证

---

## 附录 A: 关键决策记录

### ADR-01: 互斥模式而非双实例并存

**决策**：采用互斥模式（per-user OR system），而非 v2 的双实例并存。

**原因**：
- TUN 劫持所有流量，per-user core 在 TUN 模式下无效
- 双实例增加复杂度，用户需要记住 `--root/--user` flags
- 互斥模式更简单，符合 Unix 哲学

### ADR-02: 配置始终 per-user

**决策**：即使系统服务模式，配置仍然在 `~/.config/mihomo/`。

**原因**：
- 与 clash-verge-rev 行为一致
- 配置编辑无需 root 权限
- 多用户可以有各自的配置（虽然 TUN 同时只能一个生效）

### ADR-03: 无迁移工具

**决策**：不提供 migrate 命令，用户自行 `uninstall --all`。

**原因**：
- 过度设计
- `uninstall --all` 已能清理旧安装
- 冲突时清晰报错，用户可根据错误信息自行修复
- Fail-fast 比自动迁移更安全

### ADR-04: CLI 内部处理权限提升

**决策**：不使用 `sudo` 前缀，CLI 内部提示密码。

**原因**：
- 与 clash-verge-rev 行为一致
- 用户体验更好（只需输入一次密码）
- 安装后所有操作无需密码（IPC 到 root 服务）

### ADR-05: 交互式安装选择

**决策**：`mihomo-cli install`（无 flag）交互式提示选择模式。

**原因**：
- 用户可能不清楚两种模式的区别
- 交互式提示可以附带简短说明
- 保留 `--system` 和 `--user` flags 用于脚本/CI 非交互式安装

### ADR-06: 移除 .service-mode marker

**决策**：移除 `~/.config/mihomo/.service-mode` marker 文件，完全依赖运行时检测。

**原因**：
- Marker 可能与实际状态不一致（服务被手动删除但 marker 还在）
- 运行时检测（检查服务是否运行）更可靠
- 简化状态管理

### ADR-07: --user/--root flags 的处理

**决策**：
- `install`/`uninstall` 命令保留 `--user` 和 `--system` flags（用于非交互式安装/卸载）
- 其他命令（start/stop/restart/status/select/delay 等）移除 `--user`/`--root` flags；默认自动检测，公开保留 `--system` 作为显式 system service override

**原因**：
- 安装/卸载需要明确指定类型（交互式或 flag）
- 运行时命令自动检测更简单，减少用户认知负担
- 避免 `--root` 和 `--system` 语义混淆（统一用 `--system`）

---

## 附录 B: 参考实现

### Clash-Verge-Rev 的关键设计

- **服务安装**：一次性，需管理员密码
- **TUN 开关**：通过 IPC 与 root 服务通信，无需密码
- **配置位置**：`~/.config/clash-verge/`（per-user）
- **单实例**：不支持多用户同时运行

### mihomo-cli v3 的改进

- **多用户支持**：Per-user 模式下支持多用户隔离（Windows/Linux）
- **CLI 界面**：命令行而非 GUI
- **互斥模式**：显式支持 per-user 和 system 两种模式

---

**文档结束**
