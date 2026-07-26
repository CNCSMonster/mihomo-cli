# 多文件配置方案

> 状态: 已确认
> 日期: 2026-07-16

## 1. 现状与问题

### 当前文件布局

```
~/.config/mihomo/
├── config.yaml           ← 订阅内容 + comment 标记 + 用户规则（双重身份）
├── rules.yaml            ← 用户规则
├── dns-policy.yaml       ← DNS 策略
└── subscription_urls.yaml ← 订阅 URL 列表
```

### 问题

| 问题 | 说明 |
|------|------|
| **职责混乱** | `config.yaml` 既是"用户可编辑的"又是"程序自动生成的" |
| **comment 标记脆弱** | `# === USER RULES START/END ===` 非标准 YAML，依赖字符串匹配 |
| **缩进检测复杂** | 增量编辑需要检测现有缩进，容易出错（见 `3806c39`） |
| **刷新覆盖规则** | `config --refresh` 直接覆盖 config.yaml，用户规则丢失 |
| **切换订阅需重新下载** | 只保存 active 订阅内容，切换需要重新下载 |

## 2. 新方案：多文件 + 多订阅源 + 全量生成

### 2.1 文件布局

```
~/.config/mihomo/
├── subscriptions/
│   ├── active              ← 当前激活的订阅 ID（纯文本文件）
│   ├── sub-abc123.yaml     ← 订阅 A 的内容（规范化 Clash YAML）
│   ├── sub-def456.yaml     ← 订阅 B 的内容
│   └── ...
├── subscriptions.yaml      ← 订阅元数据列表
├── rules.yaml              ← 用户规则（不变）
├── dns-policy.yaml         ← DNS 策略（不变）
└── config.yaml             ← 生成物（禁止手动编辑）
```

### 2.2 文件职责

| 文件 | 可编辑 | 说明 |
|------|--------|------|
| `subscriptions/*.yaml` | ✅ 用户可编辑 | 订阅内容，用户可自定义 proxy-groups 等 |
| `subscriptions/active` | ❌ 程序管理 | 当前激活的订阅 ID |
| `subscriptions.yaml` | ❌ 程序管理 | 订阅元数据（URL、更新时间） |
| `rules.yaml` | ✅ 用户可编辑 | 用户规则 |
| `dns-policy.yaml` | ✅ 用户可编辑 | DNS 策略 |
| `config.yaml` | ❌ 禁止编辑 | 纯生成物，mihomo 读取此文件 |

### 2.3 subscriptions.yaml 格式

```yaml
- id: sub-abc123
  url: "https://example.com/sub?token=xxx"
  updated: 2026-07-15T10:30:00Z
- id: sub-def456
  url: "https://example.com/sub?token=yyy"
  updated: 2026-07-10T08:00:00Z
```

> 注：不存储 `name` 字段，显示时直接截取 URL。

### 2.4 subscription 文件语义

- **用户可编辑**：用户可手动修改 proxy-groups、节点配置等
- **程序可覆盖**：`config --refresh` 时下载并覆盖
- **merge 时校验**：YAML 语法错误或关键字段缺失时，报错但不覆盖 config.yaml

## 3. 工作流程

### 3.1 操作触发

| 操作 | CLI 命令 | TUI 操作 | 行为 |
|------|----------|----------|------|
| 切换订阅 | `--switch <id>` | `Enter` | 改变 active 指针 → merge → restart |
| 刷新当前 | `--refresh` | `r` | 下载 → 覆盖 active 订阅文件 → merge |
| 刷新全部 | `--refresh-all` | `R` | 下载所有订阅 → 覆盖所有文件 → merge |
| 添加订阅 | `--add <url>` | `a` | 下载 → 保存新文件 → 更新元数据 |
| 删除订阅 | `--remove <id>` | `d` | 删除文件 → 更新元数据 |
| 规则变更 | `rule add` | - | 更新 rules.yaml → 提示 restart |
| DNS 策略变更 | `dns policy add` | - | 更新 dns-policy.yaml → 立即 merge |
| 启动/重启 | `start` / `restart` | - | merge → 启动 mihomo |

### 3.2 merge 流程

```
merge_user_config():
  1. 读取 subscriptions/active → 获取 active ID
  2. 读取 subscriptions/<active-id>.yaml → 校验 YAML + 关键字段
  3. 读取 rules.yaml → 提取规则列表
  4. 读取 dns-policy.yaml → 提取 DNS 策略
  5. 生成 config.yaml:
     - 订阅的 proxies/proxy-groups/dns 等
     - rules: 用户规则 + 订阅规则（按 position 排序）
     - dns.nameserver-policy: 用户 DNS 策略
     - external-controller-unix: 注入
  6. 原子写入 config.yaml（失败不覆盖）
```

### 3.3 校验失败处理

```
subscription.yaml 校验失败:
  → 报错，保留旧 config.yaml
  → 不启动 mihomo
  → 提示用户检查 subscription 文件

rules.yaml / dns-policy.yaml 校验失败:
  → 报错，但可继续（这些是可选的）
```

## 4. TUI 界面设计

### 4.1 界面布局

