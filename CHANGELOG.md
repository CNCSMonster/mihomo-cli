# Changelog

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
