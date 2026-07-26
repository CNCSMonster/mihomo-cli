# 多文件配置 — 实现计划

> 状态: 草案
> 日期: 2026-07-16
> 前置文档: [multi-file-config.md](./multi-file-config.md)

## 1. 依赖变更

### 新增 Cargo 依赖

```toml
chrono = { version = "0.4", features = ["serde"] }  # 订阅更新时间
rand = "0.8"                                          # ID 生成（8 位 hex）
```

不需要其他新依赖。`serde_yaml` 已有，`tempfile` 已有（测试用）。

---

## 2. 数据结构变更

### 2.1 `utils.rs` — `AppPaths` 扩展

```rust
// 现有字段保留，新增 3 个路径方法：
impl AppPaths {
    /// ~/.config/mihomo/subscriptions/
    pub fn subscriptions_dir(&self) -> PathBuf {
        self.config_dir.join("subscriptions")
    }

    /// ~/.config/mihomo/subscriptions.yaml
    pub fn subscriptions_meta_path(&self) -> PathBuf {
        self.config_dir.join("subscriptions.yaml")
    }

    /// ~/.config/mihomo/subscriptions/active
    pub fn active_file_path(&self) -> PathBuf {
        self.subscriptions_dir().join("active")
    }

    /// ~/.config/mihomo/subscriptions/<id>.yaml
    pub fn subscription_file_path(&self, id: &str) -> PathBuf {
        self.subscriptions_dir().join(format!("{id}.yaml"))
    }
}
```

**注意**：`subscription_urls_path()` 方法保留但标记 `#[deprecated]`，旧代码在过渡期仍可使用。

