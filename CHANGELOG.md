# Changelog

## 2026-08-09

### 🔒 Security

- **L1-L7 七层安全防护** — 完整实现七层安全防护体系：
  - **L1** TUN 操作强制 root 权限（CLI 自动 sudo + daemon peer UID 检查）
  - **L2** TUN 配置隔离（TUN config 独立存储，per-user config 的系统级快照）
  - **L3** Unix token 认证（root-only server token + per-user client token + 授权表）
  - **L4** Socket 权限审计（维持 0o666，通过 L3 token + peer UID 授权）
  - **L5** daemon 非 root 运行（ADR-21：daemon 以 `mihomo` 用户运行 + AmbientCapabilities）
  - **L6** 符号链接攻击防护（`O_NOFOLLOW` + `symlink_metadata()` 检查）
  - **L7** 日志脱敏 + 级别控制（`sanitize_url()` + `sanitize_sensitive()` + DEBUG 级别）
- **威胁模型文档** — 补充场景 7-10 分析，明确安全边界

### ✨ Features

- **config 单一事实来源（ADR-22）** — 删除 system store（`/var/lib/mihomo-cli/config.yaml`），统一配置管理。TUN config 作为 per-user config 的系统级快照独立存储
- **daemon 自动拉起 core（ADR-19）** — daemon 启动时自动检测并启动 core 进程，实现 autostart 闭环
- **DNS fake-ip filters 管理** — `dns fake-ip-filter add/remove/list` 支持自定义 fake-ip 过滤规则

### 🐛 Bug Fixes

- **TUN 权限恢复** — 恢复 `validate_tun_peer_is_root()` 确保只有 root 可执行 `tun on/off`，修复 L5 实施后的安全回归
- **macOS autostart 语义对齐** — 修复 launchctl enable/disable override 竞态问题
- **Windows user core 启动修复** — 修复 cmd start title 引号问题导致的 os error 2

### 🔧 Refactoring

- **删除 legacy sc.exe install plan** — service-manager 完全接管 Windows 服务管理

### 📚 Documentation

- **安全设计文档** — 新增 `docs/SECURITY.md`，包含竞品安全对比、威胁模型、七层防护设计
- **架构图更新** — `docs/architecture.md` 补充 ADR-21/22 架构变更
- **归档废弃文档** — 移除 `SPEC-system-config-separation.md`（已被 ADR-22 替代）

### ⚠️ Breaking Changes

- **TUN 操作需要 root 权限** — `tun on/off` 现在需要 root 权限，CLI 会自动调用 sudo。普通用户执行时会提示输入密码

---

## 2026-08-02 (incremental)

### 🐛 Bug Fixes
- **Windows user/system presence 缺口** — Windows 模式此前 `service_file: None` 导致 `status`/`start`/`config --fix` 恒判"未安装"；修复：User 模式 install 写入 `.user-installed` 标记文件、inventory 用 `sc query mihomo` 探测 System 服务

---

## 2026-08-02

### ✨ Features
- **`select --node` 非交互切换** — `mihomo-cli select -g <group> --node <node>` 直接切换指定节点（无 `--node` 时仍为 TUI），便于脚本/CI 确定性切换
- **system-proxy 支持** — reqwest 启用 system-proxy，订阅/geo/core 下载自动尊重 `HTTPS_PROXY`/`HTTP_PROXY` 环境变量（未设置则直连，与 curl 约定一致）

