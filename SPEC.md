# SPEC

> Mihomo CLI — cross-platform setup & control tool for Mihomo (Clash.Meta) proxy

---

## 1. Architecture

### 1.1 Instance Modes

当前采用两种**互斥**实例模式，运行时自动检测：

| 模式 | 运行身份 | TUN | 配置文件 | API Socket |
|------|---------|:---:|---------|------------|
| **System Service** | root (systemd/launchd) | ✅ | per-user `~/.config/mihomo/` | `/var/run/mihomo/mihomo.sock` |
| **Per-user** | 当前用户 | ❌ | `~/.config/mihomo/` | `$XDG_RUNTIME_DIR/mihomo/mihomo.sock` |

**互斥原因**：TUN 在网络层劫持所有流量，per-user 系统代理被忽略。

**关键设计**：配置始终 per-user（`~/.config/mihomo/config.yaml`），即使 System Service 模式。这样安装只需一次密码，日常操作（TUN 开关、配置编辑）无需提权。

### 1.2 Mode Resolution (Clash-Verge-like UX)

CLI 默认无需 `--system`/`--user` 标志，运行时自动检测：

```
1. System daemon IPC 可连接？ → System 模式
2. Per-user core API socket 可连接？ → User 模式
3. 两者都不可用 → 回退到已安装的服务文件
4. 无任何状态 → 默认 User 模式
5. 两者都可连接 → 报错（互斥冲突）
```

`--system` 标志仅用于脚本/排障场景显式覆盖。

### 1.3 Architecture Overview

```
┌──────────────────────────────────────────────────────────┐
│  mihomo-cli (Rust, single binary)                        │
│                                                          │
│  ┌─────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  │
│  │ main.rs │  │ daemon   │  │ config   │  │ installer│  │
│  │  CLI    │  │  IPC     │  │  订阅/合并│  │  内核下载 │  │
│  └────┬────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘  │
│       │            │              │              │        │
│  ─────┼────────────┼──────────────┼──────────────┼─────   │
│       │    IPC     │     API      │              │        │
│       ▼            ▼              ▼              │        │
│  ┌─────────┐  ┌──────────────────────────┐       │        │
│  │ Daemon  │  │  Mihomo Core (代理引擎)   │       │        │
│  │ (root)  │  │  /var/run/mihomo/         │       │        │
│  │ service │  │  mihomo.sock              │       │        │
│  └─────────┘  └──────────────────────────┘       │        │
└──────────────────────────────────────────────────────────┘

IPC (System 模式):
  CLI ←→ Daemon (/var/run/mihomo/service.sock) ←→ Mihomo Core

API (所有模式):
  CLI ←→ Mihomo Core (unix socket / named pipe)
```

### 1.4 IPC Protocol (System Daemon)

System 模式下，CLI 不直接管理 Mihomo Core，而是通过 daemon IPC：

```rust
// CLI → Daemon
enum DaemonCommand {
    StartCore  { config_path: PathBuf },
    StopCore,
    RestartCore { config_path: PathBuf },
    EnableTun  { config_path, stack, dns_hijack },
    DisableTun,
    GetStatus,
}

// Daemon → CLI
enum DaemonResponse {
    Success { message: String },
    Error   { message: String },
    Status  { running, tun_enabled, core_pid, config_path },
}
```

**传输**：Unix domain socket (`/var/run/mihomo/service.sock`)，length-prefixed JSON。

**安全边界**：
- daemon 不信任客户端传入的 binary 路径，固定使用 `/usr/local/lib/mihomo/mihomo`
- config_path 必须为 per-user 的绝对路径、不含 `..`、owner uid 匹配调用方
- daemon 启动 core 前校验 config 中的 API endpoint 等于当前系统端点

### 1.5 IPC 认证（跨平台）

**当前状态**：
- Windows：token 认证已实现（`windows_service_token()`）
- Unix：无 token 认证（socket 权限 `0o666`）

**计划改进**：
- Unix 平台扩展 token 认证机制
- Token 文件权限 `0o644`（所有用户可读取）
- Socket 权限 `0o666`（所有用户可连接）
- Daemon 侧 peer UID 检查（TUN 操作需要 root）

### 1.6 Lifecycle Concurrency & Readiness

对齐 clash-verge-service（OWNER_LIFECYCLE_LOCK + 服务端 readiness）的架构，
当前 System 模式 lifecycle 遵循以下设计（源自 docs/SPEC-system-lifecycle-ipc.md，
BUG-13 修复落地）：

**并发控制：CLI 永不持有生命周期锁。**

| 模式 | 串行化机制 | 说明 |
|------|-----------|------|
| System | daemon `OWNER_LIFECYCLE_LOCK`（进程内 tokio Mutex） | CLI 只发 IPC 命令，不持文件锁 |
| User | systemd job 队列 | systemd 保证服务状态变更串行 |

**daemon OWNER_LIFECYCLE_LOCK**：
- 所有生命周期命令（StartCore/StopCore/RestartCore/EnableTun/DisableTun）**全程持锁**
  ——含 core spawn 与 readiness 等待，并发客户端排队执行不交错
- GetStatus 只读，不经过生命周期锁
- 认证与 payload 校验在锁外完成（无效请求不占锁），对齐 clash-verge
  "validates its payload in between, and must keep doing so before it waits
  on a contended lock"

**readiness 归属：完全在 daemon，CLI 不重复检查。**

```
CLI start --system
  └─ IPC StartCore → daemon
       └─ spawn core → 轮询 core API 就绪（≤15s，500ms 间隔）
       └─ 就绪 → Success{ "core started and API ready" }
       └─ 超时 → kill core + Error
  └─ CLI 收到 Success 即完成（不轮询）
```

