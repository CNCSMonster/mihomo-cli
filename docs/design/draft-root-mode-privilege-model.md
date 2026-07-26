# Draft SPEC: Root/User Instance Model v2

> 状态: Draft / 提案，分阶段实现中；尚未完成  
> 日期: 2026-07-24  
> 目标平台: macOS / Linux / Windows  
> 关联文档: [macos-socket-dir-fix.md](macos-socket-dir-fix.md)、[known-issues-fix-plan.md](known-issues-fix-plan.md)、[concurrent-config-lock.md](concurrent-config-lock.md)

## 0. 当前状态说明

本文是 **v2 权限与实例模型的实施 SPEC 草案**。当前代码正在分阶段切换到该模型；M1/M2 及部分 macOS M3 已实现，但 Linux/Windows parity、完整 privileged config store、迁移闭环和 E2E 仍未完成。近期 hotfix 与过渡实现只解决了旧模型上的具体问题：

- root LaunchDaemon 创建用户目录下的 root-owned `run/`，导致 CLI 无法访问 socket；
- `uninstall --all` 无法删除旧 root-owned 残留；
- 从 Clash Verge Rev 导入配置时保留了外部 `external-controller-unix` 路径，导致 CLI 查错 socket。

这些问题有同一根因：**root 实例和 user 实例没有被显式建模，root 服务仍使用用户 home 下的配置、binary、socket、log。**

v2 的目标是消除这个混合模型，而不是继续叠加局部补丁。

---

## 1. Problem Statement

mihomo-cli 当前同时承担：

1. 安装 mihomo core binary；
2. 管理配置、订阅、规则、DNS、override；
3. 安装/控制后台服务；
4. 通过 Unix socket / named pipe 调用 mihomo API。

但现有路径模型没有区分实例归属：

```text
macOS root 模式现状：
  服务: /Library/LaunchDaemons/io.mihomo.plist，root 启动
  binary: /Users/<user>/.local/bin/mihomo
  config: /Users/<user>/.config/mihomo/config.yaml
  socket: /Users/<user>/.config/mihomo/run/mihomo.sock
  log: /Users/<user>/.config/mihomo/mihomo.log
  CLI: 普通用户执行
```

这造成：

- root 服务在用户目录中创建 root-owned runtime 文件；
- 普通用户 CLI 无法访问 root-owned socket/run dir；
- uninstall/backup/restore/config 写操作遇到权限残留；
- root system service 被绑定到某个用户 home；
- 多用户登录时，root 服务实例和当前用户 CLI 指向可能不一致；
- 外部配置中的 runtime controller 字段会污染 mihomo-cli 的 API endpoint。

v2 必须以“实例”为一等概念，先确定操作的是 root 实例还是 user 实例，再派生路径、服务、权限和 API endpoint。

---

## 2. Goals / Non-goals

### 2.1 Goals

1. 支持 macOS / Linux / Windows 三平台。
2. 每个平台均定义 root/system 模式与 user 模式的安装、启动、停止、重启、状态、配置、卸载语义。
3. root 模式是系统级共享实例；user 模式是每用户独立实例。
4. root 模式不得依赖任一用户 home 作为 binary/config/socket/log 的主路径。
5. user 模式不得写系统目录，不要求管理员权限。
6. CLI 所有命令通过统一 `InstanceContext` 解析路径、服务目标和权限策略。
7. 导入/生成/合并配置时，mihomo-cli 拥有 runtime controller 字段所有权，外部配置不能覆盖 socket/pipe endpoint。
8. 迁移旧 root 模式时可回滚、可解释、可验证。
9. 所有写操作保持现有 post-validate + rollback 原则。

### 2.2 Non-goals for v2 Phase A

Phase A 不追求一次性实现全部高级能力：

- 不实现多 root 实例；root/system 模式始终全机唯一。
- 不默认支持 root 模式任意 `--config-dir`。
- 不要求第一阶段实现专用 group 权限模型。
- 不要求第一阶段完整重写所有 config/rule/dns 写路径；可以先实现 root install/start/status/restart/uninstall 与迁移闭环，再迁移写操作层。
- 不解决所有网络服务 UI 体验问题，例如 system proxy 枚举所有网络设备可作为独立任务。

---

## 3. Definitions

### 3.1 InstanceMode

```rust
enum InstanceMode {
    Root,
    User,
}
```

- `Root`: 系统级实例。一个 OS 安装中最多一个 root instance。唯一支持 TUN 的后台服务实例。
- `User`: 当前用户实例。每个用户可以有一个默认 user instance；后续可通过 `--config-dir` 支持多个隔离 user instance。

### 3.2 InstanceContext

所有命令必须先解析出 `InstanceContext`，后续逻辑不得直接调用全局 `utils::config_dir()` / `utils::mihomo_path()` 推断实例。

```rust
struct InstanceContext {
    os: TargetOs,
    mode: InstanceMode,
    paths: InstancePaths,
    service: ServiceTarget,
    api: ApiEndpoint,
    permissions: PermissionModel,
}
```

### 3.3 InstancePaths

```rust
struct InstancePaths {
    core_binary: PathBuf,
    config_dir: PathBuf,
    config_file: PathBuf,
    start_script: Option<PathBuf>,
    runtime_dir: Option<PathBuf>,
    socket_or_pipe: ApiEndpoint,
    log_file: Option<PathBuf>,
    service_file: Option<PathBuf>,
    service_marker: PathBuf,
    backup_dir: PathBuf,
}
```

### 3.4 ApiEndpoint

```rust
enum ApiEndpoint {
    UnixSocket(PathBuf),
    WindowsNamedPipe(String),
    Tcp(SocketAddr), // fallback only, not default
}
```

### 3.5 PermissionModel

```rust
enum PermissionModel {
    DirectUser,
    PrivilegedSystem,
}
```

- `DirectUser`: 普通文件 API 与普通 service manager command。
- `PrivilegedSystem`: 涉及系统目录和 system service 的写操作通过提权执行。

---

## 4. Cross-platform Path Matrix

### 4.1 macOS

| 资源 | root/system 模式 | user 模式 |
|------|------------------|-----------|
| core binary | `/Library/Application Support/mihomo/bin/mihomo` | `~/.local/bin/mihomo` |
| config dir | `/Library/Application Support/mihomo/config` | `~/.config/mihomo` |
| config file | `/Library/Application Support/mihomo/config/config.yaml` | `~/.config/mihomo/config.yaml` |
| subscriptions | `/Library/Application Support/mihomo/config/subscriptions/` | `~/.config/mihomo/subscriptions/` |
| rules/dns/override | root config dir 内 | user config dir 内 |
| geo data | root config dir 内 | user config dir 内 |
| start script | `/Library/Application Support/mihomo/start.sh` | `~/.config/mihomo/start.sh` |
| runtime dir | `/var/run/mihomo` | `~/.config/mihomo/run` |
| API socket | `/var/run/mihomo/mihomo.sock` | `~/.config/mihomo/run/mihomo.sock` |
| log | `/Library/Logs/mihomo/mihomo.log` | `~/.config/mihomo/mihomo.log` |
| service | `/Library/LaunchDaemons/io.mihomo.plist` | `~/Library/LaunchAgents/io.mihomo.plist` |
| service domain | `system/io.mihomo` | `gui/$UID/io.mihomo` |
| service marker | `/Library/Application Support/mihomo/config/.service-mode` | `~/.config/mihomo/.service-mode` |

