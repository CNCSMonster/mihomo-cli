# AI/脚本 JSON 输出（第一阶段）

> 读者：使用者
> 回答：全局 `--json` 的 envelope 结构与 `status --json` 字段语义
> 权威：本文为用户手册一部分

`mihomo-cli` 提供全局 `--json` flag。JSON 模式下 stdout 只输出统一 envelope，不混入 emoji、TUI 或进度文本；过程中的提示与告警信息统一输出至 stderr，诊断/错误细节后续会继续迁移到结构化字段。

当前第一批已对齐的低风险命令：

```bash
mihomo-cli --json version
mihomo-cli --json status
mihomo-cli --json config --validate
```

统一 envelope 字段：

```json
{
  "ok": true,
  "command": "status",
  "data": {},
  "warnings": [],
  "error": null,
  "meta": { "schema_version": 1, "cli_version": "..." }
}
```

人类默认输出保持不变；更多命令的 `--json` 支持见内部路线图（不在此仓）。

`group` 编辑命令（`add` / `edit` / `remove` / `delete` / `reset`）的 `--json` 报告体追加 `selection_cleanup` 字段：`delete` 动作下为 `unpinned` / `absent` / `failed`，其它动作为 `null`。该字段为追加面，既有字段与取值语义不变，旧消费者可忽略。

## `status --json` 数据字段语义

`data` 中的状态语义字段与人类可读摘要**同源同判**（同一判定函数），脚本可据此替代人眼判断：

| 字段 | 语义 |
|---|---|
| `health`、`tun` | 与 `mihomo-cli status` 文本输出的 `Health:`/`TUN:` **逐字相同**（同一判定函数的两个调用者）。TUN 未通过 revision/journal attestation 时 `tun` 恒为 `unknown`（`tun status` 显示 `unknown (recovery required)`）；`health` 恒不为 `healthy`，取与文本相同的降级值（`degraded`/`ready`/`inactive`/`unknown`…） |
| `core.tun` | Core `/configs` 的**原始**观察值（`true`/`false`/`null`），未经 attestation，仅供诊断；`true` 不代表 TUN 已被证明收敛 |
| `configuration` | `applied` 仅当运行 snapshot revision 与 active intent revision 可观察、匹配且 attestation 通过；其余情况为 `unknown` 或 `out of date` |
| `pending_generation` | 待应用代际三态（仅 system 实例，user 实例为 `null`）：`{"state":"none"}`（代际状态可读且无 pending）、`{"state":"pending","id":"<generation>"}`（可读且有 pending）、`{"state":"unknown"}`（代际信任根以当前身份不可读——**可能存在未应用更新**，不等于无 pending；运行 `mihomo-cli restart --system` 会请求管理员授权检查并应用） |

因此 `{"health":"healthy","tun":"enabled","configuration":"unknown"}` 这类自相矛盾组合不会出现：`tun=enabled` 只在完整 attested 的 `TunRunning` 上出现，此时 revision 与 runtime attestation 均已可观察，`configuration` 不会因 attestation 缺失落到 `unknown`；`health=healthy` 只在 TUN 已 attested（`enabled`）或已确认关闭（`disabled`）时出现，绝不与 `tun=unknown` 同时出现。

**`health` 是开放字符串，不是封闭枚举**：其值恒等于文本输出的 `Health:` 行，当前取值集合为 `healthy` / `degraded` / `ready` / `inactive` / `unknown` / `no-systemd` / `not-installed` / `installed-not-running`。消费者应判断「是否等于 `healthy`」或「是否属于非 healthy 集合」，不要 switch 穷举——新增诊断态会在 minor 版本内追加。
