# mihomo-cli 用户手册

> 读者：使用者
> 回答：本手册有哪些主题、按什么场景选哪一篇
> 权威：本文为用户手册一部分

手册按场景拆成独立主题页，每页只回答一个问题；命令的完整说明在各主题页正文，本索引只负责指路。

## 按场景选入口

| 我想…… | 读这里 |
|---|---|
| 从零装好并跑通 | [quickstart.md](quickstart.md) |
| 让脚本 / AI 稳定解析输出 | [json.md](json.md) |
| 换机器、重装、卸载、启停服务、确认装在哪 | [install.md](install.md) |
| 升级 CLI 或服务二进制、搞清什么时候要 sudo | [upgrade.md](upgrade.md) |
| 加订阅、改配置、加规则、调 DNS、看日志、备份恢复 | [subscription.md](subscription.md) |
| 选节点、测延迟、查连接与出口 IP、设 shell/系统代理 | [nodes.md](nodes.md) |
| 开 / 关 TUN、彻底恢复直连 | [tun.md](tun.md) |
| 用 `override.yaml` 做高级配置覆盖 | [override.md](override.md) |
| 报错、卡住、想从头诊断或重置 | [troubleshooting.md](troubleshooting.md) |
| 只知道命令名，想找它写在哪一页 | [commands.md](commands.md) |
| 术语看不懂 | [glossary.md](glossary.md) |

推荐首读顺序：`quickstart` → `install` → `subscription` → `nodes` → `tun`；`json` / `override` / `troubleshooting` / `glossary` / `commands` 按需查阅。

## 主题清单（供 `mihomo-cli manual` 解析）

<!-- manual-topics -->
| slug | 标题 | 文件 |
|---|---|---|
| index | mihomo-cli 用户手册 | README.md |
| quickstart | 快速开始 | quickstart.md |
| json | AI/脚本 JSON 输出（第一阶段） | json.md |
| install | 安装、升级与服务控制 | install.md |
| upgrade | 升级 mihomo-cli | upgrade.md |
| subscription | 订阅、配置、规则与备份 | subscription.md |
| nodes | 节点、连接与出口 | nodes.md |
| tun | TUN 透明代理 | tun.md |
| override | 高级配置（override.yaml） | override.md |
| troubleshooting | 平台说明与故障排查 | troubleshooting.md |
| commands | 命令索引 | commands.md |
| glossary | 术语 | glossary.md |
<!-- /manual-topics -->
