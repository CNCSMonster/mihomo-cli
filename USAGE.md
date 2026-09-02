# mihomo-cli 使用手册

> 完整命令参考、选项说明与使用示例

---

## 快速开始

从零开始安装并运行 system service：

```bash
# 1. 安装 system service、Core、Geo 与授权基础设施；无订阅时生成 direct-only 配置并启动普通 Core/API，TUN 保持关闭
mihomo-cli install --system --yes

# 2. 导入本地 Clash/Mihomo YAML 配置
mihomo-cli config --import /path/to/config.yaml --activate --yes

# 3. system service 正在运行时，导入会受管 promotion 并等待 daemon/Core API；仅当输出为 pending/unknown/recovery，或需要应用 pending generation 时再执行：
# mihomo-cli restart --system

# 4. 查看当前代理组/节点
mihomo-cli list
```

如需全机透明代理，在基础服务/Core API readiness 已证明后再显式执行；命令成功仍须以 TUN runtime attestation 确认，不能替代真实目标数据面验证：

```bash
mihomo-cli tun on
```

如需彻底重装，可显式清理所有 mihomo-cli 实例后重复上述流程：

```bash
mihomo-cli uninstall --all --yes
```

---

## AI/脚本 JSON 输出（第一阶段）

`mihomo-cli` 提供全局 `--json` flag。JSON 模式下 stdout 只输出统一 envelope，不混入 emoji、TUI 或进度文本；诊断/错误细节后续会继续迁移到结构化字段。

当前第一批已对齐的低风险命令：

```bash
mihomo-cli --json version
mihomo-cli --json status
mihomo-cli --json config --validate
```

统一 envelope 字段：

```json
{
  "ok": true,
  "command": "status",
  "data": {},
  "warnings": [],
  "error": null,
  "meta": { "schema_version": 1, "cli_version": "..." }
}
```

人类默认输出保持不变；更多命令的 `--json` 支持见 ROADMAP。

## 安装位置说明

使用源码安装时：

```bash
cargo install --path .
```

`mihomo-cli` 二进制会安装到 `$CARGO_HOME/bin/mihomo-cli`；未设置 `CARGO_HOME` 时通常是 `~/.cargo/bin/mihomo-cli`。如果直接运行 `mihomo-cli` 找不到命令，请确认 `~/.cargo/bin` 已加入 `PATH`。

开发阶段如果只是临时验证，也可以直接运行：

```bash
./target/release/mihomo-cli status
```

---

## 命令参考

### 安装部署

#### `install` (alias: `i`)

下载/校验 mihomo Core、Geo 并安装服务与授权基础设施。`install --system --yes` 在没有订阅源时生成并校验 direct-only 基础配置，启动普通 Core/API，但不启用 TUN，也不访问订阅 URL；之后可通过 `config --import` 或 `config --add` 替换为订阅配置。TUN 不是 install 的模式选择结果，而是安装完成后的显式可选操作。

```bash
mihomo-cli install              # 交互式安装基础设施；选择 user/system 上下文不代表启用 TUN
mihomo-cli install --user       # 显式安装普通代理模式（无需 sudo）
mihomo-cli install --system     # 显式安装 system service（高级/脚本/排障）
mihomo-cli install --user --skip-config --yes    # 非交互：安装 user service，跳过订阅交互和确认
mihomo-cli install --system --skip-config --yes  # 非交互：安装 system service，跳过订阅交互和确认
mihomo-cli install --force      # 强制重新安装
mihomo-cli install --github-mirror https://ghproxy.com/  # core/geo 下载走 GitHub 镜像（大陆网络加速）
```

**安装步骤：**

1. 下载并校验 mihomo Core 二进制与 Geo 数据
2. system 模式额外安装 `mihomo-cli daemon`、service unit 和授权基础设施
3. 若用户显式提供订阅或导入配置，则生成/校验当前用户的 `~/.config/mihomo/config.yaml`；否则生成并校验 direct-only 基础配置
4. 安装结束启动普通 Core 并等待 API readiness；不访问订阅 URL，不启用 TUN；之后可执行 `config --add` 或 `config --import`，运行中的 system service 会受管 promotion 新配置；只有输出 `pending`/`unknown`/recovery，或存在 pending generation 时才需要 `restart`

> 💡 `--github-mirror`：Mihomo core 二进制和 geo 数据（geoip/geosite）等 GitHub public assets 下载的镜像前缀。国内网络直连 GitHub 慢时可指定镜像站（如 `https://ghproxy.com/`）；不影响订阅下载。

