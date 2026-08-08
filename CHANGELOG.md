# CHANGELOG

> mihomo-cli 变更记录，由 commit 历史自动生成

---

## 0.2.0 (2026-08-09)

### ⚠️ Breaking Changes

#### 删除 system store (ADR-22)

根据 ADR-22，mihomo-cli 已删除 system store（例如 `/var/lib/mihomo-cli` 等系统级配置存储）。从本版本开始，配置的单一事实来源固定为当前用户配置目录：

```text
~/.config/mihomo/config.yaml
```

**影响**：已部署过 system 模式的用户，旧版本创建的 system store 目录和文件可能仍残留在系统中。这些残留不再作为配置来源，也不应继续依赖。

**迁移**：建议先清理旧部署残留，再重新安装 system service：

```bash
mihomo-cli uninstall --all
mihomo-cli install --system
```

**原因**：system store 违反单一事实来源原则；在 ADR-21 的最小权限架构下，继续保留 system store 没有实际收益，反而增加配置路径歧义和迁移成本。

---

## 2026-08-03

### ✨ Features
- **`autostart` 命令** — `mihomo-cli autostart on|off|status` 统一三平台开机自启控制；
  **默认不开机自启**（ADR-17），显式开启。Windows user 模式用注册表 Run 键 + `.vbs`
  隐藏窗口（登录静默启动，无黑窗）
- **Windows 服务化**（ADR-16）— daemon 通过 `windows-service` crate 实现 SCM 协议层，
  `StartServiceCtrlDispatcher` + Stop/Shutdown 停机链路；修复 CI 实测的 StartService 87
- **Windows 服务安装 service-manager** — 替代手写 sc.exe（binPath 引号正确转义，
  修复 87 根因）；autostart 默认 demand（不自启）+ OnFailure 重启策略

### 🔒 Security
- **named pipe SDDL 访问控制** — daemon pipe 限制 SYSTEM + Administrators + 安装者 SID
  （install 落盘 installer-sid，daemon 运行时读取）；`first_pipe_instance` 防 pipe 抢占
- **daemon IPC token 双校验** — install 生成 32 字节随机 token，双副本（服务端
  `%ProgramData%\mihomo\service-token` + 客户端 `<config_dir>\service-client-token`）；
  CLI 附加 token，daemon 校验。pipe 访问 API 不验证 secret（mihomo 文档明确），
  必须靠 token 防越权
- **elevated 检测 windows-sys TokenElevation** — 弃用 `net session` hack 与停更的
  is_elevated crate

### 🧪 Validation
- **Windows System 模式 E2E 全绿** — GitHub Actions 跨平台矩阵 12/12 通过
  （build/unit/user-e2e/system-e2e × ubuntu/macos/windows）
- **macOS launchctl print 解析纯函数 + 单元测试** — 抽为跨平台可测函数，
  覆盖 5 形态（running/not running/disabled/未加载/未知），真机形态留 N2a
- **Windows core 启动修复** — `cmd start` title 加引号（os error 2 根因），
  此路径首次被 N1c 强断言覆盖
- **legacy sc.exe install plan 清理** — service-manager 接管后删除死代码

---

## 2026-08-02

### ✨ Features
- **`select --node` 非交互切换** — `mihomo-cli select -g <group> --node <node>` 直接切换指定节点（无 `--node` 时仍为 TUI），便于脚本/CI 确定性切换（b2dd36b）

