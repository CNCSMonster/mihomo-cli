# SPEC: Windows 支持完善方案（Windows Service + named pipe ACL）

- 状态: **草案 → 已评审修订**（2026-08-03 二轮 review 通过，有条件进入实施）
- 日期: 2026-08-02
- 范围: mihomo-cli Windows 二等公民支持完善——让 Windows System 模式服务可启动、安全、可管理

## 1. 背景与问题

CI 三平台矩阵验证（cross-platform-e2e.yml）暴露 Windows System 模式无法启动服务：

```
[SC] CreateService SUCCESS
[SC] StartService FAILED 87:  (ERROR_INVALID_PARAMETER)
事件日志 7000: "The Mihomo Proxy Service service failed to start"
```

**根因**：`mihomo-cli daemon` 是普通命令行进程，不调用 Windows SCM 协议
（`StartServiceCtrlDispatcher`/`RegisterServiceCtrlHandlerExW`）。SCM 启动服务时
等待进程报告"已启动"，但 daemon 只跑自己的主循环（监听 named pipe）→ 超时失败。

**关联问题**（调研中发现）：
- **W1 daemon 非合法服务**：StartService 87（本方案核心）
- **W2 named pipe 无 ACL**：`ServerOptions::new()` 默认 ACL，非特权进程可连 system daemon
- **W3 服务安装手写 sc.exe**：引号/token 拆分易错（已修，但脆弱）
- **W4 elevated 检测不标准**：`net session` hack
- **W5 daemon 生命周期与 SCM 无集成**：无 Stop/Shutdown 控制处理
- **W6 服务创建代码两套并存**：instance.rs 与 service.rs（legacy）binPath 语义漂移

## 2. 参考

`3rdparty/Proxy-RS`（同架构单二进制 + 子命令）的 Windows 服务实现：
- `windows-service = "=0.8.1"`（Mullvad VPN 维护，SCM 协议封装）
- `service-manager = "=0.11.0"`（跨平台服务安装/卸载）
- named pipe 安全：**仅 token 校验**（`service_ipc.rs`），icacls 只用于 token 文件/目录 ACL
  —— pipe 本身**没有 SDDL**。本方案的 pipe 级 SDDL **强于 Proxy-RS**（在其 token 方案上
  增加 pipe 访问控制层）
- 安装者 SID 落盘（`service-owner.json`）供 daemon 运行时读取——本方案对齐此模式

## 3. 方案设计

### 3.0 新增：跨平台统一开机自启控制（`autostart` 命令）

**目标**：macOS/Linux/Windows 三平台开机自启统一可配置，CLI 切换，**默认不开机自启**。

**新增命令**：
```bash
mihomo-cli autostart on      # 开启开机自启（当前实例模式）
mihomo-cli autostart off     # 关闭开机自启
mihomo-cli autostart status  # 查询自启状态
mihomo-cli autostart on --system  # 指定模式（可选）
```

**三平台实现矩阵**：

| 平台/模式 | on | off | status |
|-----------|-----|------|--------|
| Linux system | `systemctl enable mihomo` | `systemctl disable mihomo` | `systemctl is-enabled mihomo` |
| Linux user | `systemctl --user enable mihomo` | `systemctl --user disable mihomo` | `systemctl --user is-enabled mihomo` |
| macOS system | `launchctl enable system/io.mihomo` | `launchctl disable system/io.mihomo` | `launchctl print system/io.mihomo` |
| macOS user | `launchctl enable gui/UID/io.mihomo` | `launchctl disable gui/UID/io.mihomo` | `launchctl print gui/UID/io.mihomo` |
| Windows system | `sc config mihomo start= auto` | `sc config mihomo start= demand` | `sc qc mihomo`（解析 START_TYPE） |
| Windows user | 注册表 Run 键 + .vbs 隐藏（见下） | 删除 Run 值 | `reg query` 存在性 |

**Windows user 自启实现**（决策：注册表 Run 键 + .vbs 隐藏窗口，对齐 Proxy-RS autostart.rs）：
- `on`：写 `%APPDATA%\mihomo\autostart.vbs`
  （`WScript.Shell.Run "mihomo-cli start", 0, False`，参数 0 = 隐藏窗口）
  → `reg.exe ADD HKCU\...\Run /v mihomo-cli /d "wscript.exe //B //NoLogo <vbs>" /f`
- `off`：`reg.exe DELETE HKCU\...\Run /v mihomo-cli /f` + 删除 .vbs
- `status`：`reg.exe QUERY HKCU\...\Run /v mihomo-cli`（成功 = 已启用）
- **用户视角**：仅见 `autostart on/off/status`，登录静默启动无黑窗，不易误删

