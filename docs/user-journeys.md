# User Journeys

> 记录真实用户/AI 操作场景，作为 ROADMAP、SPEC、USAGE 的需求来源。
>
> 这里不替代功能规格；每条旅程只保留场景、目标、主流程、产品要求和关联任务。

### 统一配置变更判据

本节服从主 `SPEC.md` §12.2.1 的三层结果合同：`intent`/transaction、`control_plane` 和 `runtime_attestation`。所有旅程中的 `config add/import/switch/refresh/remove`、rule、DNS 和 override 写操作，都必须区分“用户 intent 已提交”和“当前运行 Core 已应用”：

- Core stopped 时，合法 intent 可安全落盘，结果为 `pending`，必须提示显式 `mihomo-cli restart [--system]`；不得报告 runtime 已应用。
- 普通运行实例只有在受管 reload/restart 证据成立时才返回 `runtime_applied`；证据不足或 Core/API 不可达时返回 `unknown`。
- system TUN active 时，完整的统一 promotion dispatcher 仍是目标合同；当前实现会阻止通用 `/configs` 旁路，但尚未让所有 active effective-config 入口统一完成 candidate、snapshot、`CoreApplied` 与 `IntentCommitted`。
- dispatcher 的成功必须由 snapshot/revision、journal 和当前 Core API runtime attestation 共同证明；candidate、snapshot promotion 或 `CoreApplied` 单独不等于 active intent 已提交。
- 任一失败必须保留 last-known-good 或返回 `RecoveryRequired`；状态未知时不得把配置、TUN 或网络宣称为已应用/已恢复。

## 测试证据矩阵

旅程的产品目标与测试证据是两回事。只有标记为“隔离 journey”或“真实 external probe”且对应硬断言通过时，才可报告该范围已通过；CLI smoke 仅证明命令合同，不能证明数据面或完整用户结果。

| 旅程 | 当前最高证据 | 测试入口 | 当前边界 |
|---|---|---|---|
| J001 离线订阅 | 隔离 journey | `just test-container-required j001-offline-subscription` | 覆盖 fetch→离线 import→validate，不验证真实节点数据面 |
| J002 受限网络安装 | CLI smoke | `just test-container-required j002-restricted-network-install` | 未覆盖真实 GitHub 失败→mirror fallback→安装 |
| J003 system service clean reinstall | Contract-tested（隔离 fake Core/systemd control-plane） | `just test-systemd-contract` | 覆盖 `uninstall --all → install --system → config --import → restart → daemon/Core API control-plane readiness`；不证明真实 Mihomo Core、真实 TUN/interface、DNS/DIRECT、代理组出口或公网 data plane；Real-Core/Full-journey 仍为 Planned |
| J003b system runtime recovery/reset | Planned | `just test-systemd-contract`（待补故障注入场景） | 预置 legacy/RecoveryRequired/权限异常/残留 runtime 后，普通用户只执行 `restart`；先自动恢复，必要时确认后仅 reset mihomo 管理的 runtime，保留 intent/config，不要求手工删除内部文件；需覆盖 reset 中断恢复和外部残留拒绝 |
| J004 外部服务路由 | 无可信 fixture | 无 | 未验证真实代理组路由 |
| J005 内网直连 | CLI smoke | `just test-container-required j005-company-intranet-direct` | 未验证真实内网 DNS / DIRECT 数据面 |
| J006 固定节点 | Real-Core-tested（Linux systemd + 真实订阅选择/重放） | `tests/manual/test-real-subscription-select-systemd.sh` | 已验证 system 实例 select→持久化→restart replay→unpin；未覆盖 user 实例自启、TUN active、daemon 自身重启、节点漂移、macOS/Windows 原生环境及完整数据面 |
| J007 shell proxy | 隔离 systemd contract | `just test-systemd-contract` | 覆盖命令输出，不单独证明父 shell 流量 |
| J008 恢复直连 | 隔离 systemd contract | `just test-systemd-contract` | 覆盖关闭动作，不证明宿主所有业务流量恢复 |
| J009 订阅漂移 | 隔离 journey | `just test-container-required j009-subscription-drift-warning` | 覆盖刷新 warning / 恢复指引 |

### 真实订阅 Core 数据面 probe 的边界

`tests/manual/test-real-subscription-core-dataplane.sh` 是显式人工触发的真实订阅 Core 数据面 probe，不属于 CI，也不读取或写入用户的运行配置。它要求：