**超时边界**：daemon 生命周期操作 ≤15s < CLI 请求超时 20s。
客户端始终收到 daemon 的业务结论而非传输错误。

**daemon 崩溃恢复**：CLI 检测 daemon IPC 不可达 + core.pid 存在 → 提示
`mihomo-cli daemon --recover` 或 `stop --system`（见 cmd_lifecycle_resolved）。

### 1.7 Permission Matrix

| 操作 | Per-user | System Service |
|------|----------|---------------|
| 安装服务 | 无需 root | 需 root（一次性） |
| 启动/停止 core | 无需 root | 无需 root（IPC） |
| TUN on/off | ❌ 不可用 | 无需 root（IPC） |
| 编辑配置 | 无需 root | 无需 root |
| API 命令 | 无需 root | 无需 root |

---

## 2. Install Flow

### 2.1 Four-step Install

```
[1/4] Mihomo core binary  — 下载到 /usr/local/lib/mihomo/mihomo
[2/4] Service files       — 写 systemd unit / launchd plist
[3/4] Configuration       — 生成 config.yaml（订阅+规则+controller）
[4/4] Geo data files      — 预下载 geoip.metadb + GeoSite.dat
```

### 2.2 Idempotency & --force

| 组件 | 默认 `install` | `install --force` |
|------|:---:|:---:|
| [1/4] binary | 有效 → 跳过；无效/不存在 → 下载 | 无脑重下 |
| [2/4] service files | 存在 → 跳过 | 无脑重写 |
| [3/4] config | 有效 → 跳过（`mihomo -t` 验证）；无效 → 重新生成 | 无脑重写 |
| [4/4] geo | 完整 → 跳过；缺失/损坏 → 下载 | 无脑重拉 |

**geo 完整性判定**：先检查文件存在 + 尺寸（geoip > 8MB, GeoSite > 2MB），通过后再跑 `mihomo -t` 验证。

**当前状态**：✅ 已实现——install 逐项预检 binary/service/config/geo 有效性（上述表格即当前行为），且 `[3/4]` 对复用的旧 config 做 controller endpoint 校正（BUG-17 修复），避免 endpoint 不匹配导致 core 拒绝启动。

### 2.3 Geo Pre-download (ADR-04)

`[4/4]` 步骤在 install 时预下载 `geoip.metadb` 和 `GeoSite.dat`：

- **目的**：防止鸡生蛋死锁（mihomo 启动需要 geo 文件，但代理未启动无法下载 GitHub）
- **下载策略**（与 core binary 不同）：

| 维度 | Core binary | Geo files |
|------|------------|-----------|
| 断点续传 | HTTP Range on `.part` | 每个 URL 重新下载（`.tmp` 换源时删除） |
| 重试 | 同 URL 3 次退避 | 不同 URL 依次尝试，每个 2 次 |
| 验证 | GzDecoder CRC32 | 尺寸 + 首字节魔数 + `mihomo -t` |
| 临时文件 | `.part`，解压前删除 | `.tmp`，成功后 rename |

- **URL 优先级**：GitHub 直连 → `--proxy` URL（如有）→ `gh-proxy.com` → `mirror.ghproxy.com` → `ghproxy.com`
- **原子写入**：`.tmp` → `rename` 到目标文件

### 2.4 Core Binary Download (ADR-07, ADR-08)

- 下载地址：`https://github.com/MetaCubeX/mihomo/releases/download/{version}/mihomo-{target}-{version}.{ext}`
- 断点续传：HTTP Range + `.part` 文件，跨进程重启可恢复
- 3 次重试，退避 1s → 2s → 4s
- `.part` 在解压前立即删除（防损坏残留）
- 无 SHA256 校验（deferred）：HTTPS + Content-Length + Gzip CRC32 已提供合理基线

---

## 3. Config System

### 3.1 Multi-file Architecture

```
~/.config/mihomo/
├── config.yaml              ← 最终配置（合并产物）★ 单一事实来源（ADR-22）
├── rules.yaml               ← 用户规则
├── dns-policy.yaml          ← DNS 策略
├── override.yaml            ← 高级覆盖
├── subscriptions.yaml       ← 订阅元数据
├── subscriptions/<id>.yaml  ← 下载的订阅内容
├── subscriptions/active     ← 活跃订阅 ID
├── .rules-position          ← 规则插入位置 (front/back)
├── geoip.metadb             ← IP 地理位置数据库
└── GeoSite.dat              ← 域名分类数据库
```

**`config.yaml` 是单一事实来源**（ADR-22）：daemon 直接读取此文件，不再有独立的 system store 副本。旧 system store 路径（`/var/lib/mihomo-cli/config.yaml` 等）已废弃。

### 3.2 Config Generation

多订阅状态模型：

- `subscriptions.yaml` 保存订阅源元数据，例如 ID、URL、UA、更新时间；不保存完整节点配置。
- `subscriptions/active` 保存当前 active subscription ID，是指向订阅缓存的指针。
- `subscriptions/<id>.yaml` 保存该订阅上次成功下载/转换后的 Clash YAML，是 last known good local copy。
- `config.yaml`（`~/.config/mihomo/config.yaml`）是 active subscription cache 与本地 overlay layers（rules / DNS policy / fake-ip-filter / override）合并后的最终生成产物——也是所有模式的**单一事实来源**（ADR-22），daemon 直接读取此文件。
- 运行中的 mihomo core 是另一层 runtime 状态；`config.yaml` 写入成功不等于 runtime 已 reload。配置命令必须明确提示 reload 成功或需要 `restart`。