---

#### `config` (alias: `c`)

多订阅语义：

- `config --refresh` 访问远端 URL，刷新当前 active subscription 的本地缓存，并重新生成/校验 `config.yaml`。
- `config --refresh-all` 刷新所有已保存订阅缓存，但最终 `config.yaml` 仍由 active subscription 生成。
- `config --switch <id>` 切换 active subscription，并重新生成/校验 `config.yaml`；切换使用本地订阅缓存，通常不需要访问远端 URL。
- 对普通、非 TUN-active 的实例，配置变更在生成并校验 `config.yaml` 后，只有受管 reload/restart 成功且当前 Core/API 目标 revision 得到观察，才能报告运行时已应用；reload 需要等待 daemon/Core API 时必须先释放配置锁。Core 停止时合法配置可落盘并返回 `pending`，提示执行 `mihomo-cli restart`。
- system TUN active 时，完整的 `apply_active_intent` dispatcher（candidate/revision、snapshot promotion、`CoreApplied`、compare-and-commit、`IntentCommitted`）仍是目标合同，尚未由所有 config/rule/DNS/override/TUI 入口统一实现和验收；当前实现会阻止 TUN-active 的通用 `/configs` 旁路，不能把该 gate 视为完整事务保证。
- 配置写入/生成成功不一定表示运行中的 mihomo core 已经使用新配置；如结果未明确报告 `runtime_applied`，应按返回的 `pending`/`unknown`/`RecoveryRequired` 状态处理，不得仅凭文件写入成功推断已应用。
- `rules.yaml`、`dns-policy.yaml`、`dns-fake-ip-filter.yaml`、`override.yaml` 会叠加到 active subscription 之上。


管理订阅和配置。无参数时启动交互式 TUI 界面。

```bash
# 配置管理（当前实现仍使用 flat action flags；subcommand-only 设计见 `docs/SPEC-config-cli.md`，尚未作为现行接口发布）
mihomo-cli config                         # 无参数进入交互式 TUI
mihomo-cli config -u '<url>'              # 添加/更新订阅 URL
mihomo-cli config --add '<url>'           # 添加订阅源
mihomo-cli config --remove <ID>           # 删除订阅源
mihomo-cli config --list                  # 列出订阅源
mihomo-cli config --switch <ID>           # 切换订阅
mihomo-cli config --info [ID]             # 查看订阅信息
mihomo-cli config --refresh               # 刷新 active 订阅
mihomo-cli config --refresh-all           # 刷新全部订阅
mihomo-cli config --probe '<url>'         # 探测订阅 UA/格式
mihomo-cli config --set-ua <ID> auto      # 设置订阅 UA
mihomo-cli config --validate              # 验证 config.yaml
mihomo-cli config --fix                   # 修复 controller
mihomo-cli config --import <file>         # 导入本地配置
mihomo-cli config -y                      # 跳过确认提示

# 已实现的离线 fetch 子命令
mihomo-cli config fetch '<url>' -o config.yaml
mihomo-cli config fetch '<url>' -o config.yaml --user-agent "clash/v1.0.0"
```

订阅切换和删除遵守 last-known-good 规则：`config --switch <ID>` 只使用本地已验证缓存，不访问远端；目标缓存缺失或损坏时会在 active pointer 改变前失败。当前 active subscription 不能直接用 `config --remove` 删除；需要先切换到另一个已验证订阅，再删除旧 ID：

```bash
mihomo-cli config --switch <replacement-id>
mihomo-cli config --remove <old-id>
```

如果没有可验证的替代订阅，remove 会拒绝执行并保持原 active、缓存和配置不变。

**交互式 TUI 快捷键：**

| 按键 | 功能 |
|------|------|
| `↑` / `k` | 光标上移 |
| `↓` / `j` | 光标下移 |
| `Enter` | 切换到选中的订阅 |
| `r` | 刷新活跃订阅 |
| `R` | 刷新所有订阅 |
| `a` | 添加新订阅 |
| `d` | 删除光标所在订阅 |
| `Esc` / `q` | 退出 TUI |

**支持的订阅格式：**
- `vmess://` 链接
- Base64 编码的节点列表
- Clash YAML 订阅

**`fetch` 说明：**

