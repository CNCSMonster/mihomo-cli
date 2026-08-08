# mihomo-cli 分层架构

> ADR-21 决策：daemon 以非 root 用户运行，mihomo core 通过 AmbientCapabilities 获得精确权限。
> 本文档描述当前实现 + ADR-21 目标架构。
> ADR-22 决策：删除 system store，config 单一事实来源为 `~/.config/mihomo/config.yaml` (`intent_config_file`)。

## 总体分层

```
┌─────────────────────────────────────────────────────────────────┐
│                         用户 (User)                              │
│                  mihomo-cli tun on / restart / select ...       │
└──────────────────────────┬──────────────────────────────────────┘
                           │ CLI 命令
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│              mihomo-cli 前台进程 (CLI Client)                     │
│                                                                 │
│  职责：                                                          │
│  - clap 参数解析                                                 │
│  - 读写 intent_config_file (~/.config/mihomo/config.yaml)       │
│  - 合并用户规则 / DNS 策略 / override.yaml                       │
│  - TUI 交互 (select / config 交互模式)                           │
│  - 发送 IPC 命令给 daemon                                        │
│  ⚠ 不需要 root 权限                                              │
└──────────────────────────┬──────────────────────────────────────┘
                           │ Unix Socket IPC
                           │ (/var/run/mihomo/service.sock)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│         mihomo-cli daemon (后台守护进程, 非 root)  [ADR-21]       │
│                                                                 │
│  职责：                                                          │
│  - 接收 CLI 的 IPC 命令                                          │
│  - 管理 mihomo core 进程生命周期 (start/stop/restart)            │
│  - readiness 检测 (等 core API 就绪再回复 CLI)                   │
│  - 调用 mihomo core REST API (PATCH /configs 切换 TUN 等)       │
│  - 并发生命周期锁 (OWNER_LIFECYCLE_LOCK)                         │
│  ✅ 以 mihomo 用户运行，不需要 root                               │
│  ⚠ 文件 I/O 仅限 mihomo 用户目录                                  │
└──────────────────────────┬──────────────────────────────────────┘
                           │ Unix Socket REST API
                           │ (/var/run/mihomo/mihomo.sock)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                    mihomo core (代理内核)                         │
│                                                                 │
│  职责：                                                          │
│  - 读取 config.yaml 启动代理服务                                 │
│  - HTTP/SOCKS5 代理 (mixed-port, L7 应用层)                      │
│  - TUN 虚拟网卡 (L3 网络层, 全机透明代理)                        │
│  - DNS 解析 (fake-ip / redir-host)                              │
│  - 规则匹配 + 流量转发                                           │
│  - REST API (/configs, /proxies, /connections ...)              │
│                                                                 │
│  权限 (通过 systemd AmbientCapabilities):                        │
│  - CAP_NET_ADMIN: 创建 TUN 设备 + 修改路由表                     │
│  - CAP_NET_RAW: 原始套接字 (TUN 所需)                            │
│  - CAP_NET_BIND_SERVICE: 绑定端口 53 (DNS hijack)               │
│  ✅ 不需要完整 root，仅 capabilities                              │
└─────────────────────────────────────────────────────────────────┘
```

## 权限模型对比

```
当前实现 (root daemon):
  systemd(root) → daemon(root) → core(root)
  ⚠ daemon 被攻破 = root 泄露 (参见 CVE-2025-50505)

ADR-21 目标 (最小权限):
  systemd → daemon(mihomo 用户) → core(mihomo 用户 + capabilities)
  ✅ daemon 被攻破仅暴露 mihomo 用户权限
  ✅ core 的 capabilities 由 CapabilityBoundingSet 约束
```

## `tun on` 时序图 (当前实现, 含 bug)

```
User          CLI Client              Daemon                  mihomo core
 │               │                      │                         │
 │  tun on       │                      │                         │
 │──────────────>│                      │                         │
 │               │                      │                         │
 │               │ 1. snapshot_file()   │                         │
 │               │    读 intent_config_file ❌                    │
 │               │    (~/.config/mihomo/config.yaml)              │
 │               │    ── 失败 ──>       │                         │
 │               │    (权限/路径不可读)   │                         │
 │               │                      │                         │
 │  Error: cannot read config for TUN   │                         │
 │<──────────────│                      │                         │
```

## `tun on` 时序图 (config 路径修复后)

```
User          CLI Client              Daemon                  mihomo core
 │               │                      │                         │
 │  tun on       │                      │                         │
 │──────────────>│                      │                         │
 │               │                      │                         │
 │               │ 1. snapshot_file()   │                         │
 │               │    读 intent_config_file ✓                    │
 │               │    (~/.config/mihomo/config.yaml)              │
 │               │                      │                         │
 │               │ 2. set_instance_tun_config()                   │
 │               │    读 ~/.config/mihomo/config.yaml ✓          │
 │               │    改 tun.enable = true                        │
 │               │    改 tun.stack / dns-hijack                   │
 │               │    写回 intent_config_file ✓                   │
 │               │                      │                         │
 │               │ 3. IPC: EnableTun     │                         │
 │               │ ───────────────────> │                         │
 │               │                      │                         │
 │               │                      │ 4. PATCH /configs       │
 │               │                      │    { tun: { enable } }  │
 │               │                      │ ─────────────────────> │
 │               │                      │                         │
 │               │                      │         5. 创建 TUN 设备│
 │               │                      │ (CAP_NET_ADMIN 授权)    │
 │               │                      │            修改路由表    │
 │               │                      │            DNS 劫持     │
 │               │                      │                         │
 │               │                      │    6. 200 OK            │
 │               │                      │ <───────────────────── │
 │               │                      │                         │
 │               │    7. Success        │                         │
 │               │ <────────────────── │                         │
 │  ✅ TUN on    │                      │                         │
 │<──────────────│                      │                         │
```

## 配置文件路径 (Linux System 模式)

```
~/.config/mihomo/config.yaml          ← intent_config_file；配置单一事实来源 (ADR-22)
│                                         CLI 的 config --import / switch / refresh
│                                         规则 / DNS 策略 / override 合并后的主配置
│                                         restart 时 daemon 直接传给 core 的路径
│                                         tun on/off 时 CLI 读写 TUN 设置
│                                         daemon 通过目录权限直接读取此文件
│
/var/run/mihomo/mihomo.sock           ← mihomo core REST API endpoint
/var/run/mihomo/service.sock          ← daemon IPC endpoint
/var/log/mihomo/mihomo.log            ← core 日志
/etc/systemd/system/mihomo.service    ← systemd service 文件
```

> ADR-22：`/var/lib/mihomo-cli/config.yaml`（旧 `config_file` / system store）已删除，不再作为 daemon merge 后的最终产物或 fallback。所有配置意图与运行时启动配置统一来自 `~/.config/mihomo/config.yaml`。

## 参考

- ADR-21: System daemon 最小权限 — AmbientCapabilities 替代 root daemon (SPEC.md)
- [1] mihomo 官方 service 文件: https://wiki.metacubex.one/en/startup/service
- [2] clashtui: https://github.com/JohanChane/clashtui
- [3] Clash Verge Rev CVE-2025-50505: https://www.sentinelone.com/vulnerability-database/cve-2025-50505
- [4] NixOS Mihomo: https://wiki.nixos.org/wiki/Mihomo