这种分层允许：切换订阅时不必立即联网；网络不好时仍可用上次成功缓存；刷新失败时可以保留旧的可用配置。

实际合并顺序：

1. active subscription cache 作为 base config。
2. 合并用户规则 `rules.yaml`。
3. 合并 DNS `nameserver-policy`（`dns-policy.yaml`）。
4. 合并 DNS `fake-ip-filter`（`dns-fake-ip-filter.yaml`）。
5. 注入默认 mixed port（当订阅未提供 `mixed-port` / `port` / `socks-port` 时）。
6. 注入 runtime controller 字段。
7. 应用高级覆盖 `override.yaml`。
8. 再次强制注入 runtime controller 字段。

第 8 步是防御性设计：`override.yaml` 允许高级用户覆盖普通 Mihomo 配置，但不能破坏 `mihomo-cli` / daemon / core 之间的 API 通信路径。因此 `external-controller-unix`、API socket/pipe、controller secret 等 CLI-managed runtime 字段会在 override 后重新注入。

合并语义：
- YAML map 递归合并
- list 和 scalar 直接替换
- runtime controller 字段由 CLI 管理，并会在 override 后重新注入

**写入目标**：合并产物直接写入 `~/.config/mihomo/config.yaml`（per-user config 目录）。不再有独立的 system store 副本——daemon 和 core 均读取此文件（ADR-22）。

### 3.3 config 单一事实来源（ADR-22）

所有运行模式（System Service / Per-user）下，`config.yaml` 统一存放在 per-user config 目录，是唯一权威配置源：

| 平台 | 路径 |
|------|------|
| Linux | `~/.config/mihomo/config.yaml` |
| macOS | `~/.config/mihomo/config.yaml` |
| Windows | `%USERPROFILE%\.config\mihomo\config.yaml` |

- daemon 通过目录权限访问用户 config（ADR-21 最小权限架构），不再维护独立的 system store 副本
- 所有配置变更（订阅切换、规则编辑、DNS 策略、override）均写入此文件
- 旧 system store 路径（`/var/lib/mihomo-cli/`、`/Library/Application Support/mihomo-cli/`、`%ProgramData%\mihomo-cli\`）已废弃，详见 ADR-22

### 3.4 Subscription Processing

1. `flag=clashmeta` + 多 UA 候选（`clash-meta` → `clash-verge` → `clash`）
2. 检测响应是否为 Clash YAML
3. 非 YAML → 尝试 base64 解码 → 转换 vmess/trojan 为 Clash YAML
4. 保存后运行 `mihomo -t` 验证

### 3.5 DNS Policy

CLI 接口：`mihomo-cli dns policy add <MATCH> <TARGET>`
- MATCH：域名后缀（如 `internal.example.com`）
- TARGET：DNS 服务器 IP（逗号分隔多个）
- 实现：写入 `dns-policy.yaml`，merge 时注入 `config.yaml` 的 `dns.nameserver-policy`

### 3.6 TUN 配置隔离

**背景**：TUN 是系统级功能，一旦开启会影响所有用户的流量。为防止低权限用户的配置影响系统级行为，TUN 配置采用隔离设计。

**设计**：
- TUN 使用的配置存储在系统级位置：
  - Linux: `/var/lib/mihomo-cli/tun-config.yaml`
  - macOS: `/Library/Application Support/mihomo-cli/tun-config.yaml`
  - Windows: `%ProgramData%\mihomo-cli\tun-config.yaml`
- TUN 配置是 per-user config 的**系统级快照**（derived artifact），不是 source of truth
- 启动 TUN 时，CLI 先更新启动用户的 per-user config，再通过特权写入将其复制为系统级 TUN config 快照；daemon 随后使用该 TUN config 启动/重载 core
- 只有 root 可以修改 TUN 配置（通过 CLI 自动 sudo）

**与单一事实来源的关系**：
- per-user config（`~/.config/mihomo/config.yaml`）仍然是单一事实来源
- TUN 配置是 derived artifact，类似 `.pyc` 文件是 `.py` 的编译产物
- 详见 ADR-22 补充决策

---

## 4. Service Management

### 4.1 Platform Support

| 平台 | System Service | Per-user Service |
|------|---------------|-----------------|
| Linux | systemd (`/etc/systemd/system/mihomo.service`) | systemd user (`~/.config/systemd/user/mihomo.service`) |
| macOS | LaunchDaemon (`/Library/LaunchDaemons/io.mihomo.plist`) | LaunchAgent (`~/Library/LaunchAgents/io.mihomo.plist`) |
| Windows | Windows Service | Direct process |

### 4.2 systemd Unit

```
[Unit]
Description=Mihomo CLI System Daemon
After=network-online.target
[Service]
Type=simple
ExecStart=/usr/local/bin/mihomo-cli daemon
Restart=on-failure
RuntimeDirectory=mihomo
RuntimeDirectoryMode=0755
[Install]
WantedBy=multi-user.target
```

`RuntimeDirectory=mihomo` 确保 `/var/run/mihomo/` 在服务启动时创建。

### 4.3 macOS launchd (Modern API)

| 操作 | 命令 |
|------|------|
| Install | `launchctl bootstrap system <plist>` |
| Uninstall | `launchctl bootout system/io.mihomo` |
| Start | `launchctl kickstart -k system/io.mihomo` |
| Stop | `launchctl kill SIGTERM system/io.mihomo` |

### 4.4 Socket Lifecycle

Unix socket 由 mihomo core 创建，CLI 负责连接。若 socket 被外部删除：
- 代理流量不受影响（内核态转发）
- CLI 管理命令不可用
- 恢复：`mihomo-cli restart`

### 4.5 Uninstall (planned)

**默认**：TUI 多选框，选择要删除的组件：

```
┌──────────────────────────────────────┐
│  Uninstall mihomo-cli (system)       │
│                                      │
│  [x] Stop & remove service           │
│  [ ] Remove core binary              │
│      /usr/local/lib/mihomo/mihomo    │
│  [ ] Remove config & data            │
│      /home/ubt/.config/mihomo/       │
│  [ ] Remove geo data                 │
│      geoip.metadb + GeoSite.dat      │
│                                      │
│  Space: toggle  Enter: confirm  Esc: cancel │
└──────────────────────────────────────┘
```

**CLI flags**：与 TUI 选项一一对应，作为预选：

| Flag | 预选 TUI 选项 |
|------|-------------|
| `--remove-binary` | 勾选 core binary |
| `--remove-config` | 勾选 config & data |
| `--remove-geo` | 勾选 geo data |
| `--all` | 全选（= 三个 flag 的快捷方式） |
| `--yes` | 跳过 TUI，直接执行 |
| `--dry-run` | 预览，不执行 |

**行为矩阵**：

```bash
mihomo-cli uninstall                        # TUI，仅 service 默认勾选
mihomo-cli uninstall --remove-binary        # TUI，service + binary 预选
mihomo-cli uninstall --all --yes            # 非交互，全删
mihomo-cli uninstall --remove-geo --yes     # 非交互，service + geo
mihomo-cli uninstall --dry-run              # 预览，不执行
```

**日志从不自动删除**：`/var/log/mihomo/mihomo.log` 和 `~/.config/mihomo/mihomo.log` 始终保留。

### 4.6 Concurrent Safety (ADR-13: flock)

文件变更操作（config、rule、dns、backup）使用 flock 排他锁保护：

- 锁文件：`{config_dir}/.mihomo-cli.lock`
- 机制：POSIX `flock(LOCK_EX)`，10s 超时
- 范围：覆盖"读输入 → 合并 → 写 config.yaml → 热重载"全部关键段
- 崩溃安全：fd 关闭自动释放，不残留锁
- 只读操作（status、select、delay）不加锁
- Windows 暂退化为无锁

所有配置落盘使用 `utils::atomic_write_file()`（写 `.tmp` → fsync → rename）。

---

## 5. exit-ip Command

`mihomo-cli exit-ip` 查询出口 IP，目标模式互斥：

| Flag | 含义 |
|------|------|
| `--node <NAME>` | 指定节点 exit IP |
| `--group <NAME>` | 代理组当前节点 exit IP |
| `--url <URL>` | URL 按规则解析后的估算出口 |
| `--direct` | 系统直连出口 IP |

这替代了旧 `mihomo-cli ip` 命令的模糊语义（旧命令只表示"通过当前 mihomo 访问 echo 服务的观测出口"，不反映真实系统出口或指定节点出口）。

---

## 6. Error Handling

所有错误遵循：**检测 → 报告 → 修复建议**

```text
❌ 错误描述
  原因分析
  修复建议（具体命令）
  排查命令（-v 或日志路径）
