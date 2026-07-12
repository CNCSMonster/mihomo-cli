# Changelog

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
