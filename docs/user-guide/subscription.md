# 订阅、配置、规则与备份

> 读者：使用者
> 回答：如何管理订阅与配置、自定义路由规则、DNS 策略、日志与备份
> 权威：本文为用户手册一部分

## `config` (alias: `c`)

多订阅语义：

- `config --refresh` 访问远端 URL，刷新当前 active subscription 的本地缓存，并重新生成/校验 `config.yaml`。
- `config --refresh-all` 刷新所有已保存订阅缓存，但最终 `config.yaml` 仍由 active subscription 生成。
- `config --switch <id>` 切换 active subscription，并重新生成/校验 `config.yaml`；切换使用本地订阅缓存，通常不需要访问远端 URL。
- 对普通、非 TUN-active 的实例，配置变更在生成并校验 `config.yaml` 后，只有受管 reload/restart 成功且当前 Core/API 目标 revision 得到观察，才能报告运行时已应用；reload 需要等待 daemon/Core API 时必须先释放配置锁。Core 停止时合法配置可落盘并返回 `pending`，提示执行 `mihomo-cli restart`。
- system TUN active 时，完整的 `apply_active_intent` dispatcher（candidate/revision、snapshot promotion、`CoreApplied`、compare-and-commit、`IntentCommitted`）仍是目标合同，尚未由所有 config/rule/DNS/override/TUI 入口统一实现和验收；当前实现会阻止 TUN-active 的通用 `/configs` 旁路，不能把该 gate 视为完整事务保证。
- 配置写入/生成成功不一定表示运行中的 mihomo core 已经使用新配置；如结果未明确报告 `runtime_applied`，应按返回的 `pending`/`unknown`/`RecoveryRequired` 状态处理，不得仅凭文件写入成功推断已应用。
- `rules.yaml`、`dns-policy.yaml`、`dns-fake-ip-filter.yaml`、`override.yaml` 会叠加到 active subscription 之上。
- 配置重生成需要读取上一份 `config.yaml` 以携带 TUN 控制键。读取失败（权限或 IO 错误）时命令直接失败、`config.yaml` 保持原样，不会按「没有 TUN 意图」重新生成覆盖；只有文件不存在（首次生成）才按无历史意图处理。

管理订阅和配置。无参数时启动交互式 TUI 界面。

```bash
# 配置管理（当前实现仍使用 flat action flags；subcommand-only 设计见 ../spec/config-cli.md，尚未作为现行接口发布）
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

## `autostart`

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

## `logs`

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

## `rule`

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

## `dns`

管理 DNS 路由策略（nameserver-policy）。dns 命令自动检测当前实例；`--system` 仅用于脚本/排障时显式指定 system service context。DNS 配置读写使用 resolved `AppPaths`。

```bash
# DNS 策略管理
mihomo-cli dns policy add internal.example.com 192.0.2.53           # 添加策略
```

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

> 如果目标是「内网 IP 直接走系统路由、公网走代理」，除了域名规则，还要在 `tun.route-exclude-address` 排除实际内网网段；完整做法与验证见 [TUN 透明代理](tun.md) 的「内网直连 + 外网代理（split tunneling）」。

```bash
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

## `backup` / `restore`

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