`mihomo-cli config fetch '<subscription-url>' -o config.yaml` 会复用订阅下载能力（UA 协商、`flag=clashmeta`、Clash YAML/base64/vmess/raw 识别与转换、YAML 结构校验），仅把可导入的 Clash YAML 写到 `-o/--output` 指定文件。它不会修改本机 `~/.config/mihomo/`、`subscriptions/`、`active`、`rules.yaml` 等状态。

输出文件可能包含私有节点和 token，请按敏感文件处理。复制到目标机器后可执行：

```bash
mihomo-cli config --import config.yaml
```

**`--import` 说明：**

从本地文件导入配置，支持：
- Clash YAML 格式（直接使用）
- Base64 编码的 vmess/trojan 链接（自动转换）
- 原始节点列表（自动转换）

适用于：
- DNS 污染无法直接下载订阅
- 离线环境导入配置
- 从其他机器迁移配置

**`--fix` 说明：**

如果配置缺少 API socket controller（导致 `status` 显示 "unresponsive"），此命令会自动添加。注意：**controller 变更需要 restart 才能生效**。

---

#### `uninstall` (alias: `u`)

卸载 mihomo-cli。

```bash
mihomo-cli uninstall            # 停止服务 + 删除服务
mihomo-cli uninstall --all      # 删除一切（binary + service + config）
```

---

#### `update` (alias: `up`)

更新 mihomo 核心二进制到最新版本。

```bash
mihomo-cli update
```

---

#### `upgrade`

检查 GitHub 上最新 mihomo core 版本，并升级 Mihomo core。

```bash
mihomo-cli upgrade        # 交互确认后升级
mihomo-cli upgrade --yes  # 非交互：跳过确认直接升级
```

---

#### `version`

显示 mihomo-cli 构建信息和当前 mihomo core 版本。

```bash
mihomo-cli version
```

---

#### `dashboard` (alias: `dash`)

实时状态仪表盘（TUI）。

```bash
mihomo-cli dashboard    # 或 mihomo-cli dash
```

---

### 服务控制

#### `start`

启动 mihomo 服务。自动检测当前实例模式（system service / per-user），无需指定模式。

```bash
mihomo-cli start                # 启动当前实例（自动检测模式）
mihomo-cli start --system       # 强制 system service 实例（排障用）
```

启动/重启边界：`start` 保留为兼容和高级实例控制命令；普通用户主旅程使用 `restart`，它负责显式启动目标 Core 并等待 readiness。普通、非 TUN-active 实例的配置导入、规则或 DNS 修改若未能完成受管 reload/restart，会保留已写入的有效配置并提示执行 `mihomo-cli restart`；system TUN active 时必须改走 promotion dispatcher，不得用该提示替代 snapshot/Core 收敛。不会由只读查询隐式启动 Core。

---

#### `stop`

停止 mihomo 服务。

```bash
mihomo-cli stop
```

---

#### `restart`

重启 mihomo 服务。

```bash
mihomo-cli restart
```

---

#### `status`

查看只读状态概览。默认只采集本地控制面与当前可观察运行态，不执行公网/出口 probe、sudo、写入或隐式 recovery；Core/API 或 revision attestation 不可证明时显示 `unknown`，不能从配置文件或历史缓存推断 enabled/disabled。

```bash
mihomo-cli status               # 共享 StatusSnapshot 的简要摘要
mihomo-cli status -v            # 详细本地诊断；仍不执行外网 probe 或隐式修复
mihomo-cli exit-ip --group NAME # 显式数据面出口探测
mihomo-cli doctor               # 显式只读诊断与恢复指引
```

**输出内容：**

- 实例模式、Core 和 API 可达性
- TUN 的 revision-attested 运行态；未证明时为 `unknown`
- system proxy、shell proxy 和当前 Core rule mode
- Default route 与配置应用状态；system service 在 Core API ready 后，仅当 daemon 记录的启动配置 revision 与当前 intent config 匹配时显示 `applied`，不匹配时显示 `out of date`
- 出口 IP 仅由显式 `exit-ip`/probe 命令提供，不属于默认 status
- status 不自动执行诊断修复；需要用户显式执行输出中的下一步命令

---

### 日常使用

#### `select`

选择节点——**无 `--node` 时进入 crossterm TUI**（j/k vim 快捷键 + / 过滤），**有 `--node` 时非交互 CLI 直接切换**。