```bash
CLASH_CONFIG_URL='...' \
MIHOMO_REAL_CORE=/path/to/real/mihomo \
tests/manual/test-real-subscription-core-dataplane.sh
```

脚本把 URL 仅作为一次性容器环境变量传入；容器使用 `--rm`、临时 config/state、只读挂载当前 CLI 与真实 Core。它验证当前 CLI 转换订阅、真实 Core 配置校验、非 TUN proxy 数据面、真实 TUN API/interface 与存活连接；不得输出或记录 URL、订阅内容、节点、token、出口 IP。它不执行或验证 `mihomo-cli install`、daemon、restart、`tun on`、`status`，因此不替代 system service 用户旅程，也不应在 CI 使用真实订阅。

---

## J001 Offline subscription fetch for server

### Situation

目标服务器无法访问私有订阅 URL，或访问结果不稳定/格式不符合预期；另一台本地机器可以访问该 URL。

### Goal

在可联网机器上使用 `mihomo-cli` 的订阅下载能力完成 UA 协商、格式识别、转换与校验，生成可导入的 Clash YAML 文件；复制到目标服务器后，由服务器上的 `mihomo-cli config --import` 导入并生成本机可用配置。

### Flow

1. 在可联网机器生成配置文件：
   ```bash
   mihomo-cli config fetch '<subscription-url>' -o config.yaml
   # 如供应商要求固定 UA：
   mihomo-cli config fetch '<subscription-url>' -o config.yaml --user-agent "clash/v1.0.0"
   ```
2. 复制到目标服务器：
   ```bash
   scp config.yaml user@server:/tmp/mihomo-config.yaml
   ```
3. 在目标服务器导入并激活配置：
   ```bash
   mihomo-cli config --import /tmp/mihomo-config.yaml --activate --yes
   ```
4. 在目标服务器验证并启动：
   ```bash
   mihomo-cli config --validate
   mihomo-cli restart   # `start` 也可作为兼容别名，复用 restart 的前置条件和结果语义
   ```

### Product requirements

- `config fetch` 只生成输出文件，不修改本机 `~/.config/mihomo/`。
- `config fetch` 复用现有订阅下载能力：UA 协商、`flag=clashmeta`、Clash YAML/base64/vmess/raw 识别与转换、YAML 结构校验。
- `config fetch` 的输出必须可被 `config --import` 导入。
- `config --import` 在目标机器上重新生成/修正 controller endpoint。
- 默认脱敏订阅 URL/token；输出文件本身可能包含私有节点信息，应提示用户视为敏感文件。
- 支持全局 `--json`，返回格式识别、节点数、输出路径、warning 等机器可读信息；全局错误 JSON 仍属于后续 AI CLI Contract 治理范围。
- Geo/core 不属于本旅程主线；它们是公开资源，继续由 install/start 的 proxy/mirror/fallback 机制处理。

### Related roadmap

- `ROADMAP.md` → `P0 — 离线订阅 Fetch 旅程`

---

## J002 Install GitHub public dependencies in a restricted network

### Situation

目标机器有网络，但访问 GitHub 不稳定、很慢或被网络拓扑限制。受影响资源主要是 Mihomo core binary 和 Geo 文件。

### Non-goal

完全无网络机器不属于本旅程目标。本项目不优先支持必须手动传输所有文件的纯离线安装。

### Goal

用户希望直接在目标机器完成安装，不手动下载/拷贝公开依赖文件。

### Flow

1. 用户执行 system service 安装（非交互调用显式指定模式和确认）：
   ```bash
   mihomo-cli install --system --yes
   ```
2. 没有订阅 URL 时，安装仍自动生成并校验 direct-only `config.yaml`，启动普通 Core 并等待 API readiness；不访问订阅 URL、不启用 TUN。
3. 如果 GitHub 直连失败，工具自动尝试内置 mirror：
   ```text
   GitHub → gh-proxy.com → mirror.ghproxy.com → ghproxy.com
   ```
4. 如果用户知道可用镜像，可在同一次非交互 system 安装中显式指定：
   ```bash
   mihomo-cli install --system --yes --github-mirror https://example-mirror.com/
   ```
5. 下载成功后继续安装步骤：
   ```text
   artifact download/validation → pending generation → service/access verification
   ```
