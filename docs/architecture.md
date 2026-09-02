# mihomo-cli 分层架构

> ADR-21：Linux daemon 以非 root 用户运行，mihomo core 通过 AmbientCapabilities 获得精确权限；macOS/Windows 保留平台特权服务模型。
> 本文档的权限图描述 Linux system 模式。
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
│  - 通过受权 IPC 转发 allowlisted Mihomo Core API 请求             │
│  - 在固定 system context 校验 snapshot、revision 和配置路径      │
│  - 并发生命周期锁 (OWNER_LIFECYCLE_LOCK)                         │
│  ✅ 以 mihomo 用户运行，不需要 root                               │
│  ⚠ 仅访问受管路径；system snapshot/journal 只按固定上下文、 │
│     owner/mode/no-follow/hash 规则校验，不接受调用方路径     │
└──────────────────────────┬──────────────────────────────────────┘
                           │ Unix Socket REST API
                           │ (/var/run/mihomo/mihomo.sock)
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                    mihomo core (代理内核)                         │
│                                                                 │
│  职责：                                                          │
│  - 读取实例上下文指定的配置启动代理服务                         │
│    user mode 使用已验证 intent config；system mode 使用          │
│    `active-config.yaml`；system/TUN 使用由事务派生的               │
│    `tun-config.yaml` 和显式 `-f`                                 │
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
Linux 当前实现 (最小权限):
  systemd → daemon(mihomo 用户) → core(mihomo 用户 + capabilities)
  ✅ daemon 被攻破仅暴露 mihomo 用户权限
  ✅ core 的 capabilities 由 CapabilityBoundingSet 约束

平台例外:
  macOS LaunchDaemon(root)；Windows SCM(SYSTEM)
  两者依赖 IPC 认证、peer 身份和 config owner 校验收缩边界
```

## Core API forwarding boundary

System-mode `CoreApiRequest` is an authorized request to the currently running Core, not a lifecycle command. The daemon validates the authenticated peer and the complete request before forwarding it; it must not call `ensure_core_running`, download or repair resources, start or restart Core, or mutate TUN state while handling this request.

The forwarding contract is deny-by-default:

- `GET` is limited to explicitly listed read-only resources required by the current CLI, including `/configs`, `/proxies`, `/connections`, and other paths only when added to the formal SPEC.
- `PUT`/`PATCH` are limited to explicitly listed non-TUN runtime mutations. Each method/path/query/body field, target instance, and body size is validated against the command-specific allowlist.
- `POST`, `DELETE`, unknown methods, unknown paths, cross-instance endpoints, query fields, body fields, oversized bodies, config paths, and arbitrary configuration bytes are rejected unless the formal SPEC explicitly lists them.
- Any `tun`, `dns-hijack`, system-route, config-path, or equivalent TUN-control input is rejected by the generic proxy. TUN changes use only the dedicated root-peer `ApplySystemTunSnapshot`/`DisableTun` transaction with candidate, revision, fixed snapshot, and runtime attestation.
- If Core/API is not ready, the request returns `Incomplete` or `Unknown` with the explicit `mihomo-cli restart [--system]` next step; it never starts Core implicitly.

An existing protocol enum, legacy handler, or client helper is not permission to forward that operation. Until an old branch is migrated to this contract, it must fail closed. Tests must cover rejection of unknown methods/paths/query/body fields, TUN-shaped generic mutations, lifecycle side effects, and requests made while Core is stopped.

## `tun on` 时序图（当前事务模型）

```
User          CLI Client              Daemon                  mihomo core
 │               │                      │                         │
 │  tun on       │                      │                         │
 │──────────────>│                      │                         │
 │               │                      │                         │
 │               │ 1. 无副作用 preflight │                         │
 │               │    service/IPC/config/capability                │
 │               │    冲突与 owner/no-follow 校验                 │
 │               │                      │                         │
 │               │ 2. sudo re-exec       │                         │
 │               │    root peer gate      │                         │
 │               │                      │                         │
 │               │ 3. 生成 immutable candidate                         │                         │
 │               │    校验 expected_revision                         │
 │               │    持久化 candidate/pending                         │                         │
 │               │                      │                         │
 │               │ 4. IPC:               │                         │
 │               │    ApplySystemTunSnapshot                        │
 │               │    `{ expected_revision, stack, dns_hijack }`                       │
 │               │ ───────────────────> │                         │
 │               │                      │ 5. 固定 system context  │
 │               │                      │    root peer/owner/path │
 │               │                      │    revalidation          │
 │               │                      │                         │
 │               │                      │ 6. 原子提交受保护 snapshot│
 │               │                      │    journal + rollback    │
 │               │                      │    tun-config.yaml       │
 │               │                      │                         │
 │               │                      │ 7. 显式以 -f tun-   │
 │               │                      │    config.yaml 启动 │
 │               │                      │    /重载 Core       │
 │               │                      │ ─────────────────────> │
 │               │                      │                         │
 │               │                      │                         │ 8. 创建 TUN、路由、DNS │
 │               │                      │                         │    并由 API 观察运行态 │
 │               │                      │                         │    runtime_tun == enabled│
 │               │                      │                         │    + revision/journal/ │
 │               │                      │                         │      instance attestation│
 │               │                      │ <───────────────────── │
 │               │                      │ 9. readiness + /configs │
 │               │                      │    runtime_tun == enabled│
 │               │                      │    + launched/active revision、│
 │               │                      │      journal=IntentCommitted、│
 │               │                      │      instance attestation │
 │               │                      │                         │
 │               │ 10. 成功/回滚结果     │                         │
 │               │    仅完整 attestation 才能报告 TunRunning；     │
 │               │    否则 unknown/RecoveryRequired               │
 │               │ <────────────────── │                         │
 │  ✅ TUN on    │                      │                         │
 │<──────────────│                      │                         │
