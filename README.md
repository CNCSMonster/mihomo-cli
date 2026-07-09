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
# 1. 安装 mihomo 核心 + 系统服务（自动下载，无需手动装依赖）
mihomo-cli install

# 2. 启动服务（默认 TUN 关闭，不影响当前网络）
mihomo-cli start

# 3. 添加订阅源（支持 vmess://、base64、Clash YAML，自动格式转换）
mihomo-cli config -u '<your-subscription-url>'

# 4. fzf 模糊搜索选择节点
mihomo-cli select

# 5. 开启 TUN 透明代理
mihomo-cli tun on

# ✅ 完成 — 所有流量自动通过代理
```

**其他常用操作：**

```bash
mihomo-cli ip          # 探测出口 IP（直连 vs 代理，诊断 TUN 状态）
mihomo-cli status      # 运行状态概览
mihomo-cli proxy on    # 设置当前终端的 http_proxy 环境变量（用 eval）
mihomo-cli restart     # 重启服务
mihomo-cli tun off     # 关闭 TUN
```

## 命令列表

### 安装部署

| 命令 | 说明 |
|------|------|
| `mihomo-cli install` | 下载核心 + 生成配置 + 安装开机自启（交互式） |
| `mihomo-cli config` | 配置订阅链接（支持 vmess://、base64、Clash YAML） |
| `mihomo-cli uninstall` | 卸载服务（可选保留全部文件） |
| `mihomo-cli update` | 更新 mihomo 核心 |
| `mihomo-cli version` | 版本信息 |

### 服务与连接

| 命令 | 说明 |
|------|------|
| `mihomo-cli start [--user]` | 启动 mihomo 服务 |
| `mihomo-cli stop [--user]` | 停止 mihomo 服务 |
| `mihomo-cli restart [--user]` | 重启 mihomo 服务 |
| `mihomo-cli status` | 运行状态概览（含出口 IP） |

### 日常使用

| 命令 | 说明 |
|------|------|
| `mihomo-cli select` | fzf 交互式选择节点（支持模糊搜索） |
| `mihomo-cli list` | 列出所有代理组及当前节点 |
| `mihomo-cli delay` | 测试组内节点延迟 |
| `mihomo-cli tun on/off` | 启用/关闭 TUN 虚拟网卡 |
| `mihomo-cli ip` | 探测出口 IP（直连显示 ISP 真实 IP，代理显示节点出口 IP，支持 `--url` 测试特定 URL 路由，自动标记 LAN 地址） |
| `mihomo-cli proxy on/off` | 输出 shell 代理环境变量（`eval "$(mihomo-cli proxy on)"`） |
| `mihomo-cli conn` | 查看活跃连接（`--flush` 关闭全部） |
| `mihomo-cli rule` | 管理自定义路由规则（添加/删除/导入/导出） |
| `mihomo-cli completions` | 生成 shell 自动补全（bash/zsh/fish） |

> 💡 **所有命令均支持 `-h` / `--help` 查看详细用法**，例如 `mihomo-cli install -h`、`mihomo-cli config -h`。

### `mihomo-cli ip` 详解

探测两条网络路径的出口 IP，诊断 TUN 状态：

```bash
$ mihomo-cli ip

=== Exit IP Probe ===

  TUN mode:      disabled

  Direct:        120.231.212.245 (China) via api.ip.sb
  Mihomo proxy:  2406:da12:f1d:9000:7f25:e2fc:49a7:a01e (South Korea) via api.ip.sb

  ✓ Normal: direct shows ISP, proxy shows node exit
```

**LAN 地址检测：**

局域网地址（10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, fc00::/7, fe80::/10）会标记 `[LAN]`：

```
  Direct:        192.168.1.100 [LAN] via api.ip.sb
  ⚠ Direct route exits via LAN address (192.168.1.100)
```

**测试特定 URL 路由：**

```bash
$ mihomo-cli ip --url https://github.com

=== Exit IP Probe ===

  TUN mode:      disabled

  Direct:        120.231.212.245 (China) via api.ip.sb (after target)
  Mihomo proxy:  43.203.158.60 (South Korea) via api.ip.sb (after target)

  ✓ Normal: direct shows ISP, proxy shows node exit
```

这会先请求目标 URL，再检查出口 IP，用于验证 mihomo 规则配置是否正确。


### `mihomo-cli rule` 详解

管理用户自定义路由规则，规则会在 mihomo 启动/重载时自动合并到配置中。

**基本用法：**

```bash
# 添加规则（让特定域名走直连）
mihomo-cli rule add DOMAIN-SUFFIX,company.com,DIRECT
mihomo-cli rule add IP-CIDR,192.168.0.0/16,DIRECT

# 添加规则（指定代理组）
mihomo-cli rule add "DOMAIN-SUFFIX,google.com,节点选择"

# 列出所有规则
mihomo-cli rule list

# 删除规则（按索引）
mihomo-cli rule remove 2

# 清空所有规则
mihomo-cli rule clear --yes

# 导入/导出规则文件
mihomo-cli rule import ./my-rules.yaml
mihomo-cli rule export ./backup.yaml

# 设置默认插入位置（front=优先，back=兜底）
mihomo-cli rule position front
mihomo-cli rule position back
```

**工作原理：**

1. 规则存储在 `~/.config/mihomo/rules.yaml`（YAML 格式，与 mihomo 配置兼容）
2. 插入位置配置在 `~/.config/mihomo/.rules-position`（默认 `front`）
3. 每次启动/重载时，从 `config.original.yaml`（原始订阅配置）读取并合并用户规则到 `config.yaml`
4. 规则变更后自动调用 mihomo API 热重载

**插入位置说明：**

- `front`（默认）：用户规则优先级最高，覆盖订阅配置中的规则
- `back`：用户规则作为兜底，订阅配置优先

> 💡 **典型场景**：让公司内网、本地服务走直连，而不影响订阅配置中的其他规则。

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

## 术语说明

- **ISP** (Internet Service Provider)：互联网服务提供商，指你的宽带或移动网络运营商。`mihomo-cli ip` 命令中"Direct"路径显示的是 ISP 分配给你的真实公网 IP。

## 核心设计理念

- **安装 + 控制一体**：一个二进制完成从零部署到日常使用
- **订阅自动转换**：自动识别并转换 vmess:// / base64 / Clash YAML 三种格式
- **fzf 交互体验**：`dialoguer` 实现模糊搜索节点选择，无需记节点名
- **零运行时依赖**：不依赖 curl、jq、python3、fzf 外部工具
- **完善 CLI 体验**：clap 提供自动补全、模糊命令提示、`--help` 文档
- **真正的跨平台**：macOS LaunchDaemon + Linux systemd，统一命令接口

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
├── rules.rs         用户自定义路由规则管理
├── ui.rs            交互式 fuzzy-select
└── utils.rs         工具函数
```

## License

MIT
