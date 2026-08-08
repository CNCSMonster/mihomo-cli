# SPEC: config CLI（subcommand-only）

> 状态：设计稿  
> 日期：2026-08-06  
> 范围：重设计 `mihomo-cli config` 命令表面；不引入 profile/template/sub 新顶层概念。

## 1. 背景与问题

当前 `config` 同时承担交互式 TUI、订阅管理、离线 fetch、UA 探测、配置验证/修复等职责，但 CLI 表面主要由一组 flat flags 组成：

```bash
mihomo-cli config -u <url>
mihomo-cli config --add <url>
mihomo-cli config --remove <id>
mihomo-cli config --switch <id>
mihomo-cli config --refresh
mihomo-cli config --refresh-all
mihomo-cli config --info [id]
mihomo-cli config --probe <url>
mihomo-cli config --set-ua <id> <ua|auto>
mihomo-cli config --validate
mihomo-cli config --fix
mihomo-cli config --import <file>
mihomo-cli config fetch <url> -o <file>
```

这造成三个问题：

1. **动作和修饰混在一起**：`--add`/`--remove`/`--switch` 是动作，却表现为 flag；`--dry-run`/`--yes`/`--json` 才是真正的修饰。
2. **组合语义不清**：多个动作 flag 同时出现时应谁优先、是否冲突、是否允许，很难从命令形状看出来。
3. **AI/脚本不友好**：自动化调用需要一个稳定、可枚举、易解析的动作集合；flat flags 会扩大误用空间。

因此当前采用 **subcommand-only** 设计：所有“做什么”都变成子命令，flag 只保留“怎么做”。

## 2. 设计原则

### 2.1 判断标准

| 类型 | 放在哪里 | 例子 |
|------|----------|------|
| 做什么 | subcommand | `add`、`remove`、`switch`、`refresh`、`validate` |
| 对谁做 | positional argument | `<url>`、`<id-or-name>`、`<file>` |
| 怎么做 | flag | `--yes`、`--dry-run`、`--json`、`--ua`、`--activate` |

### 2.2 不做的事

- 不新增 `sub` 顶层命令：订阅仍归属 `config`。
- 不新增 `profile`：本阶段只处理 active subscription，不做多套工作区/运行档案。
- 不新增 `template`：LAN DIRECT 等场景继续用 `rule`/`dns` 显式配置。
- 不保留旧 flat action flags 兼容层：这是一次从头设计式的 CLI 表面整理。

### 2.3 与现有领域模型的关系

`config` 管理的是配置源和生成产物：

- active subscription 是当前配置基底。
- subscription cache 是订阅的 last known good 本地缓存。
- `rules.yaml`、`dns-policy.yaml`、`dns-fake-ip-filter.yaml`、`override.yaml` 叠加在 active subscription 之上。
- 写入/生成配置与运行时生效是两阶段：命令应明确报告是否 reload 成功；失败时给出 `mihomo-cli restart` 建议。

## 3. 目标命令表面

### 3.1 总览

```bash
mihomo-cli config                         # 交互式 TUI
mihomo-cli config add <url>
mihomo-cli config import <file>
mihomo-cli config remove <id-or-name>
mihomo-cli config switch <id-or-name>
mihomo-cli config list
mihomo-cli config info [id-or-name]
mihomo-cli config refresh [id-or-name]
mihomo-cli config refresh-all
mihomo-cli config validate
mihomo-cli config fix
mihomo-cli config probe <url>
mihomo-cli config fetch <url> -o <file>
mihomo-cli config ua set <id-or-name> <ua|auto>
```

### 3.2 全局/通用 flags

`config` 子树可用的通用 flags：

```bash
--system          # 高级/排障：强制 system service context 做 validation/reload
--json            # 全局 flag，stdout 只输出 JSON envelope
-y, --yes         # 跳过确认
--dry-run         # 预览，不写入、不 reload/restart
```

> 实现上 `--json` 仍是顶层全局 flag；文档按用户心智说明为 config 可用。

### 3.3 子命令细节

#### `config add <url>`

添加订阅源，下载/转换/校验，写入 subscription metadata/cache，必要时生成 `config.yaml`。

```bash
mihomo-cli config add '<url>'
mihomo-cli config add '<url>' --activate --yes
mihomo-cli config add '<url>' --no-activate
mihomo-cli config add '<url>' --ua 'clash-verge/v2.0.4'
mihomo-cli config add '<url>' --dry-run --json
```

Flags：

