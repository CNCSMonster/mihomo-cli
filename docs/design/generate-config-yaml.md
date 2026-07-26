# generate_config_yaml 详细设计

> 状态: 草案
> 日期: 2026-07-16
> 前置文档: [multi-file-config.md](./multi-file-config.md), [multi-file-config-impl.md](./multi-file-config-impl.md)

## 1. 概述

`generate_config_yaml` 是多文件配置方案的核心函数，负责从以下源文件生成最终的 `config.yaml`：

| 输入 | 来源 | 说明 |
|------|------|------|
| `subscription.yaml` | `subscriptions/<active-id>.yaml` | 订阅内容（proxies, proxy-groups, dns 等） |
| `user_rules` | `rules.yaml` | 用户自定义规则 |
| `dns_policies` | `dns-policy.yaml` | 用户 DNS 策略 |
| `rule_position` | `rules_position.yaml` | 规则插入位置（front/back） |

| 输出 | 说明 |
|------|------|
| `config.yaml` | mihomo 读取的最终配置文件 |

## 2. 函数签名

```rust
/// 从订阅内容 + 用户规则 + DNS 策略生成最终 config.yaml
pub fn generate_config_yaml(
    sub_yaml: &serde_yaml::Value,      // 订阅内容（已解析）
    user_rules: &[String],              // 用户规则列表
    dns_policies: &[DnsPolicy],         // 用户 DNS 策略
    rule_position: RulePosition,        // 规则插入位置
) -> anyhow::Result<String>
```

## 3. 生成流程

```
┌─────────────────────────────────────────────────────────────┐
│                    generate_config_yaml                      │
├─────────────────────────────────────────────────────────────┤
│ 1. 深拷贝 subscription YAML                                 │
│ 2. 合并 rules                                               │
│    ├─ 读取订阅中的 rules（如果有）                            │
│    ├─ 根据 rule_position 决定顺序                            │
│    └─ 写入合并后的 rules                                     │
│ 3. 合并 dns.nameserver-policy                               │
│    ├─ 读取订阅中的 dns.nameserver-policy（如果有）            │
│    ├─ 用户策略覆盖同域名                                     │
│    └─ 写入合并后的 nameserver-policy                         │
│ 4. 注入 external-controller                                 │
│    ├─ Unix: external-controller-unix                         │
│    └─ Windows: external-controller-pipe                      │
│ 5. 序列化为 YAML 字符串                                      │
│ 6. 返回结果                                                 │
└─────────────────────────────────────────────────────────────┘
```

## 4. 详细实现

### 4.1 深拷贝订阅 YAML

```rust
let mut config = sub_yaml.clone();
let config_map = config.as_mapping_mut()
    .ok_or_else(|| anyhow::anyhow!("Subscription is not a valid YAML mapping"))?;
```

### 4.2 合并 rules

**输入**：
- `sub_rules`: 订阅中的 rules（`Option<Vec<String>>`）
- `user_rules`: 用户规则（`&[String]`）
- `rule_position`: `Front` 或 `Back`

**逻辑**：

```rust
let sub_rules = config_map
    .get("rules")
    .and_then(|v| v.as_sequence())
    .map(|seq| {
        seq.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();

let merged_rules = match rule_position {
    RulePosition::Front => {
        // 用户规则在前，订阅规则在后
        let mut rules = user_rules.to_vec();
        rules.extend(sub_rules);
        rules
    }
    RulePosition::Back => {
        // 订阅规则在前，用户规则在后
        let mut rules = sub_rules;
        rules.extend(user_rules.iter().cloned());
        rules
    }
};

// 写入合并后的 rules
config_map.insert(
    serde_yaml::Value::String("rules".to_string()),
    serde_yaml::Value::Sequence(
        merged_rules
            .into_iter()
            .map(serde_yaml::Value::String)
            .collect()
    ),
);
```

**示例**：

```yaml
# 订阅 rules
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
  - DOMAIN-SUFFIX,github.com,Proxy

# 用户 rules
- DOMAIN-SUFFIX,company.com,DIRECT
- IP-CIDR,10.0.0.0/8,DIRECT

# 合并后 (position: front)
rules:
  - DOMAIN-SUFFIX,company.com,DIRECT
  - IP-CIDR,10.0.0.0/8,DIRECT
  - DOMAIN-SUFFIX,google.com,Proxy
  - DOMAIN-SUFFIX,github.com,Proxy
```

### 4.3 合并 dns.nameserver-policy

**输入**：
- `sub_dns`: 订阅中的 dns 配置（`Option<Mapping>`）
- `sub_ns_policy`: 订阅中的 nameserver-policy（`Option<Mapping>`）
- `dns_policies`: 用户 DNS 策略（`&[DnsPolicy]`）

