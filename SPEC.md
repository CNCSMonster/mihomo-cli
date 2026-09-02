# SPEC

> Mihomo CLI — cross-platform setup & control tool for Mihomo (Clash.Meta) proxy

> **权威性与状态：** 本文件是项目唯一的产品与架构权威 SPEC。本文档区分 `Implemented`、`Contract-tested`、`Real-Core-tested` 和 `Planned`，不得把局部代码存在描述为完整用户旅程已完成。

## 0. Unified Product Contract

### 0.1 Product goal

`mihomo-cli` 面向普通用户表达目标，而不是管理底层状态。普通用户的主产品动作是：

```text
install → config import/add/refresh → restart → rule / dns / exit-ip
uninstall（按明确范围清理）
```

服务、daemon、Core、权限、socket、systemd、TUN 和网络恢复由既有命令内部收敛。底层 flag/命令可保留给脚本、测试和排障，但不属于普通用户主路径。自动修复必须在原命令流程内完成；安全且无业务副作用的修复可自动执行，有副作用或影响全机网络的修复必须先确认，需要管理员权限时由 CLI 内部请求 OS 授权。

**统一入口与副作用边界**：

- 所有用户可感知的安装、更新、启动、停止、重启、恢复、版本校准和服务协调都必须以 `mihomo-cli` 为入口；用户不应被要求直接执行 `systemctl`、`launchctl`、`sc.exe` 或其他平台服务管理器命令。
- `install` 负责下载、校验和准备变更。它不得为了更新正在运行的 daemon/Core 而静默停止任何运行实例；安装阶段必须先完成所有可能依赖现有代理的网络操作。
- `restart` 是用户明确授权的变更应用边界。它可以在必要时停止/重启 Core 或 daemon，切换已准备的版本，并等待 daemon IPC 和 Core API 恢复 ready；它也是受管运行态的统一收敛入口：先尝试安全恢复，必要时在用户确认后重置 mihomo 管理的 runtime。
- `--yes` 只确认当前命令已经明确声明的交互步骤，不把 `install` 变成可以静默制造网络中断的命令；需要应用运行中二进制更新时仍由显式 `mihomo-cli restart` 执行。

### 0.2 Canonical user journeys

基础服务主旅程是唯一的 clean reinstall 合同。订阅源不是安装或 Core 启动的必要条件；安装必须先建立一份可运行的本地基础配置，之后用户可以再添加订阅：

```text
uninstall --all --yes
→ install --system --yes
→ status（direct-only Core/API 可用，TUN disabled）
→ config import/add/refresh（可选，替换或重建 UserEffectiveConfig）
→ restart --system（需要时显式应用新配置）
```

- `uninstall --all` 是显式删除范围；`--yes` 只跳过应用确认，不扩大范围，也不绕过 sudo/OS 授权。
- 没有订阅源的 `install --system --yes` 仍必须生成并校验 `config.yaml`，其中至少包含 `mode: rule`、受管 mixed port、受管 controller endpoint、空 `proxies`/`proxy-groups` 和 `MATCH,DIRECT` 兜底规则；不得把无订阅误报为无配置。
- 基础配置只提供 direct-only 控制面和普通代理端口，不访问订阅 URL，不启用 TUN，不代表存在代理节点或公网代理能力。
- 安装完成后 Core 默认启动并等待 API readiness；失败时保留基础设施和已验证配置，返回可修复的 `Incomplete`/`Failed`，不得声称代理已可用。
- 已有运行实例的升级安装必须先把新 daemon/Core 和相关资源写入独立、完整且已校验的 pending generation；`install` 不得覆盖正在运行的可执行文件，也不得因版本更新自动停止 daemon/Core。随后由显式 `mihomo-cli restart` 应用 pending generation。
- `config add/import` 首次成功输入默认替换 direct-only 基础配置并生成新的 `UserEffectiveConfig`；下载、解析、校验或应用失败时保留 last-known-good 基础配置/旧配置。
- `restart` 仍是显式配置应用入口；安装后的 Core 已运行时，后续配置命令只有取得受管 reload/restart 和 runtime attestation 才能报告已应用。
- TUN 和真实公网数据面是基础旅程之后的独立阶段，不能用基础服务 contract 替代真实 TUN/data-plane 证据。

可选 TUN 旅程必须显式开启全机网络能力：

```text
基础服务可用 + 已验证用户配置
→ tun on（内部 sudo/root peer gate）
→ Core API 观察 runtime TUN enabled
→ 可选真实/隔离数据面验证
```

安装默认生成 direct-only 基础配置，但**不默认启用 TUN**。`--skip-config` 只跳过可选订阅输入，不得跳过本地基础配置生成、校验或 Core 启动。

可选 TUN 的关闭合同同样必须由运行态证明：`tun off` 经过 root peer gate 与受管事务后，必须由当前 Core API 观察 `runtime_tun == disabled`；随后 `restart --system` 再次观察仍为 disabled。仅写入 `tun.enable: false`、删除 snapshot 或返回 daemon success 均不能单独构成关闭成功。

可选 TUN 的启用合同也必须保持跨重启：`tun on` 经过 root peer gate 与受管事务后，必须由当前 Core API 观察 `runtime_tun == enabled`；随后 `restart --system` 必须重新观察 Core 运行且 `runtime_tun == enabled`。仅写入 `tun.enable: true`、保留 snapshot 或返回 daemon success 均不能单独构成启用成功。

### 0.2.1 Existing journey coverage

`docs/user-journeys.md` 是场景与证据索引；下表把 J001–J009 绑定到本 SPEC 的统一合同。专项文档只能补充命令参数、fixture 和测试步骤，不能改变下列不变量：

| Journey | 主合同绑定 | 必须保持的用户可观察边界 |
|---|---|---|
| J001 离线订阅 | §3.4、§3.5、`config fetch/import/validate` | fetch 不写 active 用户配置；import 原子提交并校验；Core 停止时仍可导入；私有 URL/节点/凭据不进入普通输出或测试日志 |
| J002 受限网络安装 | §2.1–§2.5、§12.2 | Core/Geo 下载共享受限重试与 mirror fallback；失败不覆盖有效 artifact；无订阅 install 生成并校验 direct-only 基础配置，启动普通 Core，但不启动 TUN 或访问订阅 URL |
| J003 clean reinstall | §0.2、§2.1、§12.2 | `uninstall --all --yes → install --system --yes` 后直接拥有可运行 direct-only Core/API；随后可选 `config import/add → restart --system` 替换配置；Ready 只表示基础设施与 Core/API readiness，不表示公网或 TUN data plane |
| J003b system/TUN | §3.7、§1.4、§12.2–§12.2.1 | 先 import/restart，再显式 `tun on/off`；intent→snapshot promotion 必须 root peer/revision 受管；runtime TUN 只能由当前 Core API 观察 |
| J004 语义代理组路由 | §3.8、§12.2 | 规则绑定稳定策略/代理组；运行时选择不擅自改规则；真实外部服务结果只能由显式 probe/真实 data-plane 证据证明 |
| J005 内网 DIRECT | §3.6、§3.7、§12.2 | fake-ip-filter、DNS policy 和 DIRECT rule 分层提交；规则匹配不等于 DNS 或网络成功；TUN 下仍不得绕过 snapshot/promotion |
| J006 固定节点 | §3.8、§5、§12.2 | `select` 只改变代理组选择并写入当前实例的 selection intent；Core restart/reload 后由受管路径在 API ready 后有界重放；不改规则；出口稳定性必须由显式 `exit-ip` 或真实 probe 观察，不能由控制面 Ready 推断 |
| J007 shell proxy | §3.7.1、§12.2 | `proxy on/off` 只输出/清理当前 shell 的环境变量，不启动或停止 Core，不能声称改变父 shell 或全机流量 |
| J008 恢复直连 | §0.5、§3.9、§12.2 | TUN、system proxy、shell proxy、Core/service 分层关闭；`stop` 不承诺清理父 shell；未知状态不得汇总为已恢复 |
| J009 订阅漂移 | §3.4–§3.5、§12.2 | refresh/switch 发现策略组、节点或规则目标漂移时只告警并保留旧状态；不猜测、不自动改规则/节点；漂移检测本身无法可靠完成时返回 `unknown`，失败保留 last-known-good |

旅程证据必须按 §0.4 标注；CLI smoke、fake Core contract 或单次 probe 不得升级为 `Full-journey-tested`。

### 0.3 Command surface

| 层级 | 命令/动作 | 合同 |
|---|---|---|
| 普通用户主面 | `install`, `uninstall`, `restart`, `config`, `rule`, `dns`, `exit-ip` | 目标导向、错误可修复；不要求用户理解 instance/daemon/Core |
| 高级/脚本面 | `--system`, `--user`, `--yes`, `--dry-run`, `--json`、兼容旧 `ip` 以及现有非-config 兼容子命令 | 保留既有兼容行为，不作为普通用户教程主路径；`config` 的 action flat flags 不属于当前兼容表面，旧写法只保留在迁移映射中 |
| 内部实现 | daemon IPC、Core API、systemd/launchd/SCM、sudo re-exec、socket/path | 不作为产品入口，不在错误中要求用户手工执行底层操作 |

`start` 与 `restart` 的语义必须统一：`restart` 是主面命令；`start` 仅为兼容/高级别名，不能在文档中定义另一套状态机。`status`/`doctor`/`tun status` 若保留，只是只读高级诊断视图；它们不能成为修复入口，不能触网、sudo、写文件或隐式 recovery。出口/公网探测必须是显式 probe（现有 `exit-ip` 兼容命令）而非默认 status 行为。

### 0.4 Authority and evidence levels

设计结论必须同时标注行为状态与证据层级：

- `Implemented`：代码路径存在并有单元/集成证据；
- `Contract-tested`：隔离 fake Core/systemd/IPC contract 通过；
- `Real-Core-tested`：同架构真实 Mihomo Core 通过；
- `Full-journey-tested`：真实用户命令顺序和所声明的数据面均通过；
- `Planned`：仅设计或测试计划，不能宣称已支持。

`status`/控制面通过不能推出公网代理通过；真实公网 probe 失败必须保留网络、节点、目标站点和重试证据，不能直接归因于 CLI 缺陷，也不能因一次重试成功宣称永久可靠。

### 0.5 Shared result states

| 结果 | 含义 |
|---|---|
| `Ready` | 本命令声明的控制面目标已达到并被同一次观察证明 |
| `Incomplete` | 基础设施/旧运行态仍安全保留，但目标未完成；给出一个阻断原因和一个下一步 |
| `Failed` | 本次变更失败；已回滚则明确说明回滚，无法证明回滚则进入 recovery blocker |
| `Unknown` | 关键状态无法可靠观察；不能降级成 healthy/off |
| `RecoveryRequired` | journal/manifest/残留身份无法安全证明；变更命令阻断，信息命令只读显示恢复要求 |

- `NotObserved`：没有执行显式 probe，或 probe 所需的目标/路线/运行态证据不可用；不表示失败或成功。
- `Reachable`：在记录目标、路线、节点/代理组和时间后，显式 probe 或受控真实业务请求成功，并取得完整响应证据。
- `Degraded`：取得部分成功证据，但存在明确的超时、重试、节点切换、错误率或目标子路径失败；不得简化为完全可用。
- `Unavailable`：在固定目标和有限重试预算内取得明确失败证据；只表示本次数据面观察不可用，不自动证明 Core、配置或 CLI 损坏。

公网不可达不自动等于 Core/服务失败：控制面可 `Ready`，数据面另行标记 `NotObserved`、`Degraded` 或 `Unavailable`。应用不得因一次数据面连接失败自动删除节点、修改代理组或重启 Core。

### 0.6 Shared status snapshot contract

`status`、`status --json`、`tun status` 和 `doctor` 必须从同一次 `StatusSnapshot` 派生；formatter 不得直接访问 daemon、Core API、配置文件、service manager 或网络 client。默认查询零 sudo、零外网、零写入、零隐式 recovery。

- `configured_tun` 只表示用户 intent；安全读取失败、文件缺失或字段缺失时必须为 `unknown`，不得因为 daemon 可达、service 已安装或 Core 已停止而回填 `disabled`。`runtime_tun`、`rule_mode` 和 live ports 只能来自当前 Core API `/configs`。Core 停止、API 不可达、字段缺失、revision attestation 缺失或解析失败时，`runtime_tun` 必须为 `unknown`。
- `StatusSnapshot` 必须同时携带本次观察到的 `launched_snapshot_revision`、`active_snapshot_revision`、`active_intent_revision`、journal 状态和 attestation 结果；这些字段缺失或无法证明归属于当前 instance 时，相关 revision 状态为 `unknown`，不能由 daemon 缓存、文件名、path equality 或历史成功结果补齐。
- `runtime_tun == enabled` 只有在 launched snapshot revision、active snapshot revision 和 active intent revision 均可观察、彼此匹配、journal 已达到 `IntentCommitted` 且通过当前 Core/API attestation 时，才能汇总为 `TunRunning`。enabled 但任一 revision 缺失、不一致、过期或 journal 未提交时为 `TunRunningUnattested`：原始 `runtime_tun` 可保留为 enabled 供诊断，但所有用户可见 TUN 状态和 Health 判定必须显示 `unknown`/`recovery required`，不得显示已收敛或 `healthy`。
- Core/API 不可达、`/configs` 缺少 TUN 字段，或 runtime 与 snapshot/intent 无法建立可信关联时为 `TunStateUnknown`；用户可见 TUN 状态为 `unknown`，破坏性 TUN/config 变更阻断，直到显式 `restart` 触发内部受管恢复重新建立证明。
- `tun status` 只显示运行态和 attestation/recovery 结论；`doctor` 可并列显示 configured intent、raw runtime observation 和 attestation verdict，但只有两个可观察且已 attested 的值不同，才报告配置漂移。`status` 默认摘要不得暴露 revision、journal、路径或 PID，只能显示 `TUN: enabled` 对应已 attested 的 `TunRunning`，其余显示 `unknown` 或 `recovery required`。
- `Configuration: applied` 只有在 active intent revision 与当前运行 snapshot/launched revision 均可观察并通过 attestation 时允许；pending、failed、recovery 和任何 revision unknown 映射为对应状态或 `unknown`，不得仅凭 config 文件写入成功报告 applied。
- 默认摘要不包含 `Exit`、公网 probe、订阅 URL/token、路径、PID、socket、日志或节点凭据；出口 IP 和公网数据面只能由显式 `ip`/`exit-ip` 或用户明确要求的 probe 获取。
- `status --verbose` 只能在同一快照后追加诊断字段；live ports 必须来自该次当前 Core API 观察，不得回退到持久配置或历史缓存。诊断字段失败只显示 `unknown`，不改变默认摘要语义。
- `doctor` 可以追加明确标记的本地只读诊断，但不得重读或覆盖快照中的 runtime 状态，不得触发 sudo、外网、写入或 recovery。


### 1.1 Instance Modes

当前采用两种**互斥**实例模式，运行时自动检测：

