# 节点、连接与出口

> 读者：使用者
> 回答：如何选节点、列组、测延迟、查连接与出口 IP、设置 shell/系统代理
> 权威：本文为用户手册一部分

## `select`

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

## `group`

管理代理组覆盖（自建组、成员补丁与提供者引用）。所有修改基于当前 active 订阅，订阅更新时不丢失增量配置。

```bash
mihomo-cli group list                              # 列出所有代理组及其来源（native/custom/patched）
mihomo-cli group show Apple服务                    # 查看组定义与当前生效 patch
mihomo-cli group create 自建分流 --type select --member 香港01 --use provider-a  # 自建组
mihomo-cli group add Apple服务 --member SG-provider-a   # 向已有组追加节点成员
mihomo-cli group add Apple服务 --use provider-a        # 向已有组追加 proxy-provider 引用
mihomo-cli group remove Apple服务 --member 香港02    # 从组中移除节点成员
mihomo-cli group remove Apple服务 --use provider-a     # 从组中解绑 provider 引用
mihomo-cli group delete 自建分流                   # 删除自建组或隐藏原生组
mihomo-cli group reset Apple服务                   # 恢复原生组到订阅默认定义
```

---

## `list`

列出所有代理组及当前节点；若存在持久选择，会同时显示 `[pinned: ...]`，当持久选择尚未应用时显示 `[pinned: ..., not applied]`。

```bash
mihomo-cli list                   # 查看 当前实例所有代理组
```

---

## `delay`

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

## `conn`

查看**瞬时活跃**连接。

```bash
mihomo-cli conn         # 查看 当前实例连接列表
mihomo-cli conn --flush # 关闭 当前实例所有连接
```

> ⚠️ `conn` 只显示**当前正在传输**的连接——请求一旦完成连接即关闭并从列表消失。
> 要查看**已经发生的**请求匹配了什么规则/走了什么策略（含已关闭连接），
> 用 `mihomo-cli logs`（内核日志逐条记录 `match ... using ...`），而不是 `conn`。

---

## `exit-ip`

查询节点、代理组、URL 路由或直连路径的出口 IP。该命令要求显式选择一个目标模式，不会默认猜测代理组。

```bash
mihomo-cli exit-ip --node "Korea 01"              # 查询具体节点出口 IP
mihomo-cli exit-ip --group "节点选择"             # 查询代理组当前有效出口 IP
mihomo-cli exit-ip --url https://github.com       # 查询 URL 按规则会走到的出口估算
mihomo-cli exit-ip --direct                       # 查询系统直连出口 IP
```

目标模式互斥：`--node` / `--group` / `--url` / `--direct` 一次只能使用一个。

`--url` 会先按当前规则解析 URL/host 会走到哪个 policy/node，再通过 IP echo 服务估算该节点出口 IP。它不承诺目标网站本身观测到的源 IP 一定相同。

---

## `ip`（兼容/弱化）

`mihomo-cli ip` 保留为兼容命令，只表示“通过当前 mihomo 规则访问 IP 查询服务时的观测出口”。它不代表系统直连 IP、TUN 状态、某个指定节点出口，或任意 URL 的真实出口。新用法建议改用 `exit-ip`。

```bash
mihomo-cli ip            # 兼容：当前 mihomo 访问 IP echo 服务的出口
```

---

## `proxy`

输出 shell 代理环境变量（配合 `eval` 使用）。这是 terminal-only proxy helper，不是 service lifecycle：`proxy on/off` 不会启动或停止 mihomo；启动/停止服务请使用 `restart` / `stop`。

```bash
mihomo-cli restart                       # 先显式启动并等待 mihomo readiness
eval "$(mihomo-cli proxy on)"          # 将 http_proxy / https_proxy 应用到当前 shell
curl https://example.com               # 当前 shell 中的命令走代理
eval "$(mihomo-cli proxy off)"         # 清理当前 shell 的代理环境变量
```

不提供 `proxy exec`；如只想临时代理单条命令，可由用户自行设置 `HTTP_PROXY` / `HTTPS_PROXY` 环境变量。无 root/admin 或不希望影响全机网络时可使用本方式；默认推荐路径仍是 TUN。

---

## `system-proxy`

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