### 2.2 `config.rs` — 新增 `SubscriptionMeta`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionMeta {
    pub id: String,
    pub url: String,
    pub updated: DateTime<Utc>,
}
```

`subscriptions.yaml` 就是 `Vec<SubscriptionMeta>` 的 YAML 序列化。

### 2.3 ID 生成

```rust
/// 生成 `sub-` + 8 位随机 hex，如 `sub-a1b2c3d4`
pub fn generate_subscription_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let hex: String = (0..4)
        .map(|_| format!("{:02x}", rng.gen::<u8>()))
        .collect();
    format!("sub-{hex}")
}
```

---

## 3. 核心函数设计

### 3.1 订阅元数据管理（`config.rs`）

| 函数 | 签名 | 职责 |
|------|------|------|
| `load_subscriptions_at` | `(paths: &AppPaths) -> Result<Vec<SubscriptionMeta>>` | 读取 `subscriptions.yaml`，不存在返回空 Vec |
| `save_subscriptions_at` | `(paths: &AppPaths, subs: &[SubscriptionMeta]) -> Result<()>` | 原子写入 `subscriptions.yaml` |
| `get_active_id_at` | `(paths: &AppPaths) -> Result<Option<String>>` | 读取 `subscriptions/active` 文件内容（trim），不存在返回 None |
| `set_active_id_at` | `(paths: &AppPaths, id: &str) -> Result<()>` | 写入 `subscriptions/active` |
| `find_subscription` | `(subs: &[SubscriptionMeta], id: &str) -> Option<&SubscriptionMeta>` | 按 ID 查找 |

### 3.2 订阅 CRUD（`config.rs`）

| 函数 | 签名 | 职责 |
|------|------|------|
| `add_subscription_at` | `(paths: &AppPaths, url: &str) -> Result<String>` | 下载 → 保存 `<id>.yaml` → 更新元数据 → 返回新 ID |
| `remove_subscription_at` | `(paths: &AppPaths, id: &str) -> Result<()>` | 删除 `<id>.yaml` → 更新元数据 → 如果是 active 则清除 active |
| `refresh_subscription_at` | `(paths: &AppPaths, id: &str) -> Result<()>` | 根据元数据中的 URL 重新下载 → 覆盖 `<id>.yaml` → 更新 `updated` |
| `refresh_all_at` | `(paths: &AppPaths) -> Result<()>` | 遍历所有订阅 → 逐个 refresh |
| `switch_subscription_at` | `(paths: &AppPaths, id: &str) -> Result<()>` | 校验 ID 存在 → 写 active → merge → restart |

### 3.3 merge 流程重写（`config.rs`）

```rust
/// 重写后的 merge_user_config_at
pub fn merge_user_config_at(paths: &AppPaths) -> anyhow::Result<()> {
    // 1. 获取 active subscription ID
    let active_id = get_active_id_at(paths)?
        .ok_or_else(|| anyhow::anyhow!("No active subscription. Run: mihomo-cli config --add <URL>"))?;

    // 2. 读取 active subscription 文件
    let sub_path = paths.subscription_file_path(&active_id);
    let sub_content = std::fs::read_to_string(&sub_path)
        .map_err(|e| anyhow::anyhow!(
            "Failed to read subscription file {}: {}\n  \
             Run: mihomo-cli config --refresh  to re-download",
            sub_path.display(), e
        ))?;

    // 3. 校验 subscription YAML
    let sub_yaml: serde_yaml::Value = serde_yaml::from_str(&sub_content)
        .map_err(|e| anyhow::anyhow!(
            "Subscription file {} is not valid YAML: {}\n  \
             The file may have been manually edited with a syntax error.",
            sub_path.display(), e
        ))?;
    validate_subscription_yaml(&sub_yaml)?;

    // 4. 读取用户规则和 DNS 策略（可选，失败不阻塞）
    let user_rules = if paths.rules_path().exists() {
        crate::rules::load_rules_at(paths).unwrap_or_default()
    } else {
        Vec::new()
    };
    let dns_policies = if paths.dns_policy_path().exists() {
        crate::dns::load_policies_at(paths).unwrap_or_default()
    } else {
        Vec::new()
    };

    // 5. 生成 config.yaml（全量生成，不再增量编辑）
    let config_content = generate_config_yaml(&sub_yaml, &user_rules, &dns_policies)?;

    // 6. 校验生成的 YAML
    serde_yaml::from_str::<serde_yaml::Value>(&config_content)
        .map_err(|e| anyhow::anyhow!("Generated config is invalid YAML: {}", e))?;

    // 7. 原子写入 config.yaml
    utils::atomic_write_file(&paths.config_path().display().to_string(), &config_content)?;

    crate::log!("Merged config: {} rules, {} DNS policies", user_rules.len(), dns_policies.len());
    Ok(())
}
```

### 3.4 config.yaml 全量生成（`config.rs`）

> 详细设计见 [generate-config-yaml.md](./generate-config-yaml.md)

```rust
/// 从订阅内容 + 用户规则 + DNS 策略生成最终 config.yaml
fn generate_config_yaml(
    sub_yaml: &serde_yaml::Value,
    user_rules: &[String],
    dns_policies: &[crate::dns::DnsPolicy],
    rule_position: RulePosition,
    config_dir: &str,
) -> anyhow::Result<String>
```

**关键设计**：全量生成意味着不再需要 tree-sitter 增量编辑 `merge_rules` / `merge_dns_policies`。
`yaml_editor.rs` 中的 `merge_rules` 和 `merge_dns_policies` 方法在新流程中**不再使用**，但保留以兼容旧代码路径（标记 `#[allow(dead_code)]`）。

**核心逻辑**：
1. 深拷贝订阅 YAML
2. 合并 rules（根据 rule_position 决定顺序）
3. 合并 dns.nameserver-policy（用户策略覆盖同域名）
4. 注入 external-controller-unix/pipe
5. 序列化为 YAML 字符串

### 3.5 校验函数

```rust
/// 校验订阅 YAML 包含必要字段
fn validate_subscription_yaml(yaml: &serde_yaml::Value) -> anyhow::Result<()> {
    if yaml.get("proxies").is_none() && yaml.get("proxy-providers").is_none() {
        anyhow::bail!(
            "Subscription does not contain `proxies` or `proxy-providers`.\n  \
             The downloaded content may not be a valid Clash subscription."
        );
    }
    Ok(())
}
```

---

## 4. CLI 命令变更（`main.rs`）

### 4.1 `Command::Config` 枚举扩展

```rust
Config {
    /// Subscription URL (兼容旧用法: 添加为新订阅并激活)
    #[arg(short, long)]
    url: Option<String>,
    /// Fix the existing config file
    #[arg(long)]
    fix: bool,
    /// Refresh active subscription
    #[arg(long)]
    refresh: bool,
    /// Refresh all subscriptions
    #[arg(long, name = "refresh-all")]
    refresh_all: bool,
    /// Import config from a local file
    #[arg(long)]
    import: Option<String>,
    /// Switch to a specific subscription by ID
    #[arg(long)]
    switch: Option<String>,
    /// Add a new subscription source
    #[arg(long)]
    add: Option<String>,
    /// Remove a subscription by ID
    #[arg(long)]
    remove: Option<String>,
    /// List all subscription sources
    #[arg(long)]
    list: bool,
},
```

