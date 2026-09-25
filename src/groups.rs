use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

const GROUP_FIELDS: &[&str] = &[
    "name",
    "type",
    "proxies",
    "use",
    "url",
    "interval",
    "lazy",
    "timeout",
    "max-failed-times",
    "disable-udp",
    "interface-name",
    "routing-mark",
    "include-all",
    "include-all-proxies",
    "include-all-providers",
    "filter",
    "exclude-filter",
    "exclude-type",
    "expected-status",
    "hidden",
    "icon",
    "strategy",
    "tolerance",
];

const GROUP_TYPES: &[&str] = &["select", "url-test", "fallback", "load-balance", "relay"];
pub const BUILTIN_POLICIES: &[&str] =
    &["DIRECT", "REJECT", "REJECT-DROP", "REJECT-NO-DROP", "PASS"];

pub const RESERVED_NAMES: &[&str] = &[
    "DIRECT",
    "REJECT",
    "REJECT-DROP",
    "REJECT-NO-DROP",
    "PASS",
    "GLOBAL",
    "COMPATIBLE",
    "MATCH",
    "PROXY",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupPatch {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_proxies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_proxies: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupsOverlay {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prepend: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub append: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delete: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub patches: BTreeMap<String, GroupPatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMode {
    /// Active CLI mutation mode: fail-fast on unknown members, cycles, or duplicate names.
    ActiveStrict,
    /// Passive subscription refresh/import mode: graceful pruning of missing members,
    /// injecting DIRECT fallback for regular groups or REJECT for relay groups if all members missing.
    #[allow(dead_code)]
    PassiveTolerant,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MergeReport {
    pub warnings: Vec<String>,
    pub pruned_members: Vec<(String, String)>, // (group_name, member_name)
    pub fallbacks: Vec<(String, String)>,      // (group_name, fallback_policy)
}

impl GroupsOverlay {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read groups overlay: {}", path.display()))?;
        let overlay = serde_yaml::from_str(&content)
            .with_context(|| format!("failed to parse groups overlay: {}", path.display()))?;
        Ok(overlay)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("groups overlay has no parent directory"))?;
        crate::utils::ensure_dir_all_no_follow(parent)?;
        #[cfg(unix)]
        {
            let config_dir_has_setgid = parent
                .parent()
                .and_then(|p| p.parent().or(Some(p)))
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| crate::utils::mode_has_setgid(std::os::unix::fs::MetadataExt::mode(&m)))
                .unwrap_or(false);
            if config_dir_has_setgid {
                if let Some(grandparent) = parent.parent() {
                    let _ = crate::utils::set_directory_mode_no_follow(grandparent, 0o2755);
                }
                let _ = crate::utils::set_directory_mode_no_follow(parent, 0o2755);
            }
        }
        let content = serde_yaml::to_string(self)?;
        crate::utils::atomic_write_file_for_original_user(&path.display().to_string(), &content)
    }

    pub fn merged_groups(
        &self,
        original: &[Value],
        known_proxies: &HashSet<String>,
        known_providers: &HashSet<String>,
    ) -> Result<Vec<Value>> {
        self.merged_groups_with_mode(
            original,
            known_proxies,
            known_providers,
            MergeMode::ActiveStrict,
        )
        .map(|(groups, _)| groups)
    }

    pub fn merged_groups_with_mode(
        &self,
        original: &[Value],
        known_proxies: &HashSet<String>,
        known_providers: &HashSet<String>,
        mode: MergeMode,
    ) -> Result<(Vec<Value>, MergeReport)> {
        let mut report = MergeReport::default();
        let mut groups =
            Vec::with_capacity(self.prepend.len() + original.len() + self.append.len());

        // 1. Prepend自建组
        for custom in &self.prepend {
            validate_custom_group(custom)?;
        }
        groups.extend(self.prepend.iter().cloned());

        // 2. 原生组就地微调（过滤 delete，应用 patches）
        let mut matched_patches = HashSet::new();
        for group in original {
            let Some(name) = group_name(group) else {
                continue;
            };
            // 过滤删除列表
            if self.delete.iter().any(|d| d == name) {
                continue;
            }

            if let Some(patch) = self.patches.get(name) {
                matched_patches.insert(name.to_string());
                let mut patched_group = group.clone();
                let map = patched_group
                    .as_mapping_mut()
                    .ok_or_else(|| anyhow::anyhow!("proxy group `{name}` must be a mapping"))?;

                // 在主动模式下，add 与 remove 不得有交集
                if mode == MergeMode::ActiveStrict {
                    for add in &patch.add_proxies {
                        if patch.remove_proxies.contains(add) {
                            bail!("proxy group `{name}` patch cannot simultaneously add and remove member `{add}`");
                        }
                    }
                }

                let mut current_members = group_members(group)?;
                // 先过滤掉 remove_proxies
                current_members.retain(|item| !patch.remove_proxies.contains(item));
                // 再追加 add_proxies（保持顺序并去重）
                for add in &patch.add_proxies {
                    if !current_members.contains(add) {
                        current_members.push(add.clone());
                    }
                }

                map.insert(
                    Value::String("proxies".into()),
                    Value::Sequence(current_members.into_iter().map(Into::into).collect()),
                );
                groups.push(patched_group);
            } else {
                groups.push(group.clone());
            }
        }

        // 3. 退化检查：如果 patch 引用的原生组在上游已消失
        for patch_target in self.patches.keys() {
            if !matched_patches.contains(patch_target) {
                report.warnings.push(format!(
                    "Patch target group `{patch_target}` no longer exists in upstream subscription; skipping"
                ));
            }
        }

        // 4. Append自建组
        for custom in &self.append {
            validate_custom_group(custom)?;
        }
        groups.extend(self.append.iter().cloned());

        // 5. 校验与容错（基于 MergeMode）
        validate_and_prune_groups(
            &mut groups,
            known_proxies,
            known_providers,
            mode,
            &mut report,
        )?;

        Ok((groups, report))
    }
}

pub fn validate_group_name(name: &str) -> Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        bail!("proxy group name cannot be empty");
    }
    let char_count = name.chars().count();
    if !(1..=128).contains(&char_count) {
        bail!("proxy group name length must be between 1 and 128 characters, got {char_count}");
    }
    if name.starts_with('.') || name.starts_with('-') {
        bail!("proxy group name cannot start with '.' or '-': `{name}`");
    }
    for ch in name.chars() {
        if ch.is_control() {
            bail!("proxy group name contains control characters: `{name}`");
        }
        if matches!(
            ch,
            ':' | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '!'
                | '|'
                | '>'
                | '%'
                | '@'
                | ','
                | '\''
                | '"'
                | '`'
        ) {
            bail!("proxy group name contains forbidden YAML character `{ch}`: `{name}`");
        }
    }
    let upper = name.to_ascii_uppercase();
    for reserved in RESERVED_NAMES {
        if upper == *reserved {
            bail!("proxy group name cannot use reserved policy name `{reserved}`: `{name}`");
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn apply_smart_defaults(map: &mut serde_yaml::Mapping, group_type: &str) {
    match group_type {
        "url-test" | "fallback" => {
            if !map.contains_key(Value::String("url".into())) {
                map.insert(
                    Value::String("url".into()),
                    Value::String("http://www.gstatic.com/generate_204".into()),
                );
            }
            if !map.contains_key(Value::String("interval".into())) {
                map.insert(Value::String("interval".into()), Value::Number(300.into()));
            }
        }
        "load-balance" => {
            if !map.contains_key(Value::String("url".into())) {
                map.insert(
                    Value::String("url".into()),
                    Value::String("http://www.gstatic.com/generate_204".into()),
                );
            }
            if !map.contains_key(Value::String("interval".into())) {
                map.insert(Value::String("interval".into()), Value::Number(300.into()));
            }
            if !map.contains_key(Value::String("strategy".into())) {
                map.insert(
                    Value::String("strategy".into()),
                    Value::String("consistent-hashing".into()),
                );
            }
        }
        _ => {}
    }
}

pub fn parse_group(source: &str) -> Result<Value> {
    let value: Value = serde_yaml::from_str(source).context("failed to parse proxy group YAML")?;
    validate_custom_group(&value)?;
    Ok(value)
}

pub fn validate_group(group: &Value) -> Result<()> {
    let map = group
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("proxy group must be a YAML mapping"))?;
    for key in map.keys() {
        let key = key
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("proxy group fields must have string names"))?;
        if !GROUP_FIELDS.contains(&key) {
            bail!("unknown proxy group field `{key}`")
        }
    }
    let name = map
        .get(Value::String("name".into()))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("proxy group name is required"))?;
    let group_type = map
        .get(Value::String("type".into()))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("proxy group `{name}` type is required"))?;
    if !GROUP_TYPES.contains(&group_type) {
        bail!(
            "unsupported proxy group type `{group_type}`; supported types: {}",
            GROUP_TYPES.join(", ")
        )
    }
    for field in ["proxies", "use"] {
        if let Some(value) = map.get(Value::String(field.into())) {
            if !value.is_sequence() {
                bail!("proxy group `{name}` field `{field}` must be a list")
            }
        }
    }
    Ok(())
}