### 4.2 Linux

| 资源 | root/system 模式 | user 模式 |
|------|------------------|-----------|
| core binary | `/usr/local/lib/mihomo/mihomo` | `~/.local/bin/mihomo` |
| config dir | `/etc/mihomo` | `~/.config/mihomo` |
| config file | `/etc/mihomo/config.yaml` | `~/.config/mihomo/config.yaml` |
| subscriptions | `/etc/mihomo/subscriptions/` | `~/.config/mihomo/subscriptions/` |
| rules/dns/override | root config dir 内 | user config dir 内 |
| geo data | root config dir 内 | user config dir 内 |
| start script | optional; systemd may call binary directly | optional; systemd user may call binary directly |
| runtime dir | `/run/mihomo` | `$XDG_RUNTIME_DIR/mihomo` |
| API socket | `/run/mihomo/mihomo.sock` | `$XDG_RUNTIME_DIR/mihomo/mihomo.sock` |
| log | journald preferred; optional `/var/log/mihomo/mihomo.log` | journald preferred; optional `~/.config/mihomo/mihomo.log` |
| service | `/etc/systemd/system/mihomo.service` | `~/.config/systemd/user/mihomo.service` |
| service domain | systemd system | systemd user |
| service marker | `/etc/mihomo/.service-mode` | `~/.config/mihomo/.service-mode` |

Linux root systemd unit SHOULD use:

```ini
RuntimeDirectory=mihomo
RuntimeDirectoryMode=0755
WorkingDirectory=/etc/mihomo
ExecStart=/usr/local/lib/mihomo/mihomo -d /etc/mihomo
```

If group-based socket access is later adopted, `RuntimeDirectoryMode` and group ownership must change accordingly.

### 4.3 Windows

Windows does not have Unix root/user semantics. v2 maps modes as follows:

| 资源 | system/Admin service 模式 | user/direct 模式 |
|------|---------------------------|------------------|
| core binary | `%ProgramData%\mihomo\bin\mihomo.exe` | `%LOCALAPPDATA%\mihomo\bin\mihomo.exe` |
| config dir | `%ProgramData%\mihomo\config` | `%LOCALAPPDATA%\mihomo\config` |
| config file | `%ProgramData%\mihomo\config\config.yaml` | `%LOCALAPPDATA%\mihomo\config\config.yaml` |
| runtime API | `\\.\pipe\mihomo-system` | `\\.\pipe\mihomo-%USERNAME%` 或 user SID suffix |
| log | `%ProgramData%\mihomo\logs\mihomo.log` | `%LOCALAPPDATA%\mihomo\logs\mihomo.log` |
| service | Windows Service `mihomo` | no persistent service in Phase A, or Scheduled Task in later phase |
| service marker | `%ProgramData%\mihomo\config\.service-mode` | `%LOCALAPPDATA%\mihomo\config\.service-mode` |

Windows Phase A scope:

- Admin/system mode uses Windows Service and requires elevation for install/start/stop/restart/uninstall.
- User mode may be implemented as direct detached process first; persistent user service can be added later via Scheduled Task.
- Named pipe path must be mode-specific to avoid user/system endpoint collision.

---

## 5. Instance Resolution Rules

### 5.1 CLI flags

Add symmetric mode selectors:

```bash
mihomo-cli --root status
mihomo-cli --user status
mihomo-cli install --root
mihomo-cli install --user
```

Compatibility:

- Existing `--user` subcommand flags continue to work.
- No explicit `--root` currently exists; v2 adds it for clarity.

### 5.2 Default resolution

Default resolution is not allowed to inspect service files ad hoc. It must consume the structured `InstanceInventory` described in §12.0.1 and preserve the selected `ResolutionSource`.

When mode is omitted:

1. If command is `install`, prompt user to choose root/user unless non-interactive flags specify mode.
2. If exactly one service exists, use that mode.
3. If both root and user services exist:
   - read explicit flag if provided;
   - otherwise show both and default to root only for read-only `status`, but require explicit mode for mutating commands.
4. If no service exists:
   - for read-only status, show both expected locations and suggest install;
   - for start/restart, use marker if present; otherwise prompt or error in non-TTY.

### 5.3 `--config-dir`

`--config-dir <path>` is **user/direct mode only**.

Invalid combinations:

```bash
mihomo-cli --root --config-dir ./foo status
mihomo-cli install --root --config-dir ./foo
```

These must fail with:

```text
--config-dir is only supported for user mode; root mode uses the fixed system config directory.
```


### 5.4 Environment override and test isolation

`MIHOMO_CLI_CONFIG_DIR` is a user-mode isolation override used by CLI tests and advanced users. It is part of the resolution contract, not a legacy shortcut.

Rules:

1. When `MIHOMO_CLI_CONFIG_DIR` is set to a non-empty path and mode is omitted, config-file commands must use that path as a user-mode `AppPaths` and must not infer a real installed root/user service from the host.
2. Explicit `--user` may use `MIHOMO_CLI_CONFIG_DIR`.
3. Explicit `--root` must ignore `MIHOMO_CLI_CONFIG_DIR`; root/system mode always uses fixed system paths.
4. Lifecycle commands must not accidentally control a real system service because a config-dir override is active. Tests that exercise lifecycle behavior must use explicit service fixtures or mocked service presence.
5. `status -v` and diagnostics should report when an env override affected resolution.

This rule prevents integration tests from reading or mutating a developer's real `~/.config/mihomo` or system instance.

### 5.5 Resolution source

Mode resolution should preserve why a mode/path was selected:

```rust
enum ResolutionSource {
    ExplicitFlag,
    EnvOverride,
    ServicePresence,
    ServiceMarker,
    InteractivePrompt,
    LegacyDetection,
}
```

`status -v` should print the source, for example `resolved by: explicit --root` or `resolved by: MIHOMO_CLI_CONFIG_DIR`, so mixed legacy states are debuggable.

### 5.6 Flag placement and compatibility

Canonical v2 syntax is subcommand-local mode selection:

```bash
mihomo-cli status --root
mihomo-cli status --user
mihomo-cli install --root
mihomo-cli install --user
mihomo-cli restart --root
mihomo-cli uninstall --root --all
```

If global flags such as `mihomo-cli --root status` are added later, they must be aliases for the same `ModeRequest` and must not create a second parsing path. Documentation should prefer the canonical subcommand-local form until global aliases are implemented and tested.


---

## 6. Permission Model

### 6.1 System/root mode

System mode requires privileges for:

- install/uninstall service;
- start/stop/restart system service;
- writing system config directory;
- writing system binary;
- writing logs/runtime directories at install time;
- migration from legacy user-home root mode.

CLI must use a single privileged execution helper that:

