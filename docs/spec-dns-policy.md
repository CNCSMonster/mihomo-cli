# SPEC: mihomo-cli dns — DNS Policy Management

## Motivation

mihomo 的 DNS 模块和规则引擎是独立的。`DOMAIN-SUFFIX → DIRECT` 只控制流量出口，不控制 DNS 解析。当 mihomo 使用公共 DNS（如 `114.114.114.114`）时，内部域名（如 `gitlab.rd.ubtrobot.com`）无法解析，导致 DIRECT 连接在 DNS 阶段就失败。

`nameserver-policy` 是 mihomo DNS 的配置项，允许指定特定域名的 DNS 查询路由到指定上游。本功能提供 CLI 接口管理该配置。

## Subcommands

```
mihomo-cli dns policy add <MATCH> <TARGET>
    Add a DNS routing policy: MATCH domain → TARGET DNS servers

    MATCH   Domain suffix pattern (e.g. ubtrobot.com)
    TARGET  DNS server IP, or comma-separated list (e.g. 10.10.1.251,10.10.1.120)

mihomo-cli dns policy list
    List all DNS policies (1-based index)

mihomo-cli dns policy remove <INDEX|MATCH>
    Remove a policy by index or by match value

mihomo-cli dns status
    Show current DNS configuration summary (reads from mihomo API)
```

## Examples

```bash
# Single DNS server
$ mihomo-cli dns policy add company.internal 10.10.1.251
  ✓ Policy added: company.internal → 10.10.1.251
  ✓ Config reloaded — DNS policy is now active

# Multiple DNS servers
$ mihomo-cli dns policy add ubtrobot.com 10.10.1.251,10.10.1.120
  ✓ Policy added: ubtrobot.com → 10.10.1.251,10.10.1.120
  ✓ Config reloaded — DNS policy is now active

$ mihomo-cli dns policy list
  DNS policies:
  1. company.internal → 10.10.1.251
  2. ubtrobot.com → 10.10.1.251,10.10.1.120

$ mihomo-cli dns policy remove ubtrobot.com
  ✓ Policy removed: ubtrobot.com
  ✓ Config reloaded

$ mihomo-cli dns status
  DNS: enabled (fake-ip)
  Default nameservers: 114.114.114.114, 223.5.5.5, 119.29.29.29
  Fake-IP range: 28.0.0.1/8
  Listen: 127.0.0.1:1053
  Policies:
    1. company.internal → 10.10.1.251
```

## TARGET Format

`TARGET` 必须是 DNS 服务器的实际 IP 地址，**不支持**伪协议（如 `system://`）。

- 单 IP：`10.10.1.251`
- 多 IP：`10.10.1.251,10.10.1.120`（逗号分隔，不含空格）

在合并到 `config.yaml` 时：
- 单 IP → YAML 标量：`ubtrobot.com: 10.10.1.251`
- 多 IP → YAML 序列：`ubtrobot.com:\n  - 10.10.1.251\n  - 10.10.1.120`

## Storage

```
~/.config/mihomo/dns-policy.yaml     ← user-defined DNS policies
~/.config/mihomo/config.yaml         ← merged mihomo config
```

`dns-policy.yaml` format:

```yaml
policies:
- domain: ubtrobot.com
  target: 10.10.1.251,10.10.1.120
- domain: company.internal
  target: 10.10.1.251
```

## Merge Strategy

`merge_user_config()` 读取 `config.original.yaml`（或 `config.yaml`），合并 `dns-policy.yaml` 到 `dns.nameserver-policy`，写入 `config.yaml`。与 rules 合并在同一次操作中完成。

## Hot-Reload

DNS 策略通过两步生效：
1. **写入文件**：策略持久化到 `dns-policy.yaml`，然后 `merge_user_config()` 写入 `config.yaml`
2. **API PATCH**：调用 `PATCH /configs` 推送到 mihomo 内存

**已知限制**：mihomo 的 `PATCH /configs` 对 DNS 配置热重载不可靠（可能返回 204 但不生效）。如果 `dns status` 仍显示旧值，需要 `mihomo-cli restart`（`restart` 内部调用 `merge_user_config()` 后再重启）。

## Scope & Limitations

- 只管理 `nameserver-policy` — 不管理 `default-nameserver`、`enhanced-mode`、`fake-ip-range` 等。这些由订阅配置决定。
- `dns status` 只读，数据来自 mihomo API (`GET /configs`)。
- 策略是域名后缀匹配（匹配 `*.ubtrobot.com` 和 `ubtrobot.com`）。
- TARGET 必须是 IP 地址，不支持主机名。

## Files Changed

| File | Change |
|------|--------|
| `src/main.rs` | Add `Dns` command, `DnsAction`, `DnsPolicyAction` enums, `cmd_dns()` |
| `src/config.rs` | `merge_rules_to_config` → `merge_user_config` (merges both rules + DNS) |
| `src/dns.rs` | New file: load/save `dns-policy.yaml`, add/remove/list, `to_nameserver_policy()` |
| `src/mihomo_api.rs` | Add `get_config()`, update `reload_configs()` |
| `src/utils.rs` | Add `dns_policy_path()` |
| `SPEC.md` | Document new command |