pub fn validate_custom_group(group: &Value) -> Result<()> {
    validate_group(group)?;
    let name = group_name(group).ok_or_else(|| anyhow::anyhow!("proxy group name is required"))?;
    validate_group_name(name)?;
    Ok(())
}

#[allow(dead_code)]
pub fn validate_groups(
    groups: &[Value],
    known_proxies: &HashSet<String>,
    known_providers: &HashSet<String>,
) -> Result<()> {
    let mut report = MergeReport::default();
    let mut cloned = groups.to_vec();
    validate_and_prune_groups(
        &mut cloned,
        known_proxies,
        known_providers,
        MergeMode::ActiveStrict,
        &mut report,
    )
}

fn validate_and_prune_groups(
    groups: &mut [Value],
    known_proxies: &HashSet<String>,
    known_providers: &HashSet<String>,
    mode: MergeMode,
    report: &mut MergeReport,
) -> Result<()> {
    let mut names = HashSet::new();
    let mut graph = HashMap::<String, Vec<String>>::new();

    // 基础结构校验与重名检查
    for group in groups.iter() {
        validate_group(group)?;
        let name = group_name(group)
            .expect("validated group has name")
            .to_string();
        if !names.insert(name.clone()) {
            bail!("duplicate proxy group name `{name}`");
        }
    }

    let group_names = names.clone();

    // 成员有效性校验与被动剪枝
    for group in groups.iter_mut() {
        let name = group_name(group)
            .expect("validated group has name")
            .to_string();
        let group_type = group
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let is_relay = group_type == "relay";

        let uses = group
            .get("use")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        for provider in uses {
            if !known_providers.contains(provider) {
                bail!("proxy group `{name}` references unknown proxy provider `{provider}`")
            }
        }

        let map = group.as_mapping_mut().expect("group is mapping");
        let had_proxies = map.contains_key(Value::String("proxies".into()));
        let has_providers = map
            .get(Value::String("use".into()))
            .and_then(Value::as_sequence)
            .is_some_and(|s| !s.is_empty());
        let has_include_all = map
            .get(Value::String("include-all".into()))
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || map
                .get(Value::String("include-all-proxies".into()))
                .and_then(Value::as_bool)
                .unwrap_or(false)
            || map
                .get(Value::String("include-all-providers".into()))
                .and_then(Value::as_bool)
                .unwrap_or(false);

        if !had_proxies && !has_providers && !has_include_all {
            bail!("proxy group `{name}` must specify `proxies`, `use`, or `include-all`");
        }

        if let Some(proxies_val) = map.get_mut(Value::String("proxies".into())) {
            if let Some(seq) = proxies_val.as_sequence_mut() {
                let mut valid_members = Vec::new();
                for member_val in seq.iter() {
                    let member = member_val.as_str().unwrap_or("").to_string();
                    if BUILTIN_POLICIES.contains(&member.as_str()) {
                        if is_relay {
                            bail!("relay group `{name}` cannot use built-in policy `{member}`");
                        }
                        valid_members.push(member_val.clone());
                        continue;
                    }
                    if group_names.contains(&member) || known_proxies.contains(&member) {
                        valid_members.push(member_val.clone());
                        continue;
                    }

                    match mode {
                        MergeMode::ActiveStrict => {
                            bail!("proxy group `{name}` references unknown member `{member}`");
                        }
                        MergeMode::PassiveTolerant => {
                            report.pruned_members.push((name.clone(), member.clone()));
                            report.warnings.push(format!(
                                "Node `{member}` no longer in subscription; dropped from group `{name}`"
                            ));
                        }
                    }
                }

                // 容错兜底：若成员全被剔除（且无 proxy-providers），仅在 PassiveTolerant 模式触发
                if mode == MergeMode::PassiveTolerant
                    && had_proxies
                    && valid_members.is_empty()
                    && !has_providers
                    && !has_include_all
                {
                    if is_relay {
                        // Relay组全失效时：安全降级为 REJECT，严禁注入 DIRECT 防泄露真实 IP
                        valid_members.push(Value::String("REJECT".into()));
                        report.fallbacks.push((name.clone(), "REJECT".into()));
                        report.warnings.push(format!(
                            "Relay group `{name}` has no available members; falling back to REJECT to prevent traffic leak"
                        ));
                    } else {
                        // 普通组全失效：注入 DIRECT 兜底保活
                        valid_members.push(Value::String("DIRECT".into()));
                        report.fallbacks.push((name.clone(), "DIRECT".into()));
                        report.warnings.push(format!(
                            "Proxy group `{name}` has no available members; injecting DIRECT fallback"
                        ));
                    }
                } else if mode == MergeMode::ActiveStrict
                    && had_proxies
                    && valid_members.is_empty()
                    && !has_providers
                    && !has_include_all
                {
                    bail!("proxy group `{name}` must have at least one proxy member or provider");
                }

                *seq = valid_members;
            }
        }

        let final_members = group_members(group)?;
        graph.insert(name, final_members);
    }

    // 全局 DAG 环路检测
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for group in graph.keys() {
        visit_group(group, &graph, &mut visiting, &mut visited)?;
    }

    Ok(())
}