```

---

## 7. Core Design Principles

1. **前置条件检查**：每个命令执行前校验状态（如 start 前检查 geo 完整性）
2. **关键操作原子化**：`.tmp` → rename、flock 锁保护
3. **失败路径也测试**：不只测 happy path
4. **写后验证**：所有 config 写入后立即 `mihomo -t` 验证
5. **诊断输出结构化**：`--json` 支持机器可读

---

## 8. Installation Layout

```
/usr/local/lib/mihomo/mihomo          ← System mode core binary
/usr/local/bin/mihomo-cli             ← System mode CLI
~/.local/bin/mihomo                   ← User mode core binary

~/.config/mihomo/                     ← 配置目录（两种模式共享）
├── config.yaml, rules.yaml, dns-policy.yaml, override.yaml
├── subscriptions.yaml, subscriptions/
├── geoip.metadb, GeoSite.dat
├── .rules-position, .mihomo-cli.lock

/var/run/mihomo/                      ← System mode runtime
├── service.sock    (daemon IPC)
├── mihomo.sock     (core API)
└── core.pid

$XDG_RUNTIME_DIR/mihomo/mihomo.sock   ← User mode core API
/tmp/mihomo-$UID/mihomo.sock          ← macOS user mode core API

/etc/systemd/system/mihomo.service             ← Linux system service
/Library/LaunchDaemons/io.mihomo.plist         ← macOS system service
```

---

## 9. Glossary

| 术语 | 含义 |
|------|------|
| Mihomo | 代理核心（原 Clash.Meta） |
| Daemon IPC | System 模式下 CLI 与 root daemon 的通信 socket |
| Core API | CLI 与 mihomo 核心的通信 socket |
| Subscription | 远程代理节点列表 URL |
| TUN | 虚拟网卡透明代理 |
| Geo data | geoip.metadb + GeoSite.dat，规则匹配数据库 |
| flock | 文件锁，并发保护机制 |
| Hot Reload | `/configs` API 热更新配置 |
| Atomic Write | `.tmp` → rename 原子写入 |

---

## 10. Roadmap

### Done

| Feature | Notes |
|---------|-------|
| Architecture | 互斥双模式、自动检测、IPC daemon |
| Clash-Verge-like UX | 免 `--system` 日常操作 |
| Geo pre-download | 镜像 fallback、原子写入 |
| Core download resume + retry | HTTP Range、`.part` 管理 |
| Rule management | serde_yaml 校验、标记区块编辑 |
| DNS policy | nameserver-policy CLI 管理 |
| exit-ip command | 多目标模式（node/group/url/direct） |
| flock concurrent safety | 配置变更排他锁 + 原子写 |
| Config multi-file | 订阅/rules/dns/override 分层合并 |
| macOS modern launchctl | bootstrap/bootout/kickstart；BUG-16 后 Stop 用 `launchctl kill SIGTERM`（停进程不卸载 job），bootout 仅用于 uninstall |
| Install 智能前置检查 | 逐项检查 binary/service/config/geo 有效性，有效则跳过；`--force` 全量重装；复用旧 config 时自动校正 controller endpoint（BUG-17） |
| Uninstall TUI + 粒度控制 | TUI 多选框；`--remove-binary/--remove-config/--remove-geo` flags 预选；`--all --yes` 非交互全删；`--dry-run` 预览 |

### Planned

| Feature | Notes |
|---------|-------|
| Desktop notification | D-Bus/notification center |
| Rule group support | rule-provider / rule-groups |
| `--json` diagnostic output | 机器可读 status/ip |

---

## 11. Architecture Decision Records

### ADR-01: Unix socket over HTTP controller

Unix socket 作为唯一 API 通信方式，安全性更好，不暴露网络端口。

### ADR-02: 配置热重载 vs 重启

配置变更通过 `/configs` 热重载，controller 变更需 restart。

### ADR-03: 单二进制分发

纯 Rust，零运行时依赖。

### ADR-04: 预下载 Geo 数据

install 时预下载 `geoip.metadb` + `GeoSite.dat`，防止鸡生蛋死锁。详见 §2.3。

### ADR-05: 规则标记合并法

用户规则通过 `# === USER RULES START/END ===` 标记合并到 config.yaml。