### 4.2 `cmd_config` 重写

```rust
async fn cmd_config(
    url: Option<String>,
    fix: bool,
    refresh: bool,
    refresh_all: bool,
    import: Option<String>,
    switch: Option<String>,
    add: Option<String>,
    remove: Option<String>,
    list: bool,
) -> anyhow::Result<()> {
    let paths = AppPaths::from_system();

    // 优先级：明确操作 > 兼容旧用法 > TUI
    if list { /* 列出所有订阅 */ }
    if let Some(id) = switch { /* 切换订阅 */ }
    if let Some(url) = add { /* 添加订阅 */ }
    if let Some(id) = remove { /* 删除订阅 */ }
    if refresh_all { /* 刷新全部 */ }
    if refresh { /* 刷新当前 */ }
    if fix { /* 修复 config */ }
    if let Some(file) = import { /* 导入为新订阅，询问是否激活 */ }
    if let Some(u) = url { /* 兼容旧用法: 添加并激活 */ }

    // 无参数 → TUI
    show_subscription_menu().await
}
```

### 4.3 TUI 重写：`show_subscription_menu`

替换现有的 `show_config_menu`，按设计文档 4.1 节实现：

```rust
async fn show_subscription_menu() -> anyhow::Result<Option<String>> {
    // 读取 subscriptions.yaml
    // 显示列表，截取 URL 显示，(active) 标记
    // 快捷键: Enter=切换(自动restart), r=刷新当前, R=刷新全部, a=添加(弹出输入框), d=删除, Esc=返回
    // ...
}
```

---

## 5. 文件修改清单

### 5.1 修改现有文件

| 文件 | 修改内容 | 影响范围 |
|------|----------|----------|
| `Cargo.toml` | 添加 `chrono`, `rand` 依赖 | 编译 |
| `utils.rs` | `AppPaths` 新增 4 个路径方法；`subscription_urls_path` 标记 deprecated | 低风险 |
| `config.rs` | 新增 `SubscriptionMeta` 结构体；新增订阅 CRUD 函数；重写 `merge_user_config_at`；新增 `generate_config_yaml`；新增 `validate_subscription_yaml` | **核心变更** |
| `main.rs` | `Command::Config` 扩展 5 个新参数；重写 `cmd_config`；重写 `show_config_menu` → `show_subscription_menu` | 中等 |
| `installer.rs` | `install` 流程中 `setup_config_interactive` 改用 `add_subscription_at` | 低风险 |

### 5.2 不修改的文件

| 文件 | 原因 |
|------|------|
| `yaml_editor.rs` | `merge_rules` / `merge_dns_policies` 在新流程中不再被调用，但保留不删（向后兼容） |
| `rules.rs` | 不变，rules.yaml 的读写逻辑完全复用 |
| `dns.rs` | 不变，dns-policy.yaml 的读写逻辑完全复用 |
| `ui.rs` | 不变，proxy node 选择与订阅管理无关 |
| `mihomo_api.rs` | 不变 |
| `service.rs` | 不变 |

---

## 6. 实现顺序（按依赖关系）

### Phase 1: 基础设施（无外部依赖）

**Task 1.1** — 添加 Cargo 依赖
- 文件: `Cargo.toml`
- 变更: 添加 `chrono` 和 `rand`
- 验证: `cargo check` 通过

**Task 1.2** — `AppPaths` 扩展
- 文件: `utils.rs`
- 新增方法: `subscriptions_dir()`, `subscriptions_meta_path()`, `active_file_path()`, `subscription_file_path(id)`
- 标记 `subscription_urls_path()` 为 `#[deprecated]`
- 测试: 单元测试验证路径正确性

### Phase 2: 数据层（依赖 Phase 1）

**Task 2.1** — `SubscriptionMeta` 结构体 + ID 生成
- 文件: `config.rs`
- 新增: `SubscriptionMeta` 结构体、`generate_subscription_id()` 函数
- 测试: ID 格式验证、序列化/反序列化

**Task 2.2** — 订阅元数据读写
- 文件: `config.rs`
- 新增: `load_subscriptions_at()`, `save_subscriptions_at()`
- 测试: 空文件、不存在、正常读写、并发安全（原子写入）

**Task 2.3** — Active 文件读写
- 文件: `config.rs`
- 新增: `get_active_id_at()`, `set_active_id_at()`
- 测试: 不存在返回 None、正常读写

