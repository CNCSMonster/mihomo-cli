# User Journeys

> 记录真实用户/AI 操作场景，作为 ROADMAP、SPEC、USAGE 的需求来源。
>
> 这里不替代功能规格；每条旅程只保留场景、目标、主流程、产品要求和关联任务。

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
3. 在目标服务器导入：
   ```bash
   mihomo-cli config --import /tmp/mihomo-config.yaml
   ```
4. 在目标服务器验证并启动：
   ```bash
   mihomo-cli config --validate
   mihomo-cli start
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

1. 用户执行普通安装：
   ```bash
   mihomo-cli install
   ```
2. 如果 GitHub 直连失败，工具自动尝试内置 mirror：
   ```text
   GitHub → gh-proxy.com → mirror.ghproxy.com → ghproxy.com
   ```
3. 如果用户知道可用镜像，可显式指定：
   ```bash
   mihomo-cli install --github-mirror https://example-mirror.com/
   ```
4. 下载成功后继续安装步骤：
   ```text
   core binary → service files → config/skip-config → geo files
   ```
5. 如果配置暂时没有，仍然完成 core/geo 安装，但不自动启动 service。

### Product requirements

- core binary 和 Geo 文件使用一致的 GitHub fallback 策略。
- `--github-mirror` 同时影响 core binary 和 Geo 文件。
- `--skip-config` 只跳过订阅交互和依赖配置的 service 启动，不跳过 Geo 下载。
- 下载失败时输出明确原因和下一步建议。
- 公开依赖下载逻辑不处理私有订阅 URL；私有订阅受限场景见 J001。

### Related roadmap

- `ROADMAP.md` → `BUG-19: core binary 下载未使用 Geo 同等 GitHub mirror fallback`
- `ROADMAP.md` → `BUG-18: install --skip-config 错误跳过 Geo 下载`


---

## J003 TUN-first installation with explicit TUN control

### Situation

用户倾向总是使用 TUN 全局代理；只有没有 root/admin 权限时才降级到普通 user mode。

### Goal

安装阶段只准备能力，不默认开启 TUN；system service 安装后，普通用户可通过 `tun on/off` 控制 TUN。

### Flow

```bash
mihomo-cli install --system --yes
mihomo-cli config -u '<subscription-url>' --yes
mihomo-cli tun on
```

已有配置时：

```bash
mihomo-cli install --system --yes --skip-config
mihomo-cli config --import config.yaml --yes
mihomo-cli tun on
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

远程服务器开启 TUN 时，建议先在普通 user mode 下完成并验证配置，再停止 user mode、切到 system/TUN。这样可以先排除订阅、DNS、规则和代理组配置错误，再承担 TUN 路由/权限风险。

推荐流程：

```bash
# 1. 普通代理模式准备配置
mihomo-cli install --user --yes
mihomo-cli config -u '<subscription-url>' --yes
mihomo-cli start

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

# 5. 切到 system/TUN
mihomo-cli stop
mihomo-cli install --system --yes --skip-config
mihomo-cli tun on
```

产品要求：

- `tun on` 不隐式修改 rule/DNS 配置；它只负责开启 TUN 运行态。
- LAN/管理网段保护通过 `rule add` 显式配置，并应在 `tun on` 前完成。
- 公司/内网域名保护继续使用 J005 的 fake-ip-filter + DNS policy + DIRECT rule 三件套。
- user mode 和 system mode 共享用户配置源；system/TUN 启动时应复用已验证的用户 intent config，并修正 runtime controller endpoint。
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

- 规则负责稳定意图，代理组负责可变选择。
- 文档主推规则目标使用内置策略或代理组名，不主推具体节点名。
- `rule add` 应尽量校验目标策略/代理组是否存在；不存在时提示 `rule policies` / `list`。
- `select` 是节点切换入口；不新增 `rule select`，除非未来证明有必要。
- `rule test` 应明确其结果是静态估算还是 runtime 真实匹配。
- 未来可通过 shell completion / 模糊匹配降低 group/node 输入成本。

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

- J005 默认推荐 `system`，表示使用操作系统/VPN 已配置的 DNS。
- 具体 DNS IP 只作为高级用法；示例 IP 不是特殊值，必须由用户替换为自己环境的 DNS。
- fake-ip 模式下，内网域名通常需要同时配置 fake-ip-filter、DNS policy 和 DIRECT rule：fake-ip-filter 决定是否返回真实 IP，DNS policy 决定如何解析，rule 决定流量如何路由。
- `dns fake-ip-filter add` 和 `dns policy add` 修改后应清楚提示是否已热更新；如果不能保证，应提示 `mihomo-cli restart`。
- `rule test` 应明确它验证的是路由规则匹配，不等于 DNS 解析一定成功。

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

- 对 OpenAI 等出口稳定性敏感的服务，默认建议固定到具体稳定节点。
- 自动测速、故障转移、负载均衡子代理组适合速度/可用性优先场景，但不作为此类服务的默认推荐。
- `list` 应清楚展示代理组类型和成员类型，帮助用户区分具体节点与子代理组。
- `select` 应允许选择具体节点，也应允许选择该组成员中的子代理组。
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
   mihomo-cli start
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
- `proxy on` 不启动 mihomo；启动/停止服务应使用 `start` / `stop` / `restart`。
- `proxy on/off` 默认只输出 shell 语句；需要用户用 `eval` 应用到当前 shell。
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

- 不新增统一 `rescue` 命令。
- `stop` 只承诺停止 mihomo core/service，不承诺清理所有外部环境影响。
- `stop` 不应被文档描述为可清理父 shell 环境变量；子进程无法修改父 shell env。
- system proxy 是 OS 状态，应使用 `system-proxy off` 明确关闭。
- TUN 应使用 `tun off` 明确关闭。
- `status` 应尽量显示 TUN、system proxy、core/service 等状态，并给出下一步建议。
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
- drift detection 只报告事实，不自动修复。
- 不根据名字相似度自动迁移规则目标或节点选择。
- warning 应包含：失效规则、缺失策略/代理组、失效 group selection、建议查看命令。
- `--json` 应返回 structured warnings，方便 AI agent 决策。
- 如果 drift 已导致 `mihomo -t` 配置校验失败，则应保持现有失败/回滚语义。

### Related roadmap

- `ROADMAP.md` → Subscription drift detection after refresh/switch
- `ROADMAP.md` → AI CLI Contract & JSON 基础