### 🐛 Bug Fixes
- **BUG-17** — `install` 复用旧 config 时不校正 controller endpoint，导致 System 模式 core 拒绝启动；修复：`[3/4]` 步骤对已存在 config 先校验 endpoint，不匹配则就地校正
- **BUG-16** — macOS `stop` 误用 `launchctl bootout` 卸载 job，导致后续 `start` 失败；改用 `launchctl kill SIGTERM`（停进程不卸载，与 start 的 kickstart 对称）
- **BUG-15** — `install --user` 写出的 start.sh 缺可执行权限（0644），launchd exec 报 EX_CONFIG；修复：非特权写入应用 planned mode
- **BUG-14** — `mihomo_path()` Windows 路径缺 `\bin\` 段；改为委托 `instance::planned_paths` 单一事实来源
- **BUG-13** — System 模式 InstanceLock 锁文件权限失败被误报为锁冲突；修复：CLI 移除文件锁，daemon 内 `OWNER_LIFECYCLE_LOCK` 串行化 lifecycle，readiness 迁入 daemon
- **BUG-11/12** — XDG_RUNTIME_DIR 回退路径硬编码 UID 1000；修复：优先 /proc/self/loginuid，回退 `id -u`

### 🔧 Refactoring
- **`--proxy` 改名为 `--github-mirror`** — 原 flag 实为 geo 下载的 GitHub 镜像前缀，改名明确语义，不留兼容别名
- **清理死代码** — 删除 mihomo_api 已验证零调用的死函数（endpoint_is_alive、probe_ip_fast 等）

### 📚 Documentation
- **文档与代码一致性审计** — 修复 USAGE/SPEC/ROADMAP/README/CHANGELOG 12 项过时描述（select --node、install 预检、Uninstall TUI、`--root`→`--system` 等）
- **conn vs logs 用途区分** — 明确 `conn` 只显示瞬时活跃连接，历史连接（含已关闭连接的规则匹配）查 `logs`

### 🧪 Validation
- **M3 macOS 双模式 E2E 完成** — User + System 全生命周期、TUN on/off（utun 网卡真实创建）、真实代理访问（YouTube/Google Docs/OpenAI）、v3 互斥保护
- **M4 Linux E2E 完成** — colima VM（Ubuntu 24.04 + systemd 255）User + System 全流程 + TUN

---

## 2026-07-27 (v0.4.1)

### ✨ Features
- **Build metadata** — `mihomo-cli --version` 和 `mihomo-cli version` 现在显示完整的构建信息：git commit hash、分支、构建时间、目标平台等

---

## 2026-07-27

### ✨ Features
- **v3 互斥架构** — 基于 instance inventory 的单一实例解析模型，消除 root/user 双实例歧义；自动检测并解析当前活跃实例
- **IPC 协议骨架** — daemon + IPC 通信机制，支持 TUN 模式通过 IPC 控制系统服务
- **实例感知命令** — `api`/`rule`/`dns`/`backup` 等命令自动解析实例路径，无需手动指定 `--system`/`--user`
- **v2 迁移命令** — `mihomo-cli migrate` 清理 legacy root 模式残留，显示证据并执行迁移
- **冲突检测** — 安装时检测已有实例，提供清晰错误消息而非静默覆盖
- **TUI 导航增强** — 支持 Ctrl-N/Ctrl-P 上下导航，过滤模式下保留方向键导航

### 🐛 Bug Fixes
- **macOS launchd 现代化** — 统一使用 bootstrap/bootout/kickstart Modern API (ADR-12)，移除 legacy load/unload
- **macOS KeepAlive** — 改用 `KeepAlive.Crashed` 替代 `KeepAlive=true`，避免手动 stop 后被自动重启
- **macOS socket 迁移** — socket 路径从 `/tmp/mihomo` 迁移到 `~/.config/mihomo/run`，消除权限冲突
- **Windows 提权** — 修复 privileged flag、direct start 和 system-proxy 支持
- **并发配置锁** — `flock(2)` 文件锁保护 B 类操作（rules/dns/subscriptions 变更），可重入，10s 超时
- **订阅激活** — 新增 `--activate`/`--no-activate` 标志；非首订阅 TTY 交互询问 / 非 TTY 提示 `--switch`
- **Geo 下载** — 添加 `--proxy` 标志，修复跨进程 resume 和 .part 残留清理
- **status 静默探测** — 服务状态检查不再输出探测错误，只在最终结果中展示

### 🔧 Refactoring
- **移除双实例歧义** — 不再报"两个实例都在运行"错误，改为自动解析活跃实例
- **术语统一** — `--root` 重命名为 `--system`，移除非 install/uninstall 命令的 `--system`/`--user` flags
- **实例解析链路** — service/api/rule/dns/backup 命令全部通过 instance inventory 解析路径
- **计划模式** — install/uninstall/start/stop/restart 等操作改为显式 plan 执行，便于审计和测试

### 📚 Documentation
- **SPEC.md 更新** — 反映 v3 架构：instance inventory、IPC 协议、互斥设计
- **USAGE.md 更新** — 补充 v3 命令行为变化、migrate 命令、实例解析说明

### ⚠️ Changed
- **移除 `--system`/`--user` 透传** — 仅 `install`/`uninstall` 保留模式选择，其他命令自动解析实例
- **移除全局配置包装** — legacy `get_config_dir()`/`get_socket_path()` 等全局函数移除，改为实例感知版本

---

## [0.3.1] - 2026-07-13

### Fixes
- Geo file download integrity validation (four-layer check)

## [0.3.0] - 2026-07-13

### Features
- macOS LaunchAgent (user mode) support
- Interactive root/user mode selection during install

### Fixes
- System TLS certificate handling via rustls-native-certs
- Subscription download reliability improvements
- Permission and config validation improvements
- API endpoint corrections (delay 404 fix)

### Improvements
- Enhanced diagnostics and error guidance
- Better install flow UX
- Improved uninstall cleanup

## [0.2.0] - 2026-07-10

### Features
- User-defined routing rule management via `mihomo-cli rule`
- DNS policy management for per-domain nameserver configuration
- Enhanced `mihomo-cli ip` diagnostics with TUN status and LAN detection

### Improvements
- GeoIP/GeoSite bootstrap reliability validation
- Expanded documentation for rule management and DNS policies

## [0.1.0] - 2026-07-08

Initial release.

- Cross-platform setup and control CLI for Mihomo proxy
- One-command installation with subscription auto-conversion
- Interactive proxy node selection with fuzzy search
- TUN mode toggle and connection management
- Shell completions for bash/zsh/fish
- Pre-built binaries for Linux, macOS, and Windows