6. 已有运行实例时，安装不覆盖正在运行的 daemon/Core，也不停止服务；如果存在更新，安装会明确提示后续执行 `mihomo-cli restart`。首次安装且没有运行实例时，可以继续启动并验证 direct-only Core/API。
7. 如果后续执行 `config add/import`，运行中的 system service 会受管 promotion 校验后的订阅配置；只有输出 `pending`/`unknown`/recovery，或需要应用 pending generation 时才执行 `mihomo-cli restart --system`。

### Product requirements

- core binary 和 Geo 文件使用一致的 GitHub fallback 策略。
- `--github-mirror` 同时影响 core binary 和 Geo 文件。
- `--skip-config` 只跳过订阅交互，不跳过 direct-only 基础配置生成、Geo 下载或 Core 启动；它不访问订阅 URL，也不启用 TUN。
- 下载失败时输出明确原因和下一步建议。
- 公开依赖下载逻辑不处理私有订阅 URL；私有订阅受限场景见 J001。

### Related roadmap

- `ROADMAP.md` → `BUG-19: core binary 下载未使用 Geo 同等 GitHub mirror fallback`
- `ROADMAP.md` → `BUG-18: install --skip-config 错误跳过 Geo 下载`


---

## J003 System service clean reinstall

### Situation

用户希望从零开始安装 mihomo-cli，但不应先理解 TUN、Core 或 daemon；安装完成后可导入配置，运行中的 system service 会受管 promotion，基础设施与 Core/API 控制面即可使用。仅在导入返回 pending/unknown/recovery 或需要应用 pending generation 时才显式重启。该结果不表示公网代理或 TUN 数据面已经验证成功。

### Goal

完成最小基础服务旅程；订阅可选，安装后先以 direct-only 基础配置运行：

```bash
mihomo-cli uninstall --all -y
mihomo-cli install --system --yes
# 此时 config.yaml 存在，普通 Core/API 已 ready，TUN disabled
mihomo-cli config --import /path/to/config.yaml --activate --yes  # 可选：运行中的 system service 会受管 promotion
# 仅当导入返回 pending/unknown/recovery，或需要应用 pending generation 时：
# mihomo-cli restart --system
```

安装阶段准备 system service、Core、Geo、授权基础设施，并生成/校验 direct-only 基础配置；首次安装且没有运行实例时可启动普通 Core。已有运行实例的升级只下载、校验并准备 pending generation，不停止 daemon/Core、不制造网络中断。运行中的 system service 在配置导入后受管 promotion 并等待 daemon/Core API；Core 停止或导入结果为 pending/unknown/recovery 时，显式 `restart` 负责恢复运行态或应用 pending generation。这里的 readiness 只证明声明的控制面目标，不证明公网连接、代理组出口、DNS/DIRECT 或 TUN 数据面。

### Product requirements

- `uninstall --all` 是显式删除范围；`-y` 只跳过应用确认，不扩大删除范围。
- 无订阅且没有运行实例的 `install --system --yes` 必须非交互完成基础设施安装，生成并校验 direct-only 配置，启动普通 Core/API，并保持 TUN 停止；已有运行实例的升级不得因安装 daemon/Core 而自动停止服务，必须准备 pending generation 并提示显式 `mihomo-cli restart`。
- `config --import` 在 Core 未运行时也必须成功保存合法配置，不因 reload 不可用而丢失导入结果。
- `restart --system` 必须使用已导入配置应用 pending generation（如有），仅在必要时重启 daemon，并等待 daemon/Core API readiness。
- TUN 是基础服务可用后的可选阶段，不属于 clean reinstall 的必要步骤。

### Evidence boundary

`just test-systemd-contract` 在隔离 systemd、临时用户配置、fake Core 和临时运行时中验证命令顺序、Core 停止/启动状态、daemon IPC 与 Core API socket。它不等价于真实 Mihomo TUN 或外网代理数据面验证；后者由 `tests/manual/test-real-subscription-core-dataplane.sh` 独立覆盖。

### Related roadmap

- `ROADMAP.md` → 基础安装与配置旅程

## J003b system runtime recovery/reset（Planned）

#### Situation

用户无法知道机器此前是否安装过旧版本、是否中断过 TUN 操作或是否残留过 transaction；直接执行 `mihomo-cli restart` 不应要求用户阅读 journal、理解 phase 或手工删除运行状态文件。

#### Target flow