**DnsPolicy 结构**：

```rust
pub struct DnsPolicy {
    pub match_pattern: String,  // 如 "+.google.com" 或 "company.com"
    pub target: String,         // 如 "system" 或 "114.114.114.114"
}
```

**逻辑**：

```rust
// 获取或创建 dns mapping
let dns = config_map
    .entry(serde_yaml::Value::String("dns".to_string()))
    .or_insert(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

let dns_map = dns.as_mapping_mut()
    .ok_or_else(|| anyhow::anyhow!("dns is not a mapping"))?;

// 获取现有的 nameserver-policy（如果有）
let mut ns_policy = dns_map
    .get("nameserver-policy")
    .and_then(|v| v.as_mapping())
    .cloned()
    .unwrap_or_default();

// 合并用户策略（用户策略覆盖同域名）
for policy in dns_policies {
    let key = if policy.match_pattern.starts_with("+.") {
        policy.match_pattern.clone()
    } else {
        format!("+{}", policy.match_pattern)  // 自动添加 + 前缀表示后缀匹配
    };
    
    // 处理多 DNS 服务器（逗号分隔）
    let targets: Vec<&str> = policy.target.split(',').map(|s| s.trim()).collect();
    let value = if targets.len() == 1 {
        serde_yaml::Value::String(targets[0].to_string())
    } else {
        serde_yaml::Value::Sequence(
            targets.into_iter()
                .map(|s| serde_yaml::Value::String(s.to_string()))
                .collect()
        )
    };
    
    ns_policy.insert(serde_yaml::Value::String(key), value);
}

// 写入合并后的 nameserver-policy
dns_map.insert(
    serde_yaml::Value::String("nameserver-policy".to_string()),
    serde_yaml::Value::Mapping(ns_policy),
);
```

**示例**：

```yaml
# 订阅 dns
dns:
  enable: true
  enhanced-mode: fake-ip
  nameserver:
    - 223.5.5.5
  nameserver-policy:
    "+.streaming.com": 8.8.8.8

# 用户 dns-policy.yaml
policies:
  - domain: company.com
    target: system
  - domain: internal.corp
    target: 10.10.1.251,114.114.114.114

# 合并后
dns:
  enable: true
  enhanced-mode: fake-ip
  nameserver:
    - 223.5.5.5
  nameserver-policy:
    "+.streaming.com": 8.8.8.8
    "+.company.com": system
    "+.internal.corp":
      - 10.10.1.251
      - 114.114.114.114
```

### 4.4 注入 external-controller

**逻辑**：

```rust
// 移除可能存在的旧 controller 配置
config_map.remove(&serde_yaml::Value::String("external-controller".to_string()));
config_map.remove(&serde_yaml::Value::String("external-controller-unix".to_string()));
config_map.remove(&serde_yaml::Value::String("external-controller-pipe".to_string()));
config_map.remove(&serde_yaml::Value::String("external-ui".to_string()));

// 注入新的 controller
if cfg!(target_os = "windows") {
    config_map.insert(
        serde_yaml::Value::String("external-controller-pipe".to_string()),
        serde_yaml::Value::String(r"\\.\pipe\mihomo".to_string()),
    );
} else {
    let socket_path = format!("{}/mihomo.sock", paths.config_dir().display());
    config_map.insert(
        serde_yaml::Value::String("external-controller-unix".to_string()),
        serde_yaml::Value::String(socket_path),
    );
}
```

### 4.5 序列化为 YAML

```rust
let output = serde_yaml::to_string(&config)?;
Ok(output)
```

## 5. 完整实现