### Phase 3: 核心逻辑（依赖 Phase 2）

**Task 3.1** — 订阅校验
- 文件: `config.rs`
- 新增: `validate_subscription_yaml()`
- 测试: 有效/无效 YAML、缺少 proxies 字段

**Task 3.2** — config.yaml 全量生成
- 文件: `config.rs`
- 新增: `generate_config_yaml()`
- 职责: 从 subscription YAML + user rules + DNS policies 生成最终 config
- 测试: 
  - 基本生成（只有订阅内容）
  - 带用户规则（front/back）
  - 带 DNS 策略
  - 空规则/空策略
  - external-controller 注入
  - 生成结果必须是有效 YAML

**Task 3.3** — 重写 `merge_user_config_at`
- 文件: `config.rs`
- 变更: 完全重写，使用新的全量生成流程
- 测试: 
  - 正常 merge
  - active 不存在时报错
  - subscription 文件损坏时报错（保留旧 config.yaml）
  - rules.yaml 不存在时正常继续

### Phase 4: 订阅 CRUD（依赖 Phase 3）

**Task 4.1** — 添加订阅
- 文件: `config.rs`
- 新增: `add_subscription_at()`
- 流程: 下载 → 校验 → 保存 `<id>.yaml` → 更新元数据 → 如果是第一个则设为 active
- 测试: 正常添加、URL 不可达、重复 URL

**Task 4.2** — 删除订阅
- 文件: `config.rs`
- 新增: `remove_subscription_at()`
- 流程: 删除文件 → 更新元数据 → 如果删除的是 active 则清除 active
- 测试: 删除 active、删除非 active、删除不存在的 ID

**Task 4.3** — 刷新订阅
- 文件: `config.rs`
- 新增: `refresh_subscription_at()`, `refresh_all_at()`
- 流程: 从元数据取 URL → 下载 → 覆盖文件 → 更新 `updated`
- 测试: 正常刷新、URL 不可达（保留旧文件）

**Task 4.4** — 切换订阅
- 文件: `config.rs`
- 新增: `switch_subscription_at()`
- 流程: 校验 ID 存在 → 写 active → merge
- 测试: 正常切换、ID 不存在

### Phase 5: CLI 和 TUI（依赖 Phase 4）

**Task 5.1** — CLI 参数扩展
- 文件: `main.rs`
- 变更: `Command::Config` 添加 `--switch`, `--add`, `--remove`, `--list`, `--refresh-all`
- 测试: `cargo check`、各参数解析正确

**Task 5.2** — `cmd_config` 重写
- 文件: `main.rs`
- 变更: 处理所有新参数，路由到对应函数
- 兼容旧用法: `config -u <URL>` → `add_subscription_at` + `switch`
- 测试: 各命令路径

**Task 5.3** — TUI 重写
- 文件: `main.rs`
- 变更: `show_subscription_menu()` 替换 `show_config_menu()`
- 按设计文档 4.1 节实现界面和快捷键
- 测试: 手动测试交互流程

### Phase 6: 集成和清理（依赖 Phase 5）

**Task 6.1** — `install` 流程适配
- 文件: `main.rs` (`setup_config_interactive`, `apply_subscription`)
- 变更: 使用 `add_subscription_at` 替代 `save_config`
- 测试: 全新安装流程

**Task 6.2** — 旧代码清理
- 标记不再使用的函数为 `#[allow(dead_code)]` 或删除
- `save_config()` → 被 `add_subscription_at` 内部使用，保留但简化
- `show_config_menu()` → 删除，被 `show_subscription_menu()` 替代
- `utils::read_subscription_urls()` 等 → 标记 deprecated

**Task 6.3** — 旧文件清理
- 删除 `subscription_urls.yaml`（如果存在）
- 删除 `.subscription-url`（如果存在，旧版单 URL 文件）
- 在 `uninstall --all` 中确保清理 `subscriptions/` 目录

**Task 6.4** — 端到端测试
- 完整流程测试:
  1. `mihomo-cli config --add <URL>` → 添加 + 激活
  2. `mihomo-cli config --list` → 列出订阅
  3. `mihomo-cli config --switch <id>` → 切换
  4. `mihomo-cli config --refresh` → 刷新当前
  5. `mihomo-cli config --refresh-all` → 刷新全部
  6. `mihomo-cli config --remove <id>` → 删除
  7. `mihomo-cli config` → TUI 交互

---

