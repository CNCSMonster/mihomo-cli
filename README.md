# mihomo-cli

跨平台的 Mihomo（Clash.Meta）命令行工具，将安装部署与日常控制集成在一个二进制文件中，支持 Linux、macOS 和 Windows。

[English](README_en.md) · [用户手册](docs/user-guide/README.md) · [变更记录](CHANGELOG.md) · [许可证](LICENSE)

## 功能

- 安装和管理 Mihomo Core、配置与服务。
- 导入订阅或 Clash/Mihomo YAML，管理代理组、节点、规则和 DNS。
- 使用交互式选择器切换节点，或通过命令行进行非交互操作。
- 在 system service 模式下按需显式开启或关闭 TUN。

## 安装

从 [Releases](https://github.com/CNCSMonster/mihomo-cli/releases) 下载适合平台的二进制文件，或使用 Rust 从源码安装：

```bash
cargo install --git https://github.com/CNCSMonster/mihomo-cli
```

克隆仓库后也可运行 `cargo install --path .`。默认安装目录为 `~/.cargo/bin`；请将其加入 `PATH`。

## 快速开始

以下命令安装 system service，再导入已有配置。安装会下载 Core 与 Geo 数据并启动普通 Core；默认不启用 TUN。

```bash
mihomo-cli install --system --yes
mihomo-cli config --import /path/to/config.yaml --activate --yes
mihomo-cli status
mihomo-cli list
```

如果导入结果提示 pending、unknown 或需要恢复，请按提示处理，必要时显式执行 `mihomo-cli restart --system`。没有管理员权限时，可使用 `mihomo-cli install --user` 安装普通代理模式。

需要透明代理时，在有效配置和 system service 就绪后，才显式执行 `mihomo-cli tun on`；关闭使用 `mihomo-cli tun off`。`mihomo-cli tun status` 无法证明当前运行态时会显示 `unknown`。服务或 TUN 状态就绪不等于已验证目标网络的数据面连通性。

更多操作与注意事项见[用户手册](docs/user-guide/README.md)；命令行帮助可通过 `mihomo-cli --help` 查看。

## 本地开发与测试

本项目支持标准 `cargo` 命令，也提供了便捷的 `justfile` 任务入口（需安装 [just](https://github.com/casey/just)）：

```bash
# 1. 快速检查代码格式与 Clippy Lint
just check    # 或 cargo fmt --check && cargo clippy --all-targets -- -D warnings

# 2. 运行单元与集成测试
just test     # 或 cargo test

# 3. 本地构建
just build    # 或 cargo build
just build-release # 或 cargo build --release
```

运行 `just --list` 可查看完整的可用命令列表（含容器端到端测试与交叉编译任务）。

## 反馈

问题和建议请提交 [GitHub Issue](https://github.com/CNCSMonster/mihomo-cli/issues)。

## 许可证

[MIT](LICENSE)