```bash
mihomo-cli select                                  # 平铺 当前实例所有代理组（TUI）
mihomo-cli select --group 节点选择                  # 限定 当前实例某个代理组（TUI）
mihomo-cli select -g 节点选择 --node 韩国KR-HY2     # 非交互：切换 节点选择 组到指定节点
mihomo-cli select --unpin --group 节点选择          # 取消该组的持久选择（运行态不切换）
mihomo-cli select --unpin --all                    # 清除当前实例全部持久选择
```

> 💡 `--node` 用于脚本/CI 中确定性切换节点；`--node` 必须配合 `--group` 使用。选择成功后会保存为当前实例的选择意图，Core 重启或配置 reload 后自动在 API ready 时重放；若节点已不存在，`list` 会显示 `[pinned: ..., not applied]`。

**快捷键：**

| 按键 | 功能 |
|------|------|
| `j` / `↓` | 光标下移 |
| `k` / `↑` | 光标上移 |
| `g` | 跳到顶部 |
| `G` | 跳到底部 |
| `/` | 进入过滤模式（输入关键词搜索） |
| `Backspace` | 过滤模式下删除字符 |
| `Enter` | 确认切换节点 |
| `Esc` | 取消 / 退出过滤模式 |

---

#### `list`

列出所有代理组及当前节点；若存在持久选择，会同时显示 `[pinned: ...]`，当持久选择尚未应用时显示 `[pinned: ..., not applied]`。

```bash
mihomo-cli list                   # 查看 当前实例所有代理组
```

---

#### `delay`

测试组内所有节点的延迟。

```bash
mihomo-cli delay                  # 默认测试 当前实例的"节点选择"组
mihomo-cli delay --group ChatGPT  # 测试 当前实例指定组
mihomo-cli delay --refresh        # 忽略缓存，重新测试
mihomo-cli delay --cache-ttl 60   # 只复用 60 秒内缓存
mihomo-cli delay --fastest        # 自动选择测试成功的最快节点
```

**输出示例：**

```
香港-优化3-Gemini: 83ms
韩国KR-HY2: 111ms
台湾-优化3: 179ms
日本-优化3: 334ms
```

---

#### `autostart`

控制开机自启（boot/login 启动）。**默认不开机自启**（ADR-17）——安装后需显式开启。

```bash
mihomo-cli autostart on              # 开启开机自启（当前实例模式）
mihomo-cli autostart off             # 关闭开机自启
mihomo-cli autostart status          # 查询自启状态
mihomo-cli autostart on --system     # 指定 system 模式
mihomo-cli autostart on --user       # 指定 user 模式
```

**各平台机制：**

| 平台/模式 | on | off | status |
|-----------|-----|------|--------|
| Linux system | `systemctl enable mihomo` | `systemctl disable` | `systemctl is-enabled` |
| Linux user | `systemctl --user enable` | `systemctl --user disable` | `systemctl --user is-enabled` |
| macOS system | `launchctl enable system/io.mihomo` | `launchctl disable` | `launchctl print` |
| macOS user | `launchctl enable gui/UID/io.mihomo` | `launchctl disable` | `launchctl print` |
| Windows system | `sc config mihomo start= auto` | `start= demand` | `sc qc` |
| Windows user | 注册表 Run 键 + .vbs 隐藏 | 删除 Run 值 | `reg query` |

> 💡 Windows user 模式用注册表 Run 键 + `.vbs` 脚本隐藏窗口（登录静默启动，无黑色控制台窗口），
> 不易被误删。默认不开机自启，`autostart on` 显式开启。

---

#### `tun`

开关 TUN 透明代理模式。

```bash
mihomo-cli tun on                  # 配置已验证且 system/Core ready 后显式开启 TUN
mihomo-cli tun on --yes            # 非交互：跳过应用确认；缺少基础设施或配置时仍然阻断并给出下一步
mihomo-cli tun off                 # 关闭 system service TUN
mihomo-cli tun status              # 自动检测并查看 TUN 状态
mihomo-cli tun on --stack gvisor   # 使用 gvisor 栈
mihomo-cli tun on --stack system   # 使用 system 栈
mihomo-cli tun on --dns-hijack     # 启用 DNS 劫持（默认 any:53）
mihomo-cli tun on --dns-hijack any:53 # 指定 DNS 劫持目标
```

| 选项 | 说明 |
|------|------|
| `--stack <system\|gvisor\|mixed>` | TUN 栈选择 |
| `--dns-hijack [TARGET]` | 启用 DNS 劫持，不带值默认 `any:53` |
| `-y, --yes` | 非交互确认；需要安装/切换 system service 时直接执行 |

