use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::{HashMap, HashSet};
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
];

const GROUP_TYPES: &[&str] = &["select", "url-test", "fallback", "load-balance", "relay"];
const BUILTIN_POLICIES: &[&str] = &["DIRECT", "REJECT", "REJECT-DROP", "PASS"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupsOverlay {
    #[serde(default)]
    pub prepend: Vec<Value>,
    #[serde(default)]
    pub append: Vec<Value>,
    #[serde(default)]
    pub delete: Vec<String>,
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
        let content = serde_yaml::to_string(self)?;
        crate::utils::atomic_write_file_for_original_user(&path.display().to_string(), &content)
    }

    pub fn merged_groups(
        &self,
        original: &[Value],
        known_proxies: &HashSet<String>,
        known_providers: &HashSet<String>,
    ) -> Result<Vec<Value>> {
        let mut groups =
            Vec::with_capacity(self.prepend.len() + original.len() + self.append.len());
        groups.extend(self.prepend.iter().cloned());
        groups.extend(
            original
                .iter()
                .filter(|group| {
                    group_name(group)
                        .is_none_or(|name| !self.delete.iter().any(|deleted| deleted == name))
                })
                .cloned(),
        );
        groups.extend(self.append.iter().cloned());
        validate_groups(&groups, known_proxies, known_providers)?;
        Ok(groups)
    }
}

pub fn parse_group(source: &str) -> Result<Value> {
    let value: Value = serde_yaml::from_str(source).context("failed to parse proxy group YAML")?;
    validate_group(&value)?;
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

pub fn validate_groups(
    groups: &[Value],
    known_proxies: &HashSet<String>,
    known_providers: &HashSet<String>,
) -> Result<()> {
    let mut names = HashSet::new();
    let mut graph = HashMap::<String, Vec<String>>::new();
    for group in groups {
        validate_group(group)?;
        let name = group_name(group)
            .expect("validated group has name")
            .to_string();
        if !names.insert(name.clone()) {
            bail!("duplicate proxy group name `{name}`")
        }
        let members = group_members(group)?;
        graph.insert(name, members);
    }

    let group_names = names.clone();
    for (group, members) in &graph {
        let is_relay = groups
            .iter()
            .find(|item| group_name(item) == Some(group.as_str()))
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            == Some("relay");
        for member in members {
            if BUILTIN_POLICIES.contains(&member.as_str()) {
                if is_relay {
                    bail!("relay group `{group}` cannot use built-in policy `{member}`")
                }
                continue;
            }
            if group_names.contains(member) || known_proxies.contains(member) {
                continue;
            }
            bail!("proxy group `{group}` references unknown member `{member}`")
        }
    }

    for group in groups {
        let name = group_name(group).expect("validated group has name");
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
    }

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
    Ok(value
        .as_sequence()
        .expect("validated proxies list")
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
    if visiting.contains(group) {
        bail!("proxy group reference cycle detected at `{group}`")
    }
    if !visited.insert(group.to_string()) {
        return Ok(());
    }
    visiting.insert(group.to_string());
    if let Some(members) = graph.get(group) {
        for member in members {
            if graph.contains_key(member) {
                visit_group(member, graph, visiting, visited)?;
            }
        }
    }
    visiting.remove(group);
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
                "name: g-{group_type}\ntype: {group_type}\nproxies: [DIRECT]\nuse: [p]\nurl: http://example.test\ninterval: 300\nlazy: true\ntimeout: 5000\nmax-failed-times: 5\ndisable-udp: true\ninterface-name: eth0\nrouting-mark: 1\ninclude-all: true\ninclude-all-proxies: true\ninclude-all-providers: true\nfilter: x\nexclude-filter: y\nexclude-type: Direct\nexpected-status: 204\nhidden: true\nicon: i"
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
    fn applies_cvr_sequence_order_and_delete() {
        let overlay = GroupsOverlay {
            prepend: vec![value("name: p\ntype: select\nproxies: [DIRECT]")],
            append: vec![value("name: a\ntype: select\nproxies: [DIRECT]")],
            delete: vec!["origin".into()],
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
}