## 7. 测试策略

### 7.1 单元测试

所有新函数都使用 `AppPaths::for_test(tmp.path())` 在临时目录中测试，与现有测试模式一致。

关键测试用例：

| 模块 | 测试 | 验证点 |
|------|------|--------|
| `generate_subscription_id` | 格式正确、唯一性 | `sub-` 前缀 + 8 hex |
| `load/save_subscriptions_at` | 序列化往返 | YAML 格式正确 |
| `get/set_active_id_at` | 读写 active 文件 | 不存在返回 None |
| `validate_subscription_yaml` | 有效/无效输入 | 缺少 proxies 报错 |
| `generate_config_yaml` | 各种组合 | 输出有效 YAML + controller |
| `merge_user_config_at` | 完整流程 | active 不存在报错 |
| `add_subscription_at` | 添加逻辑 | 第一个自动激活 |
| `remove_subscription_at` | 删除逻辑 | 删 active 清除指针 |
| `switch_subscription_at` | 切换逻辑 | ID 不存在报错 |

### 7.2 集成测试

在 `tests/` 目录下（或 `config.rs` 的 `#[cfg(test)]` 中）：

```
test_full_workflow:
  1. 创建临时目录
  2. add_subscription (mock 下载)
  3. 验证 subscriptions.yaml + active + sub-xxx.yaml 存在
  4. add_subscription (第二个)
  5. switch 到第一个
  6. 验证 active 文件内容
  7. remove 第二个
  8. 验证 subscriptions.yaml 只剩一个
```

### 7.3 手动测试清单

- [ ] 全新安装：`mihomo-cli install` → 输入 URL → 验证文件布局
- [ ] 多订阅管理：添加 2+ 订阅 → 切换 → 验证 config.yaml 内容变化
- [ ] 刷新：修改 subscription 文件 → refresh → 验证被覆盖
- [ ] 错误恢复：损坏 subscription 文件 → merge → 验证报错且不覆盖 config.yaml
- [ ] TUI 交互：所有快捷键功能正确
- [ ] 旧命令兼容：`config -u <URL>` 仍然可用

---

## 8. 风险和注意事项

### 8.1 `generate_config_yaml` 的 YAML 序列化

当前 `save_config` 使用 `serde_yaml::to_string` 序列化，格式已经验证可行。
`generate_config_yaml` 需要处理：
- 保留订阅中的 `proxies`, `proxy-groups`, `dns` 等字段
- 注入 `external-controller-unix/pipe`
- 合并 `rules`（用户规则 + 订阅规则，按 position 排序）
- 合并 `dns.nameserver-policy`

**风险**：`serde_yaml::to_string` 的输出格式可能与用户手动编辑的格式不同（如引号、缩进）。
**缓解**：全量生成意味着格式统一，不需要保留原始格式。这是设计目标。

### 8.2 `yaml_editor.rs` 的过渡

新流程不再需要 tree-sitter 增量编辑，但 `ensure_controller` 仍被 `fix_existing_config` 使用。
`merge_rules` 和 `merge_dns_policies` 在新 `merge_user_config_at` 中不再调用。

**决策**：保留 `yaml_editor.rs` 不修改，在 `merge_rules` / `merge_dns_policies` 上添加 `#[allow(dead_code)]`。

### 8.3 下载失败处理

`add_subscription_at` 和 `refresh_subscription_at` 都涉及网络下载。
复用现有的 `download_sub_smart`，失败时：
- 不创建/不覆盖 subscription 文件
- 不更新元数据
- 返回错误信息

### 8.4 并发安全

当前 mihomo-cli 是单进程 CLI 工具，不存在并发问题。
原子写入（`.tmp` + `rename`）防止中断导致的文件损坏。

---

## 9. 文件布局变化示例

### 变更前

```
~/.config/mihomo/
├── config.yaml              ← 混合内容（订阅 + 用户规则 + DNS）
├── rules.yaml
├── dns-policy.yaml
└── .subscription-urls       ← 纯 URL 列表
```

### 变更后

```
~/.config/mihomo/
├── subscriptions/
│   ├── active               ← "sub-abc12345"
│   ├── sub-abc12345.yaml    ← 机场 A 的完整订阅内容
│   └── sub-def67890.yaml    ← 机场 B 的完整订阅内容
├── subscriptions.yaml       ← 元数据列表
├── rules.yaml               ← 不变
├── dns-policy.yaml          ← 不变
└── config.yaml              ← 纯生成物
```
