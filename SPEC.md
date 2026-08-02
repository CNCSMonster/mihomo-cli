# SPEC

> Mihomo CLI — cross-platform setup & control tool for Mihomo (Clash.Meta) proxy

---

## 1. Architecture (v3)

### 1.1 Instance Modes

v3 采用两种**互斥**实例模式，运行时自动检测：

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
- daemon 启动 core 前校验 config 中的 API endpoint 等于 v3 系统端点

### 1.5 Lifecycle Concurrency & Readiness

对齐 clash-verge-service（OWNER_LIFECYCLE_LOCK + 服务端 readiness）的架构，
v3 的 System 模式 lifecycle 遵循以下设计（源自 docs/SPEC-system-lifecycle-ipc.md，
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

### 1.6 Permission Matrix

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
├── config.yaml              ← 最终配置（合并产物）
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

### 3.2 Config Generation

合并顺序：订阅内容 → `override.yaml` → 用户规则 → DNS 策略 → controller 注入

合并语义：
- YAML map 递归合并
- list 和 scalar 直接替换
- runtime controller 字段在合并后重新注入

### 3.3 Subscription Processing

1. `flag=clashmeta` + 多 UA 候选（`clash-meta` → `clash-verge` → `clash`）
2. 检测响应是否为 Clash YAML
3. 非 YAML → 尝试 base64 解码 → 转换 vmess/trojan 为 Clash YAML
4. 保存后运行 `mihomo -t` 验证

### 3.4 DNS Policy

CLI 接口：`mihomo-cli dns policy add <MATCH> <TARGET>`
- MATCH：域名后缀（如 `ubtrobot.com`）
- TARGET：DNS 服务器 IP（逗号分隔多个）
- 实现：写入 `dns-policy.yaml`，merge 时注入 `config.yaml` 的 `dns.nameserver-policy`

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
| v3 Architecture | 互斥双模式、自动检测、IPC daemon |
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