| 模式 | 运行身份 | TUN | 用户工作区配置 | 系统运行态配置 | API Socket |
|------|---------|:---:|--------------|--------------|------------|
| **System Service** | 非 root `mihomo` daemon/Core（Linux）；平台特权服务模型按平台例外 | ✅ | per-user `~/.config/mihomo/` (用户专属) | `/var/lib/mihomo-cli/active-config.yaml` (系统隔离) | `/var/run/mihomo/mihomo.sock` |
| **Per-user** | 当前用户 | ❌ | `~/.config/mihomo/` | `~/.config/mihomo/config.yaml` | `$XDG_RUNTIME_DIR/mihomo/mihomo.sock` |

**互斥原因**：TUN 在网络层影响全机流量，不能与同机另一受管实例并行管理。Linux system daemon 不是 root helper；它由 systemd 以专用 `mihomo` 用户运行，Core 以同一用户运行并只获得受限 capabilities。macOS/Windows 的服务身份是平台例外，仍必须通过 IPC、peer/token 和配置校验收缩边界。

**关键设计（IPC 配置下发与运行态物理隔离 - ADR-25）**：
- **用户空间（`~/.config/mihomo/`）**：由普通用户 100% 拥有（`0755` 或 `0700`，`user:user`）。仅用于存储订阅元数据、自定义规则 `rules.yaml`、DNS 策略及用户意图配置。后台 Daemon 和 Core **绝不直接读取或写入该目录**。
- **系统空间（`/var/lib/mihomo-cli/`）**：根目录由 `root:mihomo` 管理并保持 `0770`，允许非 root `mihomo` daemon 写入其固定运行时文件；事务、generation state 等子资产继续按各自 writer contract 使用受保护的属主和模式。存放 Core 当前活跃配置 `active-config.yaml`、TUN 快照 `tun-config.yaml`、事务日志 `transactions/` 以及 Geo 数据库。
- **IPC 下发机制**：当执行 `restart`、`tun on` 或配置热更新时，CLI 在本地完成配置合并与校验后，将最终 YAML 内容通过本地 IPC Socket（Unix Domain Socket / Named Pipe）直接推送到 Daemon。Daemon 写入自身系统状态目录后控制 Core 启动或重载。
- **消除权限副作用**：彻底消除跨域文件系统权限摩擦，不��需要 Linux `setgid`、不再需要 `$HOME` 路径穿越（`o+x`）修补、不再需要跨用户 `transactions/` 权限自愈，并且从根本上消除了针对用户主目录的符号链接提权攻击面（Security #4）。

### 1.2 Mode Resolution (Clash-Verge-like UX)

CLI 默认无需 `--system`/`--user` 标志，运行时自动检测：

```
1. System daemon IPC 可连接？ → System 模式
2. Per-user core API socket 可连接？ → User 模式
3. 两者都不可用 → 回退到已安装的服务文件
4. 无任何状态 → 默认 User 模式
5. 两者都可连接 → 报错（互斥冲突）
```

`--system` 标志仅用于脚本/排障场景显式覆盖。

### 1.3 Architecture Overview

```
┌──────────────────────────────────────────────────────────┐
│  mihomo-cli (Rust, single binary)                        │
│                                                          │
│  ┌─────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  │
│  │ main.rs │  │ daemon   │  │ config   │  │ installer│  │
│  │  CLI    │  │  IPC     │  │  订阅/合并│  │  内核下载 │  │
│  └────┬────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘  │
│       │            │              │              │        │
│  ─────┼────────────┼──────────────┼──────────────┼─────   │
│       │    IPC     │     API      │              │        │
│       ▼            ▼              ▼              │        │
│  ┌─────────┐  ┌──────────────────────────┐       │        │
│  │ Daemon  │  │  Mihomo Core (代理引擎)   │       │        │
│  │ (mihomo)│  │  /var/run/mihomo/         │       │        │
│  │  user   │  │  mihomo.sock              │       │        │
│  └─────────┘  └──────────────────────────┘       │        │
└──────────────────────────────────────────────────────────┘

IPC (System 模式):
  CLI ←→ Daemon (/var/run/mihomo/service.sock) ←→ Mihomo Core

API (所有模式):
  CLI ←→ Mihomo Core (unix socket / named pipe)
```

### 1.4 IPC Protocol (System Daemon)

System 模式下，CLI 不直接管理 Mihomo Core，而是通过 daemon IPC 下发配置并控制生命周期。Daemon 接收已验证的配置内容，写入系统受管状态目录：

```rust
// CLI → Daemon
enum DaemonCommand {
    // 启动/重启 System Core，携带经过 CLI 校验的完整有效配置 Payload 及版本号
    StartSystemCore {
        config_content: String,
        config_revision: String,
    },
    RestartSystemCore {
        config_content: String,
        config_revision: String,
    },
    // 应用 System TUN 快照并切换 Core
    ApplySystemTunSnapshot {
        expected_revision: String,
        snapshot_content: String,
        stack: TunStack,
        dns_hijack: bool,
    },
    StopCore,
    DisableTun,
    GetStatus,
    CoreApiRequest {
        instance: InstanceMode,
        method: String,
        path: String,
        query: Option<String>,
        body: Option<Vec<u8>>,
        body_size: usize,
    },
}

// Daemon → CLI
enum DaemonResponse {
    Success { message: String },
    Error   { message: String },
    Status  {
        snapshot: StatusSnapshot,
    },
    CoreApi { data: Vec<u8> },
}
```

**安全���状态边界（ADR-25 物理隔离与 IPC 下发）**：
- **零跨域路径访问**：Daemon 固定将活跃配置写入系统专属受控路径 `/var/lib/mihomo-cli/active-config.yaml`（Linux）/ `/Library/Application Support/mihomo-cli/active-config.yaml`（macOS）/ `%ProgramData%\mihomo-cli\active-config.yaml`（Windows），TUN 快照写入 `tun-config.yaml`；不再要求或接受客户端传入的本地文件系统路径进行服务端跨域读取。
- **配置版本一致性证明**：CLI 与 Daemon 通过 `config_revision` 确保传输 Payload 与生效配置一致，防止并发覆盖。
- **Core Binary 路径固定**：Daemon 启动 Core 时固定使用受管系统二进制（如 `/usr/local/lib/mihomo/mihomo`）。
- **运行态事务收敛**：`transactions/` 目录和 `tun-journal.json` 完全置于 `/var/lib/mihomo-cli/transactions/`，属主为 `mihomo:mihomo`（`0o750`），彻底杜绝跨用户权限冲突与 `Permission denied`。
- **Core API 代理与安全闸门**：System 模式的日常 Core API 请求由 daemon 在认证后按 method/path allowlist 代理；只允许当前命令所需的只读查询和非 TUN 控制操作（如 `select` 代理组）。严禁通过通用 API 绕过 TUN 专用事务。
- **状态观测独立性**：`runtime_tun` 严格通过受权 Core API `/configs` 实时观察，Core 停止或 API 不可达时显示 `unknown`，不得由 Daemon 从静态磁盘文件反推。

**安全边界**：
- daemon 不信任客户端传入的 binary 路径，固定使用 `/usr/local/lib/mihomo/mihomo`
- user/non-system client-path lifecycle 若为兼容实现，可接受当前原始用户 home 下的绝对路径；必须不含 `..`，且 owner uid 匹配调用方。该边界不适用于 system lifecycle
- system lifecycle 的配置输入必须由 CLI 在本地读取、合并、校验并以 `config_content + config_revision` 通过 IPC 传递；daemon 不接受调用方配置路径，不直接读取用户 home。daemon 只将 payload 写入固定 system context，并从固定 managed path 启动 Core。
- system TUN 使用固定 system context 下的受保护 `tun-config.yaml` snapshot；TUN 专用请求只接受事务 fence/revision 等受管输入，不接受任意来源路径或未经事务约束的配置 bytes。
- daemon 启动 core 前校验 config 中的 API endpoint 等于当前系统端点
- System 模式的日常 Core API 请求由 daemon 在认证后按 method/path allowlist 代理；只允许当前命令所需的只读查询和非 TUN 控制操作，默认拒绝未知 method、path、query、body 字段以及所有跨 instance 的 endpoint
- `CoreApiRequest` 的最小可验证合同为：`GET` 只读访问 `/configs`、`/proxies`、`/connections` 及正式 SPEC 明确列入的只读资源；`PUT`/`PATCH` 只允许正式 SPEC 明确列入的非 TUN runtime mutation，path、query、JSON 字段、body 大小和目标 instance 必须逐项 allowlist 校验；`POST`、`DELETE` 和任意未列入的 method/path 默认拒绝。保留协议 enum 或旧 handler 不构成允许该行为的证据，未迁移的旧分支必须 fail-closed。
- 当前唯一允许由通用 Core API forwarding 承载的非 TUN mutation 是语义代理组 `select` 的运行时选择：仅允许当前 instance 的规范化代理组 endpoint、空 query 和固定 schema 的目标成员 body；组名、成员名、method、path、query、body 字段和 body size 均须逐项校验，禁止 wildcard path、任意配置字段或跨 instance 请求。该 forwarding 只改变当前 Core 的运行时选择；`select` 的持久 intent/active effective-config 提交仍由 CLI 的配置事务负责。system TUN 为 `TunRunning` 时，`select` 必须进入统一 promotion dispatcher，不能绕过 dispatcher 直接 PUT Core API；Core stopped 或 API 不可达时按 §12.2.1 返回 `pending`/`unknown`，不得隐式启动 Core。未来新增非 TUN mutation 必须先在本节登记其规范 method/path/query/body/size/instance 合同和回滚语义，未登记的一律拒绝。
- `delay`、`exit-ip`、`rule test` 等只读或 probe 动作只能使用已登记的 GET/显式 probe 合同；不得因为已有 client helper 或 Core API endpoint 存在，就自动获得 forwarding 权限。
- 对 `CoreApiRequest` 的 `/configs` PUT/PATCH 和 `/proxies/*` PUT，daemon 必须先观察当前 Core `/configs` 的 runtime TUN：观察到 `enabled` 或无法观察时 fail-closed，返回 promotion/restart 指引；只有明确观察到 `disabled` 才允许非 TUN forwarding。该 gate 已实现，但它只是阻断 TUN-active 旁路，不等同于完整的 active-config promotion dispatcher。
- `CoreApiRequest` 是当前 Core 的受权访问，不是 lifecycle 请求：daemon 在处理它时不得调用 `ensure_core_running`、下载/修复资源、启动或重启 Core；Core/API 未 ready 时直接返回 `Incomplete`/`Unknown` 及唯一的 `mihomo-cli restart [--system]` 下一步。需要改变 Core 生命周期的动作只能走显式 `restart`/专用 promotion dispatcher。
- 通用 Core API 代理必须拒绝任何涉及 `tun`、`dns-hijack`、系统路由或等价 TUN 控制字段的 path、query 或 body；不得通过伪造 `PATCH /configs`、传入 `tun.enable`、替换 config path 或提交完整配置 bytes 绕过 TUN 专用事务。对允许的非 TUN mutation，body 必须按命令定义的字段集合逐项拒绝未知字段，不能只检查一个顶层键或仅依赖 path。
- TUN 只能走额外要求 root peer UID、固定 system context、candidate/revision 和 snapshot attestation 的专用命令
- Core socket 不直接开放给普通用户

**传输**：Unix domain socket (`/var/run/mihomo/service.sock`)，length-prefixed JSON。

### 1.5 IPC 认证（跨平台）

**当前证据状态**：本节定义跨平台 IPC 认证合同；具体平台实现和旅程证据必须按 §0.4 单独标注，不能由 token 文件或 SDDL 设计本身推导为 `Implemented`。
- Windows：要求 token 双副本（`service-token` + `service-client-token`）与 named pipe ACL/SDDL，并分别验证 SCM、IPC 授权和 Core/TUN 旅程。
- Unix：要求 peer UID、per-user client token 和授权表三者共同校验；Linux 非 root daemon 与 macOS 特权服务按平台边界分别验证。
- Unix 不使用独立 server token；认证材料是 peer UID、per-user client token 和授权表三者。
- **Per-user client token**：`~/.config/mihomo/service-token`，权限 `0o600 <user>:<user>`，CLI 连接时携带。
- **授权表**：`/var/lib/mihomo-cli/authorized-clients.json`，记录被授权的 (uid, client token) 对。Linux 为 `0o640 root:mihomo`，供非 root daemon 只读；macOS 为 `0o600 root:wheel`。
- **Socket 权限**：保持 `0o666`，允许所有本机用户连接；授权边界在 daemon 应用层完成。
- **Daemon 校验**：
  - 若 peer UID == 0（root），直接放行 token 校验（保留 root 排障/管理路径）。
  - 非 root 用户：client token 必须存在于授权表，且 Unix socket peer UID 与授权表中该 token 归属 UID 一致。
- **TUN 操作额外限制**：`ApplySystemTunSnapshot` / `DisableTun` 仍要求 peer UID == 0（root）。`ApplySystemTunSnapshot` 是 `tun on` 的内部 apply/update 请求；`DisableTun` 是 `tun off` 的内部请求。
- **管理命令**：`mihomo-cli access grant --user <name>` / `access revoke --user <name>` / `access list` / `access status` 由 root 维护授权表。

**计划改进（后续）**：
- 长期考虑将 token 文件保护扩展到 macOS Keychain / Linux keyring（超出当前阶段）

### 1.6 Lifecycle Concurrency & Readiness

当前 System 模式 lifecycle 遵循本 SPEC §1.4–§1.6 的 IPC、锁和 readiness 合同；平台实施细节见 `docs/architecture.md` 和对应平台专项文档。（历史 BUG-13 修复背景仅作追溯。）

**并发控制：CLI 永不持有生命周期锁。**

| 模式 | 串行化机制 | 说明 |
|------|-----------|------|
| System | daemon `OWNER_LIFECYCLE_LOCK`（进程内 tokio Mutex） | CLI 只发 IPC 命令，不持文件锁 |
| User | systemd job 队列 | systemd 保证服务状态变更串行 |

**锁顺序与跨进程边界（目标合同，尚未由统一路径完整实现）**：用户配置锁与 daemon 生命周期锁不得嵌套持有。未来所有需要 runtime apply 的 active-config mutation 都应由唯一 `apply_active_intent` dispatcher 管理，并遵守以下 durable 状态机；当前实现仅在已覆盖的 TUN 事务路径满足相应阶段，`config`/rule/DNS/override/TUI 变更尚未统一接入。锁的释放与重新取得是目标协议的一部分，后续实现不得自行简化：