### 🐛 Bug Fixes
- **BUG-17** — `install` 复用旧 config 时不校正 controller endpoint，导致 System 模式 core 拒绝启动；修复：`[3/4]` 步骤对已存在 config 先校验 endpoint，不匹配则就地校正（e54ce35）
- **BUG-16** — macOS `stop` 误用 `launchctl bootout` 卸载 job，导致后续 `start` 失败；改用 `launchctl kill SIGTERM`（停进程不卸载，与 start 的 kickstart 对称）（116e745）
- **BUG-15** — `install --user` 写出的 start.sh 缺可执行权限（0644），launchd exec 报 EX_CONFIG；修复：非特权写入应用 planned mode（03bc0cd）
- **BUG-14** — `mihomo_path()` Windows 路径缺 `\bin\` 段；改为委托 `instance::planned_paths` 单一事实来源（7a505f3）

### 🧪 Validation
- **M3 macOS 双模式 E2E 完成** — User + System 全生命周期、TUN on/off（utun4 网卡）、真实代理访问（YouTube/Google Docs/OpenAI）、v3 互斥保护（ea0715b）
- **M4 Linux E2E 完成** — colima VM（Ubuntu 24.04 + systemd 255）User + System 全流程 + TUN（2026-08-01）

### 📝 Docs
- **Windows 二等公民决策** — Windows 验证改走 pub 仓库 CI runner，不占用真机（e0cf3ec）
- **conn vs logs 用途区分** — `conn` 只显示瞬时活跃连接，历史连接查 `logs`（e92fa2f）

---

## 2026-07-22

> 完整方案与实施细节见 [docs/archive/2026-07-22-bugfix-batch.md](docs/archive/2026-07-22-bugfix-batch.md)

### 🐛 Bug Fixes
- **macOS socket 迁移** — socket 路径从 `/tmp/mihomo` 迁移到 `~/.config/mihomo/run`，消除 root/user 权限冲突和多用户抢占；迁移时检查旧 socket 活性
- **user 模式 restart** — 修复 macOS user 模式下 restart 错误触发 sudo 的问题，改用 `launchctl unload/load`
- **重复 start 幂等** — macOS 已加载 agent 时 `start` 不再报错，改用 `launchctl start` / `kickstart -k`
- **root 模式 stop** — macOS root 模式全面改用 `bootstrap/bootout/kickstart` 现代命令
- **端口缺失** — merge 时自动注入 `mixed-port: 7897`；`get_port` 支持 `mixed-port → port → socks-port` 回退
- **system-proxy Wi-Fi 硬编码** — 枚举所有启用网络服务，不再硬编码 Wi-Fi
- **订阅激活** — 新增 `--activate`/`--no-activate` 标志；非首订阅 TTY 交互询问 / 非 TTY 提示 `--switch`
- **注释误导** — 修正 `launchctl start` 行为描述

### ✨ Features
- **`upgrade` 命令** — 查询 GitHub latest release，比较当前版本，交互确认后下载替换并重启服务
- **`install --version`** — 支持指定 mihomo core 版本
- **`status` 显示 core 版本** — 通过 mihomo API `/version` 端点获取
- **并发配置锁** — `flock(2)` 文件锁保护 B 类操作（rules/dns/subscriptions 变更 + merge），可重入，10s 超时

### 🔧 Refactoring
- **原子写** — rules/dns/active ID 写入统一改为 `atomic_write_file`（.tmp → rename）
- **`mihomo_api::socket_path()`** — 去重复，复用 `utils::socket_dir()`

---

## 2026-07-18

### ⚠️ Changed
- **移除 `ip` 命令** — 通用出口 IP 由 `status` 展示；目标域名路由判断使用 `rule test <host>`。不再提供容易误导的 `ip --url`，因为 IP 查询网站和目标 URL 可能命中不同 Mihomo 规则，无法可靠代表目标真实出口。后续等待 Mihomo/兼容内核提供指定节点 fetch 响应体或专用出口 IP 探测 API 后再规划。*（注：后以 deprecated 形式保留，出口探测改由 `exit-ip` 承担）*

### ✨ Features
- **select crossterm TUI** — 替换 `dialoguer::FuzzySelect`，使用 crossterm raw mode 实现真正的键盘快捷键：j/k 导航、g/G 跳转首尾、/ 进入过滤模式、Backspace 删除、Esc 退出过滤或取消
- **config 订阅管理 TUI** — `mihomo-cli config` 无参数时启动 crossterm TUI 界面，支持 ↑↓/j/k 导航、Enter 切换订阅、r 刷新活跃订阅、R 刷新全部、a 添加、d 删除、Esc/q 退出
- **crossterm 依赖** — 新增 `crossterm` crate，移除 `dialoguer` 的 `fuzzy-select` feature（`dialoguer` 仍用于 `Input`/`Confirm`/`Password`/`Select` 等非 TUI 场景）

### 🔧 Refactoring
- **ui.rs 重写** — `flat_select()` 和 `select_node()` 改为 crossterm raw mode 实现，支持过滤模式切换、滚动窗口、当前节点 ★ 标记
- **show_subscription_menu 重写** — 从 `dialoguer::Select` 嵌套菜单改为 crossterm 单循环 TUI，消除 Esc 后需要再选 action 的二次交互

### 📚 Documentation
- **USAGE.md 全面更新** — 补充所有命令的完整选项说明（system-proxy、logs、backup/restore、dns template、rule types/policies/test、override.yaml、TUN 增强选项）
- **SPEC.md 更新** — 补充 §2.6 Config TUI、§2.7 多订阅管理、§2.8 Backup/Restore、§2.9 System Proxy、§2.10 Logs、§2.11 Override.yaml、§2.12 DNS Templates
- **README/README_en/CONTRIBUTING** — 更新项目结构（补充 dns.rs、backup.rs、system_proxy.rs、yaml_editor.rs），dialoguer → crossterm 描述
- **CLI help 文本** — 更新 config/select 的 about 描述，反映 TUI 交互方式
- **移除不存在的命令** — 从文档中移除 `version` 和 `completions`（代码中未实现）

---


## 2026-07-17

### ✨ Features
- **多订阅源管理** — 支持添加、切换、删除多个订阅源，每个订阅独立存储。`config --add/--remove/--list/--switch` 命令管理订阅列表
- **订阅 UA 探测** — `config --probe <url>` 测试不同 User-Agent 返回的订阅格式，`--set-ua` 固定订阅使用的 UA
- **配置验证与回滚** — `config --validate` 验证配置语法，写操作失败时自动回滚到修改前状态
- **override.yaml 覆盖** — 支持 `~/.config/mihomo/override.yaml` 任意字段覆盖，深度合并到最终配置
- **TUN 增强选项** — `tun on --stack gvisor|system` 选择 TUN 栈，`--dns-hijack` 启用 DNS 劫持
- **延迟测试优化** — `delay --cache-ttl` 复用缓存结果，`--fastest` 自动选择最快节点
- **多文件配置方案** — 规则、DNS 策略、订阅元数据分离存储，`rules.yaml`/`dns-policy.yaml`/`subscriptions.yaml`

### 🐛 Bug Fixes
- **跨平台 socket 路径** — Linux 使用 `$XDG_RUNTIME_DIR/mihomo/mihomo.sock`，macOS 使用 `/tmp/mihomo/mihomo.sock`
- **订阅格式协商** — 自动检测并协商 Clash 兼容格式，避免返回 Surge/Quantumult X 等非兼容格式
- **规则推断修复** — 不再从代理名称推断规则，避免误匹配
- **配置写入验证** — 所有配置写入后立即验证 YAML 合法性，失败时回滚
- **installer 断点续传** — 修复忽略 resume range 的问题，确保大文件下载可恢复
- **规则缩进检测** — 支持 0 缩进的配置文件，自动检测并保持一致缩进风格

### 🔧 Refactoring
- **深度可测试性重构** — 50+ 个测试抽象提交，将核心逻辑分解为可独立测试的纯函数
  - installer 下载流、进度条计划、文件操作抽象
  - service 命令计划、sudo 调度策略抽象
  - config TUI 消息、DNS 命令消息抽象
  - socket API 传输层抽象
- **计划模式复用** — service install/uninstall/command 计划可复用，减少重复代码
- **mihomo API 客户端抽象** — 分离 HTTP 请求构建与执行，便于测试

### 🧪 Testing
- **E2E 配置合并框架** — `tests/e2e/config_merge.rs` 测试 fixture 配置 → 合并规则 → `mihomo -t` 验证完整流程
- **测试覆盖率提升** — 大量单元测试覆盖 installer、service、config、mihomo_api 等核心模块
- **CI 覆盖率基线** — `scripts/coverage.sh` 强制 23% 最低覆盖率，CI 自动检查

### 📚 Documentation
- **多文件配置设计文档** — 配置分离方案详见 SPEC.md §3.1
- **订阅 UA 边界说明** — CONTRIBUTING.md 明确 UA 探测只面向 Clash/Mihomo 兼容格式
- **tree-sitter V3 文档更新** — 更新 SPEC.md、BUGS.md 反映 tree-sitter-yaml CST 编辑方案

---

## 2026-07-15

### 🐛 Bug Fixes
- **YAML 编辑 V3** — 引入 tree-sitter-yaml CST 精确编辑，替代字符串拼接方式。修复规则合并破坏 YAML 缩进导致 mihomo 启动失败的 bug（BUGS.md #1）。失败时显式报错而非静默降级（ADR-10, ADR-11）
- **stdout/stderr 合并诊断** — mihomo 错误信息输出到 stdout，但代码只读 stderr，导致误报 "binary corrupted"。提取 `combine_output()` 辅助函数，所有诊断点同时读取 stdout + stderr（5 处修复）
- **YAML 规范化** — 订阅内容解析验证后再保存，防止格式问题导致 mihomo 解析失败
- **install 流程加固** — 无效配置时不启动服务、跳过配置时删除旧配置
- **TUN 默认关闭** — 避免权限问题导致 "Exit IP: unreachable"
- **V2Board 兼容性** — 使用 `flag=clashmeta` 参数获取完整 Clash YAML，避免节点丢失

### ✨ Features
- **config --import** — 支持从本地文件导入配置，自动检测 base64/vmess 格式并转换。解决 DNS 污染环境下无法获取配置的问题

### 📚 Documentation
- **设计原则** — "不要假设，验证它" 写入 CONTEXT.md
- **日志体系** — 添加日志原则和错误信息标准
- **ROADMAP** — 添加 BUG-05 到 BUG-10 记录

---

## 2026-07-10

### 🐛 Bug Fixes
- **delay 404** — 新版 mihomo 将 group delay API 路径从 `/proxies/{name}/delay` 迁移到 `/group/{name}/delay`，并修复 query 参数双重编码问题
- **uninstall --all** — 现在删除 config 目录（此前保留 config，无法彻底清理）
- **start 失败诊断** — 日志文件不存在时自动运行 `mihomo -t` 检查配置语法

### 🔧 Refactoring
- **拆分 ensure_socket_or_fix** — 分离为 `socket_needs_fix()` 和 `apply_socket_fix()`，上层灵活组合
- **修复 controller 检查** — status verbose 只匹配 `external-controller-unix`，排除 TCP 模式误判

### 📚 Documentation
- 建立 6 文档体系：README, USAGE, SPEC, ROADMAP, CONTEXT, CONTRIBUTING
- SPEC 添加 ADR 节（6 条架构决策记录）
- AGENTS.md / CLAUDE.md 作为 README 符号链接

---

## 2026-07-09

### ✨ Features
- **可排查性优化专项** — 错误引导、socket 检测、日志落地、status 诊断、install --dry-run、install --skip-geo
- **规则管理 V2** — 标记法合并（`# === USER RULES START/END ===`）+ 原子写入

### 🔧 Refactoring
- **AppPaths** — 引入统一路径管理层，解耦测试中的 HOME 依赖，支持序列化单元测试
- **移除 HOME 依赖** — dns 和 config 测试不再依赖 HOME 环境变量

### ✨ Features (Earlier)
- DNS 路由策略管理（`dns add/list/remove/clear`）
- IP 输出重设计 + 用户自定义路由规则管理

---

## 2026-07-08

### ✨ Features

### 🐛 Bug Fixes
- 移除局域网 IP 重复标记

### 🔧 Refactoring
- IP probe 模块 code review 修复

### 📚 Documentation
- README 添加 IP 命令详细文档
- 规则管理文档更新
