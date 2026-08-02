# mihomo-cli 使用手册

> 完整命令参考、选项说明与使用示例

---

## 快速开始

```bash
# 1. 安装 mihomo 核心 + 系统服务（自动下载，无需手动装依赖）
mihomo-cli install

# 2. 启动服务（默认 TUN 关闭，不影响当前网络）
mihomo-cli start

# 3. 添加订阅源（支持 vmess://、base64、Clash YAML，自动格式转换）
mihomo-cli config -u '<your-subscription-url>'

# 4. j/k 导航 + / 过滤选择节点（crossterm TUI）
mihomo-cli select

# 5. 开启 TUN 透明代理
mihomo-cli tun on

# ✅ 完成 — 所有流量自动通过代理
```

---

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

下载 mihomo 核心 + 生成配置 + 安装启动项；不带 flags 时交互选择“普通代理模式”或“TUN 全机模式”。日常 TUN 用户也可以直接从 `mihomo-cli tun on` 开始，CLI 会在需要时引导安装 system service。

```bash
mihomo-cli install              # 交互式选择：普通代理模式 / TUN 全机模式
mihomo-cli install --user       # 非交互式安装普通代理模式（无需 sudo）
mihomo-cli install --system     # 非交互式安装 system service（高级/脚本/排障）
mihomo-cli install --force      # 强制重新安装
mihomo-cli install --github-mirror https://ghproxy.com/  # geo 下载走 GitHub 镜像（大陆网络加速）
```

**安装步骤：**

1. 下载 mihomo 核心二进制（system: `/usr/local/lib/mihomo/mihomo`；user: `~/.local/bin/mihomo`）
2. system 模式额外安装 `mihomo-cli daemon` 到稳定路径
3. 配置订阅链接（交互式输入）并生成当前用户的 `~/.config/mihomo/config.yaml`
4. 安装服务（systemd/LaunchDaemon 或 user service）

> 💡 `--github-mirror`：geo 数据（geoip/geosite）下载的 GitHub 镜像前缀。国内网络直连 GitHub 慢时可指定镜像站（如 `https://ghproxy.com/`）；仅影响 geo 下载，订阅下载不走此镜像。

---

#### `config` (alias: `c`)

管理订阅和配置。无参数时启动交互式 TUI 界面。

```bash
# 交互式 TUI（键盘快捷键操作）
mihomo-cli config               # 启动订阅管理 TUI

# 快速设置/更新订阅 URL
mihomo-cli config -u '<url>'

# 多订阅管理
mihomo-cli config --add '<url>'           # 添加订阅源
mihomo-cli config --remove <ID>           # 删除订阅源
mihomo-cli config --list                  # 列出所有订阅源
mihomo-cli config --switch <ID>           # 切换到指定订阅
mihomo-cli config --info                  # 查看当前活跃订阅信息
mihomo-cli config --info <ID>             # 查看指定订阅信息

# 刷新订阅
mihomo-cli config --refresh               # 刷新活跃订阅
mihomo-cli config --refresh-all           # 刷新所有订阅

# UA 探测与设置
mihomo-cli config --probe '<url>'         # 探测候选 UA 返回格式（不写文件）
mihomo-cli config --set-ua <ID> auto      # 恢复订阅自动 UA 协商
mihomo-cli config --set-ua <ID> "clash-verge/v2.0.4"  # 固定订阅 UA
mihomo-cli config --user-agent "clash/v1.0.0"          # 本次 add/refresh 操作临时使用指定 UA（别名 --ua）

# 配置验证与修复
mihomo-cli config --validate              # 验证 config.yaml 语法（YAML + mihomo -t）
mihomo-cli config --dry-run               # 预览操作，不实际写入或重启
mihomo-cli config --fix                   # 修复配置：添加 Unix socket controller
mihomo-cli config --import <file>         # 从本地文件导入配置
mihomo-cli config -y                      # 跳过确认提示
```

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

检查 GitHub 上最新 mihomo core 版本，并交互式升级。