> TUN 前置：必须已有可读、通过校验的用户有效配置，并且 system service/daemon/Core API 已按基础旅程达到 **control-plane readiness**。这只是执行 TUN 事务所需的控制面前置，不等于 TUN runtime 已启用，也不证明公网、DNS/DIRECT、代理组出口或真实数据面可用。`tun on` 会先执行无副作用 preflight，再由 CLI 内部完成必要的授权；缺配置、Core 不可达或条件未知时非零返回，不创建 bootstrap、不写 snapshot、不改变路由。成功与否以当前 Core API runtime observation 以及 revision/journal attestation 为准；API 或 attestation 不可达时不得声称 TUN 已启用。`tun status` 只读、零 sudo、零外网，无法观察运行态时显示 `unknown`。

---

#### `conn`

查看**瞬时活跃**连接。

```bash
mihomo-cli conn         # 查看 当前实例连接列表
mihomo-cli conn --flush # 关闭 当前实例所有连接
```

> ⚠️ `conn` 只显示**当前正在传输**的连接——请求一旦完成连接即关闭并从列表消失。
> 要查看**已经发生的**请求匹配了什么规则/走了什么策略（含已关闭连接），
> 用 `mihomo-cli logs`（内核日志逐条记录 `match ... using ...`），而不是 `conn`。

---

#### `exit-ip`

查询节点、代理组、URL 路由或直连路径的出口 IP。该命令要求显式选择一个目标模式，不会默认猜测代理组。

```bash
mihomo-cli exit-ip --node "Korea 01"              # 查询具体节点出口 IP
mihomo-cli exit-ip --group "节点选择"             # 查询代理组当前有效出口 IP
mihomo-cli exit-ip --url https://github.com       # 查询 URL 按规则会走到的出口估算
mihomo-cli exit-ip --direct                       # 查询系统直连出口 IP
```

目标模式互斥：`--node` / `--group` / `--url` / `--direct` 一次只能使用一个。

`--url` 会先按当前规则解析 URL/host 会走到哪个 policy/node，再通过 IP echo 服务估算该节点出口 IP。它不承诺目标网站本身观测到的源 IP 一定相同。

#### `ip`（兼容/弱化）

`mihomo-cli ip` 保留为兼容命令，只表示“通过当前 mihomo 规则访问 IP 查询服务时的观测出口”。它不代表系统直连 IP、TUN 状态、某个指定节点出口，或任意 URL 的真实出口。新用法建议改用 `exit-ip`。

```bash
mihomo-cli ip            # 兼容：当前 mihomo 访问 IP echo 服务的出口
```

---

#### `proxy`

输出 shell 代理环境变量（配合 `eval` 使用）。这是 terminal-only proxy helper，不是 service lifecycle：`proxy on/off` 不会启动或停止 mihomo；启动/停止服务请使用 `restart` / `stop`。

```bash
mihomo-cli restart                       # 先显式启动并等待 mihomo readiness
eval "$(mihomo-cli proxy on)"          # 将 http_proxy / https_proxy 应用到当前 shell
curl https://example.com               # 当前 shell 中的命令走代理
eval "$(mihomo-cli proxy off)"         # 清理当前 shell 的代理环境变量
```

不提供 `proxy exec`；如只想临时代理单条命令，可由用户自行设置 `HTTP_PROXY` / `HTTPS_PROXY` 环境变量。无 root/admin 或不希望影响全机网络时可使用本方式；默认推荐路径仍是 TUN。

---

#### `system-proxy`

设置/取消操作系统级系统代理。

```bash
mihomo-cli system-proxy on           # 设置系统代理到当前实例 127.0.0.1:<mixed-port>
mihomo-cli system-proxy off          # 取消系统代理
```

**平台支持：**

| 平台 | 实现方式 |
|------|----------|
| macOS | `networksetup` (webproxy + securewebproxy + socksfirewallproxy) |
| Linux GNOME | `gsettings` (org.gnome.system.proxy) |
| Linux 其他 | ❌ 不支持，请使用环境变量或 TUN 模式 |

**局限性：**

- **Linux 仅支持 GNOME 桌面**（通过 gsettings）。KDE、XFCE、无头服务器等不支持，命令会报错退出
- **仅影响读取系统代理设置的应用**（GTK/GNOME 应用、部分浏览器）。命令行工具（curl、wget、codex 等）通常不读取系统代理
- **TUN 模式开启时无效**——TUN 已在内核层捕获所有流量，系统代理设置被忽略