1. **Prepare（用户配置锁内）**：命令取得 canonical user-config writer lock，读取当前 `active_intent_revision`，生成 immutable candidate，并以 `base_revision`、`candidate_revision`、owner/transaction ID、bytes hash 和 manifest 持久化 `Prepared` journal。该 journal 同时登记 dispatcher owner/generation，作为 active-config mutation gate；candidate/pending 不得覆盖 `config.yaml`、active pointer、metadata 或当前运行 snapshot。写入并 fsync journal 后，命令释放用户配置锁。
2. **Admission（重入前）**：任何后续 config/rule/DNS/override/select/backup mutation 在取得用户配置锁后，必须先读取 canonical journal。若存在未完成的 dispatcher owner，且不属于当前 transaction，必须返回可重试的 `Incomplete`/冲突结果，不得创建第二个 candidate、修改 active intent 或覆盖 journal。不得以“第二次取得配置锁”绕过该 gate。
3. **Runtime apply（daemon lifecycle 锁内）**：dispatcher 通过唯一 IPC 将 transaction ID 和 expected revision 交给 daemon；daemon 在认证与 payload 校验完成后取得 `OWNER_LIFECYCLE_LOCK`，重新验证 journal、candidate hash、instance 和固定 managed paths，然后完成 snapshot promotion、Core reload/restart、readiness 与 runtime attestation。daemon 不得在持有 lifecycle lock 时取得、等待或调用用户配置锁。
4. **CoreApplied（仍在 lifecycle 锁内持久化）**：只有当前 Core API runtime、launched snapshot revision、target instance 和 candidate revision 全部匹配时，dispatcher 才能将 journal 推进为 `CoreApplied` 并 fsync；这只证明运行 Core 已使用 candidate，不证明 user intent 已提交。若 apply/attestation 失败，daemon 按 journal 恢复 old runtime 或保留 `RecoveryRequired`，然后释放 lifecycle lock。
5. **Compare-and-commit（释放 lifecycle 锁后重新取得用户配置锁）**：daemon 完成 runtime apply 后必须先释放 `OWNER_LIFECYCLE_LOCK`，再由 dispatcher coordinator（CLI 或等价事务协调者）重新取得 canonical user-config writer lock。锁内必须原子比较：journal owner/generation 仍属于当前 transaction、journal 状态仍为 `CoreApplied`、当前 `active_intent_revision` 仍等于 `base_revision`，且 active pointer/metadata/config manifest 未被外部改变。只有比较全部通过，才能按原 manifest 提交 `config.yaml`、active pointer、metadata 及其他 active-effective-config 输入，重读并校验 revision/hash 后推进 `IntentCommitted` 并 fsync；随后清理 candidate/journal。最终提交不得在 lifecycle lock 持有期间进行。
6. **Conflict/recovery**：若 compare-and-commit 发现 revision、owner、manifest 或 journal 已变化，绝不能覆盖新 intent，也不能把旧 candidate 宣称为已应用。必须保留可证明的 journal、snapshot 和运行态，返回冲突或 `RecoveryRequired`；若新 runtime 已运行但对应 intent 不能证明，必须阻断后续破坏性 mutation，直到通过显式 restart --system 或后续受管变更触发内部受管恢复重新建立一致性。任一崩溃点都依赖 durable journal/revision 恢复，不依赖跨进程锁嵌套保证原子性。

因此，daemon lifecycle lock 的唯一顺序是“取得 → runtime apply/attestation → 推进 `CoreApplied` → 释放”；用户配置锁的最终顺序是“重新取得 → compare-and-commit → 推进 `IntentCommitted` → 释放”。任何路径都不得出现“持有用户配置锁等待 lifecycle lock”或“持有 lifecycle lock 等待用户配置锁”。

对于非 TUN 的 `select`，用户配置锁只保护持久选择 candidate/intent，并同样受 canonical journal/generation gate 约束；释放后才可请求受权 Core API runtime selection。该 runtime mutation 必须在 daemon 侧与 restart/stop 等 lifecycle 操作串行化；GET 只读查询不占 lifecycle lock。TUN active 的 `select` 不走独立 runtime PUT，而由 active-config promotion dispatcher 在同一事务中完成。

**daemon `OWNER_LIFECYCLE_LOCK`**：
- `StartSystemCore`、`StopCore`、`RestartSystemCore`、`ApplySystemTunSnapshot`、`DisableTun` 以及非 TUN `select` runtime mutation 在 daemon 侧按固定顺序串行化；生命周期操作包含 Core spawn 与 readiness 等待，并发客户端不得交错。
- `GetStatus` 和已登记的只读 `CoreApiRequest` 不占 lifecycle lock，但仍须完成认证、instance 和完整 allowlist 校验。
- 所有认证与 payload 校验在锁外完成；无效请求不得占用 lifecycle lock。

**readiness 归属：完全在 daemon，CLI 不重复检查。**

```
CLI restart --system
  └─ IPC RestartSystemCore → daemon
       └─ spawn/restart core → 轮询 core API 就绪（≤15s，500ms 间隔）
       └─ 就绪 → Success{ "core started and API ready" }
       └─ 超时 → kill core + Error
  └─ CLI 收到 Success 即完成（不轮询）
```

**超时边界**：daemon 生命周期操作 ≤15s < CLI 请求超时 20s。
客户端始终收到 daemon 的业务结论而非传输错误。

**daemon 崩溃恢复**：CLI 检测 daemon IPC 不可达时只能进入只读 residual preflight；`core.pid` 存在本身不是 Core 归属或可安全恢复的证明。若能证明残留属于当前受管 instance，才提示用户执行 `stop --system` 清理残留，或通过 `restart --system`/`tun on/off` 触发内部受管恢复收敛；无法证明身份、revision 或锁归属时返回 `RecoveryRequired`，不得自动 kill、删除或重启。

### 1.7 Permission Matrix

| 操作 | Per-user | System Service |
|------|----------|---------------|
| 安装服务/系统基础设施 | 无需 root | CLI 内部请求最小 OS 授权 |
| 启动/停止普通 Core | 当前用户执行 | 日常通过已授权 daemon IPC；必要的修复由 CLI 内部授权 |
| TUN on/off | 不可用 | 普通用户入口先无副作用 preflight，再由 CLI 内部 sudo re-exec；daemon 只接受 root peer mutation |
| 编辑用户配置 | 当前用户 | 当前原始用户；system snapshot 由受控 root transaction 生成 |
| 查询 status | 当前用户 | 当前授权用户；只读、零 sudo、零外网 |
| Core API 命令 | 当前用户 | 通过 daemon allowlist 和 token/peer 校验，不直接暴露 Core socket |

权限错误的产品输出必须说明发生了什么、当前命令内能否自动修复、下一步应执行哪个 mihomo-cli 命令；不得要求普通用户手工执行 `sudo chown`、`systemctl` 或直接操作 socket。

### 1.7.1 Fixed Runtime Boundary

当前 system 模式不要求 daemon 穿越或读取用户 home。CLI 在用户权限上下文读取并校验唯一 intent 配置，通过 IPC 发送 `config_content + config_revision`；daemon 只写入固定 system runtime，并使用固定 `-d` 与显式 `-f` 启动 Core。

- 用户配置目录不向 daemon 暴露读取权限，daemon 不枚举、解析或跟随用户 home 中的路径。
- system runtime 根目录 Linux 为 `root:mihomo 0770`，以支持 daemon 写入固定 runtime 文件；transactions、generation state、snapshot 与 Geo 文件按各自 writer/reader contract 收敛权限。
- daemon 收到 payload 后必须验证大小、YAML、managed API endpoint 和 revision，再原子写入固定 runtime；任何路径字段都不得影响 system runtime data directory 或配置选择。
- 无法读取或校验用户 intent 时，CLI 必须在 IPC 前失败并返回可执行的 `mihomo-cli` 修复/重试指引；不得让 daemon 回退读取用户 home。

成功条件是：CLI 能证明 payload 来源和 revision，daemon 能证明固定 runtime 写入，Core 能以固定 `-d` 和显式 `-f` 启动并完成 API readiness；这不要求也不允许 system daemon 直接访问用户配置树。

---

## 2. Install Flow

### 2.1 Install and direct-only bootstrap contract

`install --system --yes` 与 `install --user --yes` 都必须在没有订阅源时建立可运行的本地基础配置。这里的基础配置不是订阅占位物，而是正式的 `UserEffectiveConfig` 初始版本：

```yaml
mode: rule
mixed-port: 7897
proxies: []
proxy-groups: []
rules:
  - MATCH,DIRECT
```

安装实现必须按当前 instance endpoint 注入 controller 字段，并使用当前 Core `-t` 校验后原子写入用户 `config.yaml`。若已有合法用户配置，安装不得覆盖其业务内容，只修正 CLI 管理的 controller 字段。

安装阶段的执行顺序为：

```text
residual/config preflight → core binary → service/daemon → geo → ensure direct-only config → service readiness → Core start/API readiness
```

- 没有订阅时不读取 stdin、不请求订阅 URL；`--skip-config` 只抑制订阅交互，不抑制基础配置生成、校验或 Core 启动。
- direct-only 配置的普通 mixed port 和 controller endpoint 可供本地应用连接；空代理列表意味着不存在代理出口，所有匹配最终走 `DIRECT`。
- 安装成功不等于公网代理可用，也不等于 TUN 已启用；TUN 始终由显式 `tun on` 控制，安装不得写入 `tun.enable: true`。
- system 安装必须先等待 daemon readiness，再通过受管 lifecycle 入口启动 Core；user 安装必须通过 user service/受管进程启动 Core。Core/API readiness 失败时安装非零返回并保留已验证的基础配置。
- 后续 `config add`/`config import` 必须以已验证输入构造新 candidate，原子替换 direct-only 或旧 `UserEffectiveConfig`；下载、解析、校验、Core reload 或 promotion 失败时保留 last-known-good。
- `restart` 仍可用于显式重新应用配置；任何配置写入成功但运行态未观察到的结果必须报告 `pending` 或 `unknown`，不得伪造 `runtime_applied`。

### 2.2 Infrastructure convergence and recovery

#### 2.2.1 Install/apply 分离与代际更新

运行中的 system daemon 使用固定安装路径（Linux `/usr/local/bin/mihomo-cli`、macOS `/Library/Application Support/mihomo/bin/mihomo-cli`、Windows `%ProgramData%\\mihomo\\bin\\mihomo-cli.exe`）；服务配置不因每次更新而暴露版本路径。为避免 Windows executable lock、Unix 运行时截断以及多文件半更新，更新采用两阶段模型：

```text
install:
  下载/校验所有远程 artifact
  → 写入独立的 versions/<generation> 目录
  → 写入并 fsync manifest
  → 原子标记 pending
  → 不停止 daemon/Core

restart:
  读取并重新校验 pending generation
  → 按变更范围执行最小必要停止/切换
  → 启动并验证新 daemon/Core
  → 成功后提交 active generation，保留 previous generation
```

- generation 必须包含成套 daemon/Core artifact、哈希、协议版本和创建信息；单个 `.pending` 文件不能代表完整更新。
- `install` 的网络阶段必须先于任何可能导致代理中断的操作；下载失败不得停止现有 daemon/Core，也不得覆盖 last-known-good。
- 仅 Core 变化时，`restart` 通过 daemon 停止/替换/启动 Core，daemon 保持运行；daemon 变化时才停止并重新启动 daemon service。
- 正在运行的 daemon/Core 不得被直接 truncate/write。Windows 必须等待服务/进程完全退出后再替换；Unix/macOS 使用停止后的安全替换或同文件系统原子 rename。
- 切换失败时必须保留 active/previous generation，优先回滚到上一代；不得以“文件已复制”作为 Ready 证据。
- install 发现需要应用更新时可以成功返回“update prepared”，但必须明确提示 `mihomo-cli restart`；非交互 install 不得自行执行中断性切换。


安装必须区分：

| 结果 | 处理 |
|---|---|
| 下载阶段失败 | 保留已有有效目标；受管临时/续传文件可保留；返回非零并说明下一步 |
| binary/geo 校验失败 | 删除对应损坏临时产物，不替换已有有效目标；返回 `Failed` |
| service/access 写入失败 | 不激活不完整 service；保留诊断/journal，禁止报告 Ready |
| 后续阶段失败但前序阶段有效 | 保留可验证前序产物，记录未完成阶段；重复 install 从 preflight 继续 |
| 无法证明 artifact/service/token/table 的归属或完整性 | fail-closed，进入 `RecoveryRequired`，不得盲删或覆盖 |

若 install 需要跨多个文件/权限主体提交（service、token、authorized-client table、Geo、snapshot），必须使用受管 staged 文件、固定 canonical journal、fsync 边界和补偿恢复；journal 存在时下次 install/uninstall 先恢复或阻断，不得先删 journal 伪造 clean。安装不因网络波动自动回滚已经验证且可独立使用的基础设施，但也不得把“部分安装”报告成完整 Ready。

### 2.3 Install component preflight

| 组件 | 默认 `install` | `install --force` |
|------|:---:|:---:|
| [1/4] binary | 有效 → 跳过；无效/不存在 → 下载 | 无脑重下 |
| [2/4] service files | 存在 → 跳过 | 无脑重写 |
| [3/4] user config | 缺失时生成并校验 direct-only 基础 `UserEffectiveConfig`；已有有效用户配置时保留并校验，不因 install 覆盖业务内容 | `--force` 也不得覆盖有效用户 intent；配置替换必须通过 `config --import`、`--add`、`--switch` 等现行配置入口并遵守 last-known-good 合同 |
| [4/4] geo | 完整 → 跳过；缺失/损坏 → 下载 | 无脑重拉 |

**geo 完整性判定**：先检查文件存在 + 尺寸（geoip > 8MB, GeoSite > 2MB），通过后再跑 `mihomo -t` 验证。

**证据状态**：install 的 binary/service/geo 预检、用户 config no-follow/owner/可读性检查和 direct-only 基础配置生成必须分别按 §0.4 记录；本条合同不把局部代码或单次 contract 证据升级为完整 clean-reinstall 或真实 Core/TUN 旅程。system TUN snapshot 只由受控 promotion transaction 派生。

### 2.4 Geo Pre-download (ADR-04)

`[4/4]` 步骤在 install 时预下载 `geoip.metadb` 和 `GeoSite.dat`：

- **目的**：防止鸡生蛋死锁（mihomo 启动需要 geo 文件，但代理未启动无法下载 GitHub）
- **下载策略**（与 core binary 不同）：

| 维度 | Core binary | Geo files |
|------|------------|-----------|
| 断点续传 | HTTP Range on `.part` | 每个 URL 重新下载（`.tmp` 换源时删除） |
| 重试 | 同 URL 3 次退避 | 不同 URL 依次尝试，每个 2 次 |
| 验证 | GzDecoder CRC32 | 尺寸 + 首字节魔数 + `mihomo -t` |
| 临时文件 | `.part`，解压前删除 | `.tmp`，成功后 rename |

- **URL 优先级**：GitHub 直连 → `--proxy` URL（如有）→ `gh-proxy.com` → `mirror.ghproxy.com` → `ghproxy.com`
- **原子写入**：`.tmp` → `rename` 到目标文件

### 2.5 Core Binary Download (ADR-07, ADR-08)

- 下载地址：`https://github.com/MetaCubeX/mihomo/releases/download/{version}/mihomo-{target}-{version}.{ext}`
- 断点续传：HTTP Range + `.part` 文件，跨进程重启可恢复
- 3 次重试，退避 1s → 2s → 4s
- `.part` 在解压前立即删除（防损坏残留）
- 无 SHA256 校验（deferred）：HTTPS + Content-Length + Gzip CRC32 已提供合理基线

---

## 3. Config System

### 3.1 Multi-file Architecture

```
~/.config/mihomo/
├── config.yaml              ← 最终配置（合并产物）★ 单一事实来源（ADR-22）
├── rules.yaml               ← 用户规则
├── dns-policy.yaml          ← DNS 策略
├── override.yaml            ← 高级覆盖
├── subscriptions.yaml       ← 订阅元数据
├── subscriptions/<id>.yaml  ← 下载的订阅内容
├── subscriptions/active     ← 活跃订阅 ID
├── .rules-position          ← 规则插入位置 (front/back)
├── geoip.metadb             ← IP 地理位置数据库
└── GeoSite.dat              ← 域名分类数据库
```