```bash
mihomo-cli install --system --yes
mihomo-cli restart
```

`restart` 内部先尝试可证明的 recovery；无法精确 recovery 但能够证明所有相关 runtime、transaction 和进程都属于当前 mihomo instance 时，交互式说明会保留用户配置、重置运行状态并可能暂时中断连接，确认后执行受管 runtime reset，然后继续启动 Core。非交互执行只有显式 `--yes` 才允许 reset。用户配置、订阅、规则和 DNS 设置必须保留。

如果归属无法证明，命令不得删除或强杀外部资产；只输出普通用户可执行的 mihomo-cli 下一步，不暴露 transaction ID、journal、revision 或 phase，也不要求用户直接操作 systemd 或内部文件。

#### Acceptance

- legacy `RecoveryRequired`、Prepared、权限异常、pending generation、Core 崩溃和 user/system 混装均可作为输入状态；
- 用户只需执行既有 `install`/`restart`/TUN/清理目标命令，不需要识别内部状态；
- reset 白名单以当前 instance 管理资产为限，保留用户 intent 和 last-known-good 配置；
- reset 中途崩溃后可重入，不扩大删除范围；
- fake systemd 与真实 Mihomo Core systemd 均覆盖自动恢复、确认 reset、拒绝外部残留和最终 API readiness；
- 当前实现尚未满足本旅途，未完成前不得在发布说明中声称任意历史状态均可自动恢复。

## J003b Optional TUN control after service readiness

用户倾向使用 TUN 全局代理；只有没有 root/admin 权限时才降级到普通 user mode。`tun on` 的成功只表示受管 TUN transaction 已完成并由当前 Core API runtime observation 和 revision/journal attestation 证明；是否能访问目标服务仍需独立的数据面观察。

### Goal

安装阶段只准备能力，不默认开启 TUN；system service 安装后，普通用户可通过 `tun on/off` 控制 TUN。

### Flow

```bash
mihomo-cli install --system --yes
mihomo-cli config -u '<subscription-url>' --activate --yes
mihomo-cli restart --system
mihomo-cli tun on --yes
```

已有配置时：

```bash
mihomo-cli install --system --yes
mihomo-cli config --import config.yaml --activate --yes
mihomo-cli restart --system
mihomo-cli tun on --yes
```

### Product requirements

- 不新增 `bootstrap`。
- 不新增 `install --tun`。
- `install` 只安装，不默认开启 TUN。
- `--system` 表示安装 system service，为后续 TUN 准备权限能力。
- system service 安装后，`tun on/off` 应可由普通用户执行。
- `--yes` 跳过除 sudo/admin 密码外的非必要交互。
- user mode 是无 root/admin 权限时的 fallback。

### Remote-safe TUN flow and LAN DIRECT protection

远程服务器开启 TUN 时，建议先在普通 user mode 下完成配置层校验，再停止 user mode、切到 system/TUN。这样可以先发现订阅、DNS、规则和代理组配置的静态或控制面问题，但不能替代 system/TUN runtime attestation 或真实数据面验证。

推荐流程：

```bash
# 1. 普通代理模式准备配置
mihomo-cli install --user --yes
mihomo-cli config -u '<subscription-url>' --activate --yes
mihomo-cli restart

# 2. 如需公司/内网域名直连，使用 J005 的 DNS + DIRECT 三件套
mihomo-cli dns fake-ip-filter add corp.example.com
mihomo-cli dns policy add corp.example.com system
mihomo-cli rule add DOMAIN-SUFFIX,corp.example.com,DIRECT

# 3. 如需保护裸 IP 形式的 LAN/管理网段，显式添加 IP-CIDR DIRECT 规则
mihomo-cli rule add IP-CIDR,10.0.0.0/8,DIRECT,no-resolve
mihomo-cli rule add IP-CIDR,172.16.0.0/12,DIRECT,no-resolve
mihomo-cli rule add IP-CIDR,192.168.0.0/16,DIRECT,no-resolve
mihomo-cli rule add IP-CIDR,100.64.0.0/10,DIRECT,no-resolve

# 4. 验证配置层
mihomo-cli config --validate
mihomo-cli rule test git.corp.example.com
mihomo-cli rule test 192.168.1.1
```

停止 user service 并卸载后，若该操作清理了共享用户配置目录，必须从仍然存在的原始配置文件重新导入；不能继续引用已被卸载的 `~/.config/mihomo/config.yaml`：