### ADR-06: -v/--verbose 统一调试输出

`crate::log!()` 宏，`-v` 模式输出到 stderr。

### ADR-07: Unconditional resume check

`.part` 文件尺寸检查不依赖 attempt 计数器（跨进程可恢复）。

### ADR-08: .part cleanup before decompression

解压前删除 `.part`，防止损坏残留污染下次续传。

### ADR-09: No SHA256 checksum (deferred)

HTTPS + Content-Length + Gzip CRC32 提供合理基线。

### ADR-10: YAML 编辑策略 (serde_yaml)

使用 `serde_yaml` 校验 + 标记区块编辑 + endpoint 注入，移除 tree-sitter native 依赖。

### ADR-11: Remove fallback path

YAML 编辑失败显式报错，不静默降级。

### ADR-12: macOS launchd Modern API

统一使用 `bootstrap`/`bootout`/`kickstart`/`kill`。

### ADR-13: flock 配置并发保护

文件变更操作使用 `flock(LOCK_EX)` + 原子写入。详见 §4.5。

### ADR-14: AI 原生化方向 —— 做"被 AI 使用"的工具，不内置 AI

**状态**: ✅ 已决策 (2026-08-02)

参考 `3rdparty/clash-cli.rs` 时发现其内置了"AI 修改规则"功能，判断为错误方向，不借鉴。

mihomo-cli 的 AI 原生正确方向是作为**工具提供给上层 AI 使用**：
- 合适的 CLI 接口设计（机器可解析、确定性输出，未来可补 `--json` 结构化输出）
- 合适的 `-h` 帮助信息（AI 可直接阅读理解）
- 合适的 user guide（USAGE.md 面向 AI 可检索）
- 封装良好的 **Agent Skill**（供 Qwen/Claude/Codex 等调用）

**Why**: 内置 AI 会把工具与特定模型/服务耦合，且 AI 能力演进快于 CLI；而"良好 CLI + skill 封装"让任何上层 AI 都能复用工具能力，职责清晰。这也与本项目"零运行时依赖"的定位（ADR-03）一致——不把 AI 运行时塞进二进制。

### ADR-15: 单内核专注 —— 只做 mihomo，不引入第二内核

**状态**: ✅ 已决策 (2026-08-02)

参考 `3rdparty/Proxy-RS`（sing-box + mihomo 双内核管理器，Ratatui TUI）后确认：**当前目标只做好 mihomo 内核**，多内核管理方向不做。

**Why**: 双内核增加管理复杂度（两套路径/服务/配置模型）、测试矩阵翻倍、维护成本高；当前用户场景（mihomo 单内核）没有多内核需求。

**How to apply**: 参考 Proxy-RS 时只看其 CLI 设计、TUI 实现、内核管理思路，不借鉴多内核架构。

### ADR-16: Windows 服务架构 —— SCM 协议层 + named pipe 双校验

**状态**: ✅ 已决策 (2026-08-03)，详见 `docs/SPEC-windows-service.md`

**背景**: CI 实测 `StartService FAILED 87`——`mihomo-cli daemon` 是普通命令行进程，
不调用 SCM 协议（`StartServiceCtrlDispatcher`），SCM 等待服务报告"已启动"而永远等不到。

**决策**:
1. **保持单二进制**（ADR-03 延续）：daemon 仍是 `mihomo-cli daemon` 子命令，
   不拆独立服务 exe
2. **Windows daemon 通过 `windows-service` crate 实现 SCM 协议**：
   `service_dispatcher::start`（主线程，`#[tokio::main]` 的 block_on 满足"主线程"约束）
   → `service_main` 回调内自建 tokio runtime 跑核心循环；
   Stop/Shutdown 经 CancellationToken 停机链路优雅退出