**`config.yaml` 是用户 intent 的单一事实来源**（ADR-22）：所有配置意图、订阅合并产物和用户 overlay 都归档在该路径；它不是 system TUN Core 的直接运行时路径。旧 system store 路径（`/var/lib/mihomo-cli/config.yaml` 等）已废弃。system TUN 运行时使用由 root revalidation、candidate/revision 事务派生并保护的 `tun-config.yaml` snapshot；snapshot 不是第二配置事实来源。

### 3.2 Config Generation

多订阅状态模型：

- `subscriptions.yaml` 保存订阅源元数据，例如 ID、URL、UA、更新时间；不保存完整节点配置。
- `subscriptions/active` 保存当前 active subscription ID，是指向订阅缓存的指针。
- `subscriptions/<id>.yaml` 保存该订阅上次成功下载/转换后的 Clash YAML，是 last known good local copy。
- `config.yaml`（`~/.config/mihomo/config.yaml`）是 active subscription cache 与本地 overlay layers（rules / DNS policy / fake-ip-filter / override）合并后的用户 intent 产物，也是配置领域的唯一事实来源（ADR-22）。普通 user/non-system Core 可使用该文件；system TUN Core 不直接使用任意调用方路径，而使用受保护的 `tun-config.yaml` snapshot。
- 运行中的 mihomo core 是另一层 runtime 状态；`config.yaml` 写入成功不等于 runtime 已 reload。配置命令必须明确提示 reload 成功或需要 `restart`。

这种分层允许：切换订阅时不必立即联网；网络不好时仍可用上次成功缓存；刷新失败时可以保留旧的可用配置。

实际合并顺序：

1. active subscription cache 作为 base config。
2. 合并用户规则 `rules.yaml`。
3. 合并 DNS `nameserver-policy`（`dns-policy.yaml`）。
4. 合并 DNS `fake-ip-filter`（`dns-fake-ip-filter.yaml`）。
5. 注入默认 mixed port（当订阅未提供 `mixed-port` / `port` / `socks-port` 时）。
6. 注入 runtime controller 字段。
7. 应用高级覆盖 `override.yaml`。
8. 再次强制注入 runtime controller 字段。

第 8 步是防御性设计：`override.yaml` 允许高级用户覆盖普通 Mihomo 配置，但不能破坏 `mihomo-cli` / daemon / core 之间的 API 通信路径。因此 `external-controller-unix`、API socket/pipe、controller secret 等 CLI-managed runtime 字段会在 override 后重新注入。

合并语义：
- YAML map 递归合并
- list 和 scalar 直接替换
- runtime controller 字段由 CLI 管理，并会在 override 后重新注入

**写入目标**：合并产物直接写入 `~/.config/mihomo/config.yaml`（per-user config 目录），作为用户 intent 的唯一事实来源。system TUN snapshot 仅由受权事务从已验证 intent 派生并提交；不存在可由 daemon 任意维护的独立 system store。

### 3.3 config 单一事实来源（ADR-22）

所有运行模式（System Service / Per-user）下，`config.yaml` 统一存放在 per-user config 目录，是用户 intent 的唯一权威配置源；system TUN Core 的运行时输入由受保护 snapshot 事务派生。

| 平台 | 路径 |
|------|------|
| Linux | `~/.config/mihomo/config.yaml` |
| macOS | `~/.config/mihomo/config.yaml` |
| Windows | `%USERPROFILE%\.config\mihomo\config.yaml` |

- daemon 通过受权 system context 使用固定 system runtime：普通 system Core 使用 `active-config.yaml`，system TUN Core 使用 `tun-config.yaml`；两者都不是用户配置的第二事实来源，也不直接接受调用方提供的任意 config path
- 配置变更（订阅添加/导入、订阅切换/删除、规则编辑、DNS 策略、override）当前先按用户配置事务提交为 intent；system 模式会将已提交、已校验的内容通过受管 promotion 写入固定运行时并重新启动 Core。system TUN active 下对所有入口统一完成 candidate → snapshot → `CoreApplied` → compare-and-commit → `IntentCommitted` 仍是目标合同，尚未由统一代码路径完整实现和验收
- 旧 `config.yaml` system store 路径（`/var/lib/mihomo-cli/config.yaml`、`/Library/Application Support/mihomo-cli/config.yaml`、`%ProgramData%\mihomo-cli\config.yaml`）已废弃；`active-config.yaml`、`tun-config.yaml`、事务和 Geo 等固定运行时资产仍位于这些平台的 system runtime 目录，并按正式 SPEC 的 writer/reader contract 管理

### 3.4 Subscription Processing and weak-network contract

订阅下载必须把“远端获取”“本地缓存提交”“运行配置应用”视为三个阶段：

1. 远端请求使用固定总预算：每个请求有有限连接/读取超时；瞬时网络错误和 HTTP 5xx 可按 `1s → 2s → 4s` 进行有限重试；HTTP 401/403/404、明确格式错误和认证失败不得盲目重试；429 只有在 `Retry-After` 可解析且不超过总预算时才等待，否则立即返回可修复错误。
2. 每次请求仍可按既定 Clash-compatible UA/`flag=clashmeta`/bare URL fallback 顺序尝试，但 UA 变体不等于无限重试；总请求次数、总等待时间和最终错误必须可观察。
3. 响应先写受管临时文件，完成格式转换、YAML 结构校验和当前 Core `-t` 校验后，才 fsync 并原子提交 `subscriptions/<id>.yaml`。失败响应、截断内容、非配置 HTML 和校验失败均不得覆盖旧缓存。
4. metadata、active pointer 和最终 `config.yaml` 的更新必须在同一配置事务中按固定顺序提交；中断后由 journal 恢复到完整 old/new revision，不能出现 active 指针、cache、metadata、config.yaml 互相矛盾的“半激活”。当 system TUN 未 active 时，该配置事务可以在通过本地/Core 校验后直接提交，并将运行时应用结果单独标为 `runtime_applied`、`pending`、`failed` 或 `unknown`。当 system TUN active 时，待提交内容必须先作为 immutable candidate 持久化到受管 transaction 区域；candidate/pending 不是 active intent，也不改变 `config.yaml`、active pointer 或 metadata。root revalidation 从该 candidate 派生并原子提升受保护 snapshot，daemon 以该 snapshot 和显式 `-f` 启动新 Core；只有 Core API ready、runtime TUN 状态符合目标且 launched snapshot revision 与 candidate revision 匹配后，才将 journal 推进为 `CoreApplied`。daemon 随即释放 `OWNER_LIFECYCLE_LOCK`；dispatcher coordinator 重新取得 canonical user-config writer lock，比较 journal owner/generation、`base_revision`、当前 active revision 及 manifest/hash，全部匹配后才按同一 manifest 原子提交 `config.yaml`、active pointer、metadata 及其他 active effective-config 输入，并将 journal 推进为 `IntentCommitted`。snapshot/Core 应用、attestation、compare-and-commit 或最终持久提交失败时，必须按 journal 恢复完整 old revision，或保留 journal 并返回 `RecoveryRequired`；不得把已提升但未 attested 的 snapshot、`CoreApplied` 或仅完成 candidate 写入报告为 active intent。

`config add`/`config import` 的激活语义必须确定且不依赖隐式 TTY 状态：
- 当不存在 active subscription 时，合法的首个 `add`/`import` 默认创建 active subscription 并生成 `UserEffectiveConfig`；`--no-activate` 是显式例外，只保存非 active cache/metadata。
- 当已经存在 active subscription 时，非交互调用必须显式给出 `--activate` 或 `--no-activate`；`--yes` 只跳过确认，不替代激活选择。交互调用可以询问，但确认结果必须进入同一配置事务。
- clean reinstall 的 `config --import <valid-file> --activate --yes` 显式选择并激活导入配置，避免让 `--yes` 承担激活语义；随后 `restart --system` 才负责 runtime readiness。
- `config switch` 只使用本地已验证的 subscription cache；目标 cache 缺失、不可读或 YAML/订阅结构校验失败时，必须在修改 `subscriptions/active` 之前失败，并保持原 active pointer、`config.yaml` 与运行态不变。
- 当前 active subscription 不能直接通过 `config --remove` 删除。必须先切换到另一个已验证 cache；若没有可验证替代项，remove 必须非零返回并保持 active、cache、metadata、`config.yaml` 与 Core/TUN 不变。

配置应用结果与状态展示使用同一语义但不同层级的字段：命令结果中的 `runtime_applied`、`pending`、`failed`、`unknown` 分别映射到共享 `StatusSnapshot.Configuration` 的 `applied`、`pending`、`failed`、`unknown`。`out of date` 只允许在 active intent revision 与当前运行 Core/snapshot revision 均可观察且明确不一致时使用；Core/API 不可达、revision 缺失或 journal/recovery 状态无法判断时必须使用 `unknown`，不得猜测为 `out of date` 或 `applied`。

### 3.5 Last-known-good state

`subscriptions/<id>.yaml` 是通过完整下载/转换/校验后提交的 last-known-good cache。它不是“存在即可信”的 marker，使用前必须安全读取、验证 owner/path/hash、重新注入受管 controller 并通过当前 Core 语义校验。

| 场景 | 必须行为 |
|---|---|
| 非 active refresh 失败 | 旧 cache/metadata 保留；active/config/Core/TUN 不变；返回网络阻断 |
| active refresh 失败且旧 effective config 可验证 | 保留旧 cache、active/config/Core/TUN；返回 `Incomplete` 或“继续使用旧配置”警告，不触发重启 |
| active refresh 成功但应用失败 | active/cache/metadata 只在事务可恢复提交后更新；旧运行态优先保留；无法证明回滚时 `RecoveryRequired` |
| `config switch` | 只使用本地已验证 cache，不要求联网；cache 缺失/无效时阻断并提示 refresh |
| 当前 `config.yaml` 有效但 cache 缺失 | 允许继续运行当前 `UserEffectiveConfig`；不得为了修复 cache 覆盖当前配置 |
| cache 存在但校验失败 | 视为 `Untrusted`，不得启动/应用；保留文件供诊断，不把它当作 fallback |

`UserEffectiveConfig` 包括当前由本地导入、订阅 cache 和 overlay 生成且通过验证的配置；不另建未经契约定义的缓存目录。若网络失败但已有有效配置仍在运行，CLI 不得重启 Core、删除节点或擅自修改代理组。网络恢复后的刷新/应用必须由用户显式命令或已有明确生命周期动作触发。

### 3.6 DNS Policy

CLI 接口：`mihomo-cli dns policy add <MATCH> <TARGET>`
- MATCH：域名后缀（如 `internal.example.com`）
- TARGET：DNS 服务器 IP（逗号分隔多个）
- 实现：写入 `dns-policy.yaml`，merge 时注入 `config.yaml` 的 `dns.nameserver-policy`

### 3.7 TUN 配置隔离

TUN 是系统级功能，一旦开启会影响所有用户流量。它使用 per-user intent config 的受保护、可验证 system snapshot；snapshot 是派生产物，不是新的配置事实来源。

- Linux: `/var/lib/mihomo-cli/tun-config.yaml`
- macOS: `/Library/Application Support/mihomo-cli/tun-config.yaml`
- Windows: `%ProgramData%\mihomo-cli\tun-config.yaml`

`tun on/off` 是显式的全机网络操作：普通用户命令先做无副作用 preflight，确认 system service、daemon、有效用户配置、Core payload、配置语义和 TUN 能力前置条件；只有 preflight 成功后才由 CLI 内部 sudo re-exec，并由 daemon 的 root peer gate 接受 TUN mutation。`/dev/net/tun` 存在本身不能证明 capabilities 或真实 Core 能力。

TUN 收敛必须使用 immutable candidate/revision、固定 snapshot 路径、root revalidation、Core `-t`、API readiness 和当前 Core API `/configs` 的 runtime 观察。失败时优先保持旧 snapshot/Core/TUN；若无法证明旧态或网络资产已清理，则返回 `Failed`/`RecoveryRequired`，不得声称已开启或自动扩大清理范围。TUN active 下任何会改变 effective config 的写操作都必须走统一 candidate/snapshot promotion dispatcher；不得只 reload per-user `config.yaml` 或以路径相等推断 Core 已应用。

---

### 3.7.1 Proxy scope contract

- `system-proxy` 是平台 OS 应用层状态，必须通过平台原生操作与只读查询确认；它不改变 TUN、Core/service 或 shell 环境。
- `proxy on/off` 只是当前 CLI 进程输出 shell 环境变量设置/清理语句；输出成功不改变父 shell，`proxy off` 不改变 TUN、system proxy 或 Core/service。
- `stop` 只负责停止声明范围内的受管 Core/service；它不清理 system proxy，也不能修改父 shell 环境变量。

### 3.8 Proxy-group selection contract

`select` 的目标是改变一个已存在代理组的当前成员选择，不修改规则目标、代理组定义或订阅内容。具体节点和子代理组都属于合法成员，但目标必须存在于当前 Core/配置所观察到的该组成员集合中。

- 先校验当前 instance、组名、成员名和目标成员类型；组或成员不存在时 fail-fast，不猜测相似名称或替代节点。
- 持久选择和运行时选择是两个结果层：持久 intent 必须安全提交到受管选择/active effective-config 事务；当前 Core 选择必须通过受权 Core API runtime observation 确认。任一层失败时保留旧选择和旧运行态，不能只凭本地选择文件报告已生效。
- Core stopped 时可以提交合法持久选择，但结果为 `pending`，必须提示显式 `mihomo-cli restart [--system]`；不得由 `select` 隐式启动 Core。
- 普通运行实例可使用已登记的代理组选择 Core API forwarding，但只有 forwarding 成功并由当前 Core 观察到目标成员后，才返回 `runtime_applied`；API 不可达、成员观察缺失或运行时结果无法关联到当前 instance 时返回 `unknown`。
- system TUN 为 `TunRunning` 时，选择改变 active effective config，必须经统一 promotion dispatcher；不得先写选择文件再直接 PUT Core API。promotion、runtime selection 或持久提交任一步失败时保留 last-known-good；无法证明恢复时返回 `RecoveryRequired`。
- 选择成功不证明节点可连、目标服务可达、出口 IP 稳定或所有流量使用该选择；这些只能由显式 `exit-ip` 或真实业务数据面证据证明。

### 3.9 Restore-direct layered contract

“恢复直连”不是单一 lifecycle 命令，而是对用户实际启用的影响层分别执行关闭并重新观察后的汇总结论。各层必须独立处理：

| 影响层 | 关闭动作 | 成功证明 | 不包含的证明 |
|---|---|---|---|
| TUN | `tun off` | root peer gate、受管 snapshot/revision/journal transaction 和当前 Core API `runtime_tun == disabled` attestation；重启后再次观察仍为 disabled | 不证明 system proxy、shell 环境或目标网络可达 |
| system proxy | `system-proxy off` | 平台原生查询确认 OS 应用层代理为 disabled | 不证明 TUN、shell 环境或 Core/service 已停止 |
| shell proxy | `eval "$(mihomo-cli proxy off)"` 或重新打开 shell | 当前父 shell 后续环境中相关变量已由用户实际清理；CLI 只能证明输出清理语句 | 不证明其他终端、GUI、TUN 或 system proxy |
| Core/service | `stop` | 受管 instance 的 Core/service 已停止且通过归属观察 | 不证明 OS 代理、父 shell 或未知网络资产已清理 |

