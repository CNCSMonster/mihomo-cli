# 平台说明与故障排查

> 读者：使用者
> 回答：三平台差异、常见错误、诊断步骤与重置流程
> 权威：本文为用户手册一部分

## 平台说明

| 平台 | 架构 | 开机自启方式 | TUN 模式 |
|------|------|-------------|----------|
| macOS | ARM64 / x64 | LaunchDaemon（root）/ LaunchAgent（user） | ✅ 支持 |
| Linux | x64 / ARM64 | systemd system / systemd --user | system service 模式支持 |
| Windows | x64 | Windows Service / per-user 进程 | system service 模式支持 |

---

## 故障排查

### 常见错误与解决方案

#### 迁移后的代际混合态或代际状态无法判定

默认 `status` 和 `doctor` 不提权；权限不足不代表没有 pending，也不证明迁移成功。需要进一步观察时，显式运行：

```bash
mihomo-cli doctor --system --privileged
```

该命令在需要时请求一次管理员授权，仅检查槽位、信任状态、当前匹配及采纳需求；无法观察仍报告 unknown，不进行迁移、修复或服务停启。若确认属于非空全 untrusted 迁移现场且没有可信绑定，可由管理员按[迁移后显式采纳当前安装](install.md#迁移后显式采纳当前安装)审阅并决定是否建立新信任；有 pending 时必须在采纳命令中提供精确 ID。采纳不是历史非混合、来源可靠或健康的证明。存在可信代际的混合态不得采纳，保留现场并核实安装文件；不要手工删除槽位或标记。

#### 内网资产安全告警（Intranet Guard）

`mihomo-cli config --validate` 或 `doctor` 可能在以下拓扑给出 `⚠ [Intranet Guard]`：启用了 `sniffer.override-destination`，兜底 `MATCH` 指向代理，但 RFC1918 私网没有被 DIRECT 的 `IP-CIDR` 规则或 `tun.route-exclude-address` 完整覆盖。此时内网流量被嗅探后可能滑向代理节点并被中断。

推荐在 `override.yaml` 中显式排除三个私网段（YAML map 递归合并，list 会替换生成值）：

```yaml
tun:
  route-exclude-address:
    - 10.0.0.0/8
    - 172.16.0.0/12
    - 192.168.0.0/16
```

或在 `rules.yaml` 中添加对应的 `IP-CIDR,...,DIRECT` 规则。检查按 CIDR 区间并集判断覆盖：只写 `10.0.0.0/9` 仍是部分覆盖；`172.64.0.0/13` 等非 RFC1918 的 `172` 段不能替代 `172.16.0.0/12`。该告警只在 `tun.enable: true` 时触发；未启用 TUN 不会告警。它只识别字面 `IP-CIDR`/`GEOIP` 直连规则，`RULE-SET` 等间接私网规则不会被自动识别，需要人工确认。该告警只说明静态拓扑风险，不代表内网数据面已经验证可用。

#### `refusing to use config path ... expected config.yaml`

**原因**：daemon 版本与 CLI 版本不匹配（常见于升级二进制后 daemon 未加载新代码）。

**解决**：
```bash
mihomo-cli restart --system
```

该命令会自动检测并同步服务二进制（见 [升级 mihomo-cli](upgrade.md)），随后重启 daemon、恢复状态并回放选点。执行后用 `mihomo-cli version` 与 `mihomo-cli status` 验证。

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

### `doctor` 的 DNS / Split-DNS 判定

`doctor` 只有在**探测域本身命中 `dns.nameserver-policy`**，或**当前系统 resolver 地址位于 `tun.route-exclude-address` 内**时，才把系统 DNS 绕过显示为中性（Split-DNS/直连解析）。仅配置了无关的内网域名策略，或只排除了其他内网网段，仍会报 ❌，因为探测域 `example.com` 没有被该策略覆盖。

排查时确认：`nameserver-policy` 的键是否覆盖探测域（如 `+.example.com`），以及 `/etc/resolv.conf` 中的 resolver 地址是否落在 `route-exclude-address` 的 CIDR 内。该判定只说明配置与探测证据匹配，不证明所有内网解析或数据面已经可用。

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