- `--activate`：添加后设为 active subscription。
- `--no-activate`：添加后不切换 active。
- `--ua, --user-agent <ua>`：本次下载使用固定 UA。
- `--yes`：跳过“是否激活”等确认。
- `--dry-run`：探测/校验但不写入。

默认行为建议：

- 首个订阅默认激活。
- 已存在 active subscription 时，人类交互可询问是否激活；非交互必须通过 `--activate`/`--no-activate` 或 `--yes` 表达选择，否则 fail-fast 并提示命令。

#### `config import <file>`

从本地文件导入 Clash YAML/base64/vmess 等配置，写入本地订阅缓存并可激活。

```bash
mihomo-cli config import ./config.yaml --activate --yes
mihomo-cli config import ./config.yaml --no-activate
mihomo-cli config import ./config.yaml --dry-run
```

语义与 `add` 类似，但输入来自本地文件，不访问远端 URL。

#### `config remove <id-or-name>`

删除订阅源及其缓存。

```bash
mihomo-cli config remove hk-main --yes
mihomo-cli config remove 01H... --dry-run --json
```

要求：

- 删除 active subscription 时必须明确后续 active 状态：
  - 若还有其它订阅，人类模式可选择切换目标；自动化模式应失败并提示先 `config switch <id>` 或使用未来显式 flag。
  - 本阶段不引入复杂 `--activate-next`；保持安全、显式。

#### `config switch <id-or-name>`

切换 active subscription，并基于本地缓存重新生成/校验 `config.yaml`。

```bash
mihomo-cli config switch hk-main
mihomo-cli config switch 01H... --json
```

要求：

- 不要求联网。
- 若本地 cache 缺失或无效，应失败并建议 `config refresh <id>`。
- 成功后尝试 reload，报告 reload 是否生效。

#### `config list`

列出订阅源。

```bash
mihomo-cli config list
mihomo-cli config list --json
```

要求：

- 默认脱敏 URL token/secret。
- 标记 active subscription。
- 显示更新时间、节点数量、UA 模式、缓存状态。

#### `config info [id-or-name]`

查看当前或指定订阅详情。

```bash
mihomo-cli config info
mihomo-cli config info hk-main --json
```

要求：

- 默认目标是 active subscription。
- 默认脱敏 URL。
- JSON 中区分 metadata、cache、generated_config、runtime_apply 状态。

#### `config refresh [id-or-name]`

刷新指定订阅；不传参数时刷新 active subscription。

```bash
mihomo-cli config refresh
mihomo-cli config refresh hk-main --ua 'clash/v1.0.0'
mihomo-cli config refresh --dry-run --json
```

要求：

- 访问远端 URL，更新 subscription cache。
- 如果刷新的是 active subscription，重新生成/校验 `config.yaml` 并尝试 reload。
- 如果刷新非 active subscription，只更新缓存，不影响当前运行配置。

#### `config refresh-all`

刷新所有订阅缓存。

```bash
mihomo-cli config refresh-all
mihomo-cli config refresh-all --json
```

要求：

- 最终 `config.yaml` 仍只由 active subscription 生成。
- 单个订阅失败不应掩盖其它订阅结果；整体 JSON 返回 per-subscription result。

#### `config validate`

验证当前生成的 `config.yaml`。

```bash
mihomo-cli config validate
mihomo-cli config validate --json
```

要求：

- YAML 解析。
- Mihomo `-t` 校验（如果 core binary 可用）。
- 不修改配置。

#### `config fix`

修复 CLI-managed runtime controller 字段等安全可自动修复项。

```bash
mihomo-cli config fix
mihomo-cli config fix --dry-run --json
```

边界：

- 只修复 mihomo-cli 应负责的 runtime controller/path 字段。
- 不自动重写用户规则、DNS、订阅内容等意图配置。

#### `config probe <url>`

探测订阅 URL 在候选 UA 下返回的格式，不写文件。

```bash
mihomo-cli config probe '<url>'
mihomo-cli config probe '<url>' --json
```

要求：

- 返回候选 UA、HTTP 状态、content-type、识别格式、节点数、失败原因。
- 默认脱敏 URL。

#### `config fetch <url> -o <file>`

离线 fetch：下载/转换/校验订阅并写出指定文件，不修改本机 mihomo 状态。

```bash
mihomo-cli config fetch '<url>' -o config.yaml
mihomo-cli config fetch '<url>' -o config.yaml --ua 'clash/v1.0.0'
```