3. **named pipe 安全 = SDDL + token 双校验**：
   - SDDL 限制 SYSTEM + Administrators + 安装者 SID（安装者 SID 由 install 落盘
     `%ProgramData%\mihomo\installer-sid`，daemon 运行时读取——不能运行时取，
     daemon 是 SYSTEM 身份）
   - 32 字节随机 token 双副本（服务端 `%ProgramData%\mihomo\service-token` +
     客户端 config 目录），IPC 握手校验
   - `first_pipe_instance` 缓解 pipe 抢占钓鱼
4. **elevated 检测用 windows-sys `TokenElevation`**（弃用 `net session` hack 与
   停更的 is_elevated crate）

**Why**: Windows SCM 是唯一合法的服务托管机制；单二进制 + 双校验兼顾 ADR-03 与安全性。
参考 Proxy-RS（同架构单二进制）但其 pipe 仅 token 无 SDDL，本方案在其上增强。

**How to apply**: 实施见 `docs/SPEC-windows-service.md` S1-S8；unix 平台零影响（cfg(windows) 隔离）。

### ADR-17: 跨平台开机自启统一控制（autostart 命令 + 默认不自启）

**状态**: ✅ 已决策 (2026-08-03)，详见 `docs/SPEC-windows-service.md` §3.0

**决策**:
1. **新增 `mihomo-cli autostart on|off|status` 子命令**，三平台统一：
   - Linux system/user: `systemctl enable/disable/is-enabled`
   - macOS system/user: `launchctl enable/disable/print`
   - Windows system: `sc config mihomo start= auto/demand` + `sc qc`
   - Windows user: 注册表 `HKCU\...\Run` 键 + `.vbs` 隐藏窗口
2. **install 默认不开机自启**——用户显式 `autostart on` 才开启
   （systemd 去 enable --now / launchd RunAtLoad=false / sc start= demand）

**Why**: 三平台自启能力统一、默认不打扰用户（用户主动开启才自启）。
Windows user 用注册表 Run 键 + `.vbs`（Proxy-RS 同款）——隐蔽、不易误删、登录静默无黑窗。

**How to apply**: install 语义变化（默认不自启）需同步更新 USAGE/README 文档与测试；
`autostart` 命令走 InstanceContext 模式解析（`--system/--user` 可选）。

### ADR-18: 多用户 TUN 架构 —— per-user core 独立 + system daemon 独占 TUN 原子开关

**状态**: ✅ 已决策 (2026-08-03，codex GPT-5.5 分析 + 用户确认)

**背景**: TUN 网卡是系统级单例——只能一个进程创建/管理（utun/wintun + 系统路由）。
当前单用户部署，长期支持多用户：同一台 Linux/macOS 多用户，每用户独立代理配置
但共享 TUN 能力。

**决策**:
1. **A 架构**（GPT-5.5 推荐）：
   - **per-user core 各自独立**（无 TUN）——保留用户隔离（每用户独立 core/config）
   - **system daemon 独占 TUN**——统一管理 TUN 网卡 + 系统路由
   - 否决 C 架构（全用户共享一个 system core）——牺牲用户配置隔离
2. **TUN 仲裁：原子开关**（用户决策，弃用引用计数）：
   - `tun on` = 原子置开，`tun off` = 原子置关
   - **最后操作者生效**（谁最后 on/off 决定状态）
   - 不用引用计数（谁 on 了 +1 / off 减 1 / 归零释放）——简单直接
   - 单用户落地无需权限校验（后续多用户可加授权用户）
3. **autostart 标记：per-user**（GPT-5.5 建议）：
   - 存各自用户配置目录（`~/.config/mihomo/autostart.json`）
   - **不用全局文件混存多用户状态**（如 /etc/mihomo/）
   - 天然为多用户预留，单用户实现无需将来迁移

**Why**: TUN 单例约束 + 用户隔离需求 + 用户明确偏好简单模型（原子开关而非引用计数）。

**How to apply**:
- 当前单用户落地与 A 架构一致（system daemon + 单个 per-user core），无需预建多用户
- 多用户演进：daemon 支持多 core 实例（每用户一个）+ TUN 原子开关
- autostart 标记用 per-user 位置（`~/.config/mihomo/autostart.json`）

### ADR-20: Non-interactive Automation Contract

**状态**: ✅ 已决策 (2026-08-04)

**背景**: `mihomo-cli` 的主接口同时服务人类、脚本和 AI agent。此前发现
`upgrade` 有 `Upgrade? [y/N]` 交互确认但没有 `-y/--yes`，导致自动化场景不可用。

**决策**:
1. 任何运行中会询问用户的命令，都必须提供启动时可传入的 flag 来预先表达选择。
2. 确认类 prompt 使用 `-y/--yes` 跳过。
3. 分支选择使用显式 flag，例如 `--system/--user`、`--activate/--no-activate`、`--lan-direct/--no-lan-direct`。
4. `--json` 模式不得阻塞等待 TUI/confirm；stdout 只输出 JSON。
5. sudo/admin 密码或 OS 授权弹窗是允许的例外；无 TTY 且缺少必要选择时应失败并给出可执行建议。
6. 新增交互前必须同时补测试覆盖其非交互路径。

**Why**: 这是 AI/脚本可编排的基础契约；避免命令在自动化环境中卡死，也避免未来新增功能只适合人类手工操作。

**How to apply**: 扫描所有 `Confirm::new`、TUI、stdin prompt、`is_terminal()` 分支；为缺少预选 flag 的命令补 `--yes` 或显式选择 flag，并加入 CLI parse/行为测试。