```
Subscription sources (↑↓ select, Enter switch)
▶ 1. https://example.com/sub?tok…xxx (active)
  2. https://other.com/api/v1?to…yyy
  3. https://third.com/link?token…zzz

[r] Refresh active    [R] Refresh all
[a] Add source        [d] Delete selected
[Esc] Back
```

### 4.2 快捷键

| 按键 | 功能 | 说明 |
|------|------|------|
| `↑` / `↓` | 移动光标 | 在订阅源之间移动 |
| `Enter` | 切换订阅 | 切换到光标选中的订阅（不重新下载） |
| `r` | 刷新当前 | 刷新 active 订阅（重新下载） |
| `R` | 刷新全部 | 刷新所有订阅 |
| `a` | 添加 | 添加新订阅源 |
| `d` | 删除 | 删除光标选中的订阅源 |
| `Esc` | 返回 | 退出菜单 |

### 4.3 交互逻辑

- `▶` 指示光标当前位置
- `(active)` 标记当前激活的订阅
- `Enter` = 切换到该订阅（只改 active 指针，不下载）
- `d` = 删除光标选中的（不是 active，是光标位置）
- `r` 始终刷新 active 的，不是光标选中的

## 5. CLI 命令设计

### 5.1 新增参数

```bash
mihomo-cli config [options]

Options:
  -u, --url <URL>        设置订阅 URL（兼容旧用法）
      --refresh          刷新当前激活的订阅
      --refresh-all      刷新所有订阅
      --switch <ID>      切换到指定订阅
      --add <URL>        添加新订阅源
      --remove <ID>      删除指定订阅
      --list             列出所有订阅源
      --import <FILE>    从文件导入（兼容旧用法）
      --fix              修复 config（兼容旧用法）
```

### 5.2 命令示例

```bash
# 列出所有订阅
mihomo-cli config --list

# 切换到指定订阅
mihomo-cli config --switch sub-def456

# 刷新当前订阅
mihomo-cli config --refresh

# 刷新所有订阅
mihomo-cli config --refresh-all

# 添加新订阅
mihomo-cli config --add "https://example.com/sub?token=zzz"

# 删除订阅
mihomo-cli config --remove sub-abc123

# 无参数 → 进入 TUI
mihomo-cli config
```

## 6. 向后兼容

### 6.1 不做自动迁移

旧版用户通过 `uninstall --all` 卸载后重新 `install` 即可。

### 6.2 兼容旧命令

| 旧命令 | 新行为 |
|--------|--------|
| `config -u <URL>` | 添加为新订阅并激活 |
| `config --refresh` | 刷新当前 active 订阅 |
| `config --import <FILE>` | 导入为新订阅，询问是否激活 |

## 7. 优势对比

| 维度 | 当前方案 | 新方案 |
|------|----------|--------|
| 文件职责 | config.yaml 双重身份 | 源文件与生成物分离 |
| 标记方式 | comment 标记 | 文件隔离 |
| 编辑方式 | 增量编辑 | 全量生成 |
| 缩进处理 | 需检测现有缩进 | 统一格式，不需要检测 |
| 中断恢复 | 可能残留损坏 config | 源文件不变，生成失败不覆盖 |
| 切换订阅 | 需重新下载 | 只改指针，秒切 |
| 多订阅管理 | 只存 URL，不存内容 | 每个订阅独立文件 |
| 用户自定义 | 在 config.yaml 中修改（会被覆盖） | 在 subscription 文件中修改 |

## 8. 实现要点

### 8.1 数据结构变更

```rust
// utils.rs — AppPaths 新增路径方法（基于 config_dir 动态计算）
impl AppPaths {
    /// ~/.config/mihomo/subscriptions/
    pub fn subscriptions_dir(&self) -> PathBuf;
    /// ~/.config/mihomo/subscriptions.yaml
    pub fn subscriptions_meta_path(&self) -> PathBuf;
    /// ~/.config/mihomo/subscriptions/active
    pub fn active_file_path(&self) -> PathBuf;
    /// ~/.config/mihomo/subscriptions/<id>.yaml
    pub fn subscription_file_path(&self, id: &str) -> PathBuf;
}

// 新增结构
pub struct SubscriptionMeta {
    pub id: String,
    pub url: String,
    pub updated: DateTime<Utc>,
}
```

### 8.2 核心函数

```rust
// config.rs
pub fn merge_user_config() -> Result<()>;           // 重写
pub fn get_active_subscription() -> Result<String>; // 获取 active ID
pub fn set_active_subscription(id: &str) -> Result<()>;
pub fn list_subscriptions() -> Result<Vec<SubscriptionMeta>>;
pub fn add_subscription(url: &str) -> Result<String>; // 返回新 ID
pub fn remove_subscription(id: &str) -> Result<()>;
pub fn refresh_subscription(id: &str) -> Result<()>;
pub fn refresh_all_subscriptions() -> Result<()>;
```

### 8.3 原子写入

```rust
// 写入 config.yaml 时使用临时文件 + rename
fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
```

## 9. 待确认事项

- [x] subscription 文件的 ID 生成策略 → `sub-` + 8位随机 hex
- [x] 是否需要 `mihomo-cli migrate` 显式命令 → 不做，用户 uninstall --all 后重新 install
- [x] 订阅元数据是否需要更多字段 → 不需要 name 字段，显示时截取 URL