1. runs directly if already elevated/root/Admin;
2. tries non-interactive cached credentials;
3. prompts for password in TTY;
4. prints platform-correct manual command when non-TTY fails.

Manual command examples:

| Platform | Operation | Manual fallback |
|----------|-----------|-----------------|
| macOS root restart | restart | `sudo launchctl kickstart -k system/io.mihomo` |
| macOS root stop | stop | `sudo launchctl bootout system/io.mihomo` |
| Linux root restart | restart | `sudo systemctl restart mihomo` |
| Windows system restart | restart | Run terminal as Administrator, then `sc.exe stop mihomo` + `sc.exe start mihomo` |

### 6.2 User mode

User mode must not require admin/root privileges for normal lifecycle operations.

- macOS: LaunchAgent in current user domain.
- Linux: `systemctl --user` service.
- Windows: direct detached process in Phase A or per-user Scheduled Task in Phase B.

If a user-mode command detects root-owned files inside its config dir, it must report a legacy/root-mode contamination error and suggest migration or privileged cleanup. It must not silently continue with partial state.

---


### 6.3 PrivilegeExecutor Contract

Root/system mode must not be implemented by scattering ad-hoc `sudo`, `osascript`, `systemctl`, `launchctl`, or `sc.exe` calls across command handlers. All privileged lifecycle and file mutations must go through one execution boundary.

A concrete implementation may differ, but it must satisfy this contract:

```rust
trait PrivilegeExecutor {
    fn is_elevated(&self) -> bool;
    fn run(&self, plan: PrivilegedCommandPlan) -> Result<CommandOutput>;
    fn write_file(&self, plan: PrivilegedWritePlan) -> Result<()>;
    fn remove_path(&self, plan: PrivilegedRemovePlan) -> Result<()>;
}
```

Required behavior:

1. If already elevated (`root` on Unix, Administrator on Windows), execute directly.
2. In an interactive TTY, invoke the platform-correct elevation mechanism and allow the user to enter credentials:
   - macOS/Linux: `sudo` by default.
   - Windows: fail with an Administrator-shell instruction in Phase A unless a UAC helper is explicitly implemented.
3. In non-interactive mode, do not hang waiting for a password. Fail with a clear message and print the exact manual command the user can run.
4. Preserve stdout/stderr and exit status for diagnostics.
5. Never use broad privileged shell strings when structured argv is possible. Shell execution is allowed only for explicitly reviewed platform scripts.
6. Batch related privileged file operations into a plan when possible, so `install --root` and `uninstall --root --all` do not prompt repeatedly.
7. Every privileged operation must be auditable from `status -v` or verbose logs: operation kind, target paths, command argv, and failure reason.

Privilege error categories must be distinguishable in user output and tests:

| Category | Meaning | Example message |
|----------|---------|-----------------|
| `NotElevatedNonInteractive` | command needs privilege but cannot prompt | `Root service restart requires sudo. Run: sudo launchctl kickstart -k system/io.mihomo` |
| `AuthenticationFailed` | password/elevation failed | `sudo authentication failed; service was not changed` |
| `PermissionDeniedPath` | path cannot be read/written | `Cannot write /Library/Application Support/mihomo/config/config.yaml without elevation` |
| `CommandFailed` | service manager returned non-zero | include command and stderr excerpt |

Command handlers may ask the executor whether an operation will require elevation in order to print an up-front prompt such as:

```text
Root mode writes system files and controls a system service. sudo may ask for your password.
```

They must not bypass the executor for root/system operations. Until a mutating command is migrated to an instance-aware privileged store, it must refuse to run when the resolved target is root/system instead of silently writing the legacy user config directory. Read-only commands may be migrated earlier by resolving `AppPaths` from `InstanceContext`.

---

## 7. Root API Socket / Pipe Access Policy

### 7.1 macOS/Linux default policy for Phase A

Phase A adopts **trusted local users** policy:

```text
runtime dir: 0755
socket: created with umask 000, expected effective mode 0777 subject to OS behavior
```

Rationale:

- `status`, `select`, `delay`, `proxy on` should be usable without repeated sudo on single-user machines.
- root instance is intentionally shared across local users.

Security tradeoff:

- Any local OS user able to connect to the root API socket can influence the shared mihomo instance.
- This is acceptable only for single-person or mutually trusted local-user environments.
- Users requiring account isolation must use user mode.

### 7.2 Future stricter policy

A later phase may support group-based access:

```text
group: mihomo
runtime dir: root:mihomo 0750
socket: root:mihomo 0770
```

This requires user group membership setup and a re-login/session refresh. It is not Phase A default.

### 7.3 Windows named pipe ACL

Windows system pipe should allow the interactive installing user and Administrators by default. A later multi-user shared system mode can grant Authenticated Users if explicitly selected. The ACL must be explicit; do not rely on default named pipe security descriptors.

---


### 7.4 Socket readiness and permission verification

The root socket access policy is not considered applied merely because the start script contains `umask 000`. After service start/restart, mihomo-cli must verify the actual endpoint state.

For macOS/Linux Unix sockets, readiness verification must check:

1. expected socket path exists;
2. path type is socket where the platform exposes this metadata;
3. current CLI user can connect to the socket;
4. the API responds to a lightweight request;
5. if connection fails, report whether the failure is missing path, permission denied, connection refused, timeout, or endpoint mismatch in config.

For root/system mode under Phase A trusted-local-users policy, the implementation should additionally inspect effective socket and runtime directory modes when available:

```text
runtime dir expected: world searchable, normally 0755
socket expected: current user connectable, normally 0777 or equivalent ACL
```

If the socket is created with stricter permissions than expected, `status -v` must say that the service is running but the CLI cannot access the API endpoint, and must suggest the precise next action. Examples:

```text
API socket exists but is not connectable by the current user: /var/run/mihomo/mihomo.sock
Try: sudo launchctl kickstart -k system/io.mihomo
If it persists, run: mihomo-cli doctor --root
```

Windows named pipe readiness must check that the pipe exists and that the current user can open it. Permission failures must mention Administrator/service pipe ACLs rather than Unix chmod guidance.

---

## 8. Runtime-owned Config Fields

mihomo-cli owns runtime endpoint fields. External configs, subscriptions, Clash Verge Rev imports, and `override.yaml` must not be allowed to override them.

Runtime-owned fields:

```yaml
external-controller
external-controller-unix
external-controller-pipe
external-ui
```

Rules:

1. Before writing final `config.yaml`, remove all runtime-owned fields from the input.
2. Inject exactly one endpoint field for the resolved `InstanceContext`:
   - Unix platforms: `external-controller-unix: <ctx.api.socket>`
   - Windows: `external-controller-pipe: <ctx.api.pipe>`
3. `external-controller: ''` from Clash Verge Rev must not survive if CLI uses Unix socket/pipe.
4. `override.yaml` must be deep-merged before runtime field injection, so override cannot replace the API endpoint.
5. `config --fix` must repair wrong existing endpoint values, not only missing keys.
6. `status -v` must print both expected endpoint and, if detectable, actual endpoint from config.

Port fields:

