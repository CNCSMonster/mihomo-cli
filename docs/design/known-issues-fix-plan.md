# 已知问题修复方案汇总

> 状态: 已确认
> 日期: 2026-07-22
> 关联文档: [macos-socket-dir-fix.md](macos-socket-dir-fix.md)、[concurrent-config-lock.md](concurrent-config-lock.md)

本文档汇总 macOS 排查中发现、尚未修复的已知问题及确认方案。按优先级排序。

---

## 1. migrate 删活 socket（高）

**问题**：`migrate_legacy_socket_dir()`（src/service.rs:1536）无条件删除 `/tmp/mihomo/mihomo.sock`。
若 core 正在运行并监听旧 socket，删除后 API 端点路径消失；install 路径无自愈逻辑。

**方案**：删除前检查活性——

- socket 可连接（core 活着）→ 跳过删除，提示 "旧 socket 使用中，restart 后迁移生效"
- 不可连接（stale 文件）→ 删除文件并尝试移除空目录（现状逻辑）

**改动点**：src/service.rs `migrate_legacy_socket_dir`，复用 `mihomo_api::socket_is_alive` 的探测逻辑（注意其探测的是新路径，此处需对旧路径做 connect 探测）。

---

## 2. `proxy on` / `system-proxy on` 端口缺失（高）

**问题**：`get_port()`（src/mihomo_api.rs:799-808）只读 `mixed-port`，纯 Clash 订阅只配 `port`/`socks-port` 时报错。

**方案（已确认：C，注入 + 回退双管齐下）**：

1. **注入补全（治根）**：merge 生成 config.yaml 时检测顶层无 `mixed-port`，自动注入 `mixed-port: 7897`（与 vmess 转换模板一致，src/config.rs:689）。已有 `port`/`socks-port` 保留不动，mihomo 支持多端口共存。
2. **读取回退（兜底）**：`get_port()` 依次尝试 `mixed-port → port → socks-port`；仅命中 `socks-port` 时输出提示 "仅检测到 SOCKS 端口"。

**改动点**：src/config.rs merge 流程、src/mihomo_api.rs `get_port_with_client`。

---

## 3. root 模式 stop 停不掉（高）

**问题**：root stop 用 `launchctl stop io.mihomo`（src/service.rs:185），plist 带 `KeepAlive=true` → launchd 立即拉起。

**方案（已确认：A，root 模式全面改用现代命令）**：

| 操作 | 现在 | 改为 |
|------|------|------|
| install 加载 | `launchctl load <plist>` | `launchctl bootstrap system <plist>` |
| start | `launchctl load <plist>` | 已加载则 `kickstart system/io.mihomo`，否则 `bootstrap system <plist>` |
| stop | `launchctl stop io.mihomo`（无效） | `launchctl bootout system/io.mihomo` |
| restart | `stop + start`（无效） | `launchctl kickstart -k system/io.mihomo` |
| uninstall | `bootout system <plist>` | 不变 |

- start 侧用 `macos_service_loaded("io.mihomo")`（src/service.rs:406，现有 helper）判断 loaded 状态，保证幂等
- user 模式保持 legacy `load/unload` 不变（gui 域 bootstrap 语义与用户登录会话耦合，legacy 命令更可控）
- root/user 两套命令在 `service_start_command` / `service_stop_command` / `service_restart_commands` 中按 mode 明确分叉

**改动点**：src/service.rs 各 command 构造函数 + 对应测试。

---

## 4. flock 并发保护（中）

方案已定，见 [concurrent-config-lock.md](concurrent-config-lock.md)。未实现。

---

## 5. system-proxy 硬编码 "Wi-Fi"（中）

**问题**：`networksetup -setwebproxy Wi-Fi ...`（src/system_proxy.rs:37,43,51,112-117）在有线网 Mac 上报 "network service not found"。

**方案**：改为枚举所有启用的网络服务——

1. `networksetup -listallnetworkservices` 获取列表
2. 跳过首行提示文字和带 `*` 前缀的禁用服务
3. 对每个服务执行 set/unset proxy

**改动点**：src/system_proxy.rs on/off 两个路径。

---

## 6. 第二个订阅不自动激活（中）

**问题**：`add_subscription_at` 仅在 `subs.len() == 1` 时 set_active（src/config.rs:145-147），后续添加的订阅加了不生效，输出只说 "Added"。

**方案（已确认：参数控制 + 默认交互询问）**：

- `config -u <url>` 新增 `--activate` / `--no-activate` 标志
- 未传标志时：
  - TTY 环境 → 交互询问 "是否立即切换到该订阅？"
  - 非 TTY（脚本）→ 不切换，输出提示 "Added. Run `mihomo-cli config --switch <id>` to activate."
- 第一个订阅仍无条件激活（现状保留）

**改动点**：src/main.rs CLI 定义 + cmd 分发、src/config.rs `add_subscription_at` 返回订阅 id 供切换。

---

## 7. core 版本硬编码（低）

**问题**：mihomo core 版本钉死 v1.19.27（src/installer.rs:663）。

**方案（已确认：B，确定性优先 + 显式升级路径）**：

- 保持硬编码默认版本不变（安装结果可复现）
- `install` 新增 `--version <ver>` 参数，显式指定版本
- 新增 `mihomo-cli upgrade` 命令：查询 GitHub latest release → 与当前安装版本对比 → 用户确认后下载替换 core 二进制并 restart
- `status` 输出中显示当前 core 版本（调 API `/version`）

**改动点**：src/installer.rs、src/main.rs CLI 定义。

---

## 8. 注释误导（低）

**问题**：src/service.rs:1017-1019 注释 "use `launchctl start` to restart the job"，实际 `launchctl start` 对已运行 job 是 no-op。

**方案**：改为准确描述，如 "already loaded: `launchctl start` is a no-op if running, starts the job if stopped"。

---

## 9. delay-cache 原子写（低）

**问题**：`~/.config/mihomo/delay-cache.json` 直接覆盖写，进程中途被 kill 可能留半截 JSON。

**方案**：落盘改 "写 `.tmp` → rename"。与 flock 文档中 config/rules/subscriptions 的原子写改进共用同一个 helper（建议 `utils::atomic_write(path, contents)`）。

**改动点**：src/utils.rs 新增 helper、delay cache 写入处及各 YAML 落盘点。

---

## 实施顺序建议

| 批次 | 内容 | 理由 |
|------|------|------|
| 1 | #1 删活 socket、#8 注释 | 极小改动，清零遗留 |
| 2 | #2 端口、#3 root stop | 高优先级行为正确性 |
| 3 | #5 Wi-Fi、#6 订阅激活、#9 原子写 | 中等体验改进 |
| 4 | #4 flock | 独立中型工作 |
| 5 | #7 upgrade 命令 | 新功能，独立排期 |