### ADR-21: System daemon 最小权限——AmbientCapabilities 替代 root daemon

**状态**: ✅ 已决策 (2026-08-07)

**背景**: 当前 system daemon 以 root 运行（systemd `User=root`），daemon 本身不做任何
需要 root 的操作——文件 I/O（PID/log/socket）只需目录写权限，IPC 通信不需特权。
真正需要 root 的只有 mihomo core：创建 TUN 设备（`/dev/net/tun`）、修改系统路由表、
绑定特权端口 53（DNS hijack）。daemon 以 root 运行违反最小权限原则，一旦 daemon
被攻破（如 IPC socket 被滥用），攻击者获得完整 root 权限。

**调研参考**:

| 方案 | 来源 | 做法 | 安全性 |
|------|------|------|--------|
| systemd AmbientCapabilities | mihomo 官方文档 [1] + Arch 包 [2] | core 以专用用户运行，`AmbientCapabilities` 授权精确 capabilities | 最小权限 |
| setcap on binary | clashtui [3] | `setcap cap_net_admin,cap_net_bind_service=+ep /usr/bin/mihomo`；无 daemon，直接 systemd 管理 core | 最小权限 |
| root helper daemon | Clash Verge Rev [4] | 独立 root helper service，GUI 通过 HTTP API 通信 | CVE-2025-50505 [5]：未认证 API 导致本地提权到 root |
| Native TUN | NixOS module [6] | `tunMode` 选项授予 service 必要权限 | 同 AmbientCapabilities |

**决策**:

1. **daemon 以专用非 root 用户运行**（如 `mihomo`）
   - systemd: `User=mihomo` + `Group=mihomo`
   - macOS launchd: 对应非 root LaunchDaemon
2. **mihomo core 通过 systemd AmbientCapabilities 获得精确权限**：
   ```ini
   [Service]
   User=mihomo
   Group=mihomo
   CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE
   AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE
   ```
   - `CAP_NET_ADMIN`：创建 TUN 设备 + 修改路由表
   - `CAP_NET_RAW`：原始套接字（TUN 所需）
   - `CAP_NET_BIND_SERVICE`：绑定端口 53（DNS hijack）
3. **安装时一次性提权**（`install --system`）：
   - 创建 `mihomo` 用户/组
   - 设置目录权限（`/var/run/mihomo/`、`/var/log/mihomo/` 归属 `mihomo`）
   - 写入 systemd unit（含 AmbientCapabilities）
   - 已有的交互式提权逻辑（弹密码提示）覆盖此步骤
4. **日常运行零 root**：
   - CLI → daemon IPC：Unix socket，daemon 用户可读写
   - daemon → core：spawn 子进程（继承 capabilities）
   - core → TUN/路由/端口：capabilities 授权，无需 root
5. **macOS 方案**：
   - TUN 需要 Network Extension（Apple 机制），仍需 root helper
   - 但 helper 仅在 TUN 开关时按需提权（`osascript` 授权弹窗），非常驻 root daemon
6. **Windows 方案**：
   - 保持 ADR-16 的 SCM 服务架构（Windows 服务模型要求 SYSTEM 身份）
   - named pipe SDDL + token 双校验已是安全边界

**Why**:
- 最小权限原则：daemon 被攻破 ≠ root 泄露
- 行业验证：mihomo 官方文档 + clashtui + Arch Linux 包均采用 AmbientCapabilities/setcap
- 警示案例：Clash Verge Rev CVE-2025-50505（root helper daemon + 未认证 API = 本地提权）
- mihomo core 本身支持以非 root 用户运行（官方 systemd 示例已包含 AmbientCapabilities）

**参考文献**:
- [1] mihomo 官方文档 — Create a running service: https://wiki.metacubex.one/en/startup/service
- [2] Arch Linux mihomo deb 包 — Issue #1915: https://github.com/MetaCubeX/mihomo/issues/1915
- [3] clashtui — JohanChane/clashtui: https://github.com/JohanChane/clashtui
- [4] Clash Verge Rev — clash-verge-rev/clash-verge-rev: https://github.com/clash-verge-rev/clash-verge-rev
- [5] CVE-2025-50505 — Clash Verge Rev privilege escalation: https://www.sentinelone.com/vulnerability-database/cve-2025-50505
- [6] NixOS Wiki — Mihomo TUN mode: https://wiki.nixos.org/wiki/Mihomo

**How to apply**:
- Linux：修改 systemd unit 生成逻辑（`service.rs`/`instance.rs`），加入 `User=mihomo` + AmbientCapabilities
- `install --system` 新增：创建用户/组、设置目录权限、写入 capabilities-aware unit
- daemon（`daemon.rs`）：移除对 root 的隐式假设，确保文件 I/O 使用 daemon 用户的目录
- macOS/Windows：按平台特性分别处理（macOS 按需提权，Windows 保持 SCM）
- 迁移：已有安装需 `uninstall --system` + 重新 `install --system`

### ADR-19: System daemon 常驻分离（daemon=基础设施，core=用户功能）

**状态**: ✅ 已决策 (2026-08-03，codex GPT-5.5 分析)

**背景**: ADR-17 后 install 的 systemd unit 为 disabled（不自动启动），导致 daemon
（IPC 中介）不运行——CLI 的 start/stop 依赖 daemon，unit disabled 后 CLI 报
"服务未安装"（N2b Linux 真机验证发现）。

**决策**:
1. **daemon 常驻**（控制平面/基础设施）：
   - systemd unit `Restart=always`（崩溃自动拉起）+ install 始终 enable
   - macOS launchd KeepAlive（已实现）/ Windows SCM 服务常驻（已实现）