**无桌面环境的替代方案：**

```bash
# 方式一：环境变量（推荐，所有场景通用）
export HTTP_PROXY=http://127.0.0.1:<mixed-port>
export HTTPS_PROXY=http://127.0.0.1:<mixed-port>

# 方式二：TUN 模式（全机透明代理，需管理员权限）
mihomo-cli tun on
```

---

#### `logs`

查看 mihomo 日志文件。

```bash
mihomo-cli logs                     # 默认显示最后 50 行
mihomo-cli logs --tail 200          # 显示最后 200 行
mihomo-cli logs --level error       # 按级别过滤（debug/info/warn/error）
mihomo-cli logs --level warning     # 过滤 warning 级别
mihomo-cli logs -f                  # 持续跟随新日志
```

> 💡 **用途**：内核日志逐条记录每条连接的规则匹配与出站策略
> （`[TCP] host:443 match DomainSuffix(...) using DIRECT`），是排查
> "某个请求走了哪条规则/哪个节点"的权威来源——尤其是连接已经关闭、
> `conn` 列表里看不到的场景（查**历史**连接用 `logs`，查**活跃**连接用 `conn`）。
>
> 📍 日志文件位置：`mihomo-cli status` 的 `Logs:` 字段会标出。
> System 模式在 `/var/log/mihomo/mihomo.log`，per-user 在
> `~/Library/Logs/mihomo/mihomo.log`（macOS）/ 对应 XDG 日志路径（Linux）。
> 也可用 `logs -f` 边发请求边实时观察规则匹配。

---

#### `rule`

管理用户自定义路由规则。rule 命令自动检测当前实例；`--system` 仅用于脚本/排障时显式指定 system service context。规则读写使用 resolved `AppPaths`。

```bash
# 添加规则（当前实例）
mihomo-cli rule add DOMAIN-SUFFIX,company.com,DIRECT
mihomo-cli rule add IP-CIDR,192.168.0.0/16,DIRECT
mihomo-cli rule add "DOMAIN-SUFFIX,google.com,节点选择"
mihomo-cli rule add DOMAIN-SUFFIX,example.com,DIRECT --position front  # 指定插入位置

# 列出规则
mihomo-cli rule list         # alias: ls

# 删除规则（按 1-based 索引）
mihomo-cli rule remove 2     # alias: rm

# 移动规则位置
mihomo-cli rule move 3 1     # 将第 3 条规则移到第 1 位

# 清空所有规则
mihomo-cli rule clear --yes

# 导入/导出规则文件
mihomo-cli rule import ./my-rules.yaml
mihomo-cli rule export ./backup.yaml

# 设置默认插入位置
mihomo-cli rule position front      # 规则优先于订阅配置
mihomo-cli rule position back       # 规则兜底于订阅配置
mihomo-cli rule position            # 查看当前位置

# 查看支持的规则类型
mihomo-cli rule types

# 查看可用策略列表
mihomo-cli rule policies

# 测试域名匹配哪条规则
mihomo-cli rule test google.com
mihomo-cli rule test 192.168.1.1
```

**支持的规则类型：**

| 类型 | 说明 | 示例 |
|------|------|------|
| `DOMAIN` | 精确域名 | `DOMAIN,example.com,DIRECT` |
| `DOMAIN-SUFFIX` | 域名后缀 | `DOMAIN-SUFFIX,google.com,Proxy` |
| `DOMAIN-KEYWORD` | 域名关键词 | `DOMAIN-KEYWORD,google,DIRECT` |
| `GEOSITE` | GeoSite 集合 | `GEOSITE,cn,DIRECT` |
| `IP-CIDR` | IPv4 CIDR | `IP-CIDR,192.168.0.0/16,DIRECT` |
| `IP-CIDR6` | IPv6 CIDR | `IP-CIDR6,2001:db8::/32,DIRECT` |
| `GEOIP` | GeoIP 国家码 | `GEOIP,CN,DIRECT` |
| `SRC-IP-CIDR` | 源 IP CIDR | `SRC-IP-CIDR,10.0.0.0/8,DIRECT` |
| `SRC-PORT` | 源端口 | `SRC-PORT,443,DIRECT` |
| `DST-PORT` | 目标端口 | `DST-PORT,443,DIRECT` |
| `PROCESS-NAME` | 进程名 | `PROCESS-NAME,curl,DIRECT` |
| `PROCESS-PATH` | 进程路径 | `PROCESS-PATH,/usr/bin/curl,DIRECT` |
| `NETWORK` | tcp 或 udp | `NETWORK,tcp,DIRECT` |
| `MATCH` | 兜底规则 | `MATCH,DIRECT` |