`status`/`tun status` 只读采集这些层的当前可观察状态，不执行关闭、修复、sudo、外网 probe 或 recovery。只有所有实际启用且可观察的影响层均分别达到其关闭证明，才可输出 `restored_direct`/“恢复直连”；任一层未执行、unknown、unattested 或关闭失败时，必须输出分层结果和唯一下一步，不得汇总为恢复成功。若 TUN 状态为 `unknown`/`unattested`/`RecoveryRequired`，破坏性 TUN/config mutation 必须先阻断，并通过显式 `restart --system` 或下一次受管 TUN 变更触发内部受管恢复流程。

### 3.10 Control plane versus data plane

- `service/daemon/Core/API/TUN` 是本地控制面，可在无公网时观察；
- 节点连接、DNS、目标站点和出口 IP 是数据面，只有显式 probe 或真实业务流量才能观察；
- 默认 `status` 不触网、不下载订阅、不发起出口探测，不以旧缓存出口 IP 冒充实时健康；
- 电脑断网、代理节点失败或目标站点 TLS 失败，不得自动判定配置损坏、删除节点、改变代理组或重启 Core；Core/策略组的重连和故障转移由 Mihomo 配置语义负责；
- 数据面结果至少区分 `NotObserved`、`Reachable`、`Degraded`、`Unavailable`，并记录目标、路线、节点/代理组、时间和错误类别；一次瞬态失败不能变成永久故障结论。


### 4.1 Platform Support

| 平台 | System Service | Per-user Service |
|------|---------------|-----------------|
| Linux | systemd (`/etc/systemd/system/mihomo.service`) | systemd user (`~/.config/systemd/user/mihomo.service`) |
| macOS | LaunchDaemon (`/Library/LaunchDaemons/io.mihomo.plist`) | LaunchAgent (`~/Library/LaunchAgents/io.mihomo.plist`) |
| Windows | Windows Service | Direct process |

### 4.2 systemd Unit

```
[Unit]
Description=Mihomo CLI System Daemon
After=network-online.target
[Service]
Type=simple
User=mihomo
Group=mihomo
ExecStart=/usr/local/bin/mihomo-cli daemon
Restart=on-failure
RestartSec=2s
KillMode=control-group
TimeoutStopSec=15s
NoNewPrivileges=true
CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE
AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE
RuntimeDirectory=mihomo
RuntimeDirectoryMode=0711
[Install]
WantedBy=multi-user.target
```

`CapabilityBoundingSet`/`AmbientCapabilities` 是 system daemon 启动受管 Core 时的最小能力上界；daemon 本身不得把这些能力用于 IPC 请求以外的任意 root-like 操作，Core 子进程只能继承这组能力。`NoNewPrivileges=true`、`KillMode=control-group` 和 `TimeoutStopSec` 共同保证服务停止时不遗留由该 unit 管理的 daemon/Core 子进程。`Restart=on-failure` 只允许 service manager 恢复意外退出的 daemon/IPC 基础设施；显式 stop、uninstall 和未通过 residual/revision 校验的恢复不得被重新拉起，也不得据此宣称 Core/API/TUN ready。

`0711` 允许普通用户穿越到已知的 daemon IPC socket，但不能列出运行目录；
实际控制权限仍由授权 token 与 Unix peer UID 的双重校验决定。

`RuntimeDirectory=mihomo` 确保 `/var/run/mihomo/` 在服务启动时创建。

### 4.3 macOS launchd (Modern API)

| 操作 | 命令 |
|------|------|
| Install | `launchctl bootstrap system <plist>` |
| Uninstall | `launchctl bootout system/io.mihomo` |
| Start | `launchctl kickstart -k system/io.mihomo` |
| Stop | `launchctl kill SIGTERM system/io.mihomo` |

### 4.4 Socket Lifecycle

Unix socket 由 mihomo core 创建，CLI 负责连接。若 socket 被外部删除：
- 代理流量不受影响（内核态转发）
- CLI 管理命令不可用
- 恢复：`mihomo-cli restart`

### 4.5 Uninstall behavior contract

> 行为合同已由本节与 §0.2、§12.2–§12.4 定义；具体平台实现、完整 clean journey 和 recovery crash-point 证据仍按 §0.4 标记 `Implemented`、`Contract-tested`、`Real-Core-tested` 或 `Planned`。不得因为实现证据仍为 `Planned` 而引入另一套卸载语义。

**默认**：TUI 多选框，选择要删除的组件：

```
┌──────────────────────────────────────┐
│  Uninstall mihomo-cli (system)       │
│                                      │
│  [x] Stop & remove service           │
│  [ ] Remove core binary              │
│      /usr/local/lib/mihomo/mihomo    │
│  [ ] Remove config & data            │
│      /home/ubt/.config/mihomo/       │
│  [ ] Remove geo data                 │
│      geoip.metadb + GeoSite.dat      │
│                                      │
│  Space: toggle  Enter: confirm  Esc: cancel │
└──────────────────────────────────────┘
```

**CLI flags**：与 TUI 选项一一对应，作为预选：

| Flag | 预选 TUI 选项 |
|------|-------------|
| `--remove-binary` | 勾选 core binary |
| `--remove-config` | 勾选 config & data |
| `--remove-geo` | 勾选 geo data |
| `--all` | 全选（= 三个 flag 的快捷方式） |
| `--yes` | 跳过 TUI，直接执行 |
| `--dry-run` | 预览，不执行 |

**行为矩阵**：

```bash
mihomo-cli uninstall                        # TUI，仅 service 默认勾选
mihomo-cli uninstall --remove-binary        # TUI，service + binary 预选
mihomo-cli uninstall --all --yes            # 非交互，全删
mihomo-cli uninstall --remove-geo --yes     # 非交互，service + geo
mihomo-cli uninstall --dry-run              # 预览，不执行
```

**日志从不自动删除**：`/var/log/mihomo/mihomo.log` 和 `~/.config/mihomo/mihomo.log` 始终保留。

### 4.6 Concurrent Safety (ADR-13: flock)

文件变更操作（config、rule、dns、select、backup）使用 flock 排他锁保护：

- 锁文件：`{config_dir}/.mihomo-cli.lock`
- 机制：POSIX `flock(LOCK_EX)`，10s 超时
- 范围：TUN-active promotion 或其它需要等待 runtime 的事务，只覆盖本地安全读取、candidate/revision 形成、`Prepared` journal 持久化，以及 `CoreApplied` 后重新取得锁执行 compare-and-commit；写入 journal 并 fsync 后必须释放配置锁，不能在锁内等待 reload、Core API、snapshot promotion、sudo 或 daemon lifecycle lock
- 非 TUN、无需跨进程等待的本地 intent 提交也必须在锁内完成；若随后需要受管 reload/restart，先释放配置锁，再通过 dispatcher/IPC 执行 runtime apply，不能把“写 intent → 等待 runtime”作为同一把锁的持锁范围
- 崩溃安全：fd 关闭自动释放不残留锁；durable journal/revision 负责跨阶段恢复，不能用持锁时间延长替代事务日志
- 只读操作（status、delay、list、rule test）不加锁
- Windows 没有 POSIX `flock` 时，必须使用等价的受管单写者机制（例如 canonical named mutex/lock file），只覆盖上述本地事务阶段；不能以“暂退化为无锁”作为产品合同

所有配置落盘使用 `utils::atomic_write_file()`（写 `.tmp` → fsync → rename）。

---

### 4.7 CLI 与 System Daemon 二进制一致性与分阶段升级 (Binary Consistency & Staged Upgrade)

为维持用户态普通权限与系统高特权服务的物理隔离安全边界，系统服务在固定系统路径执行 CLI 守护进程（如 `/usr/local/bin/mihomo-cli`，macOS 为 `/Library/Application Support/mihomo/bin/mihomo-cli`，Windows 为 `%ProgramData%\mihomo\bin\mihomo-cli.exe`），用户日常使用用户态 CLI（如 `~/.cargo/bin/mihomo-cli` 或 `~/.local/bin/mihomo-cli`）。

为避免版本更新后产生的版本偏差（Version Skew）与旧 Daemon 驻留问题，系统遵循以下合约：

1. **内容哈希比对 (Content / Hash Match)**：
   - 不依赖仅在发布时递增的 SemVer 字符串，通过内容比对当前运行可执行文件与系统目标路径 `cli_binary` 的二进制一致性。
2. **安装阶段只准备 (`install --system`)**：
   - Pre-flight 检查系统 `cli_binary` 是否缺失或内容哈希不匹配；
   - 若不匹配，禁止走 FastPath 跳过；将新 daemon/Core 和相关资源写入独立、完整、已校验的 pending generation；
   - 不覆盖正在运行的 `cli_binary`，不停止 daemon/Core；向用户明确提示随后执行 `mihomo-cli restart`。
3. **显式应用 (`restart`)**：
   - `restart` 重新校验 pending generation 和当前运行态；
   - 仅 Core 变化时只重启 Core；daemon 变化时才执行 daemon service 的停止、替换、启动；
   - 通过 IPC 握手确认运行中 daemon 的内容/版本身份与协议兼容性，再等待 Core API readiness；
   - 应用成功后提交 active generation，保留 previous generation 用于失败回滚。
4. **运行时感知与 Doctor 诊断**：
   - `mihomo-cli doctor` 在 System 模式下对 Daemon 二进制一致性进行显式健康检查；
   - 日常命令运行时若检测到磁盘/运行时二进制不一致或存在 pending generation，输出显式告警并指引用户执行 `mihomo-cli restart`；
   - `status`/`doctor` 仍然只读，不下载、不停止服务、不应用 pending generation。

---

## 5. exit-ip Command

`mihomo-cli exit-ip` 查询出口 IP，目标模式互斥：

| Flag | 含义 |
|------|------|
| `--node <NAME>` | 指定节点 exit IP |
| `--group <NAME>` | 代理组当前节点 exit IP |
| `--url <URL>` | URL 按规则解析后的估算出口 |
| `--direct` | 系统直连出口 IP |

这替代了旧 `mihomo-cli ip` 命令的模糊语义（旧命令只表示"通过当前 mihomo 访问 echo 服务的观测出口"，不反映真实系统出口或指定节点出口）。

---

## 6. Error Handling

所有错误遵循：**检测 → 报告 → 修复建议**

```text
❌ 错误描述
  原因分析
  修复建议（具体命令）
  排查命令（-v 或日志路径）
```

---

## 7. Core Design Principles

1. **前置条件检查**：每个命令执行前校验状态（如 start 前检查 geo 完整性）
2. **关键操作原子化**：`.tmp` → rename、flock 锁保护
3. **失败路径也测试**：不只测 happy path
4. **写后验证**：所有 config 写入后立即 `mihomo -t` 验证
5. **诊断输出结构化**：`--json` 支持机器可读

---

## 8. Installation Layout

```
/usr/local/lib/mihomo/mihomo          ← System mode core binary
/usr/local/bin/mihomo-cli             ← System mode CLI
~/.local/bin/mihomo                   ← User mode core binary

~/.config/mihomo/                     ← 配置目录（两种模式共享）
├── config.yaml, rules.yaml, dns-policy.yaml, override.yaml
├── subscriptions.yaml, subscriptions/
├── geoip.metadb, GeoSite.dat
├── .rules-position, .mihomo-cli.lock

/var/run/mihomo/                      ← System mode runtime
├── service.sock    (daemon IPC)
├── mihomo.sock     (core API)
└── core.pid

$XDG_RUNTIME_DIR/mihomo/mihomo.sock   ← User mode core API
/tmp/mihomo-$UID/mihomo.sock          ← macOS user mode core API

/etc/systemd/system/mihomo.service             ← Linux system service
/Library/LaunchDaemons/io.mihomo.plist         ← macOS system service
```

---

## 9. Glossary

| 术语 | 含义 |
|------|------|
| Mihomo | 代理核心（原 Clash.Meta） |
| Daemon IPC | System 模式下 CLI 与服务 daemon 的通信 socket；Linux daemon 为 `mihomo` 用户，macOS 为 root，Windows 为 SYSTEM |
| Core API | CLI 与 mihomo 核心的通信 socket |
| Subscription | 远程代理节点列表 URL |
| TUN | 虚拟网卡透明代理 |
| Geo data | geoip.metadb + GeoSite.dat，规则匹配数据库 |
| flock | 文件锁，并发保护机制 |
| Hot Reload | `/configs` API 热更新配置 |
| Atomic Write | `.tmp` → rename 原子写入 |

---

## 10. Roadmap

### Done

| Feature | Notes |
|---------|-------|
| Architecture | 互斥双模式、自动检测、IPC daemon |
| Clash-Verge-like UX | 免 `--system` 日常操作 |
| Geo pre-download | 镜像 fallback、原子写入 |
| Core download resume + retry | HTTP Range、`.part` 管理 |
| Rule management | serde_yaml 校验、标记区块编辑 |
| DNS policy | nameserver-policy CLI 管理 |
| exit-ip command | 多目标模式（node/group/url/direct） |
| flock concurrent safety | 配置变更排他锁 + 原子写 |
| Config multi-file | 订阅/rules/dns/override 分层合并 |
| macOS modern launchctl | bootstrap/bootout/kickstart；BUG-16 后 Stop 用 `launchctl kill SIGTERM`（停进程不卸载 job），bootout 仅用于 uninstall |
| Install 基础设施前置检查 | 逐项检查 binary/service/geo；缺失 intent 时生成并校验 direct-only 基础配置；已有有效 intent 保留并校验；不覆盖用户业务配置；配置 endpoint 通过实例上下文注入 |
| Uninstall TUI + 粒度控制 | TUI 多选框；`--remove-binary/--remove-config/--remove-geo` flags 预选；`--all --yes` 非交互全删；`--dry-run` 预览 |

### Planned

| Feature | Notes |
|---------|-------|
| Desktop notification | D-Bus/notification center |
| Rule group support | rule-provider / rule-groups |
| `--json` diagnostic output | 机器可读 status/ip |
| Unified active-config promotion dispatcher | 当前 `config`/rule/DNS/override/TUI 变更仍主要复用 per-user merge/reload；system TUN active 下的 candidate → snapshot → CoreApplied → compare-and-commit → IntentCommitted 尚未由统一代码路径完整实现和验收 |
---

## 11. Architecture Decision Records

### ADR-01: Unix socket over HTTP controller

Unix socket 作为唯一 API 通信方式，安全性更好，不暴露网络端口。

### ADR-02: 配置热重载 vs 重启

普通、非 TUN-active 的配置变更在 endpoint 和应用证据均可证明安全时，可由配置 dispatcher 通过受管的 `/configs` 热重载；这不是授权通用 `CoreApiRequest` 任意转发，也不允许调用方提交任意 config path/bytes。controller endpoint 变更必须 restart。TUN active 下任何影响 effective config 的变更必须进入统一 candidate/revision → snapshot promotion dispatcher，不能以通用 `/configs` PATCH 或 per-user reload 代替。

### ADR-03: 单二进制分发