```bash
# 5. 停止并卸载 user service，再切到 system/TUN
mihomo-cli stop
mihomo-cli uninstall --user --yes
mihomo-cli install --system --yes
mihomo-cli config --import /path/to/config.yaml --activate --yes
mihomo-cli restart --system
mihomo-cli tun on --yes
```

产品要求：

- `config -u`/`config --import` 完成的是经校验的用户配置 candidate/intent 提交；Core 未运行时结果可为 `pending`，必须由显式 `restart --system` 进入 readiness，不能把配置落盘称为 runtime 已应用。
- system TUN active 后，任何改变 active effective config 的后续 `config`、rule、DNS 或 override 操作都必须进入统一 promotion dispatcher；不得只 reload per-user `config.yaml` 或仅提示 restart。
- `tun on` 只执行 TUN 收敛，不隐式修改 rule/DNS 或订阅选择；成功必须由当前 Core API runtime observation 和 launched snapshot revision 证明。
- user mode 和 system mode 共享用户配置源；system/TUN 启动时从已验证的用户 intent config 派生并提交受保护的 system snapshot，再由固定 system context 启动 Core；Core 不直接复用任意调用方提供的 intent path。
- 不新增 `tun on --lan-direct` / `tun on --no-lan-direct`；避免把配置变更隐藏在运行态命令中。
- 建议 LAN/管理网段至少覆盖：

```text
10.0.0.0/8
172.16.0.0/12
192.168.0.0/16
100.64.0.0/10
```

### Related roadmap

- `ROADMAP.md` → `P1 — TUN LAN DIRECT 保护`

## J004 External service routing to semantic proxy group

### Situation

用户订阅中已经包含语义代理组，例如 `OpenAI`、`Netflix`、`节点选择`。代理组内部包含多个节点，也可能包含其它代理组。用户希望把某类外部服务稳定路由到对应代理组，并在日常使用中只切换该代理组当前选择的出口节点。

### Goal

OpenAI 相关域名进入 `OpenAI` 代理组；用户切换 OpenAI 出口时不修改规则，只切换 `OpenAI` 代理组当前节点。

### Flow

1. 查看当前可用策略/代理组：
   ```bash
   mihomo-cli rule policies
   mihomo-cli list
   ```
2. 添加服务分流规则：
   ```bash
   mihomo-cli rule add DOMAIN-SUFFIX,openai.com,OpenAI
   mihomo-cli rule add DOMAIN-SUFFIX,oaistatic.com,OpenAI
   mihomo-cli rule add DOMAIN-SUFFIX,oaiusercontent.com,OpenAI
   ```
3. 验证规则匹配：
   ```bash
   mihomo-cli rule test api.openai.com
   ```
4. 选择 `OpenAI` 代理组当前出口节点：
   ```bash
   mihomo-cli select --group OpenAI --node US-01
   ```
5. 验证出口：
   ```bash
   mihomo-cli exit-ip --group OpenAI
   ```

### Product requirements

本旅程服从主 `SPEC.md` §3.8、§12.2.1；`select` 的持久 intent 提交、Core runtime selection 和真实出口数据面必须分别报告，不能把其中一层当作另外两层的成功证明。

- `rule add` 必须先校验规则语法，并尽量校验目标策略/代理组；写入用户 intent 不等于运行时已应用。
- 规则写入结果必须按统一配置变更判据报告 `runtime_applied`、`pending`、`failed` 或 `unknown`；system TUN active 时完整 dispatcher 仍是目标合同，当前实现会阻止通用 `/configs` 旁路但尚未统一覆盖所有规则入口。
- `rule test` 必须明确是静态规则匹配还是当前 Core 的 runtime 观察；静态匹配成功不等于 DNS、代理连接或外部服务成功。
- `select` 只改变指定代理组的当前选择，不修改规则；成功选择同时写入当前实例的选择意图，Core restart/reload 后在 API ready 时自动重放。
- `list` 应区分当前运行态与持久选择：一致时标记 `[pinned: X]`，未应用时标记 `[pinned: X, not applied]`；可用 `mihomo-cli select --unpin --group <GROUP>` 或 `--all` 显式清除意图，且不切换当前运行态。
- `exit-ip` 是显式数据面 probe；失败只表示本次目标/路线观察失败，不自动修改节点、规则或重启 Core。
- 规则目标不存在、节点不存在或配置应用失败时，旧规则/旧选择/旧运行态必须保持，无法证明恢复时返回 `RecoveryRequired`。