```rust
pub fn generate_config_yaml(
    sub_yaml: &serde_yaml::Value,
    user_rules: &[String],
    dns_policies: &[crate::dns::DnsPolicy],
    rule_position: RulePosition,
    config_dir: &str,
) -> anyhow::Result<String> {
    let mut config = sub_yaml.clone();
    let config_map = config.as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Subscription is not a valid YAML mapping"))?;

    // 1. 合并 rules
    let sub_rules = config_map
        .get("rules")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let merged_rules = match rule_position {
        RulePosition::Front => {
            let mut rules = user_rules.to_vec();
            rules.extend(sub_rules);
            rules
        }
        RulePosition::Back => {
            let mut rules = sub_rules;
            rules.extend(user_rules.iter().cloned());
            rules
        }
    };

    config_map.insert(
        serde_yaml::Value::String("rules".to_string()),
        serde_yaml::Value::Sequence(
            merged_rules
                .into_iter()
                .map(serde_yaml::Value::String)
                .collect()
        ),
    );

    // 2. 合并 dns.nameserver-policy
    if !dns_policies.is_empty() {
        let dns = config_map
            .entry(serde_yaml::Value::String("dns".to_string()))
            .or_insert(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

        let dns_map = dns.as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("dns is not a mapping"))?;

        let mut ns_policy = dns_map
            .get("nameserver-policy")
            .and_then(|v| v.as_mapping())
            .cloned()
            .unwrap_or_default();

        for policy in dns_policies {
            let key = if policy.match_pattern.starts_with("+.") {
                policy.match_pattern.clone()
            } else {
                format!("+{}", policy.match_pattern)
            };
            
            let targets: Vec<&str> = policy.target.split(',').map(|s| s.trim()).collect();
            let value = if targets.len() == 1 {
                serde_yaml::Value::String(targets[0].to_string())
            } else {
                serde_yaml::Value::Sequence(
                    targets.into_iter()
                        .map(|s| serde_yaml::Value::String(s.to_string()))
                        .collect()
                )
            };
            
            ns_policy.insert(serde_yaml::Value::String(key), value);
        }

        dns_map.insert(
            serde_yaml::Value::String("nameserver-policy".to_string()),
            serde_yaml::Value::Mapping(ns_policy),
        );
    }

    // 3. 注入 external-controller
    config_map.remove(&serde_yaml::Value::String("external-controller".to_string()));
    config_map.remove(&serde_yaml::Value::String("external-controller-unix".to_string()));
    config_map.remove(&serde_yaml::Value::String("external-controller-pipe".to_string()));
    config_map.remove(&serde_yaml::Value::String("external-ui".to_string()));

    if cfg!(target_os = "windows") {
        config_map.insert(
            serde_yaml::Value::String("external-controller-pipe".to_string()),
            serde_yaml::Value::String(r"\\.\pipe\mihomo".to_string()),
        );
    } else {
        let socket_path = format!("{}/mihomo.sock", config_dir);
        config_map.insert(
            serde_yaml::Value::String("external-controller-unix".to_string()),
            serde_yaml::Value::String(socket_path),
        );
    }

    // 4. 序列化
    let output = serde_yaml::to_string(&config)?;
    Ok(output)
}
```

## 6. 测试用例

### 6.1 基本生成（只有订阅内容）

```rust
#[test]
fn test_generate_basic() {
    let sub_yaml: serde_yaml::Value = serde_yaml::from_str(r#"
mixed-port: 7890
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#).unwrap();

    let result = generate_config_yaml(
        &sub_yaml,
        &[],           // 无用户规则
        &[],           // 无 DNS 策略
        RulePosition::Front,
        "/tmp/test",
    ).unwrap();

    // 验证：包含 external-controller-unix
    assert!(result.contains("external-controller-unix"));
    // 验证：rules 保持不变
    assert!(result.contains("DOMAIN-SUFFIX,google.com,Proxy"));
}
```

### 6.2 带用户规则（front）

```rust
#[test]
fn test_generate_with_rules_front() {
    let sub_yaml: serde_yaml::Value = serde_yaml::from_str(r#"
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#).unwrap();

    let user_rules = vec![
        "DOMAIN-SUFFIX,company.com,DIRECT".to_string(),
        "IP-CIDR,10.0.0.0/8,DIRECT".to_string(),
    ];

    let result = generate_config_yaml(
        &sub_yaml,
        &user_rules,
        &[],
        RulePosition::Front,
        "/tmp/test",
    ).unwrap();

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
    let rules: Vec<&str> = parsed["rules"].as_sequence().unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    
    assert_eq!(rules, vec![
        "DOMAIN-SUFFIX,company.com,DIRECT",
        "IP-CIDR,10.0.0.0/8,DIRECT",
        "DOMAIN-SUFFIX,google.com,Proxy",
    ]);
}
```

### 6.3 带用户规则（back）

```rust
#[test]
fn test_generate_with_rules_back() {
    let sub_yaml: serde_yaml::Value = serde_yaml::from_str(r#"
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#).unwrap();

    let user_rules = vec!["DOMAIN-SUFFIX,company.com,DIRECT".to_string()];

    let result = generate_config_yaml(
        &sub_yaml,
        &user_rules,
        &[],
        RulePosition::Back,
        "/tmp/test",
    ).unwrap();

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
    let rules: Vec<&str> = parsed["rules"].as_sequence().unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    
    assert_eq!(rules, vec![
        "DOMAIN-SUFFIX,google.com,Proxy",
        "DOMAIN-SUFFIX,company.com,DIRECT",
    ]);
}
```