- `mixed-port` is not strictly runtime-owned, but mihomo-cli may inject a default `mixed-port: 7897` if no `mixed-port` / `port` / `socks-port` is present.
- `get_port()` must read fallback order `mixed-port -> port -> socks-port`.

---

## 9. ConfigStore / Privileged Write Abstraction

Root config writes cannot be implemented by sprinkling `sudo tee` through command handlers. All config writes must go through a storage abstraction.

```rust
trait ConfigStore {
    fn read_to_string(&self, path: &Path) -> Result<String>;
    fn exists(&self, path: &Path) -> bool;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn atomic_write(&self, path: &Path, content: &str) -> Result<()>;
    fn remove_file(&self, path: &Path) -> Result<()>;
    fn remove_dir_all(&self, path: &Path) -> Result<()>;
    fn copy_file(&self, from: &Path, to: &Path) -> Result<()>;
    fn copy_dir_filtered(&self, from: &Path, to: &Path, filter: CopyFilter) -> Result<()>;
}
```

Implementations:

| Store | Used by | Write method |
|-------|---------|--------------|
| `UserConfigStore` | user mode | `std::fs` |
| `SystemConfigStore` | root/system mode | privileged helper: temp file + validate + privileged move |

Planning layer:

Before executing writes, command handlers must be able to derive a side-effect-free `ConfigStorePlan` from `InstanceContext`:

```rust
struct ConfigStorePlan {
    mode: InstanceMode,
    root: PathBuf,
    strategy: ConfigWriteStrategy,
    operations: Vec<ConfigStoreOperationPlan>,
}

enum ConfigStoreOperationKind {
    Read,
    EnsureDir,
    AtomicWrite,
    RemoveFile,
    RemoveDirAll,
    CopyFile,
    CopyDirFiltered,
    Validate,
    RestartService,
}
```

Required invariants:

- root/system plans use `PrivilegedStagedWrite` for writes, validation, and restart operations;
- user plans use `DirectAtomicWrite` and no privileged operations;
- final `config.yaml` writes and validation are marked rollback-capable;
- the plan root is exactly `ctx.paths.config_dir`; it must never be recomputed from global user helpers after instance resolution.

Atomic write contract:

1. write temp file in same target directory;
2. fsync/sync if supported;
3. rename/move atomically;
4. on validation failure, restore previous snapshot;
5. never leave partial config as final `config.yaml`.

Root/system store may need a local temp staging area and privileged `install`/`mv`:

```bash
sudo install -d -m 755 <dir>
sudo tee <path>.tmp
sudo mv <path>.tmp <path>
```

---

## 10. Service Semantics

### 10.1 macOS root LaunchDaemon

Install:

```bash
sudo install -d -m 755 "/Library/Application Support/mihomo"
sudo install -d -m 755 "/Library/Application Support/mihomo/bin"
sudo install -d -m 755 "/Library/Application Support/mihomo/config"
sudo install -d -m 755 "/Library/Logs/mihomo"
sudo install -d -m 755 "/var/run/mihomo"
sudo launchctl bootstrap system /Library/LaunchDaemons/io.mihomo.plist
```

Start:

- if loaded: `sudo launchctl kickstart system/io.mihomo`
- if not loaded but plist exists: `sudo launchctl bootstrap system /Library/LaunchDaemons/io.mihomo.plist`

Restart:

```bash
sudo launchctl kickstart -k system/io.mihomo
```

Stop:

```bash
sudo launchctl bootout system/io.mihomo
```

Uninstall:

```bash
sudo launchctl bootout system/io.mihomo   # best effort
sudo rm -f /Library/LaunchDaemons/io.mihomo.plist
sudo rm -rf "/Library/Application Support/mihomo"
sudo rm -rf /Library/Logs/mihomo
sudo rm -rf /var/run/mihomo
```

### 10.2 macOS user LaunchAgent

Install:

```bash
launchctl bootstrap gui/$UID ~/Library/LaunchAgents/io.mihomo.plist
```

Start/restart:

```bash
launchctl kickstart -k gui/$UID/io.mihomo
```

Stop/uninstall:

```bash
launchctl bootout gui/$UID/io.mihomo
rm ~/Library/LaunchAgents/io.mihomo.plist
```

### 10.3 Linux root systemd

Install:

```bash
sudo install -d -m 755 /etc/mihomo
sudo install -d -m 755 /usr/local/lib/mihomo
sudo systemctl daemon-reload
sudo systemctl enable --now mihomo
```

Unit requirements:

```ini
[Service]
Type=simple
ExecStart=/usr/local/lib/mihomo/mihomo -d /etc/mihomo
Restart=on-failure
RuntimeDirectory=mihomo
RuntimeDirectoryMode=0755
WorkingDirectory=/etc/mihomo
```

Start/stop/restart:

```bash
sudo systemctl start mihomo
sudo systemctl stop mihomo
sudo systemctl restart mihomo
```

### 10.4 Linux user systemd

Install:

```bash
systemctl --user daemon-reload
systemctl --user enable --now mihomo
```

Unit:

```ini
ExecStart=%h/.local/bin/mihomo -d %h/.config/mihomo
Restart=on-failure
WorkingDirectory=%h/.config/mihomo
```

Start/stop/restart:

```bash
systemctl --user start mihomo
systemctl --user stop mihomo
systemctl --user restart mihomo
```

### 10.5 Windows service/user process

System/Admin install:

```powershell
sc.exe create mihomo binPath= "<ProgramData>\mihomo\bin\mihomo.exe -d <ProgramData>\mihomo\config" start= auto DisplayName= "Mihomo Proxy Service"
sc.exe start mihomo
```

System stop/restart/uninstall:

```powershell
sc.exe stop mihomo
sc.exe start mihomo
sc.exe delete mihomo
```

User mode Phase A:

- install binary/config under `%LOCALAPPDATA%\mihomo`;
- start as detached user process;
- stop with process identification constrained to the user config dir, not broad `taskkill /IM mihomo.exe` unless no safer handle exists;
- Phase B may replace this with per-user Scheduled Task.

---


### 10.6 Install/start/restart readiness contract

A lifecycle command that claims the service is started must verify API readiness, not only process existence or service-manager success.

`install --root --start`, `install --user --start`, `start`, and `restart` may print intermediate success, but final success requires:

1. required files were installed in the resolved `InstanceContext` paths;
2. service/process manager command succeeded;
3. the service/process is observable as running;
4. final `config.yaml` contains the expected runtime endpoint for the resolved instance;
5. the expected socket/pipe is reachable by the current CLI user under that mode's access policy;
6. the API responds before timeout.

Recommended user output:

```text
✅ Service installed: system/io.mihomo
✅ Process running
✅ API endpoint ready: /var/run/mihomo/mihomo.sock
```

If service manager success is followed by API failure, the command must not print an unqualified success message. It should return non-zero for explicit `start`/`restart`; for interactive `install` it may keep installed files but must clearly report partial success:

```text
⚠ Service was installed, but API readiness failed.
  Expected endpoint: /var/run/mihomo/mihomo.sock
  Actual config endpoint: /tmp/verge/verge-mihomo.sock
  Logs: /Library/Logs/mihomo/mihomo.log
  Run: mihomo-cli status -v --root
```