pub fn group_name(group: &Value) -> Option<&str> {
    group
        .as_mapping()?
        .get(Value::String("name".into()))?
        .as_str()
}

pub fn group_members(group: &Value) -> Result<Vec<String>> {
    let Some(value) = group
        .as_mapping()
        .and_then(|map| map.get(Value::String("proxies".into())))
    else {
        return Ok(Vec::new());
    };
    let seq = value
        .as_sequence()
        .ok_or_else(|| anyhow::anyhow!("`proxies` field must be a YAML sequence"))?;
    Ok(seq
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect())
}

fn visit_group(
    group: &str,
    graph: &HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
) -> Result<()> {
    // 白色 → 已完成（黑色）：直接跳过
    if visited.contains(group) {
        return Ok(());
    }
    // 灰色 → 当前 DFS 路径上已访问（回边）：存在环
    if visiting.contains(group) {
        bail!("proxy group reference cycle detected at `{group}`")
    }
    // 标记为灰色（进行中）
    visiting.insert(group.to_string());
    if let Some(members) = graph.get(group) {
        for member in members {
            if graph.contains_key(member) {
                visit_group(member, graph, visiting, visited)?;
            }
        }
    }
    // 回溯：从灰色移除，标记为黑色（已完成）
    visiting.remove(group);
    visited.insert(group.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(source: &str) -> Value {
        serde_yaml::from_str(source).unwrap()
    }

    #[test]
    fn supports_all_cvr_group_types_and_fields() {
        for group_type in GROUP_TYPES {
            let group = parse_group(&format!(
                "name: g-{group_type}\ntype: {group_type}\nproxies: [DIRECT]\nuse: [p]\nurl: http://example.test\ninterval: 300\nlazy: true\ntimeout: 5000\nmax-failed-times: 5\ndisable-udp: true\ninterface-name: eth0\nrouting-mark: 1\ninclude-all: true\ninclude-all-proxies: true\ninclude-all-providers: true\nfilter: x\nexclude-filter: y\nexclude-type: Direct\nexpected-status: 204\nhidden: true\nicon: i\nstrategy: consistent-hashing\ntolerance: 50"
            ))
            .unwrap();
            assert_eq!(group["type"].as_str(), Some(*group_type));
        }
    }

    #[test]
    fn rejects_unknown_fields_and_cycles() {
        assert!(parse_group("name: g\ntype: select\nunknown: true").is_err());
        let groups = vec![
            value("name: a\ntype: select\nproxies: [b]"),
            value("name: b\ntype: select\nproxies: [a]"),
        ];
        let known_proxies = HashSet::new();
        let known_providers = HashSet::new();
        assert!(validate_groups(&groups, &known_proxies, &known_providers)
            .unwrap_err()
            .to_string()
            .contains("cycle"));
    }

    #[test]
    fn builtin_policies_include_reject_no_drop() {
        assert!(BUILTIN_POLICIES.contains(&"REJECT-NO-DROP"));
        let groups = vec![value("name: g\ntype: select\nproxies: [REJECT-NO-DROP]")];
        assert!(validate_groups(&groups, &HashSet::new(), &HashSet::new()).is_ok());
    }

    #[test]
    fn applies_cvr_sequence_order_and_delete() {
        let overlay = GroupsOverlay {
            prepend: vec![value("name: p\ntype: select\nproxies: [DIRECT]")],
            append: vec![value("name: a\ntype: select\nproxies: [DIRECT]")],
            delete: vec!["origin".into()],
            patches: BTreeMap::new(),
        };
        let known_proxies = HashSet::new();
        let known_providers = HashSet::new();
        let merged = overlay
            .merged_groups(
                &[value("name: origin\ntype: select\nproxies: [DIRECT]")],
                &known_proxies,
                &known_providers,
            )
            .unwrap();
        let names: Vec<_> = merged
            .iter()
            .map(|group| group_name(group).unwrap())
            .collect();
        assert_eq!(names, vec!["p", "a"]);
    }

    #[test]
    fn relay_rejects_builtin_policies() {
        let groups = vec![value("name: chain\ntype: relay\nproxies: [DIRECT]")];
        let error = validate_groups(&groups, &HashSet::new(), &HashSet::new()).unwrap_err();
        assert!(error.to_string().contains("relay"), "error: {error}");
    }

    #[test]
    fn rejects_unknown_proxy_provider_reference() {
        let groups = vec![value("name: provider-group\ntype: select\nuse: [missing]")];
        let error = validate_groups(&groups, &HashSet::new(), &HashSet::new()).unwrap_err();
        assert!(
            error.to_string().contains("unknown proxy provider"),
            "error: {error}"
        );
    }

    #[test]
    fn validates_group_name_constraints_and_reserved_words() {
        assert!(validate_group_name("").is_err());
        assert!(validate_group_name("   ").is_err());
        assert!(validate_group_name(".hidden").is_err());
        assert!(validate_group_name("-invalid").is_err());
        assert!(validate_group_name("group:name").is_err());
        assert!(validate_group_name("group[0]").is_err());
        assert!(validate_group_name("group{a}").is_err());

        // 保留策略测试（大小写不敏感）
        assert!(validate_group_name("DIRECT").is_err());
        assert!(validate_group_name("direct").is_err());
        assert!(validate_group_name("Reject").is_err());
        assert!(validate_group_name("GLOBAL").is_err());
        assert!(validate_group_name("Proxy").is_err());
        assert!(validate_group_name("MATCH").is_err());

        // 合法名称
        assert!(validate_group_name("My Group 01").is_ok());
        assert!(validate_group_name("hk-auto_select.v2").is_ok());
    }

    #[test]
    fn applies_smart_defaults_for_url_test_and_load_balance() {
        let mut map = serde_yaml::Mapping::new();
        apply_smart_defaults(&mut map, "url-test");
        assert_eq!(
            map.get("url").and_then(Value::as_str),
            Some("http://www.gstatic.com/generate_204")
        );
        assert_eq!(map.get("interval").and_then(Value::as_i64), Some(300));

        let mut lb_map = serde_yaml::Mapping::new();
        apply_smart_defaults(&mut lb_map, "load-balance");
        assert_eq!(
            lb_map.get("strategy").and_then(Value::as_str),
            Some("consistent-hashing")
        );
    }

    #[test]
    fn delta_patch_modifies_original_in_place_preserving_topology() {
        let mut patches = BTreeMap::new();
        patches.insert(
            "OriginMain".to_string(),
            GroupPatch {
                add_proxies: vec!["CustomHK".to_string()],
                remove_proxies: vec!["StaleNode".to_string()],
            },
        );
        let overlay = GroupsOverlay {
            prepend: vec![value("name: PreGroup\ntype: select\nproxies: [DIRECT]")],
            append: vec![value("name: PostGroup\ntype: select\nproxies: [DIRECT]")],
            delete: vec![],
            patches,
        };

        let original = vec![
            value("name: OriginMain\ntype: select\nproxies: [StaleNode, UpstreamNode]"),
            value("name: OriginSecond\ntype: select\nproxies: [UpstreamNode]"),
        ];

        let mut known = HashSet::new();
        known.insert("UpstreamNode".to_string());
        known.insert("CustomHK".to_string());

        let (merged, report) = overlay
            .merged_groups_with_mode(&original, &known, &HashSet::new(), MergeMode::ActiveStrict)
            .unwrap();

        assert!(report.warnings.is_empty());
        let names: Vec<_> = merged
            .iter()
            .map(|g| group_name(g).unwrap().to_string())
            .collect();
        // 拓扑顺序保留：PreGroup -> OriginMain -> OriginSecond -> PostGroup
        assert_eq!(
            names,
            vec!["PreGroup", "OriginMain", "OriginSecond", "PostGroup"]
        );

        let origin_main = merged
            .iter()
            .find(|g| group_name(g) == Some("OriginMain"))
            .unwrap();
        let members = group_members(origin_main).unwrap();
        // StaleNode 被剔除，UpstreamNode 保留，CustomHK 追加在末尾
        assert_eq!(members, vec!["UpstreamNode", "CustomHK"]);
    }

    #[test]
    fn delta_patch_preserves_new_upstream_nodes() {
        let mut patches = BTreeMap::new();
        patches.insert(
            "AutoSelect".to_string(),
            GroupPatch {
                add_proxies: vec!["Custom01".to_string()],
                remove_proxies: vec![],
            },
        );
        let overlay = GroupsOverlay {
            prepend: vec![],
            append: vec![],
            delete: vec![],
            patches,
        };

        // 模拟上游订阅更新：新增了 NewNode99
        let original = vec![value(
            "name: AutoSelect\ntype: select\nproxies: [OldNode01, NewNode99]",
        )];

        let mut known = HashSet::new();
        known.insert("OldNode01".to_string());
        known.insert("NewNode99".to_string());
        known.insert("Custom01".to_string());

        let (merged, _) = overlay
            .merged_groups_with_mode(&original, &known, &HashSet::new(), MergeMode::ActiveStrict)
            .unwrap();

        let members = group_members(&merged[0]).unwrap();
        assert_eq!(members, vec!["OldNode01", "NewNode99", "Custom01"]);
    }

    #[test]
    fn delta_patch_skips_missing_upstream_group_with_warning() {
        let mut patches = BTreeMap::new();
        patches.insert(
            "DeletedGroup".to_string(),
            GroupPatch {
                add_proxies: vec!["NodeA".to_string()],
                remove_proxies: vec![],
            },
        );
        let overlay = GroupsOverlay {
            prepend: vec![],
            append: vec![],
            delete: vec![],
            patches,
        };

        let original = vec![value(
            "name: ExistingGroup\ntype: select\nproxies: [DIRECT]",
        )];

        let (merged, report) = overlay
            .merged_groups_with_mode(
                &original,
                &HashSet::new(),
                &HashSet::new(),
                MergeMode::PassiveTolerant,
            )
            .unwrap();

        assert_eq!(merged.len(), 1);
        assert_eq!(group_name(&merged[0]), Some("ExistingGroup"));
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("DeletedGroup") && w.contains("skipping")));
    }

    #[test]
    fn delta_patch_rejects_conflicting_add_and_remove_in_strict_mode() {
        let mut patches = BTreeMap::new();
        patches.insert(
            "Main".to_string(),
            GroupPatch {
                add_proxies: vec!["ConflictNode".to_string()],
                remove_proxies: vec!["ConflictNode".to_string()],
            },
        );
        let overlay = GroupsOverlay {
            prepend: vec![],
            append: vec![],
            delete: vec![],
            patches,
        };

        let original = vec![value("name: Main\ntype: select\nproxies: [ConflictNode]")];
        let mut known = HashSet::new();
        known.insert("ConflictNode".to_string());

        let err = overlay
            .merged_groups_with_mode(&original, &known, &HashSet::new(), MergeMode::ActiveStrict)
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("cannot simultaneously add and remove"));
    }

    #[test]
    fn passive_mode_prunes_stale_members_with_warning() {
        let overlay = GroupsOverlay::default();
        let original = vec![value(
            "name: Main\ntype: select\nproxies: [AliveNode, StaleNode]",
        )];

        let mut known = HashSet::new();
        known.insert("AliveNode".to_string()); // StaleNode不在known中

        let (merged, report) = overlay
            .merged_groups_with_mode(
                &original,
                &known,
                &HashSet::new(),
                MergeMode::PassiveTolerant,
            )
            .unwrap();

        let members = group_members(&merged[0]).unwrap();
        assert_eq!(members, vec!["AliveNode"]);
        assert_eq!(
            report.pruned_members,
            vec![("Main".to_string(), "StaleNode".to_string())]
        );
        assert!(report.warnings[0].contains("StaleNode"));
    }

    #[test]
    fn passive_mode_injects_direct_fallback_for_empty_regular_group() {
        let overlay = GroupsOverlay::default();
        let original = vec![value("name: Auto\ntype: url-test\nproxies: [StaleNode]")];

        let (merged, report) = overlay
            .merged_groups_with_mode(
                &original,
                &HashSet::new(),
                &HashSet::new(),
                MergeMode::PassiveTolerant,
            )
            .unwrap();

        let members = group_members(&merged[0]).unwrap();
        assert_eq!(members, vec!["DIRECT"]);
        assert_eq!(
            report.fallbacks,
            vec![("Auto".to_string(), "DIRECT".to_string())]
        );
    }

    #[test]
    fn passive_mode_injects_reject_fallback_for_empty_relay_group() {
        let overlay = GroupsOverlay::default();
        let original = vec![value("name: Chain\ntype: relay\nproxies: [StaleHop]")];

        let (merged, report) = overlay
            .merged_groups_with_mode(
                &original,
                &HashSet::new(),
                &HashSet::new(),
                MergeMode::PassiveTolerant,
            )
            .unwrap();

        let members = group_members(&merged[0]).unwrap();
        assert_eq!(members, vec!["REJECT"]);
        assert_eq!(
            report.fallbacks,
            vec![("Chain".to_string(), "REJECT".to_string())]
        );
    }

    #[test]
    fn detects_deep_and_cross_patch_dag_cycles() {
        let mut patches = BTreeMap::new();
        // A 原生引用 B，B 通过 patch 引用 A
        patches.insert(
            "B".to_string(),
            GroupPatch {
                add_proxies: vec!["A".to_string()],
                remove_proxies: vec![],
            },
        );
        let overlay = GroupsOverlay {
            prepend: vec![],
            append: vec![],
            delete: vec![],
            patches,
        };
        let original = vec![
            value("name: A\ntype: select\nproxies: [B]"),
            value("name: B\ntype: select\nproxies: [DIRECT]"),
        ];

        let err = overlay
            .merged_groups_with_mode(
                &original,
                &HashSet::new(),
                &HashSet::new(),
                MergeMode::ActiveStrict,
            )
            .unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    /// [F-5] 自环检测：组 A 直接引用自身，DFS 应在进入灰色状态后立即报环。
    #[test]
    fn detects_self_loop_cycle() {
        let groups = vec![value("name: A\ntype: select\nproxies: [A]")];
        let err = validate_groups(&groups, &HashSet::new(), &HashSet::new()).unwrap_err();
        assert!(
            err.to_string().contains("cycle"),
            "expected cycle error, got: {err}"
        );
    }

    /// [F-4] include-all-providers 豁免：当组设置了 include-all-providers: true 时，
    /// 即使所有 proxies 成员失效，也不应注入兜底策略。
    #[test]
    fn passive_mode_skips_fallback_when_include_all_providers_is_set() {
        let overlay = GroupsOverlay::default();
        let original = vec![value(
            "name: Auto\ntype: select\nproxies: [StaleNode]\ninclude-all-providers: true",
        )];

        let (merged, report) = overlay
            .merged_groups_with_mode(
                &original,
                &HashSet::new(),
                &HashSet::new(),
                MergeMode::PassiveTolerant,
            )
            .unwrap();

        // 成员应被剪枝，但不应注入 DIRECT 兜底
        let members = group_members(&merged[0]).unwrap();
        assert!(
            members.is_empty(),
            "expected no fallback member, got: {members:?}"
        );
        assert!(
            report.fallbacks.is_empty(),
            "expected no fallback recorded, got: {:?}",
            report.fallbacks
        );
    }

    /// [F-3] 序列化/反序列化 roundtrip 测试，包含 patches 字段。
    #[test]
    fn groups_overlay_roundtrip_with_patches() {
        let mut patches = BTreeMap::new();
        patches.insert(
            "MyGroup".to_string(),
            GroupPatch {
                add_proxies: vec!["NodeA".to_string(), "NodeB".to_string()],
                remove_proxies: vec!["OldNode".to_string()],
            },
        );
        let overlay = GroupsOverlay {
            prepend: vec![value("name: Pre\ntype: select\nproxies: [DIRECT]")],
            append: vec![value("name: Post\ntype: select\nproxies: [DIRECT]")],
            delete: vec!["Obsolete".to_string()],
            patches,
        };

        // 序列化到 YAML
        let yaml = serde_yaml::to_string(&overlay).expect("serialization failed");

        // 反序列化回结构
        let restored: GroupsOverlay = serde_yaml::from_str(&yaml).expect("deserialization failed");

        assert_eq!(
            overlay, restored,
            "roundtrip mismatch:\noriginal: {overlay:#?}\nrestored: {restored:#?}"
        );

        // 明确验证 patches 字段的 add/remove 被正确还原
        let patch = restored.patches.get("MyGroup").expect("patch must exist");
        assert_eq!(patch.add_proxies, vec!["NodeA", "NodeB"]);
        assert_eq!(patch.remove_proxies, vec!["OldNode"]);
    }

    #[test]
    fn upstream_subscription_with_reserved_and_bracketed_names_is_allowed() {
        let overlay = GroupsOverlay {
            prepend: vec![value("name: CustomPre\ntype: select\nproxies: [NodeA]")],
            append: vec![],
            delete: vec![],
            patches: BTreeMap::new(),
        };

        let original = vec![
            value("name: PROXY\ntype: select\nproxies: [NodeA]"),
            value("name: GLOBAL\ntype: select\nproxies: [NodeA]"),
            value("name: 节点选择 [自动]\ntype: select\nproxies: [NodeA]"),
        ];

        let mut known = HashSet::new();
        known.insert("NodeA".to_string());

        let (merged, report) = overlay
            .merged_groups_with_mode(&original, &known, &HashSet::new(), MergeMode::ActiveStrict)
            .expect("upstream groups with reserved or bracketed names must be allowed");

        assert_eq!(merged.len(), 4);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn active_strict_rejects_empty_proxy_group() {
        let groups = vec![value("name: EmptyGroup\ntype: select\nproxies: []")];
        let err = validate_groups(&groups, &HashSet::new(), &HashSet::new()).unwrap_err();
        assert!(
            err.to_string().contains("must have at least one proxy"),
            "expected empty group error, got: {err}"
        );
    }

    #[test]
    fn active_strict_rejects_group_missing_proxies_and_providers() {
        let groups = vec![value("name: NoSource\ntype: select")];
        let err = validate_groups(&groups, &HashSet::new(), &HashSet::new()).unwrap_err();
        assert!(
            err.to_string().contains("must specify `proxies`"),
            "expected missing source error, got: {err}"
        );
    }
}