### 6.4 带 DNS 策略

```rust
#[test]
fn test_generate_with_dns策略() {
    let sub_yaml: serde_yaml::Value = serde_yaml::from_str(r#"
dns:
  enable: true
  nameserver:
    - 223.5.5.5
"#).unwrap();

    let dns_policies = vec![
        DnsPolicy { match_pattern: "company.com".to_string(), target: "system".to_string() },
        DnsPolicy { match_pattern: "internal.corp".to_string(), target: "10.10.1.251,114.114.114.114".to_string() },
    ];

    let result = generate_config_yaml(
        &sub_yaml,
        &[],
        &dns_policies,
        RulePosition::Front,
        "/tmp/test",
    ).unwrap();

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
    let ns_policy = &parsed["dns"]["nameserver-policy"];
    
    assert_eq!(ns_policy["+company.com"].as_str().unwrap(), "system");
    assert!(ns_policy["+internal.corp"].is_sequence());
}
```

### 6.5 DNS 策略覆盖

```rust
#[test]
fn test_dns_policy_override() {
    let sub_yaml: serde_yaml::Value = serde_yaml::from_str(r#"
dns:
  enable: true
  nameserver-policy:
    "+.company.com": 8.8.8.8
"#).unwrap();

    let dns_policies = vec![
        DnsPolicy { match_pattern: "company.com".to_string(), target: "system".to_string() },
    ];

    let result = generate_config_yaml(
        &sub_yaml,
        &[],
        &dns_policies,
        RulePosition::Front,
        "/tmp/test",
    ).unwrap();

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
    // 用户策略覆盖订阅策略
    assert_eq!(parsed["dns"]["nameserver-policy"]["+company.com"].as_str().unwrap(), "system");
}
```

### 6.6 订阅无 rules 字段

```rust
#[test]
fn test_no_subscription_rules() {
    let sub_yaml: serde_yaml::Value = serde_yaml::from_str(r#"
mixed-port: 7890
proxies: []
"#).unwrap();

    let user_rules = vec!["DOMAIN-SUFFIX,company.com,DIRECT".to_string()];

    let result = generate_config_yaml(
        &sub_yaml,
        &user_rules,
        &[],
        RulePosition::Front,
        "/tmp/test",
    ).unwrap();

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
    let rules: Vec<&str> = parsed["rules"].as_sequence().unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    
    assert_eq!(rules, vec!["DOMAIN-SUFFIX,company.com,DIRECT"]);
}
```

### 6.7 订阅无 dns 字段

```rust
#[test]
fn test_no_subscription_dns() {
    let sub_yaml: serde_yaml::Value = serde_yaml::from_str(r#"
mixed-port: 7890
"#).unwrap();

    let dns_policies = vec![
        DnsPolicy { match_pattern: "company.com".to_string(), target: "system".to_string() },
    ];

    let result = generate_config_yaml(
        &sub_yaml,
        &[],
        &dns_policies,
        RulePosition::Front,
        "/tmp/test",
    ).unwrap();

    let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
    assert!(parsed["dns"]["nameserver-policy"].is_mapping());
}
```

## 7. 边界情况处理

| 情况 | 处理 |
|------|------|
| 订阅无 rules 字段 | 创建空 rules，只包含用户规则 |
| 订阅无 dns 字段 | 创建空 dns，只包含 nameserver-policy |
| 用户规则为空 | rules 只包含订阅规则 |
| DNS 策略为空 | 保留订阅的 nameserver-policy |
| 用户策略与订阅策略域名冲突 | 用户策略覆盖 |
| 订阅 YAML 不是 mapping | 返回错误 |

## 8. 与旧实现的对比

| 维度 | 旧实现（tree-sitter） | 新实现（全量生成） |
|------|----------------------|-------------------|
| 编辑方式 | 增量编辑，保留格式 | 全量生成，格式统一 |
| 依赖 | tree-sitter-yaml | serde_yaml（已有） |
| 缩进处理 | 需要检测现有缩进 | 统一格式，不需要检测 |
| comment 标记 | 需要 USER RULES START/END | 不需要 |
| 错误恢复 | 可能残留损坏 config | 源文件不变，生成失败不覆盖 |
| 输出格式 | 保留原始格式 | 统一格式（serde_yaml 输出） |

## 9. 风险与缓解

| 风险 | 缓解 |
|------|------|
| serde_yaml 输出格式可能与用户期望不同 | 这是设计目标：统一格式，不需要保留原始格式 |
| 某些特殊字段可能丢失 | 深拷贝订阅 YAML，只修改特定字段 |
| 性能问题 | 订阅文件通常 < 1MB，序列化开销可忽略 |