### Related roadmap

- `ROADMAP.md` → 规则/DNS UX 收敛
- `ROADMAP.md` → `select` / `delay` 增强

---

## J005 Company intranet domains with system DNS and DIRECT routing

### Situation

用户在公司网络或 VPN 环境中，需要访问内网服务，例如：

- `git.corp.example.com`
- `wiki.corp.example.com`
- `registry.corp.example.com`

这些域名通常只能由系统/VPN 配置的内网 DNS 正确解析；同时这些流量应走局域网/VPN 直连，不应进入代理或 TUN 出口。

### Goal

`*.corp.example.com` 使用系统 DNS 解析，并且流量走 `DIRECT`；其它外网访问继续按现有规则走代理。

### Flow

1. 在 fake-ip 模式下，让公司内网域名返回真实 IP：
   ```bash
   mihomo-cli dns fake-ip-filter add corp.example.com
   ```
2. 为公司内网域名配置 DNS policy，复用系统/VPN DNS：
   ```bash
   mihomo-cli dns policy add corp.example.com system
   ```
3. 为公司内网域名配置直连规则：
   ```bash
   mihomo-cli rule add DOMAIN-SUFFIX,corp.example.com,DIRECT
   ```
4. 验证规则匹配：
   ```bash
   mihomo-cli rule test git.corp.example.com
   ```
5. 让 DNS/fake-ip-filter 改动可靠生效：
   ```bash
   mihomo-cli restart
   ```

### Product requirements

- J005 的三个配置层（fake-ip-filter、DNS policy、DIRECT rule）分别校验、分别通过各自的配置事务安全提交；三个命令之间不假设存在未定义的跨命令原子事务。只有三层命令的 intent/runtime 结果均符合统一合同后，才能报告“配置层完成”；任何一层失败都必须明确失败层，不能把其它层的成功合并成旅程成功。
- 这些写操作结果遵循统一配置变更判据：Core stopped 时为 `pending` 并提示显式 `restart`；普通运行实例需有受管 reload/restart 证据；system TUN active 时完整 dispatcher 仍是目标合同，当前实现会阻止通用 `/configs` 旁路但尚未统一覆盖所有 DNS/rule 入口。
- `rule test` 只证明规则匹配层；不能把它当作系统 DNS 解析、VPN 可达性或 DIRECT 数据面成功证据。
- 任一层自己的校验、提交或运行时应用失败时，该层必须保留自身 last-known-good；不得删除或回滚其它已经独立成功提交的层。旅程编排层应报告部分完成、失败层和下一步，而不是假装存在跨命令回滚；若单层恢复也无法证明，返回 `RecoveryRequired`。
- `restart` 成功只证明 Core/API readiness；内网 DNS 和 DIRECT 流量仍需独立的真实网络观察。

### Related roadmap

- `ROADMAP.md` → 规则/DNS UX 收敛
- `ROADMAP.md` → DNS policy reload/restart 提示改进

---

## J006 Stable service egress by pinning a proxy group node

### Situation

用户已经把某类外部服务路由到语义代理组，例如 J004 中的 `openai.com -> OpenAI`。这类服务可能对出口 IP、区域、登录态或风控较敏感；如果出口频繁变化，可能触发验证码、登录异常或区域不一致。

### Goal

保持规则不变，只把 `OpenAI` 代理组固定到一个稳定节点，确保 OpenAI 相关流量长期从同一类出口出去。

### Flow

1. 查看代理组和候选成员：
   ```bash
   mihomo-cli list
   ```
2. 选择稳定节点作为 `OpenAI` 组当前出口：
   ```bash
   mihomo-cli select --group OpenAI --node US-01
   ```
3. 验证该代理组出口：
   ```bash
   mihomo-cli exit-ip --group OpenAI
   ```
4. 验证服务域名仍匹配到 `OpenAI` 组：
   ```bash
   mihomo-cli rule test api.openai.com
   ```

### Product requirements

本旅程服从主 `SPEC.md` §3.8、§12.2.1；节点选择的持久 intent、当前 Core runtime selection 与 `exit-ip` 数据面 probe 必须分层报告。

