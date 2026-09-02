# SPEC: config CLI（subcommand-only）

> **从属与状态：** 本文是 `../SPEC.md` 的配置 CLI **目标设计/待迁移实施补充**，不是当前命令表面的权威。命令主旅程、结果状态、配置事实来源、TUN-active 应用事务、弱网与 last-known-good 语义以 `../SPEC.md` 为准。本仓库当前实现仍保留 flat action flags（如 `config --import`、`config -u`、`config --validate`），并仅实现 `config fetch <url> -o <file>` 这一子命令；下文的 subcommand-only 表面、迁移映射和对应验收标准属于后续独立迁移工作，在实现完成前不得作为当前用户旅程或验收命令。旧 flat flags 若与当前实现/正式 SPEC 不一致，也不得据本文目标设计推断出不存在的当前入口。

> 状态：设计稿  
> 日期：2026-08-06  
> 范围：重设计 `mihomo-cli config` 命令表面；不引入 profile/template/sub 新顶层概念。

> `config` 子命令的结果必须遵守主 SPEC §12.2.1 的三层结果合同：intent transaction、control-plane 和 runtime attestation。本文中的 `runtime_applied`、`pending`、`failed`、`unknown` 只描述配置变更的运行时应用层，不代表公网或目标业务数据面成功；TUN active 时只有完整 promotion journal 与当前 Core API attestation 才能返回 `runtime_applied`。

## 1. 背景与问题

当前需要从 flat flags 迁移到明确的子命令；以下命令仅作为待迁移的历史示例，不能作为当前用户旅程、实现入口或验收命令：

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
- per-user `config.yaml` 是用户 intent 的唯一事实来源；system `tun-config.yaml` 只是受保护的派生 snapshot。
- 写入/生成配置与运行时生效是两阶段。没有运行 Core 时，合法 import 仍须安全落盘，随后由用户显式执行 `restart`；不能因 reload 不可用丢失导入。
- system TUN active 时，所有会改变 active effective config 的动作必须进入统一 promotion dispatcher，经 candidate/revision、root snapshot transaction 和 Core API runtime observation；不得只执行 per-user reload 或仅提示 restart。
- refresh/add/switch 失败不得覆盖旧 active/cache/config/Core/TUN；失败时返回 `Incomplete`、`Failed` 或 `RecoveryRequired`，并给出唯一下一步。

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

默认行为：

- 当不存在 active subscription 时，首个合法订阅默认激活并生成 `UserEffectiveConfig`；`--no-activate` 显式覆盖该默认，仅保存非 active cache/metadata。
- 已存在 active subscription 时，人类交互可询问是否激活；非交互必须通过 `--activate`/`--no-activate` 明确表达选择，`--yes` 只跳过确认，不替代激活选择，否则 fail-fast 并提示命令。

#### `config import <file>`

从本地文件导入 Clash YAML/base64/vmess 等配置，写入本地订阅缓存并可激活。

```bash
mihomo-cli config import ./config.yaml --activate --yes
mihomo-cli config import ./config.yaml --no-activate
mihomo-cli config import ./config.yaml --dry-run
```

语义与 `add` 相同，但输入来自本地文件，不访问远端 URL：无 active 时默认激活并生成 `UserEffectiveConfig`；已有 active 时非交互必须显式 `--activate` 或 `--no-activate`，`--yes` 只跳过确认。

#### `config remove <id-or-name>`

删除订阅源及其缓存。

```bash
mihomo-cli config remove hk-main --yes
mihomo-cli config remove 01H... --dry-run --json
```

要求：

- 删除 active subscription 时，不能先删除 active pointer、cache 或 metadata 再询问/猜测替代项。
- 若还有其它合法订阅，人类交互可以选择替代目标；非交互调用必须显式提供替代 active 的选择（使用现有 `config switch <id>` 先切换，或使用实现定义且服从主 SPEC 的显式替代参数），否则 fail-fast。
- 若没有合法替代项，命令非零返回并保持旧 active、`config.yaml`、Core/TUN 不变；不能把 active 状态隐式变成空配置或启动空 Core。
- 删除与替代切换必须先校验 new candidate；system TUN active 时必须进入主 SPEC 规定的 `apply_active_intent` promotion dispatcher 和 durable journal，不能先删除本地文件再 reload per-user config。
- 任一步应用或最终提交失败，必须保留 last-known-good 或返回 `RecoveryRequired`；不得把 cache/pointer 已删除称为成功。

#### `config switch <id-or-name>`

切换 active subscription，并基于本地缓存重新生成/校验 `config.yaml`。

```bash
mihomo-cli config switch hk-main
mihomo-cli config switch 01H... --json
```

要求：

- 不要求联网。
- 若本地 cache 缺失或无效，应失败并建议 `config refresh <id>`。
- 成功后按当前实例状态应用：Core stopped 时只提交 intent 并提示显式 `mihomo-cli restart`；普通运行实例可按受管 reload/restart 合同应用；system TUN active 时必须进入 `apply_active_intent` promotion dispatcher，不能直接 reload per-user config。结果必须区分 `runtime_applied`、`pending`、`failed` 和 `unknown`。

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
- 如果刷新的是 active subscription，重新生成/校验 `config.yaml` 并按当前实例应用；system TUN active 时必须走统一 promotion dispatcher，Core stopped 时保留合法新 intent 并提示 `mihomo-cli restart`。刷新失败不得覆盖 active subscription、旧 cache、`config.yaml`、system snapshot 或运行中的 Core。
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
- 写操作必须区分主 SPEC §12.2.1 的三层结果，并允许保留下列配置领域摘要字段：
  - `intent`: 用户意图/配置事务结果，至少说明 candidate、active pointer、metadata、`config.yaml` 或 journal 是否安全提交；不得由该字段推导运行中的 Core 已应用。
  - `control_plane`: 本命令声明的 daemon/Core/API、受管 reload/restart 或 TUN promotion control-plane 结果；Core stopped 时可为 `pending`，未执行或无法观察时不得伪造 ready。
  - `runtime_attestation`: 当前 Core/API runtime 值与目标 revision、snapshot/intent revision、journal 和 instance 关联是否完整匹配；缺失、过期或不可达时为 `unknown`/未证明。
  - `written`: 是否安全提交 intent/config source/cache；它是 `intent` 的兼容摘要，不代表 runtime 已应用。
  - `generated`: 是否生成并通过校验 `config.yaml`；它是配置产物摘要，不代表当前 Core 已使用该版本。
  - `runtime_apply`: `runtime_applied`、`pending`、`failed` 或 `unknown`；它是 `control_plane` 与 `runtime_attestation` 在配置命令中的兼容摘要，只有受管 reload/restart 或完整 TUN promotion、以及当前 Core/API 目标 revision 观察均成立时才可为 `runtime_applied`。
  - `suggested_commands`: reload 未执行、Core stopped、观察未知或需要下一步时给出可执行命令；不能用建议命令代替本次成功证据。
- `--json` 的三层字段必须与文本结果表达同一事实；不能只因为 `written=true` 或 `generated=true` 就输出 `runtime_apply=runtime_applied`。

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