This contract prevents the legacy failure mode where LaunchDaemon was loaded but CLI commands could not reach the API.

---

## 11. Command Behavior Matrix

| Command | root/system mode | user mode |
|---------|------------------|-----------|
| `install` | requires privilege; installs system binary/config/service | no privilege; installs user binary/config/service/process |
| `start` | requires privilege; starts system service | no privilege; starts user service/process |
| `stop` | requires privilege; stops system service | no privilege; stops user service/process |
| `restart` | requires privilege; restarts system service | no privilege; restarts user service/process |
| `status` | connects root shared socket/pipe; no privilege under Phase A socket policy | connects user socket/pipe |
| `select` / `delay` / `conn` | no privilege under Phase A socket policy | no privilege |
| `config --fix` | writes system config; requires privilege | direct write |
| `config -u` / `config --import` | writes system config; requires privilege | direct write |
| `rule` / `dns` mutations | writes system config; requires privilege | direct write |
| `backup` | can read system config without privilege only if readable; write backup destination as current user | direct read/write |
| `restore` | requires privilege | direct write |
| `tun on/off` | allowed; require explicit confirmation or sudo gate if policy chooses | fail early with clear message |
| `uninstall --all` | requires privilege; removes system instance | no privilege; removes user instance |

Read-only root status/select/delay depends on socket/pipe access policy. If stricter group/root-only policy is later enabled, these commands must emit actionable permission guidance.

---

### 11.1 Transitional command coverage matrix

Until all commands are fully migrated, every command must be explicitly classified. `fail closed` means the command must refuse root/system targets rather than falling back to the user config directory.

| Command path | InstanceContext required | root/system behavior during transition | user behavior | Transitional guard |
|--------------|--------------------------|-----------------------------------------|---------------|--------------------|
| `install --root/--user` | yes | privileged staged install | direct install | none |
| `start/stop/restart --root/--user` | yes | privileged service command + readiness for start/restart | user service/process command + readiness | none |
| `status --root/--user` | yes | resolved endpoint diagnostics | resolved endpoint diagnostics | none |
| `uninstall --root/--user` | yes | remove only v2 system paths/service | remove only user paths/service | legacy leftovers guard |
| `config --fix` | yes | privileged staged write | direct write | none |
| `config --validate/--list/--info` | yes | read resolved system config if readable; report permission errors | read resolved user config | env override must win for unspecified/user |
| `backup` | yes for read path | read resolved system config if readable; backup destination is current user | direct read/write | env override must win for unspecified/user |
| `config --add/--import/--switch/--refresh/--refresh-all/--remove/--set-ua` | yes before write | fail closed until privileged store exists | direct write | required |
| `rule` mutations | yes before write | fail closed until privileged store exists | direct write | required |
| `dns` mutations | yes before write | fail closed until privileged store exists | direct write | required |
| `restore` | yes before write | fail closed until privileged store exists | direct write | required |
| `select/list/delay/conn` | yes for API endpoint | connect resolved root socket/pipe; ambiguous root+user requires explicit mode | connect resolved user socket/pipe | must not hard-code legacy socket |
| `tun on/off` | yes | allowed only after explicit root/system resolution and policy gate | fail early with clear message | required |
| `logs` | yes | read resolved log source | read resolved log source | none |
| `system-proxy` | separate OS setting command | must document whether it targets current instance endpoint | same | TBD |

Audit rule: adding a command or flag that reads/writes config, controls service state, or connects to the API requires adding a row here and tests for explicit root/user plus omitted-mode behavior.

## 12. Migration Plan

### 12.0 Migration state machine

Migration logic must classify state before mutating anything:

| State | Meaning | Default command behavior |
|-------|---------|--------------------------|
| `None` | no service and no marker | install may proceed |
| `UserOnly` | only user instance exists | root commands require explicit root install/migrate |
| `LegacyRootOnly` | system service points into a user home | lifecycle/status report legacy layout and suggest migration |
| `V2RootOnly` | only v2 root instance exists | root commands operate normally |
| `LegacyRootAndUser` | legacy root service plus user config/service | require explicit migration plan; do not infer |
| `V2RootAndUser` | both v2 instances exist | read-only status may show/default root; mutating/API commands require explicit mode |
| `LegacyRootAndV2RootConflict` | old and new root layouts both present | refuse automatic action; require migration/cleanup command |
| `ContaminatedUserConfig` | user config contains root-owned runtime leftovers | user uninstall/config writes stop with cleanup guidance |

If the legacy service references a different user home than the current user, migration must display that source explicitly and require confirmation. It must not silently copy or delete another user's files.



### 12.0.1 InstanceInventory: mandatory inspection layer

The migration state machine must be implemented as a first-class inspection result, not as scattered boolean helpers. Every command that resolves an omitted mode or handles root/system lifecycle must start from a side-effect-free inventory scan:

```rust
struct InstanceInventory {
    root: RootInstanceObservation,
    user: UserInstanceObservation,
    legacy_root: Option<LegacyRootService>,
    user_contamination: Option<UserContamination>,
}

enum InstallationState {
    None,
    UserOnly,
    LegacyRootOnly,
    V2RootOnly,
    LegacyRootAndUser,
    V2RootAndUser,
    LegacyRootAndV2RootConflict,
    ContaminatedUserConfig,
}

struct LegacyRootService {
    service_file: PathBuf,
    referenced_paths: Vec<PathBuf>,
    referenced_home: Option<PathBuf>,
    referenced_current_user_home: bool,
}
```

Rules:

1. `current_service_presence()` and mode resolution must not treat a legacy root service as a v2 root service.
2. Legacy detection must return structured evidence (`service_file`, referenced paths, referenced home), not just `bool`.
3. `status -v` must display the structured legacy evidence when present.
4. If a legacy service points to another user's home, commands must say so and require explicit confirmation before migration or cleanup.
5. The inventory scan must be read-only; migration, cleanup, install, and uninstall perform mutations only after command-specific planning and confirmation.

Legacy service parsing should inspect fields that affect execution, not arbitrary comments:

| Platform | Fields to inspect |
|----------|-------------------|
| macOS LaunchDaemon | `ProgramArguments`, `WorkingDirectory`, `StandardOutPath`, `StandardErrorPath` |
| Linux systemd | `ExecStart`, `WorkingDirectory`, relevant `Environment` entries |
| Windows Service | service binary path / arguments and configured working directory if available |

A temporary conservative string detector may exist during M3 bring-up, but M3 is not complete until it is replaced by structured inventory output and tests.

### 12.1 Legacy states

Legacy root mode is identified by at least one of:

- macOS `/Library/LaunchDaemons/io.mihomo.plist` exists and points into `/Users/<user>/.config/mihomo/start.sh`;
- Linux `/etc/systemd/system/mihomo.service` exists and points to `~/.local/bin/mihomo` or `~/.config/mihomo`;
- `.service-mode` in user config dir says `root`;
- user config dir contains root-owned runtime artifacts (`run/`, socket, log) created by system service.

### 12.2 Migration trigger

