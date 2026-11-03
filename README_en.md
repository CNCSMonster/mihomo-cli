# mihomo-cli

A cross-platform command-line tool for installing and controlling Mihomo (Clash.Meta) on Linux, macOS, and Windows.

[中文](README.md) · [User guide](docs/user-guide/README.md) (Chinese) · [Changelog](CHANGELOG.md) · [License](LICENSE)

## Features

- Install and manage Mihomo Core, configuration, and services.
- Import subscriptions or Clash/Mihomo YAML and manage proxy groups, nodes, rules, and DNS.
- Select nodes interactively or use non-interactive CLI commands.
- Explicitly enable or disable TUN when using a system service.

## Installation

Download a binary for your platform from [Releases](https://github.com/CNCSMonster/mihomo-cli/releases), or install from source with Rust:

```bash
cargo install --git https://github.com/CNCSMonster/mihomo-cli
```

Alternatively, clone the repository and run `cargo install --path .`. The default installation directory is `~/.cargo/bin`; add it to your `PATH`.

## Getting started

The following commands install the system service and import an existing configuration. Installation downloads Core and Geo data and starts the ordinary Core; it does not enable TUN by default.

```bash
mihomo-cli install --system --yes
mihomo-cli config --import /path/to/config.yaml --activate --yes
mihomo-cli status
mihomo-cli list
```

If the import reports pending, unknown, or recovery, follow its guidance and explicitly run `mihomo-cli restart --system` when needed. Without administrator access, use `mihomo-cli install --user` for normal proxy mode.

For transparent proxying, explicitly run `mihomo-cli tun on` only after the configuration and system service are ready; use `mihomo-cli tun off` to disable it. `mihomo-cli tun status` reports `unknown` when the current runtime state cannot be proven. Service or TUN readiness does not by itself verify data-plane connectivity to a destination.

See the [user guide](docs/user-guide/README.md) for details, or run `mihomo-cli --help` for command-line help.

## Development & Testing

This project supports standard `cargo` workflows as well as convenient `justfile` recipes (requires [just](https://github.com/casey/just)):

```bash
# 1. Format and Clippy checks
just check    # or: cargo fmt --check && cargo clippy --all-targets -- -D warnings

# 2. Run unit and integration tests
just test     # or: cargo test

# 3. Build locally
just build    # or: cargo build
just build-release # or: cargo build --release
```

Run `just --list` to inspect all available recipes (including container E2E and cross-compilation tasks).

## Feedback

Please open a [GitHub Issue](https://github.com/CNCSMonster/mihomo-cli/issues) for questions or suggestions.

## License

[MIT](LICENSE)
