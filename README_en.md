# mihomo-cli

**Cross-platform Mihomo CLI — setup + control, single binary, zero runtime dependencies**

[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)]()
[中文](README.md)

---

mihomo-cli is a Rust CLI tool for Mihomo (Clash.Meta). It combines **installation** and **daily control** into a single binary, with fzf-style interactive proxy selection, TUN mode toggle, vmess subscription auto-conversion, and more. Works on macOS, Linux, and Windows.

## Install

Build from source via GitHub:

```bash
cargo install --git https://github.com/CNCSMonster/mihomo-cli
```

Clone and build locally:

```bash
git clone https://github.com/CNCSMonster/mihomo-cli
cd mihomo-cli
cargo install --path .
```

[Releases](https://github.com/CNCSMonster/mihomo-cli/releases) provide pre-built binaries (Linux / macOS / Windows). Download, extract, and put in PATH.

## Quick Start

```bash
# 1. Install mihomo core + system service (auto-download, no manual dependencies)
mihomo-cli install

# 2. Start the service (TUN is off by default, won't affect current network)
mihomo-cli start

# 3. Add a subscription (supports vmess://, base64, Clash YAML, auto format conversion)
mihomo-cli config -u '<your-subscription-url>'

# 4. fzf fuzzy search to select a proxy node
mihomo-cli select

# 5. Enable TUN transparent proxy
mihomo-cli tun on

# ✅ Done — all traffic routed through proxy
```

**Other common commands:**

```bash
mihomo-cli ip          # Check current exit IP and location
mihomo-cli status      # Runtime status overview
mihomo-cli proxy on    # Set http_proxy env vars for current terminal (use eval)
mihomo-cli restart     # Restart service
mihomo-cli tun off     # Disable TUN
```

## Command Reference

### Installation

| Command | Description |
|---------|-------------|
| `mihomo-cli install` | Download core + generate config + install boot service (interactive) |
| `mihomo-cli config` | Configure subscription URL (supports vmess://, base64, Clash YAML) |
| `mihomo-cli uninstall` | Remove service (optionally remove all files) |
| `mihomo-cli update` | Update mihomo core binary |
| `mihomo-cli version` | Print version info |

### Service & Connection

| Command | Description |
|---------|-------------|
| `mihomo-cli start [--user]` | Start mihomo service |
| `mihomo-cli stop [--user]` | Stop mihomo service |
| `mihomo-cli restart [--user]` | Restart mihomo service |
| `mihomo-cli status` | Runtime status overview (includes exit IP) |

### Daily Use

| Command | Description |
|---------|-------------|
| `mihomo-cli select` | fzf interactive proxy selection (fuzzy search) |
| `mihomo-cli list` | List all proxy groups and current node |
| `mihomo-cli delay` | Test latency of nodes in a group |
| `mihomo-cli tun on/off` | Enable/disable TUN virtual NIC |
| `mihomo-cli ip` | Check current exit IP and location |
| `mihomo-cli proxy on/off` | Output shell proxy env vars (`eval "$(mihomo-cli proxy on)"`) |
| `mihomo-cli conn` | View active connections (`--flush` to close all) |
| `mihomo-cli completions` | Generate shell completions (bash/zsh/fish) |

> 💡 **All commands support `-h` / `--help`**, e.g. `mihomo-cli install -h`, `mihomo-cli config -h`.

## Platform Support

| Platform | Arch | Boot Service | TUN Mode |
|----------|------|-------------|----------|
| macOS | ARM64 / x64 | LaunchDaemon (root) | ✓ Supported |
| Linux | x64 / ARM64 | systemd user service | Extra config needed |
| Windows | x64 | sc.exe service | Admin required |

Cross-compilation via `cargo-zigbuild`, no Docker needed:

```bash
bash build.sh    # One-shot build for all 6 targets
```

## Design Principles

- **All-in-one**: Single binary for installation, configuration, and daily control
- **Auto subscription conversion**: Detects and converts vmess:// / base64 / Clash YAML automatically
- **fzf experience**: Fuzzy search proxy selection via `dialoguer`, no need to remember node names
- **Zero runtime dependencies**: No curl, jq, python3, or fzf required
- **Polished CLI**: clap-powered auto-completion, fuzzy command suggestions, `--help` docs
- **Truly cross-platform**: macOS LaunchDaemon + Linux systemd, unified command interface

## Build

```bash
# Prerequisites
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu x86_64-apple-darwin aarch64-unknown-linux-musl x86_64-unknown-linux-musl

# Build all platforms
bash build.sh

# Or build individually
cargo build --release
cargo zigbuild --target x86_64-unknown-linux-gnu --release
```

## Project Structure

```
src/
├── main.rs          CLI entry + command routing
├── mihomo_api.rs    Unix socket RESTful API client
├── config.rs        Subscription download + vmess → Clash YAML conversion
├── installer.rs     Mihomo core binary download
├── service.rs       macOS LaunchDaemon / Linux systemd
├── ui.rs            Interactive fuzzy-select
└── utils.rs         Utility functions
```

## License

MIT