Migration must be explicit in Phase A:

```bash
mihomo-cli migrate root-mode-v2
mihomo-cli install --root --migrate
```

Plain `status`, `start`, and `restart` must not silently migrate. They may detect legacy root mode and print:

```text
Legacy root-mode layout detected: system service uses files under your home directory.
Run: mihomo-cli migrate root-mode-v2
```

### 12.3 Migration steps

1. Resolve legacy owner/home from service file and current user.
2. Display plan: source paths, target paths, service changes, rollback strategy.
3. Ask confirmation unless `--yes`.
4. Create safety backup under legacy user config dir:
   - `~/.config/mihomo/backups/pre-root-v2-YYYYmmdd-HHMMSS/`
5. Stop legacy service best effort:
   - macOS: `sudo launchctl bootout system/io.mihomo`
   - Linux: `sudo systemctl stop mihomo`
   - Windows: `sc.exe stop mihomo`
6. Stage files in a temp dir:
   - include config.yaml, rules.yaml, dns-policy.yaml, override.yaml, subscriptions.yaml, subscriptions/, delay-cache.json if wanted;
   - exclude run/, sockets, logs, partial downloads, lock files, temporary files.
7. Normalize config:
   - remove runtime-owned fields;
   - inject v2 root endpoint;
   - ensure port fallback/default rules.
8. Install root/system binary into v2 path.
9. Install root/system config directory.
10. Install start script/unit/plist/service definition.
11. Start service.
12. Wait for API readiness at v2 endpoint.
13. If success:
   - leave legacy user config in place by default;
   - write migration marker;
   - print optional cleanup command.
14. If failure:
   - stop partially installed v2 service;
   - restore old service file if overwritten;
   - restart legacy service if it was running;
   - keep backup and staged files for diagnosis;
   - return non-zero.

### 12.4 Cleanup old state

After successful migration, cleanup must be explicit:

```bash
mihomo-cli migrate root-mode-v2 --cleanup-old
```

or:

```bash
mihomo-cli uninstall --legacy-root-leftovers
```

This avoids deleting user configuration prematurely.

---


### 12.5 Legacy root-owned leftovers cleanup rules

Legacy cleanup must distinguish root-owned runtime contamination from valuable user configuration. The cleanup command must be explicit and conservative.

Legacy root-owned runtime leftovers under a user config directory include:

```text
~/.config/mihomo/run/
~/.config/mihomo/run/mihomo.sock
~/.config/mihomo/mihomo.log
~/.config/mihomo/.service-mode          # only if it records root/system mode
~/.config/mihomo/start.sh               # only if referenced by a root LaunchDaemon/system service
```

User-owned configuration that must not be deleted by root cleanup unless the user explicitly requests user-mode removal includes:

```text
~/.config/mihomo/config.yaml
~/.config/mihomo/override.yaml
~/.config/mihomo/rules.yaml
~/.config/mihomo/dns-policy.yaml
~/.config/mihomo/subscriptions.yaml
~/.config/mihomo/subscriptions/
~/.config/mihomo/backups/
```

Command semantics:

| Command | Allowed cleanup | Must not do |
|---------|-----------------|-------------|
| `uninstall --root --all` | remove v2 system paths and root service | delete user config payload under `~/.config/mihomo` |
| `uninstall --user --all` | remove current user's user-mode files | remove system service or `/Library`/`/etc`/`%ProgramData%` paths |
| `uninstall --legacy-root-leftovers [--dry-run]` | remove only detected legacy root-owned runtime leftovers; `--dry-run` lists the plan without deleting | delete config/subscription/rule payload |
| `migrate root-mode-v2 --cleanup-old` | after successful migration, remove legacy service references and runtime leftovers | delete the source config unless an additional explicit user-config delete flag exists |

If `uninstall --user --all` hits root-owned leftovers, it must stop and explain the mixed state rather than partially deleting files and failing with a raw `Permission denied`:

```text
Legacy root-mode leftovers detected in your user config directory.
Run: mihomo-cli uninstall --legacy-root-leftovers
Then retry: mihomo-cli uninstall --user --all
```

All cleanup plans must support dry-run/verbose display before destructive privileged removal.

---

## 13. Validation and Diagnostics

### 13.1 `status -v`

Must print:

- resolved mode;
- mode source (explicit flag, service detection, marker, default);
- binary path and existence;
- config path and existence;
- expected API endpoint;
- actual configured API endpoint from config;
- endpoint alive/unreachable reason;
- service installed/loaded/running status;
- process running status;
- recent logs path;
- permission diagnostics if path exists but cannot be accessed.

### 13.2 Start readiness

`start`/`restart` must not consider success solely from process existence. It must verify:

1. service manager command succeeded;
2. process is running;
3. configured endpoint equals expected endpoint;
4. API endpoint responds before timeout;
5. if API fails, logs and config endpoint are shown.

### 13.3 Config validation

Every mutation that writes final `config.yaml` must:

1. parse YAML;
2. merge user state;
3. inject runtime-owned fields last;
4. write atomically;
5. run `mihomo -t -d <config_dir>` when binary exists;
6. rollback on failure.

---

## 14. Implementation Phases

### Phase A — macOS root reliability + shared abstraction

Deliverables:

- Introduce `InstanceContext`, `InstancePaths`, `ApiEndpoint`.
- Move path derivation behind context for install/status/start/restart/uninstall/config-fix.
- Implement macOS root paths outside user home.
- Keep macOS user paths as current behavior.
- Normalize runtime-owned config fields for all imports/fixes.
- Implement explicit legacy root migration for macOS.
- Ensure `uninstall --root --all` removes v2 root paths and can clean legacy leftovers.
- Update docs and tests.

Acceptance:

- Fresh macOS root install succeeds.
- `status`, `restart`, `config --fix`, `uninstall --all` work after root install.
- Clash Verge Rev import does not preserve `/tmp/verge/...` controller path.
- Legacy root layout migrates or reports clear instruction.


### 14.1 Phase A runtime switch checklist

Phase A is not complete until these macOS command paths use `InstanceContext` or an explicitly documented transitional adapter:

- `install --root` and `install --user`
- `start --root` and `start --user`
- `stop --root` and `stop --user`
- `restart --root` and `restart --user`
- `status --root -v` and `status --user -v`
- `config --fix --root` and `config --fix --user`
- `uninstall --root --all` and `uninstall --user --all`
- root legacy detection/migration diagnostics

Audit rule:

```text
No Phase A command above may derive its primary binary/config/socket/log/service path from global `utils::config_dir()` or `utils::mihomo_path()` after mode resolution.
```

Those global helpers may remain temporarily for commands not yet migrated, but their call sites must be tracked and must not be used to control a root/system instance.

### Phase B — Linux root/user parity

Deliverables:

- Apply `InstanceContext` to Linux root/user install and service control.
- Root config path `/etc/mihomo` and binary path `/usr/local/lib/mihomo/mihomo`.
- systemd `RuntimeDirectory=mihomo`.
- Linux migration from old layout.

### Phase C — Windows system/user parity

Deliverables:

