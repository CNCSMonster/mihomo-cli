# 命令索引

> 读者：使用者
> 回答：全部子命令一行一条及其对应主题
> 权威：本文为用户手册一部分

本页只做索引：每行给出命令、用途和详细说明所在的主题页（相对链接），不复制各命令的选项与示例。命令名与 `mihomo-cli --help` 的 clap 命令树一致。

全局参数：`-v, --verbose`（调试输出）、`--json`（stdout 只输出机器可读 envelope，见 [json.md](json.md)）。

## 安装与服务生命周期

| 命令 | 用途 | 主题 |
|---|---|---|
| `mihomo-cli install`（别名 `i`） | 下载/校验 Core 与 Geo、安装服务；`--system --adopt-current` 显式采纳迁移现场 | [install.md](install.md) |
| `mihomo-cli uninstall`（别名 `u`） | 停止并卸载服务；`--all` 连带删除二进制/配置/Geo | [install.md](install.md) |
| `mihomo-cli update`（别名 `up`） | 更新 mihomo Core 二进制 | [install.md](install.md) |
| `mihomo-cli upgrade` | 检查并升级 mihomo Core | [install.md](install.md) |
| `mihomo-cli version` | 显示 CLI 构建信息与 Core 版本 | [install.md](install.md) |
| `mihomo-cli start` | 启动当前实例（自动检测 system/user） | [install.md](install.md) |
| `mihomo-cli stop` | 停止 mihomo core/service | [install.md](install.md) |
| `mihomo-cli restart` | 重启并等待 readiness（普通用户主旅程入口） | [install.md](install.md) |
| `mihomo-cli status` | 只读状态概览；`-v` 输出详细本地诊断 | [install.md](install.md) |
| `mihomo-cli dashboard`（别名 `dash`） | 实时状态 TUI | [install.md](install.md) |
| `mihomo-cli use [system\|user\|auto\|status]` | 设定或查看首选实例模式 | [install.md](install.md) |
| `mihomo-cli daemon` | 服务守护进程入口（隐藏命令，由 systemd/launchd/SCM 调用） | [install.md](install.md) |
| `mihomo-cli autostart on\|off\|status` | 开机自启开关与查询 | [subscription.md](subscription.md) |

## 订阅与配置

| 命令 | 用途 | 主题 |
|---|---|---|
| `mihomo-cli config`（别名 `c`） | 无参数进 TUI；带 `-u` / `--add` / `--remove` / `--list` / `--switch` / `--info` / `--refresh` / `--refresh-all` / `--probe` / `--set-ua` / `--validate` / `--fix` / `--import` 走 flat action flags | [subscription.md](subscription.md) |
| `mihomo-cli config fetch <url> -o <file>` | 离线把订阅下载/转换为 Clash YAML，不改本机状态 | [subscription.md](subscription.md) |
| `mihomo-cli config validate` | 校验当前 `config.yaml`（等价 `config --validate`） | [subscription.md](subscription.md) |
| `mihomo-cli override path\|show\|import\|clear` | 管理 `override.yaml` 高级覆盖 | [override.md](override.md) |
| `mihomo-cli backup [dir]` | 备份当前实例的配置文件 | [subscription.md](subscription.md) |
| `mihomo-cli restore <dir>` | 从备份恢复配置文件 | [subscription.md](subscription.md) |

## 节点、连接与代理

| 命令 | 用途 | 主题 |
|---|---|---|
| `mihomo-cli select` | 选节点：无 `--node` 进 TUI，有 `--node` 非交互切换；`--unpin` 清除持久选择 | [nodes.md](nodes.md) |
| `mihomo-cli group list\|show\|create\|edit\|add\|remove\|delete\|reset` | 管理代理组覆盖（增删成员、引用 provider、隐藏/重建组） | [nodes.md](nodes.md) |
| `mihomo-cli list` | 列出代理组与当前节点（含 `[pinned: …]`） | [nodes.md](nodes.md) |
| `mihomo-cli delay` | 测组内节点延迟；`--refresh` / `--cache-ttl` / `--fastest` | [nodes.md](nodes.md) |
| `mihomo-cli conn` | 查看瞬时活跃连接；`--flush` 全部关闭 | [nodes.md](nodes.md) |
| `mihomo-cli exit-ip` | 出口 IP 探测：`--node` / `--group` / `--url` / `--direct` 四选一 | [nodes.md](nodes.md) |
| `mihomo-cli ip` | 兼容命令（弱化语义，新用法请改用 `exit-ip`） | [nodes.md](nodes.md) |
| `mihomo-cli proxy on\|off` | 输出 shell 代理环境变量（配合 `eval`） | [nodes.md](nodes.md) |
| `mihomo-cli system-proxy on\|off` | 设置/取消操作系统级系统代理 | [nodes.md](nodes.md) |
| `mihomo-cli tun on\|off\|status` | 开关 TUN 与查询 TUN 运行态 | [tun.md](tun.md) |

## 规则、DNS 与日志

| 命令 | 用途 | 主题 |
|---|---|---|
| `mihomo-cli rule add\|list\|remove\|move\|clear\|import\|export\|position\|types\|policies\|test` | 用户自定义路由规则管理 | [subscription.md](subscription.md) |
| `mihomo-cli dns policy add\|list\|remove` | DNS 路由策略（nameserver-policy） | [subscription.md](subscription.md) |
| `mihomo-cli dns fake-ip-filter add\|list\|remove` | `dns.fake-ip-filter` 源文件管理 | [subscription.md](subscription.md) |
| `mihomo-cli dns status` | 查看当前 DNS 配置 | [subscription.md](subscription.md) |
| `mihomo-cli dns template list\|apply` | 列出/应用内置 DNS 模板 | [subscription.md](subscription.md) |
| `mihomo-cli logs` | 查看 Core 日志；`--tail` / `--level` / `-f` | [subscription.md](subscription.md) |

## 诊断

| 命令 | 用途 | 主题 |
|---|---|---|
| `mihomo-cli doctor` | 只读诊断；`--system --privileged` 显式授权检查受保护代际（不自动修复） | [troubleshooting.md](troubleshooting.md) |
| `mihomo-cli status -v` | 详细本地诊断（不触网、不提权、不写入） | [install.md](install.md) |

## 手册

| 命令 | 用途 | 主题 |
|---|---|---|
| `mihomo-cli manual [topic]` | 输出内嵌用户手册：无参数为索引，带 `<topic>` 为该主题页；`--list` 列 slug/标题，`--json` 走 envelope；别名 `mihomo-cli doc` | [README.md](README.md) |