2. **core 自启由 autostart 控制**（数据平面/用户功能）：
   - daemon 启动时读 per-user autostart 标记（ADR-18）→ 决定是否自动 StartCore
   - 默认不自启（ADR-17 语义保持）

**Why**: daemon 是 CLI 操作的必要中介（StartCore/StopCore/EnableTun 走 IPC），
必须常驻；autostart 语义应作用于用户功能（core）而非基础设施（daemon）。
三平台模型一致：服务管理器保 daemon 可用，daemon 按 autostart 决定 core。

**How to apply**:
- Linux systemd unit：`Restart=on-failure` → `Restart=always` + install 恢复 enable
- daemon 启动流程：读 autostart 标记 → 决定自动 StartCore
- `autostart on/off` 命令：Linux system 分支改为写/删 per-user 标记（不再 enable/disable unit）

### ADR-22: config 单一事实来源 —— 删除 system store

**状态**: ✅ 已决策 (2026-08-08)

**背景**: 当前架构引入了 system store（`/var/lib/mihomo-cli/`、`/Library/Application Support/mihomo-cli/`、`%ProgramData%\mihomo-cli\`），其中包含 `config.yaml`（daemon 读的"渲染后配置"）、`service.token`（IPC 认证）、`backups/` 等。同时还有 `intent_config_file`（`~/.config/mihomo/config.yaml`），是 CLI 写的"用户意图配置"。

**问题**:
1. 两份 `config.yaml` 违反单一事实来源原则——daemon 读的与 CLI 写的不是同一个文件
2. ADR-21 后 daemon 改为非 root 用户，system store 的 root-owned 语义失效
3. user/system 模式行为不一致（不同配置路径），增加理解成本

**决策**:
1. **单一事实来源 = `intent_config_file`**（`~/.config/mihomo/config.yaml`）——CLI 和 daemon 都读写同一文件
2. **system store 废弃**——代码层面删除 `app_root` 字段及相关逻辑
3. **两阶段实施**：
   - **阶段 1**：增强 uninstall，清理 system store 残留（向后兼容已部署用户）
   - **阶段 2**：删除 system store 代码，`config_file` 统一指向 `intent_config_file`
4. **已部署用户**：文档记录迁移路径，不做主动迁移（用户重新 install 即完成清理）

**补充决策（TUN 配置隔离）**：
- TUN 配置采用隔离设计，存储在系统级位置（`/var/lib/mihomo-cli/tun-config.yaml` 等）
- TUN 配置是 per-user config 的系统级快照（derived artifact），不是 source of truth
- 只有 root 或安装者可以修改 TUN 配置
- 这是安全需求驱动的例外，与单一事实来源原则不冲突

**Why**:
- 两份 config 是同步问题的根源——必须明确哪份是权威来源，否则 daemon 和 CLI 的配置状态会漂移
- ADR-21 消除了 system store 存在的理由（root-owned 语义随非 root daemon 失效）
- 单一配置路径简化心智模型：user 模式与 system 模式行为一致，降低理解成本
- system store 中的 `service.token` 可迁移至 config 目录（与 IPC socket 同目录），`backups/` 可迁移至 config 目录或按需重建

**How to apply**:
- 删除 `app_root` 字段及其在 `instance.rs`/`daemon.rs`/`config.rs` 中的所有引用
- uninstall 命令增加 system store 目录清理（检测并删除 `/var/lib/mihomo-cli/`、`/Library/Application Support/mihomo-cli/`、`%ProgramData%\mihomo-cli\`）
- daemon 改为直接读取 `intent_config_file`（`~/.config/mihomo/config.yaml`），不再读 system store 的 `config.yaml`
- `service.token` 移至 config 目录（`~/.config/mihomo/service.token`），IPC 认证机制不变
- USAGE.md 补充已部署用户的迁移说明：`uninstall --system` + `install --system` 完成清理

### TUN 配置隔离（安全例外）

**背景**：TUN 是系统级功能，一旦开启会影响所有用户的流量。如果 TUN 直接使用某个用户的 per-user config，会导致：
- 低权限用户的配置被篡改后，会影响所有用户的流量（场景 3）
- 用户 A 开启 TUN 后，用户 B 无法控制 TUN 使用的配置

**决策**：TUN 配置采用隔离设计：
- TUN 使用的配置存储在 `/var/lib/mihomo-cli/tun-config.yaml`（Linux）、`/Library/Application Support/mihomo-cli/tun-config.yaml`（macOS）、`%ProgramData%\mihomo-cli\tun-config.yaml`（Windows）
- 只有 root 或安装者（有 token 的用户）可以修改 TUN 配置
- 启动 TUN 时，CLI 先更新启动用户的 per-user config，再通过特权写入将其复制为系统级 TUN config 快照；daemon 随后使用该 TUN config 启动/重载 core
- 后续用户修改自己的 per-user config，不会影响 TUN 配置
- 用户再次执行 `tun on` 时，会重新复制配置

**与单一事实来源的关系**：
- per-user config（`~/.config/mihomo/config.yaml`）仍然是单一事实来源（source of truth）
- TUN 配置是 per-user config 的**系统级快照**（derived artifact），类似 `.pyc` 文件是 `.py` 的编译产物
- 这不是概念上的混乱，而是安全需求驱动的例外

**实施计划**：
- 阶段 1：实施 L1 防护（TUN 需要 root 权限）
- 阶段 2：实施 TUN 配置隔离（复制 per-user config 到系统级位置）