**默认行为变更**（install 改造）：
- Linux systemd unit：安装时**不再** `enable --now`（去 WantedBy 或 `systemctl disable`），改为手动 `autostart on`
- macOS launchd plist：`RunAtLoad` 默认 false（或安装后 `launchctl disable`）
- Windows sc create：`start= demand`（当前是 auto）

**实现**：
- 新命令 `autostart on|off|status`（子命令形式，已定）走 InstanceContext 模式解析
  （默认当前实例，`--system/--user` 可选）
- 各平台 plan 复用/新增 `planned_autostart_plan(ctx, enable)`（对齐现有 `planned_service_plan` 模式）
- Windows user 自启：注册表 Run 键 + `.vbs` 隐藏（已定，见上）

**影响**：install 语义变化（不自启）——需更新 USAGE/README 文档 + 测试。

### 3.1 架构：保持单二进制（ADR-03），Windows daemon 补 SCM 协议层

```
mihomo-cli (单二进制)
├── CLI 命令（status/start/stop/select/...）
└── daemon 子命令（隐藏，服务进程）
    ├── unix: 直接跑主循环（launchd/systemd 直调，已是合法服务）
    └── windows: 新增 SCM 协议层（windows-service crate）
         └── service_dispatcher::start → service_main
              └── service_control_handler::register（Stop/Shutdown）
                   └── 原有 daemon 主循环（core 管理 + IPC）
```

### 3.2 依赖（精确锁定，供应链安全）

```toml
[target.'cfg(windows)'.dependencies]
windows-service = "=0.8.1"    # SCM 协议（Mullvad 生产维护）
windows-sys = { version = "=0.60", features = ["Win32_Security", "Win32_Security_Authorization", "Win32_System_Threading", "Win32_Foundation"] }
  # Win32_Security_Authorization: ConvertStringSecurityDescriptorToSecurityDescriptorW
  # Win32_System_Threading: GetTokenInformation/TokenElevation
service-manager = "=0.11.0"   # Windows 服务安装/卸载（仅 Windows 用，见决策 1）

[dependencies]
tokio-util = "=0.7"           # CancellationToken（停机链路，M1/P0-2）
```

**供应链安全**：
- 全部 `=版本` 精确锁定（防 semver 浮动/恶意 yank 波及）
- `Cargo.lock` 已提交仓库（已有）
- windows-service 来源 Mullvad（生产 VPN 厂商，非个人项目），MIT/Apache-2.0
- **不使用 is_elevated**（2019 停更个人项目）——elevated 检测用 windows-sys 自查 `TokenElevation`（P1-6）
- service-manager 来源活跃（chipsenkbeil/jacderida 维护），非 Mullvad 生态（修正 P0-4 错误论据）

### 3.3 修改点

#### M1: daemon.rs 新增 Windows 服务入口（W1/W5）

**线程模型（P0-3 修正，保持 `#[tokio::main]` 单一入口）**：
`#[tokio::main]` 的 `block_on` 在 **main 线程**执行——因此 Windows daemon 分支在
async main 开头**同步调用** `service_dispatcher::start`（仍处主线程，满足 SCM
"主线程 30 秒内调用"约束），服务模式下 runtime 无其他并发任务，阻塞 main 线程无副作用。
`service_main` 回调（SCM 线程）内部再**自建 tokio runtime** 跑核心循环。

```rust
// main.rs: 保持 #[tokio::main]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Windows daemon 子命令：同步进 dispatcher（主线程），不进 async 分发
    if cfg!(target_os = "windows") && cli.is_daemon_mode() {
        return daemon::run_windows_service();  // 同步，内部 service_dispatcher::start
    }
    run(cli).await  // 其余走正常 async CLI 分发
}

// daemon.rs
#[cfg(windows)]
pub fn run_windows_service() -> anyhow::Result<()> {
    windows_service_entry::run_dispatcher()
}

#[cfg(windows)]
mod windows_service_entry {
    const SERVICE_LABEL: &str = "mihomo";

    define_windows_service!(ffi_service_main, service_main);

    pub fn run_dispatcher() -> Result<()> {
        service_dispatcher::start(SERVICE_LABEL, ffi_service_main)  // 主线程阻塞至服务停止
    }

    fn service_main(_args: Vec<OsString>) {
        // 1. register control handler（Stop/Shutdown/Interrogate）
        // 2. 报告 Running（含 SERVICE_ACCEPT_STOP | SHUTDOWN）
        // 3. 自建 tokio runtime（Runtime::new()），跑 run_daemon_with_pipe(pipe_path, cancel)
        // 4. 收到 Stop/Shutdown → cancel.cancel() → 循环退出 → 停 core → 报告 Stopped
    }
}
```

