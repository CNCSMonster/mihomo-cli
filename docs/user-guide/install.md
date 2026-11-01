# 安装、升级与服务控制

> 读者：使用者
> 回答：mihomo-cli 装在哪、如何安装/卸载/更新、如何启停服务
> 权威：本文为用户手册一部分

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

### 两层二进制布局（用户工具 vs 服务二进制）

mihomo-cli 存在两份二进制，各有职责：

| 位置 | 角色 | 谁在用 | 更新方式 |
|---|---|---|---|
| `~/.cargo/bin/mihomo-cli` | **用户工具**（标准安装位置，你在终端运行的那个） | 你 | `cargo install` / 直接覆盖 |
| `/usr/local/bin/mihomo-cli`（Linux system 模式） | **服务二进制** | systemd 服务 `mihomo.service`（`ExecStart=... daemon`，以服务账号 `mihomo` 运行） | 由 mihomo-cli 自动同步（见 [升级 mihomo-cli](upgrade.md)） |

服务二进制放在系统路径而非家目录，是因为系统服务以独立服务账号运行，不能（也不应）依赖某个用户的家目录；systemd unit 位于 `/etc/systemd/system/mihomo.service`。

两份二进制版本应当一致：`mihomo-cli status` 会在不一致时提示 degraded。

---

## 安装部署

### `install` (alias: `i`)

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

#### system 模式的目录权限与 `mihomo` 组

Linux system 模式将 `/var/lib/mihomo-cli` 收敛为 `root:mihomo 0770`；`transactions/` 等受管运行态由服务账号 `mihomo` 写入，目录为 `mihomo:mihomo 0750`（writer 发布时可为 `2750` setgid），`mihomo` 组只读/穿越，不授予写权限。安装**不会**自动把当前用户加入 `mihomo` 组。

当执行 `install --system` 成功后，若检测到操作者（`SUDO_USER` 或当前非 root 用户）不在 `mihomo` 组，CLI 会输出一条非阻塞提示，提醒普通用户后续执行 `override import` / `status` 前需要加组；安装过程不会自动修改系统账户，也不影响安装退出码。

运行期遇到 `EACCES` 时，`override import` / `override clear` 的前置事务闸门保持 fail-closed、配置零修改，并按平台给出恢复路径：Linux 指引加入 `mihomo` 组并重新登录；macOS 按 LaunchDaemon 账号与 `root:wheel` 状态目录、Windows 按服务账号与 `%ProgramData%` DACL，指引受管系统授权或归属修复；macOS/Windows 均不会输出 Linux 组或 `usermod` 指引。

普通用户不在组内时：

- `status` / `doctor` 对不可读的特权状态保持 `unknown`（❓ 无法判定），不会伪造健康；
- `override import` / `override clear` 等配置写入会在前置事务闸门前 **fail-closed**，配置零修改；
- 工具不会因为读不到状态就自动 sudo，也不会把“读不到”当成“没有事务”。

需要以普通用户执行这些操作时，显式加入组并重新登录：

```bash
sudo usermod -aG mihomo "$USER"
# 重新登录后验证
id -nG
```

`mihomo` 组只授予读取/管理受管状态所需的权限，不改变服务边界。若无法加组，请在具备系统授权的受管上下文中执行；不要把 `status` 当作修复命令，它读取的是同一受管目录。macOS/Windows 的服务权限模型不同，按各自平台的服务账号处理。

---

### `uninstall` (alias: `u`)

卸载 mihomo-cli。

```bash
mihomo-cli uninstall            # 停止服务 + 删除服务
mihomo-cli uninstall --all      # 删除一切（binary + service + config）
```

---

### `update` (alias: `up`)

更新 mihomo 核心二进制到最新版本。

```bash
mihomo-cli update
```

---

### `upgrade`

检查 GitHub 上最新 mihomo core 版本，并升级 Mihomo core。

```bash
mihomo-cli upgrade        # 交互确认后升级
mihomo-cli upgrade --yes  # 非交互：跳过确认直接升级
```

---

### `version`

显示 mihomo-cli 构建信息和当前 mihomo core 版本。

```bash
mihomo-cli version
```

---

### `dashboard` (alias: `dash`)

实时状态仪表盘（TUI）。

```bash
mihomo-cli dashboard    # 或 mihomo-cli dash
```

> TUN 行与 `status` / `tun status` 同源同判：只有通过 revision/journal attestation 的 TUN 才显示 `✅ enabled` / `❌ disabled`，其余（含旧快照、未 attested、无法观察）显示 `unknown`；不使用 Core `/configs` 的原始 `tun.enable` 位。

---

## 服务控制

### `start`

启动 mihomo 服务。自动检测当前实例模式（system service / per-user），无需指定模式。

```bash
mihomo-cli start                # 启动当前实例（自动检测模式）
mihomo-cli start --system       # 强制 system service 实例（排障用）
```

启动/重启边界：`start` 保留为兼容和高级实例控制命令；普通用户主旅程使用 `restart`，它负责显式启动目标 Core 并等待 readiness。普通、非 TUN-active 实例的配置导入、规则或 DNS 修改若未能完成受管 reload/restart，会保留已写入的有效配置并提示执行 `mihomo-cli restart`；system TUN active 时必须改走 promotion dispatcher，不得用该提示替代 snapshot/Core 收敛。不会由只读查询隐式启动 Core。

---

### `stop`

停止 mihomo 服务。

```bash
mihomo-cli stop
```

---

### `restart`

重启 mihomo 服务。

```bash
mihomo-cli restart
```

---

### `status`

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
- system 实例下代际信任根（`root:root 0750`）以当前身份不可读时，`status` 不会把「读不动」当成「无待应用更新」：stderr 提示无法判定，并指引 `mihomo-cli doctor --system --privileged` 做显式授权的只读检查；`doctor` 对同一观测显示 ❓ 无法判定；`status --json` 报 `pending_generation.state = "unknown"`。默认 status/doctor 不因此提权

### 迁移后显式采纳当前安装

旧代际迁移后可能无法证明当前安装与可信代际一致，普通 install 和 restart 不会自动采纳。先运行 `mihomo-cli doctor --system --privileged` 只读检查；该命令会在需要时请求一次管理员授权，不修复、不迁移、不停启。

仅当检查显示非空槽位均为迁移来的未可信代际、没有可信槽位或绑定，管理员才可自行决定是否为当前安装**建立新信任**：

```bash
mihomo-cli install --system --adopt-current --dry-run
mihomo-cli install --system --adopt-current
# 仅有 pending P 时，把 P 替换为检查结果中显示的精确 ID：
mihomo-cli install --system --adopt-current --discard-pending=P
```

预览不写入、不提权，也不迁移或加锁；当前身份看不到所需信息时如实报告 unknown。实际采纳须管理员授权并确认，默认答案为否；无终端时必须显式使用 `--yes`。命令显示安装 CLI、Core 和 `geoip.metadb`、`GeoSite.dat`、`Country.mmdb` 的绝对路径、大小和完整 SHA-256，或明确标记 Geo 缺失。确认后会重新检查，文件或槽位变化即拒绝。JSON 模式的 stdout 只输出 JSON，提示和授权说明走 stderr。

采纳只写受保护的代际存储及快照副本，不下载、不覆盖安装文件、不执行 Core、不改变配置或停启服务；有 pending 时只允许组合指定精确 P 并在同一次状态提交中撤销。采纳不证明安装历史上不是混合态、来源可靠或 Core 可运行。存在可信代际的混合现场不能用它洗白；拒绝时保留现场并按诊断处理。后续仍需显式 install 准备更新，再按需显式 restart 应用。
