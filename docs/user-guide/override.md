# 高级配置

> 读者：使用者
> 回答：如何用 override.yaml 做高级配置覆盖及其合并语义
> 权威：本文为用户手册一部分

## override.yaml

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
- list 和 scalar 直接替换（`route-exclude-address` 等数组键按整表替换，不追加；`[]` 表示显式清空）
- runtime controller 字段由 `mihomo-cli` 管理，会在 override 后重新注入；不要通过 override 修改 `external-controller*`、API socket/pipe 或 controller secret
- **TUN 控制键**（`tun.enable` / `stack` / `dns-hijack` / `strict-route` / `auto-route` / `auto-detect-interface`）由 `tun on/off` 事务独占：写进 override 会被拒绝并显式告警（override 不能开关 TUN，包括 `dns-hijack` 数组）；`mtu` / `device` / `route-exclude-address` 等非控制键照常生效，并在 `tun on` 或 `restart --system` 时按 intent/override 合并进运行快照
- `tun` 必须是 YAML 映射：`tun: null` / 标量 / 数组会在写入前被拒绝（否则会整块顶掉已有 TUN 块）；`tun: {}` 不算删除
- 存在未终结的 TUN 事务（journal 非 `IntentCommitted`/`RolledBack`，或遗留事务待恢复）时，`override import/clear` 会在**写入前**拒绝，配置保持不变；先 `mihomo-cli status`，必要时 `mihomo-cli restart --system`

## 自定义出站节点与代理提供者

### 1. `proxies-append`（推荐：一等出站节点）

当需要把本地隧道（如 SSH 动态端口转发 `127.0.0.1:11080`）、私有 SOCKS5/HTTP 节点或自建 VPS 节点纳入分流时，在 `override.yaml` 中使用 `proxies-append`：

```yaml
proxies-append:
  - name: SG-provider-a
    type: socks5
    server: 127.0.0.1
    port: 11080
    udp: false
```

- **语义**：追加到订阅自带的 `proxies` 之后，不覆盖订阅原生节点；
- **一等公民能力**：
  - 出现在 `mihomo-cli list` 节点列表中；
  - 可被 `mihomo-cli group add --group <组名> --member SG-provider-a` 加入任意代理组；
  - 可被 `mihomo-cli rule add "DOMAIN-SUFFIX,apple.com,SG-provider-a"` 直接作为分流目标；
  - 可被 `mihomo-cli select -g <组名> --node SG-provider-a` 选中并持久化；
  - 订阅刷新（`config --refresh`）或配置重新合成后不丢失。

> ⚠️ 注意：直接在 `override.yaml` 中书写 `proxies:` 会触发 YAML 列表整体替换，覆盖订阅原生节点导致订阅组因缺少成员校验失败；追加单个或多个自建节点请始终使用 `proxies-append:`。

### 2. `proxy-providers`（代理集合提供者）

当需要批量引入外部节点集合时，可使用 `proxy-providers`。在 system 模式下推荐使用 `type: inline` 免落盘声明：

```yaml
proxy-providers:
  provider-a:
    type: inline
    payload:
      - name: SG-provider-a
        type: socks5
        server: 127.0.0.1
        port: 11080
        udp: false
```

- **引用方式**：Core 规定 provider 内部节点不属于顶级静态节点，不能直接写入代理组的 `proxies:` 列表或作为规则目标；须通过代理组的 `use:` 字段引用 provider：
  ```bash
  mihomo-cli group add --group Apple服务 --use provider-a
  ```
- **文件路径安全约束**：若在 system service 模式下使用 `type: file`，Mihomo Core 要求文件路径必须位于 Core home（`/var/lib/mihomo-cli`）下且对 `mihomo` 用户可读；普通用户目录（如 `/home/user/...`）会被 Core SAFE_PATHS 校验拦截。无特权环境请优先使用 `type: inline`。