> **Windows recover 路径**（P2-6 修正）：手动 `daemon --recover` 走同步 dispatcher 前
> 的检查——若 `--recover` 显式给出，说明用户手动恢复，允许走非 SCM raw loop
> （现有 `recover_daemon`），并在文档说明其绕过 SCM 的限制。

**停机链路修正（P0-2）**：现有 `run_daemon` 是无限 accept 循环，无 shutdown 通道。
改造为接收 `tokio_util::sync::CancellationToken`：
- accept 循环用 `select!` 监听 token → 收到取消则退出
- Stop 时决定 core 子进程处置（停 core 后退出，或留给系统）
- 设置 `wait_hint`（如 15s）避免 `sc stop` 挂起
- 上报 `Stopped` 在循环真正退出后

**unix 兼容**：unix 侧用 UnixListener socket（非 pipe），同样接 CancellationToken
（SIGTERM 触发取消），共享停机语义；各平台循环实现分别保留（P2-5 修正表述）。

#### M2: named pipe 安全（W2）—— SDDL + token 双校验

**设计**（对齐 Proxy-RS，P0-1 修正 API）：
1. **SDDL 层**：pipe 创建用 `create_with_security_attributes_raw`（unsafe）
   + `windows-sys` 的 `ConvertStringSecurityDescriptorToSecurityDescriptorW`
   构建 SDDL：`D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;<installer-SID>)`
   - 允许：SYSTEM（S-1-5-18）+ Administrators（S-1-5-32-544）+ 安装者 SID
   - **安装者 SID 获取与持久化**（P1-3 修正）：install 时用 `windows-sys` 的
     `GetTokenInformation`（TokenUser）取**安装进程**的 SID（CI/真机均为当前用户，
     非 SYSTEM），落盘到 `%ProgramData%\mihomo\installer-sid`（SYSTEM+Admin 可读）；
     daemon（SYSTEM 身份）启动时读该文件，构建 SDDL 时注入安装者 SID。
     （不能运行时再取——daemon 进程的 TokenUser 是 SYSTEM 自己）
2. **token 层**：install 时生成 32 字节随机 token，存双副本：
   - 服务端权威副本：`%ProgramData%\mihomo\service-token`（SYSTEM+Admin 可读）
   - 客户端副本：用户 config 目录 `service-client-token`（安装者 SID 可读）
   - CLI 连接 daemon 时，IPC 握手携带 token；daemon 校验后才接受命令
3. **pipe 抢占缓解**：创建时用 `first_pipe_instance`（P1-7）

**双副本布局**（决策 2）：
```
%ProgramData%\mihomo\service-token      ← 服务端权威（daemon 读）
<config_dir>\service-client-token       ← 客户端副本（CLI 读）
```

**CI SYSTEM 陷阱处理**：CI runner 以 runneradmin 运行，安装者 SID = runneradmin 的 SID
（不是 SYSTEM）——所以 SDDL 授权 runneradmin，CLI 能正常连接。真机以普通用户安装同理。
（这与"CI 以 SYSTEM 运行"的常见误区不同——GitHub Actions job 步骤以 runneradmin 身份跑。）

#### M3: 服务安装改造（W3）

**决策 1（P0-4 修正）**：`service-manager` **仅用于 Windows**（替代手写 sc.exe）：
- Windows install/uninstall：`<dyn ServiceManager>::native()`（0.11 实际 API）
  + `install/uninstall/start/stop`
- macOS/Linux：**保留现有手写 plan**（systemctl/launchctl Modern API，不违反 ADR-12）
- service-manager 依赖移入 `cfg(windows)`

#### M4: elevated 检测（W4）

`net session` → `windows-sys` 自查 `TokenElevation`（替代 is_elevated，P1-6）：
```rust
fn is_process_elevated() -> bool {
    // OpenProcessToken + GetTokenInformation(TokenElevation)
}
```

#### M5: 服务创建代码统一（P1-1）

清理两套并存：`instance.rs:900`（binPath `"cli" daemon`）与
`service.rs:696 windows_install_commands`（legacy binPath `"mihomo" -d`）。
统一到 service-manager（Windows）+ 删除 legacy 路径。