**工作原理：**

1. 规则存储在 resolved instance config dir 的 `rules.yaml`
2. 插入位置配置在同一 config dir 的 `.rules-position`（默认 `front`）
3. 每次启动/受管应用时合并生成最终运行配置
4. 当前实例规则变更后，非 TUN-active 模式可进入受管 reload/restart；system TUN active 时完整 promotion dispatcher 仍是目标合同，当前实现会阻止通用 `/configs` 旁路但尚未统一覆盖所有规则入口。配置锁只保护本地 candidate/journal/compare-and-commit 阶段，不在锁内等待 privileged runtime apply

---

#### `dns`

管理 DNS 路由策略（nameserver-policy）。dns 命令自动检测当前实例；`--system` 仅用于脚本/排障时显式指定 system service context。DNS 配置读写使用 resolved `AppPaths`。

```bash
# DNS 策略管理
mihomo-cli dns policy add internal.example.com 192.0.2.53           # 添加策略


### DNS fake-ip-filter 管理

在 `enhanced-mode: fake-ip` 下，内网/VPN 域名如果需要真实 DNS 结果，可用独立源文件 `dns-fake-ip-filter.yaml` 管理 `dns.fake-ip-filter`，再由 CLI 生成合并到 `config.yaml`：

```bash
mihomo-cli dns fake-ip-filter add corp.example.com     # 保存为 +.corp.example.com
mihomo-cli dns fake-ip-filter add '*.corp.example.com' # 同样规范化为 +.corp.example.com
mihomo-cli dns fake-ip-filter list
mihomo-cli dns fake-ip-filter remove corp.example.com
```

修改会重新合并并校验配置；为确保 DNS/fake-ip 行为可靠生效，请执行：

```bash
mihomo-cli restart
```

公司内网场景通常需要三者配合：

```bash
mihomo-cli dns fake-ip-filter add corp.example.com
mihomo-cli dns policy add corp.example.com system
mihomo-cli rule add DOMAIN-SUFFIX,corp.example.com,DIRECT
mihomo-cli restart
```
mihomo-cli dns policy add corp.com 192.0.2.53,192.0.2.54   # 多个 DNS 服务器
mihomo-cli dns policy list                                  # 列出当前实例策略 (alias: ls)
mihomo-cli dns policy remove 1                              # 按索引删除 (alias: rm)
mihomo-cli dns policy remove internal.example.com                   # 按域名删除

# DNS 状态
mihomo-cli dns status                                       # 查看当前实例 DNS 配置

# DNS 模板
mihomo-cli dns template list                                       # 列出可用模板
mihomo-cli dns template apply company --domain corp.example.com --target 192.0.2.53
mihomo-cli dns template apply ads                           # 应用广告过滤模板
```

**内置模板：**

| 模板 | 说明 |
|------|------|
| `company` | 将内部域名路由到公司 DNS 服务器 |
| `ads` | 将常见广告/追踪域名路由到过滤 DNS |

---


### 恢复直连 / 关闭 mihomo-cli 的影响

`mihomo-cli stop` 只停止 mihomo core/service，不会也不能清理所有外部环境状态。按层级分别关闭：

```bash
mihomo-cli status              # 查看当前实例，并显示关闭提示
mihomo-cli tun status          # 查看 TUN 是否启用
mihomo-cli tun off             # 关闭 TUN
mihomo-cli system-proxy off    # 关闭 OS system proxy
eval "$(mihomo-cli proxy off)" # 清理当前 shell env proxy
mihomo-cli stop                # 如需停止 core/service
```

如果曾在 shell 中执行过 `eval "$(mihomo-cli proxy on)"`，只能在同一个 shell 中执行 `eval "$(mihomo-cli proxy off)"` 或重新打开 shell；子进程无法修改父 shell 环境变量。

#### `backup` / `restore`

备份和恢复当前实例的配置文件。命令自动检测活跃实例；`--system` 仅用于脚本/排障时显式指定 system service context。

```bash
# 备份
mihomo-cli backup                    # 备份到 当前实例 backups/<timestamp>/
mihomo-cli backup /path/to/dir       # 备份到指定目录

# 恢复
mihomo-cli restore /path/to/backup          # 从备份恢复 当前实例（需确认）
mihomo-cli restore /path/to/backup --yes    # 跳过确认
```