- 对 OpenAI 等出口稳定性敏感的服务，默认建议固定到具体稳定节点。
- 自动测速、故障转移、负载均衡子代理组适合速度/可用性优先场景，但不作为此类服务的默认推荐。
- `list` 应清楚展示代理组类型和成员类型，帮助用户区分具体节点与子代理组。
- `select` 应允许选择具体节点，也应允许选择该组成员中的子代理组。
- 节点选择写入必须遵循统一配置变更判据：Core stopped 时为 `pending`；system TUN active 时进入 promotion dispatcher；运行 Core/API 未确认时为 `unknown`，不得仅凭本地选择文件报告已生效。
- 节点不存在、组不存在或应用失败时不得猜测替代节点；旧选择和旧运行态保持，无法证明恢复时返回 `RecoveryRequired`。
- `exit-ip` 只能证明一次显式出口观察，不代表长期稳定性或所有业务流量均使用该出口。
- 未来可通过 shell completion、模糊匹配或 `--json` 降低 group/node 输入成本。

### Related roadmap

- `ROADMAP.md` → `select` / `delay` 增强
- `ROADMAP.md` → AI CLI Contract & JSON 基础

---

## J007 Terminal-only proxy via shell environment

### Situation

用户没有 root/admin 权限，不能或不想开启 TUN；或者只希望当前终端会话里的 `git`、`curl`、`npm`、`pip` 等命令临时走代理，不影响其它应用和系统全局网络。

### Goal

在 mihomo 已运行的前提下，把代理环境变量临时应用到当前 shell；完成操作后再清理这些环境变量。

### Flow

1. 确保 mihomo 正在运行：
   ```bash
   mihomo-cli restart
   ```
2. 将代理环境变量应用到当前 shell：
   ```bash
   eval "$(mihomo-cli proxy on)"
   ```
3. 执行需要代理的终端命令：
   ```bash
   curl https://example.com
   ```
4. 清理当前 shell 的代理环境变量：
   ```bash
   eval "$(mihomo-cli proxy off)"
   ```

### Product requirements

- `proxy on/off` 是 shell env helper，不是 service lifecycle。
- `proxy on` 不启动 mihomo；启动/停止服务应使用 `restart` / `stop`。
- `proxy on/off` 默认只输出 shell 语句；需要用户用 `eval` 应用到当前 shell。
- 命令输出 shell 语句成功不等于父 shell 环境已改变；只有用户执行 `eval` 后，当前 shell 的后续命令才可能使用代理。
- `proxy off` 只生成清理当前 shell 变量的语句，不改变 TUN、system proxy、Core/service 或其它 shell/GUI 进程。
- 不新增 `proxy exec`；用户可自行临时设置 `HTTP_PROXY` / `HTTPS_PROXY` 或使用 `eval`。
- 这是无 root/admin 或不希望影响全机网络时的 fallback；主推荐路径仍是 TUN。
- 后续 `--json` 可返回 mixed port、socks port、env 建议，供 AI agent 使用。

### Related roadmap

- `ROADMAP.md` → AI CLI Contract & JSON 基础
- `ROADMAP.md` → 终端代理 env UX

---

## J008 Disable mihomo-cli effects and restore direct network

### Situation

用户开启过 TUN、system proxy、shell env proxy 或 mihomo service 后，遇到网络异常，或只是希望恢复到不经过 mihomo 的直连状态。

### Goal

用户能明确知道自己启用了哪一层代理，以及分别如何关闭；不新增统一 `rescue` 命令，而是通过现有命令完成分层恢复。

### Flow

1. 查看当前运行态：
   ```bash
   mihomo-cli status
   mihomo-cli tun status
   ```
2. 如果启用了 TUN，关闭 TUN：
   ```bash
   mihomo-cli tun off
   ```
3. 如果启用了系统代理，关闭系统代理：
   ```bash
   mihomo-cli system-proxy off
   ```
4. 如果当前 shell 使用过 env proxy，清理 shell env：
   ```bash
   eval "$(mihomo-cli proxy off)"
   ```
   或重新打开/登录 shell。
5. 如果希望停止 mihomo core/service：
   ```bash
   mihomo-cli stop
   ```

### Product requirements

本旅程服从主 `SPEC.md` §3.9、§12.2.1；“恢复直连”只能由实际启用的 TUN、system proxy、shell proxy 和 Core/service 影响层分别关闭并观察后汇总，不能由 `stop` 或单次 `tun off` 单独推出。