```

CLI 不直接写 system snapshot，也不直接调用 Core API。daemon 不以任意用户路径替代固定 system context，且不得用通用 `PATCH /configs` 伪造 TUN 事务；所有 system TUN 变更必须携带 candidate/revision，经 root revalidation、受保护 snapshot 提交和当前 Core API runtime observation。任一步骤失败时优先按 journal 执行回滚或继续恢复；无法证明精确恢复但能证明相关 runtime、事务和进程属于当前 mihomo instance 时，由既有 `restart`/TUN/清理命令在确认后执行受管 runtime reset，保留用户 intent；归属不可证明时才返回 `RecoveryRequired`，且不得要求用户手工操作内部文件。

## 受管 runtime reset（用户态自愈）

`RecoveryRequired` 不是要求普通用户理解或清理 transaction 的用户接口，而是内部状态。任何会改变运行态的既有命令都必须遵循以下顺序：

```text
目标命令（默认 restart）
  → 尝试有证据的 roll-forward / rollback
  → 仍无法精确恢复？验证 runtime、事务、残留进程是否属于当前 mihomo
  → 归属可证明：说明保留配置、重置运行状态、可能中断连接
  → 交互确认或显式 --yes
  → 停止受管 Core，必要时关闭 TUN
  → 仅清理/隔离 active-config、TUN snapshot、transaction/recovery evidence、attestation
  → 从用户 intent 重建固定 runtime
  → 启动 Core，等待 API readiness 和 runtime attestation
  → 原命令继续完成
```

reset 的白名单只包含当前 instance 管理且可重建的运行态资产；用户 intent、订阅 metadata/cache、规则、DNS policy 和其它用户配置必须保留。`status`、`doctor`、`tun status` 不得触发 reset。无法证明归属时，命令必须保留 last-known-good 和恢复证据，输出不暴露 journal/phase 的既有 `mihomo-cli` 下一步；禁止建议用户执行 `rm`、`chown`、`kill` 或 `systemctl`。

## `tun on` 当前合同补充

`tun on` 已实现的事务路径遵循 candidate/revision、受保护 snapshot、root peer gate 和当前 Core API runtime observation。下列 `CoreApplied → compare-and-commit → IntentCommitted` 完整 dispatcher 是所有 TUN-active 配置变更应收敛到的目标合同；当前并非所有 config/rule/DNS/override/TUI 入口都已统一接入或验收。

```text
普通用户 CLI
  → 无副作用 preflight（service、IPC、config、冲突、TUN capability）
  → CLI 内部 sudo re-exec + root peer gate
  → immutable candidate/revision 持久化与 root revalidation
  → `Prepared` journal + 受保护 snapshot 原子 promotion
  → daemon IPC `ApplySystemTunSnapshot`
  → Core 显式使用 `-f tun-config.yaml` 启动/重载
  → readiness + Core API `/configs` + launched revision observation
  → daemon 持久化 `CoreApplied`，释放 `OWNER_LIFECYCLE_LOCK`
  → dispatcher coordinator 重新取得用户配置锁并 compare-and-commit
  → 按 manifest 提交 intent，推进 `IntentCommitted`
  → 成功或按 journal 恢复 old revision
```

preflight 阻断时不写配置、snapshot、journal、路由或 TUN；事务、Core/API readiness 或 runtime observation 失败时执行受管回滚，无法证明恢复则返回 `RecoveryRequired`。成功结论只能来自当前 Core API；API 不可达时为 `unknown`/失败，不能由 `tun.enable` 或 daemon 缓存推断。

## 配置与运行时文件路径 (Linux System 模式)

```
~/.config/mihomo/config.yaml          ← intent_config_file；配置单一事实来源 (ADR-22)
│                                         config import / switch / refresh
│                                         规则 / DNS 策略 / override 合并后的用户意图
│                                         不作为 system Core 的直接运行时路径
│
│  已校验内容通过 IPC promotion
│                 │
│                 ├── /var/lib/mihomo-cli/active-config.yaml
│                 │     ← 普通 system Core 的固定 `-f` 运行时输入
│                 │
│                 └── TUN candidate/revision 事务
│                               │
│                               ▼
│     /var/lib/mihomo-cli/tun-config.yaml
│       ← `mihomo:mihomo 0640` 的受保护派生 snapshot；system TUN Core 的固定 `-f` 输入
│         不是第二配置事实来源；由事务提交/回滚管理
│
/var/run/mihomo/mihomo.sock           ← mihomo core REST API endpoint
/var/run/mihomo/service.sock          ← daemon IPC endpoint
/var/log/mihomo/mihomo.log            ← core 日志
/etc/systemd/system/mihomo.service    ← systemd service 文件
```

> ADR-22 仅废弃 `/var/lib/mihomo-cli/config.yaml` 这类旧的第二份 `config.yaml` system store；不会废弃 `active-config.yaml`、`tun-config.yaml` 等固定 system runtime 资产。

## 参考

- ADR-21: System daemon 最小权限 — AmbientCapabilities 替代 root daemon (SPEC.md)
- [1] mihomo 官方 service 文件: https://wiki.metacubex.one/en/startup/service
- [2] clashtui: https://github.com/JohanChane/clashtui
- [3] Clash Verge Rev CVE-2025-50505: https://www.sentinelone.com/vulnerability-database/cve-2025-50505
- [4] NixOS Mihomo: https://wiki.nixos.org/wiki/Mihomo
