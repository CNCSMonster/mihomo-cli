# mihomo-cli

**Cross-platform Mihomo CLI — setup + control, single binary, zero runtime dependencies**

[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)]()
[中文](README.md)

---

mihomo-cli is a Rust CLI tool for Mihomo (Clash.Meta). It combines **installation** and **daily control** into a single binary, with crossterm TUI interactive proxy selection (j/k navigation + / filter), TUN mode toggle, vmess subscription auto-conversion, and more. Works on macOS, Linux, and Windows.

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

`cargo install --path .` installs to `$CARGO_HOME/bin/mihomo-cli` by default; when `CARGO_HOME` is unset, this is usually `~/.cargo/bin/mihomo-cli`. Make sure `~/.cargo/bin` is in `PATH`.

[Releases](https://github.com/CNCSMonster/mihomo-cli/releases) provide pre-built binaries (Linux / macOS / Windows). Download, extract, and put in PATH.

## Minimal Recommended Workflow

Install the system infrastructure first. Without a subscription, install generates a validated direct-only config and starts ordinary Core/API with TUN disabled. `restart` proves only daemon/Core/API control-plane readiness; it does not prove public connectivity, DNS/DIRECT behavior, proxy-group egress, or TUN data-plane success. After readiness, enable TUN explicitly when system-wide transparent proxying is required; `tun on` is successful only after the managed transaction and current Core API runtime attestation prove the target state.

```bash
mihomo-cli uninstall --all --yes          # Optional: clean managed instances first
mihomo-cli install --system --yes         # Generate direct-only config and start ordinary Core; TUN remains off
mihomo-cli config --import ./config.yaml --activate --yes # Running system services promote the import immediately
# Only when import reports pending/unknown/recovery, or to apply a pending generation:
# mihomo-cli restart --system
mihomo-cli select                          # Pick a node
mihomo-cli status                          # Read-only status summary; unknown when runtime state is not provable
mihomo-cli exit-ip --group "Proxy"        # Explicit data-plane exit probe
mihomo-cli rule test baidu.com            # Check rule matching; not a data-plane probe
mihomo-cli tun on --yes                    # Enable TUN; requires transaction and runtime attestation
mihomo-cli tun status                      # Observe attested runtime state; unknown when unprovable
mihomo-cli tun off                         # Explicitly disable TUN
mihomo-cli restart --system                # Re-apply persistent state when needed
```

Without admin rights, use `mihomo-cli install --user` for normal proxy mode. `--system` selects the system service context; TUN is only available there. `start` remains a compatibility/advanced lifecycle command, while the normal workflow uses `restart`. Read-only commands do not implicitly start Core, request sudo, access the network, or perform recovery.

## Command Reference

### Installation

| Command | Description |
|---------|-------------|
| `mihomo-cli install --system` | Install system infrastructure, Core, Geo, and authorization prerequisites; without a subscription it generates direct-only config and starts ordinary Core/API with TUN disabled |
| `mihomo-cli config --import <file> --activate --yes` | Validate and commit a user configuration; current implementation uses flat config action flags |
| `mihomo-cli uninstall` | Remove service (optionally remove all files) |
| `mihomo-cli update` | Update mihomo core binary for the resolved instance |

### Service & Connection

| Command | Description |
|---------|-------------|
| `mihomo-cli start` | Compatibility/advanced lifecycle command; the normal workflow uses explicit `restart` |
| `mihomo-cli stop` | Stop the selected user or system service/core |
| `mihomo-cli restart --system` | Explicitly start or restart the system Core and wait for daemon/Core API control-plane readiness; this does not prove public or TUN data-plane success |
| `mihomo-cli status` | Read-only `StatusSnapshot` summary; no network probe or recovery, and runtime values are `unknown` when not provable |

### Daily Use

| Command | Description |
|---------|-------------|
| `mihomo-cli select` | Interactive node selector (j/k navigate, / filter, Enter select) |
| `mihomo-cli list` | List all proxy groups and current node |
| `mihomo-cli delay [--refresh] [--fastest]` | Batch-test group latency, reuse fresh cache, optionally select fastest node |
| `mihomo-cli tun on/off/status` | Enable/disable/check TUN virtual NIC (system service only); `tun status` reports `unknown` unless the current runtime and revision attestation are provable |
| `mihomo-cli proxy on/off` | Output shell proxy env vars (`eval "$(mihomo-cli proxy on)"`) |
| `mihomo-cli conn` | View active connections (`--flush` to close all) |
| `mihomo-cli rule/dns/backup/restore` | Manage per-user config for the auto-detected/resolved instance |
| `mihomo-cli exit-ip --node/--group/--url/--direct` | Probe node/group/route/direct exit IP. `mihomo-cli ip` remains as a deprecated current-proxy probe. |

> 💡 **All commands support `-h` / `--help`**, e.g. `mihomo-cli install -h`, `mihomo-cli config -h`.

## Platform Support

| Platform | Arch | Boot Service | TUN Mode |
|----------|------|-------------|----------|
| macOS | ARM64 / x64 | LaunchDaemon (system) / LaunchAgent (per-user) | ✓ Supported in system service mode |
| Linux | x64 / ARM64 | systemd system / systemd --user | ✓ Supported in system service mode |
| Windows | x64 | Windows Service / per-user process | ✓ Supported in system service mode |

Cross-compilation via `cargo-zigbuild`, no Docker needed:

```bash
bash build.sh    # One-shot build for all 6 targets
```

## Design Principles

- **All-in-one**: Single binary for installation, configuration, and daily control
- **Auto subscription conversion**: Detects and converts vmess:// / base64 / Clash YAML automatically
- **crossterm TUI**: `select` and `config` use crossterm for real keyboard shortcuts (j/k navigate, / filter); `select --node` switches non-interactively
- **Zero runtime dependencies**: No curl, jq, python3, or fzf required
- **Polished CLI**: clap derive typed argument parsing, `--help` docs
- **Truly cross-platform**: macOS LaunchDaemon/LaunchAgent + Linux systemd system/user + Windows service/user process, unified command interface; **Windows is a second-class citizen** (verified via pub repo CI runner)

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
├── main.rs          CLI entry + command routing (clap)
├── mihomo_api.rs    Unix socket REST client
├── config.rs        Subscription management + config generation
├── installer.rs     Core binary download + Geo file management
├── service.rs       Service execution + privilege layer (systemd/LaunchDaemon)
├── instance.rs      Instance Model: path matrix + mode resolution + service plans
├── daemon.rs        Daemon process (IPC + readiness + lifecycle serialization)
├── ipc.rs           Daemon IPC client
├── lock.rs          Concurrency locks
├── rules.rs         User routing rule management
├── dns.rs           DNS routing policy management
├── backup.rs        Config backup & restore
├── system_proxy.rs  System proxy (macOS/Linux)
├── ui.rs            Interactive TUI (crossterm)
├── yaml_editor.rs   serde_yaml validation + marker editor
└── utils.rs         Path/utility functions
```

## License

MIT


## Advanced config override

`mihomo-cli` supports an optional override file at `~/.config/mihomo/override.yaml`. Manage it with `mihomo-cli override path/show/import/clear`, optionally with `--system` for the system service context.
It is applied when `config.yaml` is generated from the active subscription, user rules, and DNS policies.

Example:

```yaml
proxy-groups:
  - name: Custom
    type: select
    proxies:
      - DIRECT
dns:
  enhanced-mode: redir-host
```

Merge semantics:

- YAML maps are merged recursively.
- Lists and scalar values replace the generated value.
- Runtime controller fields are re-injected after override, so `override.yaml` cannot break the mihomo API socket/pipe configuration.



## Subscription UA negotiation

When adding or refreshing a subscription URL, mihomo-cli tries a small set of Clash-compatible User-Agents sequentially and stops as soon as Clash YAML is returned, reducing rate-limit risk. Use `mihomo-cli config probe <URL>` to inspect candidate UA responses, `mihomo-cli config ua set <id> <ua|auto>` to pin or restore a subscription's UA, or the local `--user-agent` option for a single `config fetch/add/refresh` operation.

Current boundary: UA probing is intentionally limited to Clash/Mihomo-compatible configurations. The goal is to obtain the provider's original Clash YAML; mihomo-cli does not probe non-Clash ecosystems such as Surge, Quantumult X, Shadowrocket, or v2rayN by default. Support for those ecosystems should be designed as a separate future extension.

## E2E tests

E2E tests live under `tests/e2e/` and are wired into Cargo through `tests/e2e.rs`. The current scenario covers fixture config → user rule merge → `mihomo -t` validation → generated YAML validity assertion. See `docs/testing/e2e.md`.

## Delay testing cache and fastest selection

`mihomo-cli delay` batch-tests all nodes in the target group through mihomo's group delay API. Results are sorted by latency and cached in `~/.config/mihomo/delay-cache.json` for 300 seconds by default.

```bash
mihomo-cli delay --group "节点选择"
mihomo-cli delay --refresh              # ignore cache and re-test
mihomo-cli delay --cache-ttl 60          # only reuse results newer than 60s
mihomo-cli delay --fastest               # select the fastest successful node
```

## TUN options

```bash
mihomo-cli tun on --stack gvisor --dns-hijack
mihomo-cli tun on --stack system --dns-hijack any:53
mihomo-cli tun off
mihomo-cli tun status
```

`--dns-hijack` without a value defaults to `any:53`.