**备份内容：**
- `config.yaml`, `rules.yaml`, `dns-policy.yaml`
- `subscriptions.yaml`, `subscriptions/` 目录
- `.rules-position`

> 注：`override.yaml` 不在备份范围内（代码 `backup.rs` 的 `BACKUP_ITEMS` 不含 override）。

---


## 高级配置

### override.yaml

`~/.config/mihomo/override.yaml` 是高级配置覆盖文件。它用于覆盖普通 Mihomo 配置字段；日常操作优先使用 `config` / `rule` / `dns` / `select` 等命令，不推荐直接编辑 YAML。可直接编辑文件，也可用 `override` 命令管理；命令默认自动检测当前实例，`--system` 仅用于脚本/排障：

```bash
mihomo-cli override path
mihomo-cli override show
mihomo-cli override import ./override.yaml
mihomo-cli override clear --yes
```

示例：

```yaml
# override.yaml 示例
proxy-groups:
  - name: Custom
    type: select
    proxies:
      - DIRECT
dns:
  enhanced-mode: redir-host
```

**合并顺序：** active subscription cache → 用户规则 → DNS policy → DNS fake-ip-filter → 默认端口/controller 注入 → `override.yaml` → controller 再注入

**合并语义：**
- YAML map 递归合并
- list 和 scalar 直接替换
- runtime controller 字段由 `mihomo-cli` 管理，会在 override 后重新注入；不要通过 override 修改 `external-controller*`、API socket/pipe 或 controller secret

---

## 平台说明

| 平台 | 架构 | 开机自启方式 | TUN 模式 |
|------|------|-------------|----------|
| macOS | ARM64 / x64 | LaunchDaemon（root）/ LaunchAgent（user） | ✅ 支持 |
| Linux | x64 / ARM64 | systemd system / systemd --user | system service 模式支持 |
| Windows | x64 | Windows Service / per-user 进程 | system service 模式支持 |

---

## 故障排查

### 常见错误与解决方案

#### `refusing to use config path ... expected config.yaml`

**原因**：daemon 版本与 CLI 版本不匹配，或 daemon 未重启加载新代码。

**解决**：
```bash
sudo systemctl restart mihomo
# 或
mihomo-cli restart
```

#### `owner uid 0 does not match IPC peer uid 1000`

**原因**：配置文件被 root 写入，所有者变成 root，普通用户无法访问。

**解决**：
```bash
sudo chown $(whoami):$(id -gn) ~/.config/mihomo/config.yaml
```

#### `connection refused` / `No such file or directory` (socket)

**原因**：daemon 或 Core 未运行，或 API 尚未 ready。

**解决**：
```bash
mihomo-cli restart
```

#### `cannot read config ... No such file or directory`

**原因**：配置文件不存在，通常是没有添加订阅。

**解决**：
```bash
mihomo-cli config -u '<subscription-url>'
```

#### `Operation not permitted` (TUN)

**原因**：TUN 需要 root 权限。

**解决**：
```bash
# CLI 会在 TUN 操作边界内自动请求必要的系统授权
mihomo-cli tun on
```

### 诊断步骤

1. **检查状态**：
   ```bash
   mihomo-cli status
   ```

2. **查看详细日志**：
   ```bash
   mihomo-cli logs tail
   # 或
   journalctl -u mihomo -n 50  # system 模式
   ```

3. **验证配置**：
   ```bash
   mihomo-cli config --validate
   ```

4. **检查文件权限**：
   ```bash
   ls -la ~/.config/mihomo/
   ```

### 重置状态

如果问题持续，可以尝试重置：

```bash
# 停止服务
mihomo-cli stop

# 清理并重新安装
mihomo-cli uninstall --all --yes
mihomo-cli install --system --yes

# 重新添加订阅
mihomo-cli config -u '<subscription-url>' --activate --yes

# 显式启动并等待 readiness
mihomo-cli restart --system
```

---

## 术语

- **ISP**：互联网服务提供商，直连路径出口通常是 ISP 分配的公网 IP
- **TUN**：虚拟网卡模式，将系统所有流量路由通过代理
- **规则**：用户自定义路由策略，决定哪些流量走代理、哪些走直连
- **Socket**：Unix domain socket，mihomo-cli 与 mihomo 核心的通信方式（不走 HTTP 端口）
- **UA 协商**：User-Agent 协商，某些订阅服务器根据 UA 返回不同格式的配置
