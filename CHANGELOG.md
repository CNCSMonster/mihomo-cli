# Changelog

## Unreleased

- Added user-defined routing rule management via `mihomo-cli rule`, including add/list/remove/clear/import/export commands and configurable front/back insertion into generated mihomo config.
- Added DNS policy management and documentation, allowing per-domain DNS nameserver policies to be managed alongside subscription configuration.
- Improved `mihomo-cli ip` diagnostics: probes both direct and mihomo-proxy paths, supports `--url` target-route testing, reports TUN status context, and marks LAN/private exit addresses.
- Hardened config merging for user rule marker blocks with YAML-valid atomic writes and tests for rule insertion/removal behavior.
- Improved GeoIP/GeoSite bootstrap reliability by validating downloaded geo files and avoiding repeated invalid downloads during start/restart flows.
- Expanded README and SPEC documentation for rule management, DNS policy behavior, and IP diagnostics.