要求：

- 不写 `~/.config/mihomo/`。
- 不改 subscriptions/active/rules/DNS/override。
- 输出文件应是可被 `config import` 使用的 Clash YAML。

#### `config ua set <id-or-name> <ua|auto>`

设置某个订阅的 UA 策略。

```bash
mihomo-cli config ua set hk-main auto
mihomo-cli config ua set hk-main 'clash-verge/v2.0.4'
```

要求：

- `auto` 表示恢复 UA 协商。
- 固定 UA 仅影响后续 add/refresh 访问远端 URL，不改变已缓存内容；如需立即生效，提示 `config refresh <id>`。

## 4. 旧命令删除映射

当前设计不兼容旧 flat action flags。迁移映射如下：

| 旧 | 新 |
|----|----|
| `config -u <url>` | `config add <url>` |
| `config --add <url>` | `config add <url>` |
| `config --import <file>` | `config import <file>` |
| `config --remove <id>` | `config remove <id>` |
| `config --switch <id>` | `config switch <id>` |
| `config --list` | `config list` |
| `config --info` | `config info` |
| `config --info <id>` | `config info <id>` |
| `config --refresh` | `config refresh` |
| `config --refresh-all` | `config refresh-all` |
| `config --validate` | `config validate` |
| `config --fix` | `config fix` |
| `config --probe <url>` | `config probe <url>` |
| `config --set-ua <id> <ua>` | `config ua set <id> <ua>` |
| `config --user-agent <ua>` | subcommand-local `--ua/--user-agent <ua>` |

删除后，旧命令应由 clap 参数错误直接失败；错误信息可在 `after_help` 中提示“config actions are subcommands”。

## 5. JSON/非交互契约

所有 `config` 子命令在 `--json` 下必须输出统一 envelope：

```json
{
  "ok": true,
  "command": "config.add",
  "data": {},
  "warnings": [],
  "error": null,
  "meta": { "schema_version": 1, "cli_version": "..." }
}
```

要求：

- stdout 只输出 JSON。
- 诊断和进度进入 stderr。
- 默认脱敏订阅 URL token、mihomo secret、节点凭据。
- 写操作必须区分：
  - `written`: 是否写入配置源/缓存。
  - `generated`: 是否生成并校验 `config.yaml`。
  - `runtime_applied`: 是否 reload 到运行中的 core。
  - `suggested_commands`: reload 失败或需要下一步时给出可执行命令。

## 6. 实施计划

### Phase 1：CLI 表面替换

- 将 `Command::Config` 的 action flags 移除，只保留 `command: Option<ConfigSubcommand>` 与必要通用 flags。
- 扩展 `ConfigSubcommand`：`Add`、`Import`、`Remove`、`Switch`、`List`、`Info`、`Refresh`、`RefreshAll`、`Validate`、`Fix`、`Probe`、`Fetch`、`Ua`。
- 将旧 `cmd_config(ConfigCmd)` 内部逻辑拆成按 action 命名的函数，避免继续维护 flag 优先级。
- 更新 clap parse tests：旧 flat flags 应 parse fail，新 subcommands parse pass。

### Phase 2：文档和用户旅程同步

- 更新 `README.md` / `README_en.md` / `USAGE.md` 中所有 `config -u`、`config --import`、`config --validate` 等示例。
- 更新 `docs/user-journeys.md`：首次配置、离线导入、LAN DIRECT 预验证、多订阅切换全部使用 config 子命令。
- 更新 `ROADMAP.md` P0/P1 recipe。

### Phase 3：JSON 与输出治理

- 为 `config list/info/probe/add/refresh/remove/switch/ua set` 补齐 JSON data schema。
- 建立 golden tests，确保 stdout 无非 JSON 内容。
- 增加脱敏测试。

## 7. 验收标准

- `mihomo-cli config --help` 展示 subcommand-only 的动作集合，不再展示 `-u/--add/--remove/--switch/--refresh/--validate/--fix/--import/--probe/--set-ua` 等 action flags。
- `mihomo-cli config add <url>` 等价替代旧 `config -u <url>` 的主要行为。
- `mihomo-cli config validate` 等价替代旧 `config --validate`。
- `mihomo-cli config fetch <url> -o <file>` 保持离线 fetch 语义。
- 旧 flat action flags parse fail，并给出可理解的 help/usage。
- 所有文档示例不再使用旧 flat action flags。
- `cargo test`、`cargo clippy --all-targets -- -D warnings` 通过。