纯 Rust，零运行时依赖。

### ADR-04: 预下载 Geo 数据

install 时预下载 `geoip.metadb` + `GeoSite.dat`，防止鸡生蛋死锁。详见 §2.4。

### ADR-05: 规则标记合并法

用户规则通过 `# === USER RULES START/END ===` 标记合并到 config.yaml。

### ADR-06: -v/--verbose 统一调试输出

`crate::log!()` 宏，`-v` 模式输出到 stderr。

### ADR-07: Unconditional resume check

`.part` 文件尺寸检查不依赖 attempt 计数器（跨进程可恢复）。

### ADR-08: .part cleanup before decompression

解压前删除 `.part`，防止损坏残留污染下次续传。

### ADR-09: No SHA256 checksum (deferred)

HTTPS + Content-Length + Gzip CRC32 提供合理基线。

### ADR-10: YAML 编辑策略 (serde_yaml)

使用 `serde_yaml` 校验 + 标记区块编辑 + endpoint 注入，移除 tree-sitter native 依赖。

### ADR-11: Remove fallback path

YAML 编辑失败显式报错，不静默降级。

### ADR-12: macOS launchd Modern API

统一使用 `bootstrap`/`bootout`/`kickstart`/`kill`。

### ADR-13: flock 配置并发保护

文件变更操作使用 `flock(LOCK_EX)` + 原子写入。详见 §4.6。

### ADR-14: AI 原生化方向 —— 做"被 AI 使用"的工具，不内置 AI

**状态**: ✅ 已决策 (2026-08-02)

对同类 CLI 的 AI 功能设计进行评估后，决定不在 mihomo-cli 内置“AI 修改规则”能力。

mihomo-cli 的 AI 原生正确方向是作为**工具提供给上层 AI 使用**：
- 合适的 CLI 接口设计（机器可解析、确定性输出，未来可补 `--json` 结构化输出）
- 合适的 `-h` 帮助信息（AI 可直接阅读理解）
- 合适的 user guide（USAGE.md 面向 AI 可检索）
- 封装良好的 **Agent Skill**（供 Qwen/Claude/Codex 等调用）

**Why**: 内置 AI 会把工具与特定模型/服务耦合，且 AI 能力演进快于 CLI；而"良好 CLI + skill 封装"让任何上层 AI 都能复用工具能力，职责清晰。这也与本项目"零运行时依赖"的定位（ADR-03）一致——不把 AI 运行时塞进二进制。

### ADR-15: 单内核专注 —— 只做 mihomo，不引入第二内核

**状态**: ✅ 已决策 (2026-08-02)

评估 sing-box + mihomo 双内核管理方案后确认：**当前目标只做好 mihomo 内核**，多内核管理方向不做。

**Why**: 双内核增加管理复杂度（两套路径/服务/配置模型）、测试矩阵翻倍、维护成本高；当前用户场景（mihomo 单内核）没有多内核需求。

**How to apply**: 参考 Proxy-RS 时只看其 CLI 设计、TUI 实现、内核管理思路，不借鉴多内核架构。

### ADR-16: Windows 服务架构 —— SCM 协议层 + named pipe 双校验

**状态**: ✅ 已决策 (2026-08-03)，详见 `docs/SPEC-windows-service.md`

**背景**: CI 实测 `StartService FAILED 87`——`mihomo-cli daemon` 是普通命令行进程，
不调用 SCM 协议（`StartServiceCtrlDispatcher`），SCM 等待服务报告"已启动"而永远等不到。

**决策**:
1. **保持单二进制**（ADR-03 延续）：daemon 仍是 `mihomo-cli daemon` 子命令，
   不拆独立服务 exe
2. **Windows daemon 通过 `windows-service` crate 实现 SCM 协议**：
   `service_dispatcher::start`（主线程，`#[tokio::main]` 的 block_on 满足"主线程"约束）
   → `service_main` 回调内自建 tokio runtime 跑核心循环；
   Stop/Shutdown 经 CancellationToken 停机链路优雅退出
3. **named pipe 安全 = SDDL + token 双校验**：
   - SDDL 限制 SYSTEM + Administrators + 安装者 SID（安装者 SID 由 install 落盘
     `%ProgramData%\mihomo\installer-sid`，daemon 运行时读取——不能运行时取，
     daemon 是 SYSTEM 身份）
   - 32 字节随机 token 双副本（服务端 `%ProgramData%\mihomo\service-token` +
     客户端 config 目录），IPC 握手校验
   - `first_pipe_instance` 缓解 pipe 抢占钓鱼
4. **elevated 检测用 windows-sys `TokenElevation`**（弃用 `net session` hack 与
   停更的 is_elevated crate）

**Why**: Windows SCM 是唯一合法的服务托管机制；单二进制 + 双校验兼顾 ADR-03 与安全性。
参考 Proxy-RS（同架构单二进制）但其 pipe 仅 token 无 SDDL，本方案在其上增强。

**How to apply**: 实施见 `docs/SPEC-windows-service.md` S1-S8；unix 平台零影响（cfg(windows) 隔离）。

### ADR-17: 跨平台开机自启统一控制（autostart 命令 + 默认不自启）

**状态**: ✅ 已决策 (2026-08-03)，详见 `docs/SPEC-windows-service.md` §3.0

**决策**:
1. **新增 `mihomo-cli autostart on|off|status` 子命令**，三平台统一：
   - Linux system/user: `systemctl enable/disable/is-enabled`
   - macOS system/user: `launchctl enable/disable/print`
   - Windows system: `sc config mihomo start= auto/demand` + `sc qc`
   - Windows user: 注册表 `HKCU\...\Run` 键 + `.vbs` 隐藏窗口
2. **install 默认不开机自启**——用户显式 `autostart on` 才开启
   （systemd 去 enable --now / launchd RunAtLoad=false / sc start= demand）

`autostart` 只控制平台服务是否在登录/启动时被启用，不代表 daemon 已运行、Core/API 已 ready 或 TUN 已启用。它不得创建配置、启动无配置 Core、绕过显式 `restart`、TUN snapshot/revision attestation 或 `RecoveryRequired` 阻断；只读 `autostart status` 也不得触发 recovery。

**Why**: 三平台自启能力统一、默认不打扰用户（用户主动开启才自启）。
Windows user 用注册表 Run 键 + `.vbs`（Proxy-RS 同款）——隐蔽、不易误删、登录静默无黑窗。

**How to apply**: install 语义变化（默认不自启）需同步更新 USAGE/README 文档与测试；
`autostart` 命令走 InstanceContext 模式解析（`--system/--user` 可选）。

### ADR-18: 多用户 TUN 架构 —— per-user core 独立 + system daemon 独占 TUN 原子开关

**状态**: ✅ 已决策 (2026-08-03，codex GPT-5.5 分析 + 用户确认)

**背景**: TUN 网卡是系统级单例——只能一个进程创建/管理（utun/wintun + 系统路由）。
当前单用户部署，长期支持多用户：同一台 Linux/macOS 多用户，每用户独立代理配置
但共享 TUN 能力。

**决策**:
1. **A 架构**（GPT-5.5 推荐）：
   - **per-user core 各自独立**（无 TUN）——保留用户隔离（每用户独立 core/config）
   - **system daemon 独占 TUN**——统一管理 TUN 网卡 + 系统路由
   - 否决 C 架构（全用户共享一个 system core）——牺牲用户配置隔离
2. **TUN 仲裁：原子开关**（用户决策，弃用引用计数）：
   - `tun on` = 原子置开，`tun off` = 原子置关
   - **最后操作者生效**（谁最后 on/off 决定状态）
   - 不用引用计数（谁 on 了 +1 / off 减 1 / 归零释放）——简单直接
   - 单用户落地仍必须执行与多用户相同的 root peer gate 和配置/快照安全校验；不能以“单用户”省略授权或把最后操作者身份当作 TUN mutation 授权
3. **autostart 标记：per-user**（GPT-5.5 建议）：
   - 存各自用户配置目录（`~/.config/mihomo/autostart.json`）
   - **不用全局文件混存多用户状态**（如 /etc/mihomo/）
   - 天然为多用户预留，单用户实现无需将来迁移

**Why**: TUN 单例约束 + 用户隔离需求 + 用户明确偏好简单模型（原子开关而非引用计数）。

**How to apply**:
- 当前单用户落地与 A 架构一致（system daemon + 单个 per-user core），无需预建多用户
- 多用户演进：daemon 支持多 core 实例（每用户一个）+ TUN 原子开关
- autostart 标记用 per-user 位置（`~/.config/mihomo/autostart.json`）

### ADR-20: Non-interactive Automation Contract

**状态**: ✅ 已决策 (2026-08-04)

**背景**: `mihomo-cli` 的主接口同时服务人类、脚本和 AI agent。此前发现
`upgrade` 有 `Upgrade? [y/N]` 交互确认但没有 `-y/--yes`，导致自动化场景不可用。

**决策**:
1. 任何运行中会询问用户的命令，都必须提供启动时可传入的 flag 来预先表达选择。
2. 确认类 prompt 使用 `-y/--yes` 跳过。
3. 分支选择使用显式 flag，例如 `--system/--user`、`--activate/--no-activate`、`--lan-direct/--no-lan-direct`。
4. `--json` 模式不得阻塞等待 TUI/confirm；stdout 只输出 JSON。
5. sudo/admin 密码或 OS 授权弹窗是允许的例外；无 TTY 且缺少必要选择时应失败并给出可执行建议。
6. 新增交互前必须同时补测试覆盖其非交互路径。

**Why**: 这是 AI/脚本可编排的基础契约；避免命令在自动化环境中卡死，也避免未来新增功能只适合人类手工操作。

**How to apply**: 扫描所有 `Confirm::new`、TUI、stdin prompt、`is_terminal()` 分支；为缺少预选 flag 的命令补 `--yes` 或显式选择 flag，并加入 CLI parse/行为测试。

### ADR-21: System daemon 最小权限——AmbientCapabilities 替代 root daemon

**证据状态**：Linux 非 root daemon + capability contract 已定义；具体代码路径、平台安装和真实 Core/TUN 旅程必须按 §0.4 分别标注，不能由本 ADR 的 unit 片段推导为跨平台 `Implemented`。macOS/Windows 继续服从各自平台服务身份与 IPC/ACL 合同。

**背景**: 此 ADR 决策前 Linux system daemon 以 root 运行。daemon 本身不做任何
需要 root 的操作——文件 I/O（PID/log/socket）只需目录写权限，IPC 通信不需特权。
真正需要 root 的只有 mihomo core：创建 TUN 设备（`/dev/net/tun`）、修改系统路由表、
绑定特权端口 53（DNS hijack）。daemon 以 root 运行违反最小权限原则，一旦 daemon
被攻破（如 IPC socket 被滥用），攻击者获得完整 root 权限。

**调研参考**:

| 方案 | 来源 | 做法 | 安全性 |
|------|------|------|--------|
| systemd AmbientCapabilities | mihomo 官方文档 [1] + Arch 包 [2] | core 以专用用户运行，`AmbientCapabilities` 授权精确 capabilities | 最小权限 |
| setcap on binary | clashtui [3] | `setcap cap_net_admin,cap_net_bind_service=+ep /usr/bin/mihomo`；无 daemon，直接 systemd 管理 core | 最小权限 |
| root helper daemon | Clash Verge Rev [4] | 独立 root helper service，GUI 通过 HTTP API 通信 | CVE-2025-50505 [5]：未认证 API 导致本地提权到 root |
| Native TUN | NixOS module [6] | `tunMode` 选项授予 service 必要权限 | 同 AmbientCapabilities |

**决策**:

1. **Linux daemon 以专用非 root 用户运行**（`mihomo`）
   - systemd: `User=mihomo` + `Group=mihomo`
   - 用户配置树为 `<user>:mihomo`、目录 setgid，文件 `0640`；服务以原用户主组作为补充组穿越 home 与 `.config` 父目录。该组带来的额外只读/穿越能力是不用 ACL 工具的兼容性折衷。
2. **mihomo core 通过 systemd AmbientCapabilities 获得精确权限**：
   ```ini
   [Service]
   User=mihomo
   Group=mihomo
   CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE
   AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE
   ```
   - `CAP_NET_ADMIN`：创建 TUN 设备 + 修改路由表
   - `CAP_NET_RAW`：原始套接字（TUN 所需）
   - `CAP_NET_BIND_SERVICE`：绑定端口 53（DNS hijack）
3. **安装时一次性提权**（`install --system`）：
   - 创建 `mihomo` 用户/组
   - 设置目录权限（`/var/run/mihomo/`、`/var/log/mihomo/` 归属 `mihomo`）
   - 写入 systemd unit（含 AmbientCapabilities）
   - 已有的交互式提权逻辑（弹密码提示）覆盖此步骤
4. **日常运行零 root**：
   - CLI → daemon IPC：Unix socket，daemon 用户可读写
   - daemon → core：spawn 子进程（继承 capabilities）
   - core → TUN/路由/端口：capabilities 授权，无需 root
5. **macOS 方案**：
   - 当前 system LaunchDaemon 仍以 root 运行；launchd 没有 Linux capabilities 的等价机制。
   - IPC 的 peer UID + token 授权以及 config_path owner 校验仍是其应用层边界。
6. **Windows 方案**：
   - 保持 ADR-16 的 SCM 服务架构（Windows 服务模型要求 SYSTEM 身份）
   - named pipe SDDL + token 双校验已是安全边界

**Why**:
- 最小权限原则：daemon 被攻破 ≠ root 泄露
- 行业验证：mihomo 官方文档 + clashtui + Arch Linux 包均采用 AmbientCapabilities/setcap
- 警示案例：Clash Verge Rev CVE-2025-50505（root helper daemon + 未认证 API = 本地提权）
- mihomo core 本身支持以非 root 用户运行（官方 systemd 示例已包含 AmbientCapabilities）

**参考文献**:
- [1] mihomo 官方文档 — Create a running service: https://wiki.metacubex.one/en/startup/service
- [2] Arch Linux mihomo deb 包 — Issue #1915: https://github.com/MetaCubeX/mihomo/issues/1915
- [3] clashtui — JohanChane/clashtui: https://github.com/JohanChane/clashtui
- [4] Clash Verge Rev — clash-verge-rev/clash-verge-rev: https://github.com/clash-verge-rev/clash-verge-rev
- [5] CVE-2025-50505 — Clash Verge Rev privilege escalation: https://www.sentinelone.com/vulnerability-database/cve-2025-50505
- [6] NixOS Wiki — Mihomo TUN mode: https://wiki.nixos.org/wiki/Mihomo

**实施位置**:
- Linux systemd unit 与安装计划由 `instance.rs` 唯一生成，包含 `User=mihomo`、capabilities、目录权限和沙箱选项。
- daemon 从 `MIHOMO_CLI_CONFIG_DIR` 读取原用户配置，并以 peer UID + token + 授权表认证。
- macOS 保持 root LaunchDaemon，Windows 保持 SCM/SYSTEM；两者不声明已实现 Linux 的非 root 模型。
- 迁移：已有安装按用户明确的删除范围执行卸载；需要完整清理旧 system store 时使用 `mihomo-cli uninstall --all --yes`，随后重新 `mihomo-cli install --system --yes`。`--yes` 只跳过确认，不绕过 recovery、权限或归属校验


