use crate::utils::AppPaths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single DNS policy entry: domain match → target DNS
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DnsPolicy {
    #[serde(rename = "domain")]
    pub match_pattern: String,
    pub target: String,
}

impl std::fmt::Display for DnsPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} → {}", self.match_pattern, self.target)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DnsPolicyFile {
    #[serde(default)]
    pub policies: Vec<DnsPolicy>,
}

/// Load DNS policies from dns-policy.yaml using explicit paths.
pub fn load_policies_at(paths: &AppPaths) -> Result<Vec<DnsPolicy>> {
    let path = paths.dns_policy_path();
    if !Path::new(&path).exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;
    let file: DnsPolicyFile =
        serde_yaml::from_str(&content).with_context(|| "Failed to parse dns-policy.yaml")?;
    Ok(file.policies)
}

/// Load DNS policies from the system dns-policy.yaml.
pub fn load_policies() -> Result<Vec<DnsPolicy>> {
    load_policies_at(&AppPaths::from_system())
}

/// Save DNS policies to dns-policy.yaml using explicit paths.
pub fn save_policies_at(paths: &AppPaths, policies: &[DnsPolicy]) -> Result<()> {
    let path = paths.dns_policy_path();
    std::fs::create_dir_all(paths.config_dir())?;
    let file = DnsPolicyFile {
        policies: policies.to_vec(),
    };
    let content = serde_yaml::to_string(&file)?;
    let header = "# User-defined DNS routing policies
# Format:
#   - domain: <domain>
#     target: <system|IP>
# Examples:
#   - domain: ubtrobot.com
#     target: system
#   - domain: internal.corp
#     target: 10.10.1.251

";
    std::fs::write(&path, format!("{}{}", header, content))?;
    Ok(())
}

/// Save DNS policies to the system dns-policy.yaml.
#[allow(dead_code)]
pub fn save_policies(policies: &[DnsPolicy]) -> Result<()> {
    save_policies_at(&AppPaths::from_system(), policies)
}

/// Add a DNS policy using explicit paths.
pub fn add_policy_at(paths: &AppPaths, match_pattern: &str, target: &str) -> Result<()> {
    let mut policies = load_policies_at(paths)?;

    // Auto-prefix with "+." for suffix matching (matches all subdomains)
    let normalized = if match_pattern.starts_with("+.") {
        match_pattern.to_string()
    } else {
        format!("+.{match_pattern}")
    };

    // Dedup: replace if match already exists
    policies.retain(|p| p.match_pattern != normalized);
    policies.push(DnsPolicy {
        match_pattern: normalized,
        target: target.to_string(),
    });

    save_policies_at(paths, &policies)?;
    Ok(())
}

/// Add a DNS policy to the system dns-policy.yaml.
pub fn add_policy(match_pattern: &str, target: &str) -> Result<()> {
    add_policy_at(&AppPaths::from_system(), match_pattern, target)
}

/// Remove a DNS policy by match pattern or by 1-based index using explicit paths.
pub fn remove_policy_at(paths: &AppPaths, selector: &str) -> Result<String> {
    let mut policies = load_policies_at(paths)?;

    // Try index first
    if let Ok(idx) = selector.parse::<usize>() {
        if idx < 1 || idx > policies.len() {
            return Err(anyhow::anyhow!(
                "Index {} out of range (1-{})",
                idx,
                policies.len()
            ));
        }
        let removed = policies.remove(idx - 1);
        save_policies_at(paths, &policies)?;
        return Ok(removed.match_pattern);
    }

    // Try match pattern (normalize: add +. if not present)
    let normalized = if selector.starts_with("+.") {
        selector.to_string()
    } else {
        format!("+.{selector}")
    };
    let len_before = policies.len();
    policies.retain(|p| p.match_pattern != normalized);
    if policies.len() == len_before {
        return Err(anyhow::anyhow!("No policy found for: {}", selector));
    }

    save_policies_at(paths, &policies)?;
    Ok(selector.to_string())
}

/// Remove a DNS policy by match pattern or by 1-based index from the system dns-policy.yaml.
pub fn remove_policy(selector: &str) -> Result<String> {
    remove_policy_at(&AppPaths::from_system(), selector)
}

/// List all DNS policies (returns (index, policy) pairs) using explicit paths.
pub fn list_policies_at(paths: &AppPaths) -> Result<Vec<(usize, DnsPolicy)>> {
    let policies = load_policies_at(paths)?;
    Ok(policies
        .into_iter()
        .enumerate()
        .map(|(i, p)| (i + 1, p))
        .collect())
}