- 不新增统一 `rescue` 命令。
- `stop` 只承诺停止 mihomo core/service，不承诺清理所有外部环境影响。
- `stop` 不应被文档描述为可清理父 shell 环境变量；子进程无法修改父 shell env。
- system proxy 是 OS 状态，应使用 `system-proxy off` 明确关闭。
- TUN 应使用 `tun off` 明确关闭。
- `status` 只报告当前可观察的 TUN、system proxy、Core/service 和配置状态；它不执行关闭、修复、重启或隐式 recovery。用户根据报告继续显式执行 `tun off`、`system-proxy off`、`proxy off` 或 `stop`。
- 每一层关闭动作都必须有自己的结果判据：`tun off` 需要当前 Core API 的 disabled attestation，`system-proxy off` 需要平台状态确认 disabled，`proxy off` 只能证明输出了当前 shell 清理语句，`stop` 只能证明受管 Core/service 停止。
- 只有所有用户实际启用且可观察的影响层均已分别关闭，才能向用户报告“恢复直连”；任一层 unknown、unattested 或未执行时不得汇总为已恢复。
- 如果 `status` 或 `tun status` 报告 TUN 为 `unknown`、`unattested` 或 `recovery required`，不能把网络视为已恢复，也不能直接继续其它破坏性 TUN/config 操作；先按输出执行显式 `restart --system`，由内部受管恢复重新获得可证明的运行态。
- 用户手册应提供“如何关闭 mihomo-cli 造成的影响”的集中说明。

### Related roadmap

- `ROADMAP.md` → status 诊断增强：显示 system proxy / TUN / service 状态与关闭建议
- `USAGE.md` → 增加关闭/恢复直连说明

---

## J009 Subscription refresh drift warning

### Situation

用户已经基于当前订阅配置了规则和代理组选择，例如：

```bash
mihomo-cli rule add DOMAIN-SUFFIX,openai.com,OpenAI
mihomo-cli select --group OpenAI --node US-01
```

之后用户刷新或切换订阅：

```bash
mihomo-cli config --refresh
# or
mihomo-cli config --switch <id>
```

新的订阅内容可能导致：

- `OpenAI` 代理组被删除或改名。
- `US-01` 节点被删除或改名。
- `OpenAI` 组成员变化，当前选择不再有效。
- 用户规则仍指向旧的策略/代理组名。

### Goal

`mihomo-cli` 在刷新/切换后发现配置漂移并给出 warning；不猜测用户意图，不自动改规则或节点选择，由用户或 AI agent 决定如何处理。

### Flow

1. 用户刷新当前 active subscription：
   ```bash
   mihomo-cli config --refresh
   ```
2. 如果发现漂移，CLI 输出 warning，例如：
   ```text
   Warning: rule target `OpenAI` is not available in current proxy groups.
   Warning: selected node `US-01` is not available in group `OpenAI`.
   ```
3. 用户或 AI agent 查看当前可用策略和代理组：
   ```bash
   mihomo-cli rule policies
   mihomo-cli list
   ```
4. 用户或 AI agent 自行决定修复方式：
   ```bash
   mihomo-cli select --group OpenAI
   # or edit/remove/add rules explicitly
   ```

### Product requirements

- refresh/switch/import 等 active config 变化后，应尽量执行 drift detection。
- active config 变化的结果必须同时报告 intent/config transaction 与 runtime apply 层级：Core stopped 时为 `pending` 并提示显式 `restart`；system TUN active 时只有 promotion dispatcher 完成 revision/journal/Core API attestation 才能报告 `runtime_applied`；无法观察时为 `unknown`。
- drift detection 只报告事实，不自动修复。
- 不根据名字相似度自动迁移规则目标或节点选择。
- warning 应包含：失效规则、缺失策略/代理组、失效 group selection、建议查看命令。
- `--json` 应返回 structured warnings，方便 AI agent 决策。
- 如果 drift 已导致 `mihomo -t` 配置校验失败，或 refresh/switch 的任一事务阶段失败，则必须保持现有 last-known-good active/cache/config/Core/TUN；无法证明恢复时返回 `RecoveryRequired`，不得把 warning 或旧状态保留误报为新配置已应用。

### Related roadmap

- `ROADMAP.md` → Subscription drift detection after refresh/switch
- `ROADMAP.md` → AI CLI Contract & JSON 基础
