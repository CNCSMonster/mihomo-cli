# mihomo-cli

**跨平台 Mihomo CLI 工具 — 安装部署 + 日常控制，单二进制零依赖**

[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)]()
[English](README_en.md)

---

mihomo-cli 是一个用 Rust 编写的 Mihomo（Clash.Meta）命令行工具。它将 **安装部署** 和 **日常控制** 合二为一，提供 crossterm TUI 交互节点选择（j/k 导航 + / 过滤）、TUN 模式开关、vmess 订阅自动转换等功能，macOS、Linux 和 Windows 通用。

## 安装

从 GitHub 源码编译：

```bash
cargo install --git https://github.com/CNCSMonster/mihomo-cli
```

本地克隆后编译：

```bash
git clone https://github.com/CNCSMonster/mihomo-cli
cd mihomo-cli
cargo install --path .
```

`cargo install --path .` 默认安装到 `$CARGO_HOME/bin/mihomo-cli`；未设置 `CARGO_HOME` 时通常是 `~/.cargo/bin/mihomo-cli`。请确保 `~/.cargo/bin` 在 `PATH` 中。

[Release 页面](https://github.com/CNCSMonster/mihomo-cli/releases) 提供预编译二进制（Linux / macOS / Windows），下载解压放入 PATH 即可。

## 最小核心流程（推荐）

如果你需要 TUN 全机透明代理，安装 system service；如果只需要当前用户代理，安装 per-user service。两种模式互斥：TUN 会在网络层接管全机流量，不能和 per-user core 有意义地共存。

```bash
mihomo-cli install                         # 交互式选择 system service 或 per-user service
mihomo-cli install --system                # 安装 system daemon（首次需要提权，之后通过 IPC 控制）
mihomo-cli install --user                  # 安装 per-user 服务（无需 root）
mihomo-cli config -u '<subscription-url>'  # 添加订阅（始终写入当前用户配置）
mihomo-cli start                           # 自动检测运行态；都未运行时默认启动 per-user
mihomo-cli select --system                 # 显式选择 system 实例节点（通常可省略）
mihomo-cli status --verbose                # 查看服务、daemon/core/TUN 运行态
mihomo-cli ip                              # 查看当前代理出口 IP
mihomo-cli rule test baidu.com             # 检查规则会走哪个策略
mihomo-cli tun on                          # 仅 system service 模式可用，通过 daemon IPC 开启
mihomo-cli tun --system status             # 显式查看 system TUN 状态
```

没有 root 权限时可用 `mihomo-cli install --user` 安装用户级服务；per-user 模式不能启用 TUN，此时更适合使用 `eval "$(mihomo-cli proxy on)"` 给当前 shell 设置代理。

## 文档

| 文档 | 说明 |
|------|------|
| [USAGE.md](USAGE.md) | 完整命令参考与使用示例 |
| [CHANGELOG.md](CHANGELOG.md) | 变更记录 |
| [SPEC.md](SPEC.md) | 软件设计文档 |
| [CONTEXT.md](CONTEXT.md) | 领域知识与术语表 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 贡献指南 |

## 平台支持

| 平台 | 架构 | 开机自启 | TUN 模式 |
|------|------|----------|----------|
| macOS | ARM64 / x64 | LaunchDaemon（system）/ LaunchAgent（user） | system service 模式支持 |
| Linux | x64 / ARM64 | systemd system / systemd --user | system service 模式支持 |
| Windows | x64 | Windows Service / per-user 进程 | system service 模式支持 |

## 核心设计理念

- **安装 + 控制一体**：一个二进制完成从零部署到日常使用
- **订阅自动转换**：自动识别并转换 vmess:// / base64 / Clash YAML 三种格式
- **crossterm TUI 交互**：`select` 和 `config` 命令使用 crossterm 实现真正的键盘快捷键（j/k 导航、/ 过滤）
- **零运行时依赖**：不依赖 curl、jq、python3、fzf 外部工具
- **完善 CLI 体验**：clap 提供自动补全、模糊命令提示、`--help` 文档
- **真正的跨平台**：macOS LaunchDaemon/LaunchAgent + Linux systemd system/user + Windows service/user process，统一命令接口

## 构建

```bash
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl

bash build.sh  # 一键构建全部平台
```

## 项目结构

```
src/
├── main.rs          CLI 入口 + 命令路由 (clap)
├── mihomo_api.rs    Unix socket REST 客户端
├── config.rs        订阅管理 + 配置生成
├── installer.rs     核心二进制下载 + Geo 文件管理
├── service.rs       系统服务管理 (systemd/LaunchDaemon)
├── rules.rs         用户路由规则管理
├── dns.rs           DNS 路由策略管理
├── backup.rs        配置备份与恢复
├── system_proxy.rs  系统代理设置 (macOS/Linux)
├── ui.rs            交互式 TUI (crossterm)
├── yaml_editor.rs   serde_yaml 校验 + 标记区块编辑
└── utils.rs         路径/工具函数
```

## License

MIT


## 高级配置覆盖 override.yaml

`mihomo-cli` 支持可选的覆盖文件：`~/.config/mihomo/override.yaml`。可用 `mihomo-cli override path/show/import/clear` 管理，或加 `--system` 显式使用 system service context。
它会在从 active subscription、用户规则和 DNS policy 生成 `config.yaml` 时应用。

示例：

```yaml
proxy-groups:
  - name: Custom
    type: select
    proxies:
      - DIRECT
dns:
  enhanced-mode: redir-host
```

合并语义：

- YAML map 递归合并。
- list 和 scalar 直接替换生成值。
- runtime controller 字段会在 override 后重新注入，因此 `override.yaml` 不能破坏 mihomo API socket/pipe 配置。



## 订阅 UA 协商

从 URL 添加/刷新订阅时，mihomo-cli 会用少量 Clash-compatible User-Agent 串行请求，拿到 Clash YAML 后立即停止，降低触发服务端限流的风险。可用 `mihomo-cli config --probe <URL>` 查看不同 UA 返回的格式和规则数量；可用 `--user-agent` 或 `--set-ua` 固定某个订阅的 UA。

当前边界：UA 探测只面向 Clash/Mihomo 兼容配置，目标是获取供应商原始 Clash YAML；不会默认探测 Surge、Quantumult X、Shadowrocket、v2rayN 等非 Clash 生态 UA。未来如需支持其它生态，应作为独立功能扩展设计。

## E2E 测试

E2E 测试位于 `tests/e2e/`，通过 `tests/e2e.rs` 接入 Cargo。当前覆盖 fixture config → 合并用户规则 → 调用 `mihomo -t` → 断言生成 YAML 合法的核心链路。详见 `docs/testing/e2e.md`。

## 延迟测试缓存和最快节点选择

`mihomo-cli delay` 会通过当前运行态解析出的 system/per-user 实例调用 mihomo group delay API，按延迟排序输出，并默认将结果缓存到当前用户配置目录的 `delay-cache.json` 300 秒。通常自动检测；需要显式指定 system service 时使用 `--system`。`proxy`/`system-proxy`/`config`/`rule`/`dns`/`backup`/`restore` 同样支持 `--system` override。

```bash
mihomo-cli delay --system --group "节点选择"
mihomo-cli delay --system --refresh       # 忽略缓存，重新测试
mihomo-cli delay --system --cache-ttl 60   # 只复用 60 秒内缓存
mihomo-cli delay --system --fastest        # 自动选择测试成功的最快节点
```

## TUN 增强选项

```bash
mihomo-cli tun on --stack gvisor --dns-hijack
mihomo-cli tun on --stack system --dns-hijack any:53
mihomo-cli tun off
mihomo-cli tun status
```

`--dns-hijack` 不带值时默认使用 `any:53`。