```bash
mihomo-cli upgrade
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

启动 mihomo 服务。v3 自动检测当前实例模式（system service / per-user），无需指定模式。

```bash
mihomo-cli start                # 启动当前实例（自动检测模式）
mihomo-cli start --system       # 强制 system service 实例（排障用）
```

启动后自动检查 socket 可用性和 API 响应。如果配置缺少 controller，会自动修复并重启。

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

查看运行状态概览（含代理探测出口）。

```bash
mihomo-cli status               # 简要状态
mihomo-cli status -v            # 详细诊断（含日志尾部）
```

**输出内容：**

- 运行模式（Rule/Global/Direct）
- TUN 状态
- 当前节点
- 代理探测出口 + 归属地
- 异常时自动诊断建议

---

### 日常使用

#### `select`

选择节点——**无 `--node` 时进入 crossterm TUI**（j/k vim 快捷键 + / 过滤），**有 `--node` 时非交互 CLI 直接切换**。

```bash
mihomo-cli select                                  # 平铺 当前实例所有代理组（TUI）
mihomo-cli select --group 节点选择                  # 限定 当前实例某个代理组（TUI）
mihomo-cli select -g 节点选择 --node 韩国KR-HY2     # 非交互：切换 节点选择 组到指定节点
```

> 💡 `--node` 用于脚本/CI 中确定性切换节点；`--node` 必须配合 `--group` 使用。

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

列出所有代理组及当前节点。

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

#### `tun`

开关 TUN 透明代理模式。

```bash
mihomo-cli tun on                  # 开启 system service TUN（per-user 模式会失败并提示）
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

> ⚠️ TUN 模式需要 system service。日常直接运行 `mihomo-cli tun on`；如果尚未安装 system service，CLI 会交互引导安装并请求一次管理员密码。system-proxy 在 TUN 已开启时通常无效，CLI 会给出 warning。

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

输出 shell 代理环境变量（配合 `eval` 使用）。

```bash
eval "$(mihomo-cli proxy on)"          # 设置 http_proxy / https_proxy（自动检测实例）
eval "$(mihomo-cli proxy off)"         # 取消代理
```

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
3. 每次启动/重载时自动合并到该实例的 `config.yaml`
4. 当前实例规则变更后自动尝试热重载；system 写操作等待 privileged ConfigStore

---

#### `dns`

管理 DNS 路由策略（nameserver-policy）。dns 命令自动检测当前实例；`--system` 仅用于脚本/排障时显式指定 system service context。DNS 配置读写使用 resolved `AppPaths`。

```bash
# DNS 策略管理
mihomo-cli dns policy add ubtrobot.com 10.10.1.251           # 添加策略
mihomo-cli dns policy add corp.com 10.10.1.251,10.10.1.120   # 多个 DNS 服务器
mihomo-cli dns policy list                                  # 列出当前实例策略 (alias: ls)
mihomo-cli dns policy remove 1                              # 按索引删除 (alias: rm)
mihomo-cli dns policy remove ubtrobot.com                   # 按域名删除

# DNS 状态
mihomo-cli dns status                                       # 查看当前实例 DNS 配置

# DNS 模板
mihomo-cli dns template list                                       # 列出可用模板
mihomo-cli dns template apply company --domain corp.example.com --target 10.10.1.251
mihomo-cli dns template apply ads                           # 应用广告过滤模板
```

**内置模板：**

| 模板 | 说明 |
|------|------|
| `company` | 将内部域名路由到公司 DNS 服务器 |
| `ads` | 将常见广告/追踪域名路由到过滤 DNS |

---

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

`~/.config/mihomo/override.yaml` 支持任意字段覆盖，在订阅内容之后、用户规则之前合并。可直接编辑文件，也可用 `override` 命令管理；命令默认自动检测当前实例，`--system` 仅用于脚本/排障：

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

**合并顺序：** 订阅内容 → `override.yaml` → 用户规则 → DNS 策略 → controller 注入

**合并语义：**
- YAML map 递归合并
- list 和 scalar 直接替换
- runtime controller 字段会在 override 后重新注入

---

## 平台说明

| 平台 | 架构 | 开机自启方式 | TUN 模式 |
|------|------|-------------|----------|
| macOS | ARM64 / x64 | LaunchDaemon（root）/ LaunchAgent（user） | ✅ 支持 |
| Linux | x64 / ARM64 | systemd system / systemd --user | system service 模式支持 |
| Windows | x64 | Windows Service / per-user 进程 | system service 模式支持 |

---

## 术语

- **ISP**：互联网服务提供商，直连路径出口通常是 ISP 分配的公网 IP
- **TUN**：虚拟网卡模式，将系统所有流量路由通过代理
- **规则**：用户自定义路由策略，决定哪些流量走代理、哪些走直连
- **Socket**：Unix domain socket，mihomo-cli 与 mihomo 核心的通信方式（不走 HTTP 端口）
- **UA 协商**：User-Agent 协商，某些订阅服务器根据 UA 返回不同格式的配置