### ADR-19: System daemon 常驻分离（daemon=基础设施，core=用户功能）

> **历史 ADR，已被当前统一产品合同取代：** 本节保留 daemon/core 分离的历史背景，不定义当前自动恢复边界。当前合同以 §0、§1、§2、§12 为准：system install 在无订阅时生成并校验 direct-only 配置、启动普通 Core/API，但不启用 TUN；只读查询不得隐式启动或 recovery；TUN 必须走 snapshot、root peer gate 和运行态观察。

**状态**: Superseded (2026-08-19)

**历史背景**: ADR-17 后 systemd unit 的 daemon 可用性与 Core 自启曾被混为一体，导致 install、autostart 和 Core 生命周期的边界不清晰。daemon 作为 IPC/基础设施控制面与 Core 作为用户数据面应当分离，但当前合同不再允许用旧的自动拉起语义填补配置或 readiness 缺口。

**当前约束**:

1. system service 负责提供受管 daemon/IPC 基础设施；daemon/Core 的运行身份和 capability 边界服从 ADR-21。
2. service manager 的 daemon 恢复不能等同于 Core ready；daemon activation、Core running、Core API reachable 和 runtime TUN enabled 必须分别观察。
3. `install --system --yes` 无订阅时生成并校验 direct-only `UserEffectiveConfig`，启动普通 Core/API，但不访问订阅 URL、不启用 TUN。
4. `autostart` 若保留，只能控制已验证配置下的受管服务启动；不得成为 TUN recovery 或未知配置的旁路。
5. systemd 使用有限、可测试的 `Restart=on-failure`；显式 stop 不得重生 daemon/Core，崩溃恢复也不得绕过 residual preflight、revision attestation 或 `UnknownOrRecovery` 边界。

历史的 `ensure_core_running`、通过只读命令隐式启动 Core、以“config 文件存在”代替校验等方案均不再是当前实现任务；install 的 direct-only 配置生成和受管 Core 启动以 §2.1 为准。

### ADR-22: config 单一事实来源 —— 删除 system store

**状态**: ✅ 已决策 (2026-08-08)

**背景**: 当前架构引入了 system store（`/var/lib/mihomo-cli/`、`/Library/Application Support/mihomo-cli/`、`%ProgramData%\mihomo-cli\`），其中包含 `config.yaml`（daemon 读的"渲染后配置"）、`service.token`（IPC 认证）、`backups/` 等。同时还有 `intent_config_file`（`~/.config/mihomo/config.yaml`），是 CLI 写的"用户意图配置"。

**问题**:
1. 两份 `config.yaml` 违反单一事实来源原则——daemon 读的与 CLI 写的不是同一个文件
2. ADR-21 后 daemon 改为非 root 用户，system store 的 root-owned 语义失效
3. user/system 模式行为不一致（不同配置路径），增加理解成本

**决策**:
1. **单一事实来源 = `intent_config_file`**（`~/.config/mihomo/config.yaml`）——CLI 和 daemon 都读写同一文件
2. **system store 废弃**——代码层面删除 `app_root` 字段及相关逻辑
3. **两阶段实施**：
   - **阶段 1**：增强 uninstall，清理 system store 残留（向后兼容已部署用户）
   - **阶段 2**：删除 system store 代码，`config_file` 统一指向 `intent_config_file`
4. **已部署用户**：文档记录迁移路径，不做主动迁移（用户重新 install 即完成清理）

**补充决策（TUN 配置隔离）**：system TUN 的 snapshot 规则以本节后的“当前安全事务合同”为准：snapshot 是由已验证用户 intent 派生、由 root peer gate 和受控事务生成并收敛为 `mihomo:mihomo 0640` 的受保护 artifact，不能由调用方直接修改或作为第二事实来源；`tun on/off` 必须走 candidate/revision、revalidation、原子 promotion、journal/rollback 和 Core API runtime observation。

**补充决策（sudo 上下文保留）**：
- `tun on/off` 自动提权时，必须保留原始用户上下文（HOME、UID），否则路径解析会指向 `/root` 而非用户目录
- 提权前通过 `sudo _MIHOMO_CLI_ORIGINAL_HOME=$HOME _MIHOMO_CLI_ORIGINAL_UID=$UID ...` 传递
- 用户显式 `sudo mihomo-cli tun on` 时，利用 `SUDO_UID`/`SUDO_USER` 还原
- 路径解析优先级：私有环境变量 > `SUDO_UID` > `dirs::home_dir()`
- 用 `current_exe()` 而非 `args[0]` 作为 reexec 命令，避免 PATH 变化导致找错二进制

**Why**:
- 两份 config 是同步问题的根源——必须明确哪份是权威来源，否则 daemon 和 CLI 的配置状态会漂移
- ADR-21 消除了 system store 存在的理由（root-owned 语义随非 root daemon 失效）
- 单一配置路径简化心智模型：user 模式与 system 模式行为一致，降低理解成本
- system store 中的 `service.token` 可迁移至 config 目录（与 IPC socket 同目录），`backups/` 可迁移至 config 目录或按需重建

**How to apply**:
- 删除 `app_root` 字段及其在 `instance.rs`/`daemon.rs`/`config.rs` 中的所有引用
- uninstall 命令增加 system store 目录清理（检测并删除 `/var/lib/mihomo-cli/`、`/Library/Application Support/mihomo-cli/`、`%ProgramData%\mihomo-cli\`）
- daemon 改为直接读取 `intent_config_file`（`~/.config/mihomo/config.yaml`），不再读 system store 的 `config.yaml`
- `service.token` 移至 config 目录（`~/.config/mihomo/service.token`），IPC 认证机制不变
- USAGE.md 补充已部署用户的迁移说明：按明确范围执行 `mihomo-cli uninstall --all --yes`（完整清理时），再执行 `mihomo-cli install --system --yes`


### TUN 配置隔离（当前安全事务合同）

TUN 是系统级功能，一旦开启会影响所有用户流量。system TUN 不直接把任意用户路径交给 Core，也不把 snapshot 当作第二配置事实来源。

**当前合同**：
- system TUN 使用固定 system context 下受保护、`mihomo:mihomo 0640` 的 `tun-config.yaml` snapshot；具体平台路径由 instance context 派生。
- 用户 intent 的唯一事实来源仍是原始用户受管的 `~/.config/mihomo/config.yaml`；snapshot 只能由已验证 intent 派生。
- `tun on/off` 先执行无副作用 preflight，再由 CLI sudo/root re-exec 和 daemon root peer gate 完成 candidate/revision、root revalidation、Core `-t`、原子 snapshot promotion、journal 和 rollback。
- `ApplySystemTunSnapshot` 是开启或更新 TUN 的唯一内部 apply 请求；`DisableTun` 是关闭请求。普通用户不能直接写 snapshot、传入任意 config path 或任意配置 bytes。
- Core 必须通过固定 snapshot 启动（显式 `-f tun-config.yaml`）；成功结论必须同时具备 snapshot revision、API readiness 和当前 Core API `runtime_tun` 观察。
- 用户后续修改 intent 不会自动改变当前 TUN；只有显式的配置应用动作或 `tun on` promotion 才能提交新的 snapshot。

### ADR-23: Core 运行态不变量——daemon 负责自动恢复

> **历史 ADR，已被当前统一产品合同取代：** 本节保留历史决策背景，不定义当前用户可观察行为。当前合同要求无订阅 `install` 生成并校验 direct-only 配置、启动普通 Core/API；`restart --system` 是显式启动/重启和配置重新应用入口；信息查询、通用 `CoreApiRequest` 和其它未登记的只读/查询路径不得隐式启动或 recovery。只有显式 lifecycle 命令或已登记的 promotion dispatcher 才能改变 Core 生命周期，且必须服从当前 command-state contract、TUN snapshot/root peer gate 和 `SPEC.md` §12。

**状态**: Superseded (2026-08-19)

> **历史正文边界：** 从本节“背景”开始的自动恢复决策、实施步骤、示例错误消息和验收条目均为历史材料，仅供追溯，不得作为当前实现目标或用户行为合同。当前规则是：无订阅 install 生成并校验 direct-only 配置后启动普通 Core/API；`restart --system` 是显式 Core/API readiness 入口；只读命令不隐式启动或 recovery；已配置且明确要求运行态的命令必须通过受管 lifecycle/promotion dispatcher，并遵守固定 snapshot、root peer、revision attestation 和 `Unknown/RecoveryRequired` 边界。

**背景（历史，仅供追溯）**: System Service 模式下 daemon 和 core 是独立进程（CONTEXT.md 运行时状态模型）。多个用户旅程（install → tun on、upgrade → select、reboot → tun on）在 daemon 运行但 core 停止时失败，报错 "core is not running" 且无修复指引。

根因分析发现 "core 在跑" 这个不变量有 5 个断裂点：
1. install early return 不启动 core
2. daemon autostart marker 可能不存在（旧版本未创建）
3. core 启动可能因 config 问题失败
4. core 崩溃后 daemon 不自动恢复
5. 用户不知道需要手动 `start`

没有任何环节无条件保证 "core 在跑"，且每个 CLI 命令都假设 core 已在运行。

**决策（历史，已废弃）**:

1. **daemon 是 core 生命周期的唯一 owner**。当 daemon 收到需要 core 运行的命令且 core 未运行时，daemon 自动尝试拉起 core，而非直接返回错误。适用命令包括但不限于：EnableTun、DisableTun、以及所有需要 core API 的命令（select、exit-ip、delay 等）。

2. **自动恢复的条件**：daemon 知道 core binary 路径（`state.core_binary` 非空）且 config 文件存在。满足条件时自动 start_core，然后继续执行原命令。

3. **自动恢复失败时的错误消息必须包含可执行的修复步骤**：
   - core binary 不存在 → `"core binary not found at <path>. Fix: mihomo-cli install --system"`
   - config 不存在 → `"config not found at <path>. Fix: mihomo-cli config"`
   - core 启动失败（mihomo -t 校验不通过）→ `"core failed to start: <reason>. Fix: mihomo-cli config --validate"`
   - core 启动超时 → `"core did not become ready within 15s. Check: mihomo-cli logs"`

4. **CLI 层不重复实现 core 启动逻辑**。CLI 发送 IPC 命令，daemon 负责 core 生命周期。CLI 只负责：
   - 检测 daemon 是否在跑（daemon 不在 → 引导 start/install）
   - 传达 daemon 的响应（成功或带修复指引的错误）

5. **install 保证完成后系统进入运行态（历史，已废弃）**。旧方案曾要求 install 的 early return 条件从 "所有组件有效" 改为 "所有组件有效 **且** daemon 和 core 都在跑"；该行为不属于当前合同。

**否决方案（历史，已废弃）**:

- **CLI 层各自检查 core 状态**：每个命令（tun on、select、exit-ip...）都要加检查逻辑，违反 "daemon 管理 core 生命周期" 的架构原则，且容易遗漏。
- **daemon KeepAlive 自动恢复 core**：launchd/systemd 的 KeepAlive 只管 daemon 进程自身，不管 core 子进程。需要 daemon 应用层自己实现。
- **install 不管运行态（当前行为）**：导致 "install 说 done 但 tun on 不能用" 的 UX 裂缝。

**Why**:
- daemon 已经是 core 生命周期的管理者（StartCore/StopCore/RestartCore 都通过 daemon IPC），让 daemon 在需要 core 时自动拉起是自然延伸，不引入新的职责。
- 单一 owner 避免 "每个 CLI 命令都负责检查 core" 的分散逻辑，降低遗漏风险。
- 错误消息带修复指引是 ADR-20（非交互自动化契约）的延伸——即使用户看到错误，也应该知道怎么修。

**历史实施条目（不可执行，仅供追溯）**：以下旧方案曾建议修改 `EnableTun`/`DisableTun`、install early return 和 `ensure_core_running`，但这些条目已被当前合同废弃，不得据此实现或新增自动启动逻辑。当前实现只允许按 §12.2 的命令规则执行：Core 启动/恢复必须由显式 `restart` 或受管 promotion dispatcher 触发；`status`、`doctor`、`tun status`、无配置 `install` 和通用 `CoreApiRequest` 不得隐式启动或恢复。当前测试/实现任务以 §12.4 和正式测试入口为准。

### ADR-24: Core 启动失败——自动恢复与错误分类

> **历史 ADR，已被当前统一产品合同取代：** 错误分类与可执行提示仍可复用，但自动下载、自动重试和隐式 Core recovery 必须服从当前 command-state contract、弱网合同及显式 `restart` 边界；不得由历史 ADR 恢复旧 install/autostart 行为。

**状态**: Superseded (2026-08-19)

> **历史正文边界：** 从本节“背景”开始的错误分类、自动修复、重试和实施条目仅供追溯，不得作为当前隐式 recovery 合同。当前实现可以复用安全的错误分类和可执行提示，但 Core 启动必须由显式 lifecycle 命令或受管 promotion dispatcher 触发；只读查询、无配置 install 和通用 API 请求不得隐式启动、下载或改变运行态。

实际案例：GeoIP MMDB 文件缺失 → core 尝试下载但超时 → fatal exit。用户只看到 "did not become API-ready"，不知道需要下载 geo 数据。

**决策**:

1. **core 启动失败后，daemon 读取 core 日志，按错误模式分类处理**：

| 错误模式 | 检测方式 | 处理 | 理由 |
|----------|----------|------|------|
| GeoIP MMDB 缺失 | 日志含 `Can't find MMDB` 或 `can't download MMDB` | **自动下载** → 重试启动 | 无副作用，不需权限 |
| GeoSite.dat 缺失 | 日志含 `Can't find GeoSite` 或类似 | **自动下载** → 重试启动 | 同上 |
| Config 语法错误 | 日志含 `Parse config error` | **报错 + 展示错误行** | 自动修改 config 可能改错 |
| 端口被占用 | 日志含 `address already in use` | **报错 + 提示端口** | 自动杀进程有副作用 |
| 权限不足 | 日志含 `permission denied` | **报错 + 提示需要的权限** | 需要权限 |
| 未知错误 | 以上都不匹配 | **报错 + 展示日志最后 10 行** | 兜底 |

2. **自动恢复的判断标准**：

| 条件 | 行为 |
|------|------|
| 无副作用 + 不需额外权限 + 有确定的修复方式 | **自动修复并重试**（最多重试 1 次） |
| 自动修复失败或重试后仍失败 | 报错 + 展示修复过程中断点 |
| 有副作用或可能改错 | **报错 + 修复建议**，不自动执行 |
| 需要额外权限 | **报错 + 权限指引** |

3. **自动下载 geo 数据的具体逻辑**：
   - 下载目标路径：与 install 流程一致（`ctx.paths.config_dir` 下的 `geoip.metadb` / `GeoSite.dat`）
   - 下载源：与 install 流程使用相同的 GitHub release URL
   - 超时：30 秒（core 自己等 90 秒太长了）
   - 下载完成后重试 `start_core`，最多 1 次
   - 如果下载也失败 → 报错 `GeoIP data download failed. Fix: manually download or check network`

4. **日志解析的实现位置**：在 `start_core` 返回 Error 后、返回给调用者之前，daemon 读取 `core_log_file` 的最后 N 行，匹配错误模式表。匹配逻辑封装为 `classify_core_startup_failure(log_tail: &str) -> CoreFailure`。

