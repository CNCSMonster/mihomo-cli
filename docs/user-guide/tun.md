# TUN 透明代理

> 读者：使用者
> 回答：如何开关 TUN、TUN 前置与证明边界、如何彻底恢复直连
> 权威：本文为用户手册一部分

## `tun`

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

## 内网直连 + 外网代理（split tunneling）

适合同时有内网服务、云元数据/内网镜像和公网代理的机器。目标：

- 访问内网 IP 走系统主路由，不进入 mihomo；
- 访问公网继续走 TUN/代理；
- 内网域名仍由内网 DNS 解析，公网域名按需走 mihomo DNS。

### 1. 排除内网网段

在 intent `config.yaml` 的 `tun` 块中追加 `route-exclude-address`。**该键是整表替换**，要保留已有项，只追加自己的内网网段：

> `tun on` 会先把 `override.yaml` 合并进 intent，再按键级策略派生 `/var/lib/mihomo-cli/tun-config.yaml`；`restart --system` 也会做同样的合并。非控制键以 intent/override 为准，控制键仍由 `tun on/off` 事务独占。非控制键按 intent 整表取值：要从快照中删除某个键，必须在 intent 中删除后执行 `restart --system`；`[]` 表示显式清空。不要手工改 live snapshot 作为长期配置。

```yaml
tun:
  route-exclude-address:
    - 224.0.0.251/32
    - 224.0.0.252/32
    - 224.0.0.1/32
    - 239.255.255.250/32
    # 文档示例地址，按实际内网替换；不要直接排除整个 RFC1918
    - 192.0.2.0/24
    # 云元数据/链路本地；只需元数据时可收窄为 169.254.169.254/32
    - 169.254.169.254/32
    # 如有内网 IPv6
    - fe80::/10
```

应用：

```bash
mihomo-cli tun on --system --yes     # system 实例；会归一化并应用 TUN 意图
# 或修改后由 tun on 重建；不要只手工 ip route，重启后会丢失
```

被排除的 IP 会**完全绕过 mihomo**（不经过规则、日志和选点），这是内网直连的预期行为；如果希望内网流量仍经过 mihomo 规则，应改用 `IP-CIDR,...,DIRECT` 分流规则，而不是 `route-exclude-address`。

### 2. DNS：内网域名与公网域名

- **默认 systemd-resolved / dnsmasq 使用内网 DNS**：内网域名能解析到内网地址，公网查询也直接由内网 DNS 返回。这样内网直连最稳，但 目标主机本机 TUN 透明流量的 `DOMAIN`/`GEOSITE` 规则拿不到域名（应用显式走 `127.0.0.1:7890` 时不受影响）。
- 需要本机透明流量也按域名分流时，做 split-DNS：把系统 DNS 指到 mihomo 的 `127.0.0.1:1053`，再把内网域名指回内网 DNS 并禁用 fake-ip：

```bash
mihomo-cli dns fake-ip-filter add corp.example.com
mihomo-cli dns policy add corp.example.com 192.0.2.251,192.0.2.120
mihomo-cli restart --system
```

内网域名清单和 DNS 地址必须按你的环境替换；不要把未知内网域直接交给公网 DNS。

### 3. 应用层也要绕过代理

即使路由已直连，使用 HTTP(S) 代理的应用仍可能把内网请求发给代理。给这些应用设置 `NO_PROXY`：

```bash
export NO_PROXY=localhost,127.0.0.1,192.0.2.0/24,169.254.169.254,.corp.example.com
export no_proxy="$NO_PROXY"
```

### 4. 验证

```bash
ip route get 192.0.2.10              # 期望走 eno1，不是 Meta
getent hosts corp.example.com         # 期望内网地址
curl -sS -m 8 -x http://127.0.0.1:7890 https://api.ipify.org   # 公网走代理
curl -sS -m 8 --noproxy '*' http://internal.example.com                 # 内网直连
```

### 5. 边界

- 不要把 `0.0.0.0/0` 或整个 RFC1918 一股脑排除；范围越大，越多本该代理的流量被放直。
- 有 IPv6 时同时处理 ULA/link-local，否则内网 IPv6 仍可能被 TUN 接管。
- `strict-route` 与 `route-exclude-address` 修改后必须重新应用 TUN 意图并复查路由；不要依赖临时 `ip route`。

---

## 恢复直连 / 关闭 mihomo-cli 的影响

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
