# mihomo-cli

**跨平台 Mihomo CLI 工具 — 安装部署 + 日常控制，单二进制零依赖**

[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)]()

---

mihomo-cli 是一个用 Rust 编写的 Mihomo（Clash.Meta）命令行工具。它将 **安装部署** 和 **日常控制** 合二为一，提供 `fzf` 式交互节点选择、TUN 模式开关、vmess 订阅自动转换等功能，macOS 和 Linux 通用。

## 快速开始

```bash
# 下载对应平台的二进制，放到 PATH 中
chmod +x mihomo-cli

# 一键安装
mihomo-cli install

# fzf 交互式选择节点
mihomo-cli select

# 查看出口 IP
mihomo-cli ip

# 开关 TUN
mihomo-cli tun on
mihomo-cli tun off

# 查看状态
mihomo-cli status
```

## 命令列表

### 安装部署

| 命令 | 说明 |
|------|------|
| `mihomo-cli install` | 全流程安装：下载 mihomo 核心 → 配置订阅 → 安装开机自启 |
| `mihomo-cli config` | 配置订阅链接（支持 vmess://、base64、Clash YAML） |
| `mihomo-cli service` | 安装开机自启服务（macOS LaunchDaemon / Linux systemd） |
| `mihomo-cli uninstall` | 卸载服务（可选保留配置） |
| `mihomo-cli update` | 更新 mihomo 核心 |
| `mihomo-cli version` | 版本信息 |

### 日常控制

| 命令 | 说明 |
|------|------|
| `mihomo-cli select` | fzf 交互式选择节点（支持模糊搜索） |
| `mihomo-cli list` | 列出所有代理组及当前节点 |
| `mihomo-cli delay` | 测试组内节点延迟 |
| `mihomo-cli tun on/off` | 启用/关闭 TUN 虚拟网卡 |
| `mihomo-cli ip` | 查看当前出口 IP 归属地 |
| `mihomo-cli conn` | 查看活跃连接 |
| `mihomo-cli flush` | 关闭所有连接 |
| `mihomo-cli status` | 运行状态概览 |
| `mihomo-cli completions` | 生成 shell 自动补全（bash/zsh/fish） |

## 平台支持

| 平台 | 架构 | 开机自启 | TUN 模式 |
|------|------|----------|----------|
| macOS | ARM64 / x64 | LaunchDaemon（root） | ✓ 支持 |
| Linux | x64 / ARM64 | systemd user service | 需额外配置 |
| Windows | x64 | sc.exe 服务 | 需管理员权限 |

交叉编译使用 `cargo-zigbuild`，无需 Docker：

```bash
bash build.sh    # 一键构建全部 6 个平台
```

## 功能优势

- **安装 + 控制一体**：一个二进制完成从零部署到日常使用
- **订阅自动转换**：自动识别并转换 vmess:// / base64 / Clash YAML 三种格式
- **fzf 交互体验**：`dialoguer` 实现模糊搜索节点选择，无需记节点名
- **零运行时依赖**：不依赖 curl、jq、python3、fzf 外部工具
- **完善 CLI 体验**：clap 提供自动补全、模糊命令提示、`--help` 文档
- **真正的跨平台**：macOS LaunchDaemon + Linux systemd，统一命令接口

## 与同类工具对比

| | mihomo-cli | [mihomosh](https://github.com/SamuNatsu/mihomosh) | [clashtui](https://github.com/JohanChane/clashtui) | [clashctl](https://github.com/George-Miao/clashctl) | [clash-cli](https://github.com/ip2a/clash-cli) | [mihoro](https://github.com/spencerwooo/mihoro) |
|---|---|---|---|---|---|---|
| **语言** | Rust | Rust | Rust | Rust | Python + Shell | Rust |
| **形态** | CLI | CLI | **TUI** | TUI + CLI | CLI | CLI |
| **安装部署** | ✅ 一键安装 | ❌ 需手动装 mihomo | ❌ 需手动装 mihomo | ❌ 需手动装 | ✅ 有安装脚本 | ✅ Linux systemd |
| **vmess 转换** | ✅ 自动转换 | ❌ | ❌ (proxy-provider) | ❌ | ❌ | ❌ 报错 |
| **节点选择** | ✅ fzf 模糊搜索 | ✅ 列表选择 | ❌ 依赖 Web 面板 | ✅ TUI 面板 | ❌ 无 | ❌ 无 |
| **TUN 开关** | ✅ cli 切换 | ❌ | ❌ 依赖模板 | ❌ | ✅ cli 切换 | ❌ |
| **出口 IP 查询** | ✅ 内置 | ❌ | ❌ | ❌ | ❌ | ❌ |
| **连接管理** | ✅ 查看/关闭 | ✅ 查看/关闭 | ❌ | ✅ TUI | ❌ | ❌ |
| **延迟测试** | ✅ 内置 | ✅ | ❌ | ✅ TUI | ❌ | ❌ |
| **macOS** | ✅ LaunchDaemon | ✅ | ❌ | ✅ | ❌ 仅 Linux | ❌ 仅 Linux |
| **Linux** | ✅ systemd | ✅ | ✅ systemd | ✅ | ✅ systemd | ✅ systemd |
| **Windows** | ✅ sc.exe | ❌ | ✅ nssm | ✅ | ❌ | ❌ |
| **Shell 补全** | ✅ bash/zsh/fish | ✅ | ❌ | ✅ | ❌ | ❌ |
| **模糊命令提示** | ✅ clap | ❌ | ❌ | ❌ | ❌ | ❌ |
| **零依赖** | ✅ 单二进制 | ✅ 单二进制 | ✅ 单二进制 | ✅ 单二进制 | ❌ pip + shell | ❌ cargo |

## 构建

```bash
# 准备
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl

# 全部平台一键构建
bash build.sh

# 或单独构建
cargo build --release
cargo zigbuild --target x86_64-unknown-linux-gnu --release
```

## 项目结构

```
src/
├── main.rs          CLI 入口 + 命令路由
├── mihomo_api.rs    Unix socket RESTful API 客户端
├── config.rs        订阅下载 + vmess → Clash YAML 转换
├── installer.rs     Mihomo 核心二进制下载
├── service.rs       macOS LaunchDaemon / Linux systemd
├── ui.rs            交互式 fuzzy-select
└── utils.rs         工具函数
```

## License

MIT