### 3.4 验证与测试设计（基于 token 双校验）

**单元测试（Linux 可跑，纯逻辑）**：
- SDDL 字符串构造（给定 SID 生成正确 SDDL）
- token 生成/校验（长度、随机性、比对）
- install/uninstall plan 含 token 文件路径断言

**Windows CI E2E（cross-platform-e2e.yml）**：
- System install → status（system service + daemon IPC running）
- start → status（Core running）
- **token 校验负测试**：用错误 token 连接 → 应被拒绝
- stop → status（Core stopped）
- uninstall → status（未安装）+ **token 文件清理验证**
- 服务失败恢复（可选）：kill daemon → 验证 sc failure 重启

**CI 参考**：Proxy-RS 的 CI 只做 cargo test + PowerShell 脚本单测，**不做真实服务 E2E**。
我们已超出它（Windows System E2E 已验证到 CreateService SUCCESS），
修好 W1 后将是真实服务启动 E2E。

## 4. 实施顺序

| 步骤 | 内容 | 依赖 |
|------|------|------|
| S1 | 引入 windows-service + windows-sys + service-manager（Cargo.toml 精确锁定） | 无 |
| S2 | daemon.rs 拆核心循环 + CancellationToken 停机链路 + Windows 服务入口（M1） | S1 |
| S3 | elevated 检测替换为 windows-sys TokenElevation（M4） | S1 |
| S4 | named pipe SDDL + token 双校验（M2，含安装者 SID 获取） | S2 |
| S5 | 服务安装 service-manager（Windows only）+ 清理 legacy sc.exe（M3/M5） | S2 |
| S6 | 单元测试（SDDL 构造/token 校验/plan 断言） | S2-S5 |
| S7 | CI gate：**每步提交后跑 cross-platform-e2e**（Windows System E2E 从 S2 起每步验证） | S2-S6 |
| S8 | 文档同步（USAGE/README/CHANGELOG/ADR）+ autostart 章节独立评审 | S7 |

> **CI gate 策略**（审查 P1-5 修正）：S2-S6 每步完成后**立即触发 CI**（手动 dispatch），
> Windows System E2E 是本 spec 的红-绿验证目标，不允许攒到最后才跑。
> **注意**：cross-platform-e2e.yml 在 **pub 仓库**——每步验证需先 sync dev→pub
> （rsync src + commit + push）再触发（P2-7 修正）。

## 5. 决策点

**已定**：
1. **service-manager 仅 Windows 用**（P0-4 修正）：macOS/Linux 保留手写 plan，不违反 ADR-12
2. **pipe 安全 = SDDL + token 双校验**（最安全，对齐 Proxy-RS）：SDDL 限制
   SYSTEM+Admin+安装者 SID；token 双副本（服务端 %ProgramData% + 客户端 config 目录）
3. **is_elevated 不用**，elevated 检测用 windows-sys（P1-6）
4. **CI 参考 Proxy-RS**：Proxy-RS 只做 cargo test + PS 单测（无服务 E2E）；我们保持
   更强的真实服务 E2E
5. **autostart 命令形式**：`autostart on|off|status` 子命令（已定）
6. **Windows user 自启机制**：注册表 Run 键 + `.vbs` 隐藏窗口（已定，对齐 Proxy-RS
   autostart.rs；隐蔽、不易误删、无黑窗）

**待定**：
- （已解决：ADR-16 Windows 服务架构 + ADR-17 跨平台 autostart，2026-08-03 定案）

## 6. 风险

- windows-service crate 是 Windows 专用——unix 构建零影响（cfg(windows) 隔离）
- daemon 主循环拆分需保证 unix 行为不变（launchd/systemd 路径回归测试）
- named pipe ACL 过严可能阻断 CLI 连接（需 CI 验证 CLI 侧权限）
- **服务失败恢复未配置**（P1-7a）：需 `sc failure` / SERVICE_FAILURE_ACTIONS，后续补
- **daemon 日志去向**（P1-7b）：`eprintln!` 在服务上下文丢失——服务模式日志应写到
  `%ProgramData%\mihomo\mihomo.log`，排障可查
- **named pipe 抢占钓鱼**（P1-7c）：daemon 未运行时低权限用户可抢先创建同名 pipe——
  `first_pipe_instance` 缓解 + token 校验兜底
- **错误码 87 vs 1053 未闭环**（P1-8）：建议 S2 前在真机确认一次根因（SCM 协议缺失 vs binPath），
  避免修完 SCM 层又出第二只虫子