- System/Admin paths under `%ProgramData%`.
- User paths under `%LOCALAPPDATA%`.
- Mode-specific named pipes.
- Windows Service for system mode.
- User direct process or Scheduled Task strategy.

### Phase D — ConfigStore privileged writes

Deliverables:

- `ConfigStore` abstraction.
- Root config/rule/dns/restore writes through privileged store.
- Preserve rollback semantics.
- Reduce direct global path calls.

### Phase E — Advanced isolation and ergonomics

Deliverables:

- `--config-dir` for user mode only.
- group-based root socket access option.
- instance lock.
- port conflict preflight.
- two-user E2E validation.

---

## 15. Test Matrix

### 15.1 Unit tests

- `InstanceContext` resolves explicit root/user flags.
- `InstanceContext` rejects `--root --config-dir`.
- path matrix returns expected paths for macOS/Linux/Windows root/user.
- service command plans match platform/mode.
- runtime field injection replaces wrong `external-controller-unix` / pipe.
- runtime field injection removes `external-controller: ''` when socket/pipe is used.
- `ConfigStore` atomic write rollback works for user store.
- privileged write plans are generated without executing sudo in unit tests.
- migration planner excludes runtime files and includes config state files.

### 15.2 CLI integration tests with isolated dirs

- user install path uses test config dir and never writes system paths.
- `config --fix` repairs wrong socket path.
- `uninstall --all` handles root-owned legacy leftovers with privileged plan mocked.
- `status -v` reports expected vs actual endpoint mismatch.

### 15.3 macOS E2E

Root mode fresh install:

```bash
mihomo-cli uninstall --root --all --yes
mihomo-cli install --root
mihomo-cli status -v
mihomo-cli restart
mihomo-cli status
mihomo-cli config --fix
mihomo-cli uninstall --root --all
```

Expected:

- plist in `/Library/LaunchDaemons`;
- no required runtime files under `~/.config/mihomo`;
- API listens at `/var/run/mihomo/mihomo.sock`;
- status succeeds without `Permission denied` under Phase A policy.

User mode fresh install:

```bash
mihomo-cli uninstall --user --all --yes
mihomo-cli install --user
mihomo-cli status -v --user
mihomo-cli restart --user
mihomo-cli tun on --user
mihomo-cli uninstall --user --all
```

Expected:

- LaunchAgent in user domain;
- socket under `~/.config/mihomo/run`;
- `tun on --user` fails early with clear message.

Legacy migration:

- create old root plist pointing to `~/.config/mihomo/start.sh`;
- config contains `/tmp/verge/verge-mihomo.sock`;
- run dir is root-owned;
- run `mihomo-cli migrate root-mode-v2`;
- verify root v2 layout and status success.

### 15.4 Linux E2E

Root mode:

```bash
sudo mihomo-cli uninstall --root --all --yes
mihomo-cli install --root
mihomo-cli status -v --root
mihomo-cli restart --root
mihomo-cli uninstall --root --all
```

User mode:

```bash
mihomo-cli install --user
mihomo-cli status -v --user
mihomo-cli restart --user
mihomo-cli tun on --user
mihomo-cli uninstall --user --all
```

### 15.5 Windows E2E

System/Admin mode from elevated shell:

```powershell
mihomo-cli install --root
mihomo-cli status --root -v
mihomo-cli restart --root
mihomo-cli uninstall --root --all --yes
```

User mode:

```powershell
mihomo-cli install --user
mihomo-cli status --user -v
mihomo-cli restart --user
mihomo-cli uninstall --user --all --yes
```

Expected:

- system paths under `%ProgramData%`;
- user paths under `%LOCALAPPDATA%`;
- named pipe endpoint differs by mode;
- non-admin system install fails with clear elevation instruction.

---

### 15.6 Service definition regression tests

Generated root/system service definitions must be tested to reject user-home references. For root/system mode, generated service files and command plans must not contain these tokens except in tests that explicitly model legacy detection:

```text
/Users/
/home/
~
%USERPROFILE%
%LOCALAPPDATA%
```

This is a release gate for M3/M4/M5 because the original macOS failure was a root service pointing into the installing user's home.

### 15.7 CLI exit-code contract

Lifecycle commands that claim to start a service must return success only after readiness succeeds.

| Scenario | Required result | Exit code |
|----------|-----------------|-----------|
| service installed/started and API ready | success message includes endpoint | 0 |
| service installed but API not ready | partial-success diagnostic with logs and endpoint | non-zero |
| configured endpoint differs from expected endpoint | mismatch diagnostic and `config --fix --<mode>` hint | non-zero |
| socket/pipe permission denied | permission diagnostic and platform-correct remediation | non-zero |
| missing config | install/config remediation hint | non-zero |

## 16. Rollout / Compatibility

1. Keep old flags working during transition.
2. `status -v` should detect and explain legacy root layout before enforcing migration.
3. `install --root` on a machine with legacy root layout must refuse to overwrite silently; require `migrate root-mode-v2` or explicit legacy cleanup. It must not overwrite `/Library/LaunchDaemons/io.mihomo.plist` or `/etc/systemd/system/mihomo.service` when that service points into a user home.
4. Release notes must call out path migration and cleanup behavior.
5. No automatic deletion of old user config after migration.
6. Keep a `mihomo-cli doctor` or `status -v` diagnostic path for mixed legacy states.

---

## 17. Open Decisions

| ID | Decision | Default for Phase A |
|----|----------|---------------------|
| OD-01 | root socket access: world vs group vs root-only | world/local-trusted, documented |
| OD-02 | Windows user persistent service mechanism | direct process in Phase A; Scheduled Task later |
| OD-03 | Linux root binary path | `/usr/local/lib/mihomo/mihomo` |
| OD-04 | macOS root log path | `/Library/Logs/mihomo/mihomo.log` |
| OD-05 | `backup` of root config without sudo | allow if readable; restore still privileged |
| OD-06 | whether `tun on` in root mode requires sudo gate when socket is world-accessible | require confirmation/sudo gate as UX policy, not security boundary |

---

## 18. Completion Criteria

v2 is complete only when:

- all three platforms have explicit root/user path definitions in code;
- install/start/stop/restart/status/uninstall/config-fix use `InstanceContext`;
- root mode no longer stores primary runtime state under user home;
- user mode never requires admin/root for normal lifecycle;
- runtime-owned config fields are always normalized last;
- root config writes use a privileged storage abstraction or a documented staged equivalent;
- legacy root migration is explicit, reversible, and tested;
- macOS, Linux, and Windows test matrices above are either automated or documented as manually verified.

---

## 19. Requirement Traceability

This section maps the user-facing requirement to explicit SPEC coverage and required evidence. v2 must not be marked complete until every row has direct code and test evidence.