**否决方案**:

- **让 core 自己处理 geo 下载超时**：core 的默认超时是 90 秒，用户等太久。daemon 层应该控制超时并提前介入。
- **所有错误都自动修复**：config 语法错误、端口冲突等场景自动修复风险大于收益，应该让用户决定。
- **只改善错误消息不做自动修复**：用户的核心诉求是 "能用"，不是 "看到更好的报错"。能自动修的就应该自动修。

**Why**:
- 用户执行 `tun on` 或 `start` 时，期望的是 "帮我弄好"，不是 "告诉我哪里坏了"。能自动修复的问题不应该阻断用户。
- 不能自动修复的问题，错误消息必须包含足够的信息让用户自己修——展示 core 的实际 fatal 日志比 "did not become API-ready" 有用 100 倍。
- 这个原则适用于所有 core 启动场景（install、start、restart、tun on 的自动恢复），不只是 TUN。

**历史实施条目（不可执行，仅供追溯）**：以下旧方案曾建议在 `start_core` 失败后由 daemon 分类日志、自动下载 Geo 并重试。当前不得将该逻辑接入只读命令、无配置 install、通用 Core API 或隐式 `tun on` recovery；任何资源修复必须服从 §2 的 install/弱网合同和 §12.3 的 recovery boundary，并由显式 `install`、`restart` 或受管 promotion dispatcher 触发。当前实现/测试任务以 §12.2、§12.3 和 §12.4 为准，旧的 `classify_core_startup_failure`、Geo 自动下载和重试清单不构成待实现事项。

---

### ADR-25: IPC Config Synchronization & Runtime Isolation (IPC 配置下发与运行态物理隔离)

**背景与动机**：
历史架构曾尝试让非特权系统守护进程 `mihomo` 跨权限域直接读取普通用户 `$HOME/.config/mihomo/` 下的磁盘文件。该做法在 Linux 文件系统权限机制下引发了连环意外复杂度：必须维护 Linux `setgid` 继承组、必须为 `$HOME` 补齐 `o+x` 路径穿越、必须在用户目录下创建 `transactions/` 导致跨身份写入 `Permission denied`，且由于 Root 会操作用户目录而被迫引入大量针对符号链接提权（Security #4）的复杂防范逻辑。

**架构���策**：
1. **用户空间与系统空间物理隔离**：
   - 用户空间（`~/.config/mihomo/`）为纯用户私有工作区（`0755`/`0700`，`user:user`），存储订阅元数据、自定义规则 `rules.yaml` 和意图配置。后台守护进程永不跨域访问 `$HOME`。
   - 系统运行态（`/var/lib/mihomo-cli/`）根目录由 `root:mihomo` 管理并保持 `0770`，允许非 root daemon 写入固定 runtime 文件；`active-config.yaml`、TUN 快照、事务状态、generation state 和 Geo 数据按各自 writer/reader contract 使用受保护的属主与模式。
2. **IPC 配置推送机制**：
   - CLI 在本地读取并合并用户配置后，通过本地 IPC Socket（Unix Domain Socket / Named Pipe）直接将已校验的 YAML 字符串及版本号发送给 Daemon。
   - Daemon 接收 Payload 并原子写入 `/var/lib/mihomo-cli/active-config.yaml`，随后启动或重载 Core。
3. **彻底废除历史权限机制**：
   - 彻底废除 Linux `setgid`、`$HOME` 路径穿越 `o+x` 检查、跨用户 `transactions/` 权限自愈与多用户授权表。
   - 彻底消除 Root/Daemon 在普通用户主目录操作文件带来的安全隐患。

---

## 12. Command-state contract

本节只定义当前统一产品合同下的可观察行为。历史 ADR-23/24 文字若与本节的“无订阅 install 生成并校验 direct-only 配置、启动普通 Core/API；信息查询不隐式 recovery；TUN 需 root peer gate”冲突，均视为历史背景，不得作为实现依据。

### 12.1 Core and configuration states

| 状态 | 含义 | 允许的默认行为 |
|---|---|---|
| `NotInstalled` | 无受管实例 | `install` 可创建基础设施；其他变更命令给出 install 引导 |
| `InstalledNoConfig` | 基础设施已安装，但 direct-only 用户配置未能生成或校验失败的异常状态 | `status` 报告 `Incomplete`；显式 `install`/修复流程必须生成并校验基础配置；Core 不得以未校验配置启动 |
| `ConfiguredStopped` | 有已验证 UserEffectiveConfig，Core 未运行 | `restart` 启动并等待 readiness；只读查询不启动 Core |
| `ConfiguredRunning` | Core/API 可观察 | 配置变更按 mode 走 reload 或 restart；公网能力另行探测 |
| `TunRunning` | Core API 当前报告 runtime TUN enabled，且 launched snapshot revision、active snapshot revision 与 active intent revision 均可观察并通过 attestation | 所有 effective-config 变更走 TUN promotion；`tun off` 仍需 root peer gate |
| `TunRunningUnattested` | Core API 报告 runtime TUN enabled，但任一 snapshot/intent revision 缺失、不一致、过期或 journal 尚未到 `IntentCommitted` | 内部结果保留 `unknown`/`RecoveryRequired`；用户显示“运行状态需要修复”，由既有命令执行受管恢复或 reset，不得把 TUN 显示为已收敛，不得提交新的 effective config |
| `TunStateUnknown` | Core/API 不可达、`/configs` 缺少 TUN 字段，或 runtime 观察与 snapshot/intent 无法建立可信关联 | 用户显示“运行状态暂时无法确认”，破坏性变更由 `restart` 等既有命令先尝试恢复；只有归属不可证明时阻断 |
| `UnknownOrRecovery` | API/daemon/manifest/journal/残留无法可靠证明 | 查询显示“运行状态需要修复”但不暴露 transaction ID、journal 或 phase；破坏性变更由 `restart` 等既有命令先尝试恢复或受管 reset；只有归属不可证明时阻断 |

### 12.2 Command rules

- `install`：生成并校验 direct-only 基础配置或保留已有有效 UserEffectiveConfig，收敛基础设施并启动普通 Core/API；无订阅不访问 URL、不启用 TUN。网络下载失败返回 `Incomplete`/`Failed`，不得报告代理可用。
- `config`：本地导入/已验证缓存可在 Core 停止时落盘；远端 refresh 失败保留 last-known-good；TUN active 时所有影响 effective config 的写入进入统一 promotion dispatcher。
- `restart`：在有合法配置时启动/替换 Core 并等待 API readiness；执行前必须先尝试安全 recovery，若 recovery 证据不足但受管 runtime 归属可证明，则按 §12.3 在用户确认或显式 `--yes` 授权下 reset 后继续；只有归属、权限或残留身份无法证明时才阻断，并输出不要求用户操作内部状态文件的既有 mihomo-cli 下一步。
- `start`：仅为兼容/高级别名，必须复用 `restart` 的前置条件和结果语义，不得另有自动 recovery 合同。
- `rule`/`dns`：写入前校验并原子提交；影响 active effective config 时复用同一 dispatcher。
- `status`/`doctor`/`tun status`：只读；只构造一次共享 snapshot，零 sudo、零外网、零写入、零隐式 recovery。Core/API 不可达时显示 `unknown`，不得从磁盘意图推断 runtime TUN。
- `ip`/`exit-ip`：显式数据面 probe；失败只表示该次目标/路线/网络观察失败，不自动修改配置或重启 Core。
- `uninstall`：按显式范围执行阶段化 stop、可证明归属的 residual/journal recovery、artifact 清理和 credential transaction；journal、manifest、残留进程或权限归属无法安全证明时必须 fail-closed，返回 `RecoveryRequired`，不得盲删、强杀或扩大清理范围。

### 12.2.1 Command outcome layering

所有会影响配置、Core 生命周期或 TUN 的命令必须同时区分三个层级，并在文本与 JSON 结果中保持一致：

| 层级 | 证明内容 | 不包含的证明 |
|---|---|---|
| `intent` / transaction | 用户意图、candidate、active pointer、metadata、`config.yaml` 或 journal 是否已安全提交 | 不证明运行中的 Core 已使用该版本，也不证明 TUN 或网络已生效 |
| `control_plane` | 本命令声明的 daemon/Core/API、snapshot promotion 或受管生命周期目标是否已由同一次观察证明 | 不证明公网连接、目标服务可达、DNS/DIRECT 数据面或长期出口稳定 |
| `runtime_attestation` | 当前 Core API 的 runtime 值与 snapshot/intent revision、journal、instance 关联是否完整匹配 | 不证明任意业务请求或公网数据面成功 |

命令结果必须遵守以下映射：

- `config add/import/switch/refresh/remove`、rule、DNS、override 和节点选择：先报告 intent transaction 结果，再报告 `runtime_applied`、`pending`、`failed` 或 `unknown`。仅 intent 提交成功不得输出 `runtime_applied`。
- Core stopped 时，合法 intent 可提交并返回 `pending`，同时给出显式 `mihomo-cli restart [--system]`；不得为了制造 `runtime_applied` 隐式启动 Core。
- 普通运行实例只有在受管 reload/restart 已完成且当前 Core/API 观察确认目标 revision 后才返回 `runtime_applied`；Core/API 不可达或 revision 无法证明时返回 `unknown`。
- system TUN 为 `TunRunning` 时，所有 active effective-config 变更统一经 promotion dispatcher 并完成 snapshot promotion、`CoreApplied`、当前 Core/API runtime attestation 和 `IntentCommitted`，是目标合同；当前实现已阻止通用 `/configs` 旁路，但尚未由统一代码路径完整覆盖和验收所有 config/rule/DNS/override/TUI 入口。未具备该完整证明时不得报告 `runtime_applied`，只能返回 `pending`、`failed`、`unknown` 或 `RecoveryRequired`。
- `restart`/`start` 的 `Ready` 只证明声明的 daemon/Core/API control-plane readiness；无合法配置、residual/recovery blocker、API 不可达或目标 runtime 无法证明时返回 `Incomplete`、`Failed`、`Unknown` 或 `RecoveryRequired`。不得把 `Ready` 解释为公网、代理组出口、DNS/DIRECT 或 TUN 数据面成功。
- `tun on/off` 只有在受管 transaction 完成、root peer gate 通过、目标 snapshot/revision 与 instance 关联可证明，并由当前 Core API 观察得到目标 TUN 状态时才返回 `Ready`；raw API 字段、daemon success、YAML intent 或 snapshot promotion 单独不足以返回成功。运行态不可观察时返回 `Unknown`/`RecoveryRequired`。
- `ip`/`exit-ip` 和真实业务 fixture 是独立数据面证据；其成功或失败不得回填 `restart`、配置写入或 TUN 命令的 control-plane 结果。
- 任一失败必须保留 last-known-good，或留下可恢复 journal 并返回 `RecoveryRequired`；不得用旧状态仍可运行、warning 或已写入文件掩盖新目标未应用。

因此，用户可见的“已应用”“TUN enabled/disabled”“恢复直连”均是受证据等级约束的结论：缺少对应 runtime attestation 或任一实际影响层仍为 `unknown`/`unattested` 时，必须显示未知或恢复要求，不得汇总为成功。

### 12.3 Recovery boundary

CLI 不新增任何独立的恢复子命令（如 `recover`、`reset`、`repair`、`daemon --recover`）。用户只需要执行原本的目标命令；事务恢复与状态收敛由既有命令内部完成：

1. **只读诊断保持零恢复**：`status`、`tun status`、`doctor` 仅做只读采集，不修改状态、不删除事务、不申请 sudo。面向用户的输出不得要求理解 transaction ID、journal、revision 或 phase。
2. **安全恢复优先**：`restart`、`tun on/off`、`stop` 和 `uninstall` 在执行目标动作前读取受管 journal/snapshot/runtime 证据，优先执行有界 roll-forward 或 rollback。能证明安全且无额外业务副作用的修复自动完成。
3. **受管 runtime reset**：当恢复证据不足但可以证明相关目录、事务和进程均属于当前 mihomo instance 时，`restart` 或明确的清理命令可以在用户确认后重置 mihomo 管理的 runtime。重置必须：
   - 保留用户 intent、订阅 metadata/cache、规则、DNS policy 和其它用户配置；
   - 只清理或隔离可重建的 `active-config`、TUN snapshot、transaction/recovery evidence、runtime attestation 和受管临时文件；
   - 停止并重新启动当前受管 Core，必要时关闭 TUN，重新生成固定 runtime 配置并等待 API readiness；
   - 使用新的 durable operation record 记录 reset 的范围、结果和保留物，防止中途崩溃后再次扩大清理范围。
4. **副作用确认**：runtime reset 可能暂时中断代理连接或关闭 TUN。交互终端必须先说明“保留配置、重置运行状态、可能中断连接”，并询问确认；非交互流程只有在用户显式提供 `--yes` 或等价授权时才能执行。`--yes` 只授权本次已明确声明的受管 runtime reset，不扩大删除范围。
5. **归属不可证明时安全停止**：无法证明 artifact、目录、残留进程或服务归属于当前 mihomo instance 时，禁止删除、覆盖或强杀。命令必须保留 last-known-good 和可恢复证据，并输出普通用户可执行的下一步 mihomo-cli 命令；不得输出要求用户手工删除 journal、执行 `chown`、`kill`、`systemctl` 或直接操作 runtime 文件的建议。
6. **结果语义**：安全恢复或受管 reset 完成后，原命令继续执行并只报告“运行状态已修复/已重新启动”；若 reset 未完成，报告配置未修改、运行状态仍需处理，并给出唯一下一步。不得把删除文件、daemon 可达或 Core 进程存在单独当作 Ready。

因此，“无论历史状态如何”不是无条件删除所有状态，而是：用户配置永远优先保留；mihomo 自己管理且可重建的运行状态由既有目标命令自动收敛；外部归属无法证明时宁可停止，也不把风险转嫁给用户。
### 12.4 Verification obligations

ADR-23/ADR-24 的早期“daemon 自动拉起 Core”文字仅保留为历史背景；任何专项 draft、PLAN 或实现说明若与本 SPEC 冲突，必须按本 SPEC 的当前合同修正，并在证据矩阵中如实标注状态。

| 范围 | 最低证据 |
|---|---|
| clean reinstall 基础服务 | 隔离 systemd/daemon contract：all uninstall → infrastructure-only install → local import → restart → API usable |
| Core/Geo 弱网下载 | 可控 HTTP fixture：断线、Range、退避、多源 fallback、校验失败和正式文件不变 |
| 订阅 last-known-good | 旧 cache/config/Core/TUN + refresh 失败，断言旧状态完整保留；active refresh/apply 事务 crash points |
| TUN 控制面 | 隔离 privileged systemd、root peer gate、真实 Core `-t`/API runtime 观察；fake Core 不能替代真实能力 |
| TUN 数据面 | 同架构真实 Core、隔离网络 fixture 或明确标注的真实 external probe；不得用 HTTP 200 或 Core API 字段替代 |
| 公网/节点可用性 | 显式 probe，保留目标/路线/错误和重试证据；不把一次通过或失败扩展为永久保证 |
| 跨平台行为 | 行为合同统一，服务/权限/网络资产清理机制按平台单独给出证据 |
