# mihomo-cli

**跨平台 Mihomo CLI 工具 — 安装部署 + 日常控制，单二进制零依赖**

[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)]()
[English](README_en.md)

---

mihomo-cli 是一个用 Rust 编写的 Mihomo（Clash.Meta）命令行工具。它将 **安装部署** 和 **日常控制** 合二为一，提供 `fzf` 式交互节点选择、TUN 模式开关、vmess 订阅自动转换等功能，macOS 和 Linux 通用。

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

[Release 页面](https://github.com/CNCSMonster/mihomo-cli/releases) 提供预编译二进制（Linux / macOS / Windows），下载解压放入 PATH 即可。

## 快速开始

```bash
mihomo-cli install                   # 安装 mihomo 核心 + 系统服务
mihomo-cli start                     # 启动服务
mihomo-cli config -u '<subscription-url>'  # 添加订阅
mihomo-cli select                    # 选择节点
mihomo-cli tun on                    # 开启 TUN
```

## 文档

| 文档 | 说明 |
|------|------|
| [USAGE.md](USAGE.md) | 完整命令参考与使用示例 |
| [CHANGELOG.md](CHANGELOG.md) | 变更记录 |
| [ROADMAP.md](ROADMAP.md) | 功能规划与 Bug 追踪 |
| [SPEC.md](SPEC.md) | 软件设计文档 |
| [CONTEXT.md](CONTEXT.md) | 领域知识与术语表 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 贡献指南 |

## 平台支持

| 平台 | 架构 | 开机自启 | TUN 模式 |
|------|------|----------|----------|
| macOS | ARM64 / x64 | LaunchDaemon（root） | ✅ 支持 |
| Linux | x64 / ARM64 | systemd | 需额外配置 |
| Windows | x64 | sc.exe 服务 | 需管理员权限 |

## 核心设计理念

- **安装 + 控制一体**：一个二进制完成从零部署到日常使用
- **订阅自动转换**：自动识别并转换 vmess:// / base64 / Clash YAML 三种格式
- **fzf 交互体验**：`dialoguer` 实现模糊搜索节点选择，无需记节点名
- **零运行时依赖**：不依赖 curl、jq、python3、fzf 外部工具
- **完善 CLI 体验**：clap 提供自动补全、模糊命令提示、`--help` 文档
- **真正的跨平台**：macOS LaunchDaemon + Linux systemd，统一命令接口

## 构建

```bash
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl

bash build.sh  # 一键构建全部平台
```

## 项目结构

```
src/
├── main.rs          CLI 入口 + 命令路由
├── mihomo_api.rs    Unix socket RESTful API 客户端
├── config.rs        订阅下载 + vmess → Clash YAML 转换
├── installer.rs     Mihomo 核心二进制下载
├── service.rs       macOS LaunchDaemon / Linux systemd
├── rules.rs         用户自定义路由规则管理
├── ui.rs            交互式 fuzzy-select
└── utils.rs         工具函数
```

## License

MIT