| Requirement | SPEC sections | Required implementation evidence | Required verification evidence |
|-------------|---------------|----------------------------------|--------------------------------|
| macOS supports root install/start/stop/restart/status/config/uninstall | §4.1, §10.1, §11, §12, §15.3 | `InstanceContext` macOS root paths; LaunchDaemon command plans; SystemConfigStore or equivalent privileged writes; migration planner | macOS E2E root matrix passes on clean and legacy layouts |
| macOS supports user install/start/stop/restart/status/config/uninstall | §4.1, §10.2, §11, §15.3 | LaunchAgent command plans; user config paths; no privileged writes in user mode | macOS E2E user matrix passes without sudo except operations explicitly forbidden |
| Linux supports root install/start/stop/restart/status/config/uninstall | §4.2, §10.3, §11, §15.4 | systemd system unit; `/etc/mihomo`; `/run/mihomo`; privileged config writes | Linux root E2E matrix passes on systemd host |
| Linux supports user install/start/stop/restart/status/config/uninstall | §4.2, §10.4, §11, §15.4 | systemd user unit; XDG runtime socket; user config paths | Linux user E2E matrix passes without sudo |
| Windows supports system/Admin install/start/stop/restart/status/config/uninstall | §4.3, §7.3, §10.5, §11, §15.5 | Windows Service plans; `%ProgramData%` paths; named pipe ACL; elevated write paths | Windows elevated E2E matrix passes |
| Windows supports user install/start/stop/restart/status/config/uninstall | §4.3, §10.5, §11, §15.5 | `%LOCALAPPDATA%` paths; user pipe; direct process or Scheduled Task lifecycle | Windows non-admin E2E matrix passes |
| root/user mode selection is predictable | §5 | explicit `--root`/`--user`; resolver tests; conflict handling | tests for no service / one service / both services / explicit flags |
| root mode does not depend on user home | §4, §18 | no root paths derived from `dirs::home_dir()` except legacy detection/migration | grep/code audit + E2E path assertions |
| user mode does not require admin/root | §6.2, §11 | no privileged commands in user service plans | unit tests inspect plans; E2E runs without sudo |
| config imports cannot poison API endpoint | §8, §13.3 | runtime-owned fields removed and injected last | tests with Clash Verge `/tmp/verge/...` and Windows pipe mismatch |
| writes are safe and rollback-capable | §9, §13.3 | ConfigStore abstraction or documented equivalent; checked validation remains | rollback unit tests for user and privileged write plans |
| old layouts are handled | §12, §16 | explicit migration command and legacy detection diagnostics | legacy migration E2E and failure rollback test |

---

## 20. Implementation Guardrails

1. **No new global path helpers for instance-specific state.** New code must not add root/user decisions to `utils::config_dir()` or `utils::mihomo_path()` directly. It must route through `InstanceContext` or a transitional adapter with deprecation notes.
2. **No system service pointing into user home.** Tests must reject generated root/system service definitions containing `~`, `%USERPROFILE%`, `/Users/`, `/home/`, or `%LOCALAPPDATA%` paths, except where explicitly testing legacy detection.
3. **No unscoped process killing.** Stop logic must target the service manager or a process tied to the resolved config dir. Broad `pkill mihomo` / `taskkill /IM mihomo.exe` is only a last-resort diagnostic fallback and must be labeled as such.
4. **Runtime controller is injected last.** Any function that writes final `config.yaml` must apply external subscription/import/override first, then inject runtime-owned fields from `InstanceContext`.
5. **Privilege prompts must be platform-correct.** macOS commands must never suggest `systemctl`; Linux commands must never suggest `launchctl`; Windows commands must explain Administrator shell requirements.
6. **Migration is explicit.** Any code path that silently migrates root layout during `status`, `start`, or `restart` violates this SPEC.
7. **Partial root install must be recoverable.** If service bootstrap/start fails after system files are written, uninstall or rollback command must be able to remove the staged root paths.
8. **Docs follow behavior.** README/USAGE must not advertise v2 behavior until the corresponding phase is implemented and verified.

---

## 21. Release Gates

A milestone may be marked complete only when its gate is satisfied with current evidence.

### M3 macOS gate

Required on a real macOS host, including `kuku` before release:

```bash
mihomo-cli status
mihomo-cli status --root -v
mihomo-cli status --user -v
mihomo-cli uninstall --legacy-root-leftovers --dry-run
mihomo-cli install --root
mihomo-cli status --root -v
mihomo-cli restart --root
mihomo-cli list --root
mihomo-cli config --fix --root
mihomo-cli uninstall --root --all --yes
mihomo-cli install --user
mihomo-cli status --user -v
mihomo-cli restart --user
mihomo-cli tun on --user
mihomo-cli uninstall --user --all --yes
```

Pass criteria:

- legacy root LaunchDaemon pointing into `/Users/...` is reported as legacy, not as v2 root;
- fresh root install uses only `/Library/Application Support/mihomo`, `/var/run/mihomo`, `/Library/Logs/mihomo`, and `/Library/LaunchDaemons/io.mihomo.plist`;
- root lifecycle commands return success only after API readiness succeeds;
- user lifecycle commands do not require sudo;
- user cleanup does not fail on root-owned legacy runtime leftovers without an explicit cleanup diagnostic.

### M4 Linux gate

Required on a systemd Linux host:

- root service file contains `/usr/local/lib/mihomo/mihomo -d /etc/mihomo` and `RuntimeDirectory=mihomo`;
- user service file uses `%h`/user paths and `systemctl --user`;
- no root/system service definition contains `/home/`, `~`, or `$HOME`;
- root start/restart/stop use privileged systemd execution;
- user start/restart/stop run without sudo.

### M5 Windows gate

Required on Windows:

- non-admin `install --root` fails with Administrator-shell guidance;
- elevated `install --root` creates a Windows Service using `%ProgramData%\mihomo`;
- `install --user` uses `%LOCALAPPDATA%\mihomo` and does not require Administrator;
- root and user named pipe endpoints differ;
- user stop/restart targets only the mihomo process belonging to the resolved user config dir.

### M6 privileged ConfigStore gate

- No root/system config, rule, DNS, or restore mutation writes through `utils::config_dir()` or direct user-home paths.
- All privileged writes go through `PrivilegeExecutor` / `SystemConfigStore` or an explicitly reviewed staged equivalent.
- Rollback tests cover validation failure for root/system write plans.

## 22. Suggested Milestones

| Milestone | Scope | Exit criteria |
|-----------|-------|---------------|
| M1: Context skeleton | Add `InstanceContext`, path matrix tests, mode resolver, no behavior switch yet | Existing tests pass; new unit tests prove all platform/mode paths |
| M2: Runtime config ownership | Route controller injection through context; repair imports/overrides/fix/status diagnostics | Wrong endpoint tests pass across Unix/Windows plan cases |
| M3: macOS root v2 | System paths, LaunchDaemon, migration, uninstall, kuku E2E | macOS root/user E2E passes; no root runtime files in user home |
| M4: Linux parity | systemd root/user with context paths and migration | Linux root/user E2E passes |
| M5: Windows parity | `%ProgramData%`/`%LOCALAPPDATA%`, Service/user lifecycle, pipe ACL | Windows Admin/user E2E passes |
| M6: Privileged ConfigStore | root config/rule/dns/restore writes through storage abstraction | rollback tests pass for privileged write plans |
| M7: Hardening | group socket option, instance lock, port preflight | conflict and multi-user tests pass |