/// List all system DNS policies (returns (index, policy) pairs).
pub fn list_policies() -> Result<Vec<(usize, DnsPolicy)>> {
    list_policies_at(&AppPaths::from_system())
}

/// Check if any policies are defined using explicit paths.
pub fn has_policies_at(paths: &AppPaths) -> Result<bool> {
    Ok(!load_policies_at(paths)?.is_empty())
}

/// Check if any system policies are defined.
#[allow(dead_code)]
pub fn has_policies() -> Result<bool> {
    has_policies_at(&AppPaths::from_system())
}

/// Convert policies to mihomo nameserver-policy format (YAML Value mapping).
/// Each policy target is a comma-separated list of DNS server IPs.
#[allow(dead_code)]
pub fn to_nameserver_policy(policies: &[DnsPolicy]) -> serde_yaml::Value {
    let mut map = serde_yaml::Mapping::new();
    for p in policies {
        let ips: Vec<&str> = p.target.split(',').map(|s| s.trim()).collect();
        let value = if ips.len() == 1 {
            serde_yaml::Value::String(ips[0].to_string())
        } else {
            serde_yaml::Value::Sequence(
                ips.iter()
                    .map(|ip| serde_yaml::Value::String(ip.to_string()))
                    .collect(),
            )
        };
        map.insert(serde_yaml::Value::String(p.match_pattern.clone()), value);
    }
    serde_yaml::Value::Mapping(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_paths() -> (TempDir, AppPaths) {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        (tmp, paths)
    }

    #[test]
    fn test_add_and_list_policies() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "ubtrobot.com", "10.10.1.251").unwrap();
        add_policy_at(&paths, "internal.corp", "10.10.1.251").unwrap();

        let list = list_policies_at(&paths).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].1.match_pattern, "+.ubtrobot.com");
        assert_eq!(list[0].1.target, "10.10.1.251");
        assert_eq!(list[1].1.match_pattern, "+.internal.corp");
        assert_eq!(list[1].1.target, "10.10.1.251");
    }

    #[test]
    fn test_add_dedup() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "ubtrobot.com", "10.10.1.251").unwrap();
        add_policy_at(&paths, "ubtrobot.com", "10.10.1.251").unwrap(); // overwrites

        let list = list_policies_at(&paths).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1.target, "10.10.1.251");
    }

    #[test]
    fn test_remove_by_match() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "ubtrobot.com", "10.10.1.251").unwrap();
        add_policy_at(&paths, "internal.corp", "10.10.1.251").unwrap();

        let removed = remove_policy_at(&paths, "+.ubtrobot.com").unwrap();
        assert_eq!(removed, "+.ubtrobot.com");

        let list = list_policies_at(&paths).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1.match_pattern, "+.internal.corp");
    }

    #[test]
    fn test_remove_by_index() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "ubtrobot.com", "10.10.1.251").unwrap();
        add_policy_at(&paths, "internal.corp", "10.10.1.251").unwrap();

        let removed = remove_policy_at(&paths, "1").unwrap();
        assert_eq!(removed, "+.ubtrobot.com");

        let list = list_policies_at(&paths).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1.match_pattern, "+.internal.corp");
    }

    #[test]
    fn test_remove_out_of_range() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "ubtrobot.com", "10.10.1.251").unwrap();
        assert!(remove_policy_at(&paths, "5").is_err());
        assert!(remove_policy_at(&paths, "nonexistent").is_err());
    }

    #[test]
    fn test_empty_list() {
        let (_tmp, paths) = setup_test_paths();

        let list = list_policies_at(&paths).unwrap();
        assert!(list.is_empty());
        assert!(!has_policies_at(&paths).unwrap());
    }

    #[test]
    fn test_to_nameserver_policy() {
        let policies = vec![
            DnsPolicy {
                match_pattern: "ubtrobot.com".to_string(),
                target: "10.10.1.251,10.10.1.120".to_string(),
            },
            DnsPolicy {
                match_pattern: "internal.corp".to_string(),
                target: "10.10.1.251".to_string(),
            },
        ];

        let value = to_nameserver_policy(&policies);
        let mapping = value.as_mapping().unwrap();
        assert_eq!(mapping.len(), 2);

        // Single IP
        assert_eq!(mapping["internal.corp"].as_str().unwrap(), "10.10.1.251");

        // Multiple IPs
        let seq = mapping["ubtrobot.com"].as_sequence().unwrap();
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0].as_str().unwrap(), "10.10.1.251");
        assert_eq!(seq[1].as_str().unwrap(), "10.10.1.120");
    }
}
