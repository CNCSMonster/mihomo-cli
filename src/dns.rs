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
#   - domain: internal.example.com
#     target: system
#   - domain: internal.corp
#     target: 192.0.2.53

";
    crate::utils::atomic_write_file(
        &path.display().to_string(),
        &format!("{}{}", header, content),
    )?;
    Ok(())
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

        add_policy_at(&paths, "internal.example.com", "192.0.2.53").unwrap();
        add_policy_at(&paths, "internal.corp", "192.0.2.53").unwrap();

        let list = list_policies_at(&paths).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].1.match_pattern, "+.internal.example.com");
        assert_eq!(list[0].1.target, "192.0.2.53");
        assert_eq!(list[1].1.match_pattern, "+.internal.corp");
        assert_eq!(list[1].1.target, "192.0.2.53");
    }

    #[test]
    fn test_add_dedup() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "internal.example.com", "192.0.2.53").unwrap();
        add_policy_at(&paths, "internal.example.com", "192.0.2.53").unwrap(); // overwrites

        let list = list_policies_at(&paths).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1.target, "192.0.2.53");
    }

    #[test]
    fn test_remove_by_match() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "internal.example.com", "192.0.2.53").unwrap();
        add_policy_at(&paths, "internal.corp", "192.0.2.53").unwrap();

        let removed = remove_policy_at(&paths, "+.internal.example.com").unwrap();
        assert_eq!(removed, "+.internal.example.com");

        let list = list_policies_at(&paths).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1.match_pattern, "+.internal.corp");
    }

    #[test]
    fn test_remove_by_index() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "internal.example.com", "192.0.2.53").unwrap();
        add_policy_at(&paths, "internal.corp", "192.0.2.53").unwrap();

        let removed = remove_policy_at(&paths, "1").unwrap();
        assert_eq!(removed, "+.internal.example.com");

        let list = list_policies_at(&paths).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].1.match_pattern, "+.internal.corp");
    }

    #[test]
    fn test_remove_out_of_range() {
        let (_tmp, paths) = setup_test_paths();

        add_policy_at(&paths, "internal.example.com", "192.0.2.53").unwrap();
        assert!(remove_policy_at(&paths, "5").is_err());
        assert!(remove_policy_at(&paths, "nonexistent").is_err());
    }

    #[test]
    fn test_empty_list() {
        let (_tmp, paths) = setup_test_paths();

        let list = list_policies_at(&paths).unwrap();
        assert!(list.is_empty());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DnsTemplate {
    pub name: &'static str,
    pub description: &'static str,
}

pub fn dns_templates() -> &'static [DnsTemplate] {
    &[
        DnsTemplate {
            name: "company",
            description: "route one internal domain suffix to a company DNS server",
        },
        DnsTemplate {
            name: "ads",
            description: "route common ad/tracker DNS suffixes to a filtering DNS server",
        },
    ]
}

pub fn apply_template_at(
    paths: &AppPaths,
    name: &str,
    domain: Option<&str>,
    target: Option<&str>,
) -> Result<Vec<DnsPolicy>> {
    let policies = match name {
        "company" => {
            let domain = domain
                .ok_or_else(|| anyhow::anyhow!("company template requires --domain <DOMAIN>"))?;
            let target = target
                .ok_or_else(|| anyhow::anyhow!("company template requires --target <DNS>"))?;
            vec![DnsPolicy {
                match_pattern: normalize_match(domain),
                target: target.to_string(),
            }]
        }
        "ads" => {
            let target =
                target.ok_or_else(|| anyhow::anyhow!("ads template requires --target <DNS>"))?;
            vec![
                DnsPolicy {
                    match_pattern: normalize_match("doubleclick.net"),
                    target: target.to_string(),
                },
                DnsPolicy {
                    match_pattern: normalize_match("googlesyndication.com"),
                    target: target.to_string(),
                },
                DnsPolicy {
                    match_pattern: normalize_match("google-analytics.com"),
                    target: target.to_string(),
                },
            ]
        }
        other => anyhow::bail!(
            "Unknown DNS template `{}`. Run: mihomo-cli dns template list",
            other
        ),
    };

    let mut existing = load_policies_at(paths)?;
    for policy in &policies {
        existing.retain(|p| p.match_pattern != policy.match_pattern);
        existing.push(policy.clone());
    }
    save_policies_at(paths, &existing)?;
    Ok(policies)
}

fn normalize_match(match_pattern: &str) -> String {
    if match_pattern.starts_with("+.") {
        match_pattern.to_string()
    } else {
        format!("+.{match_pattern}")
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_paths() -> (TempDir, AppPaths) {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        (tmp, paths)
    }

    #[test]
    fn company_template_requires_domain_and_target() {
        let (_tmp, paths) = setup_test_paths();
        assert!(apply_template_at(&paths, "company", None, Some("10.0.0.1")).is_err());
        assert!(apply_template_at(&paths, "company", Some("corp.example"), None).is_err());
    }

    #[test]
    fn company_template_adds_policy() {
        let (_tmp, paths) = setup_test_paths();
        let added =
            apply_template_at(&paths, "company", Some("corp.example"), Some("10.0.0.1")).unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].match_pattern, "+.corp.example");
        assert_eq!(added[0].target, "10.0.0.1");
        let policies = load_policies_at(&paths).unwrap();
        assert_eq!(policies, added);
    }

    #[test]
    fn ads_template_requires_target_and_dedups() {
        let (_tmp, paths) = setup_test_paths();
        assert!(apply_template_at(&paths, "ads", None, None).is_err());
        let first = apply_template_at(&paths, "ads", None, Some("94.140.14.14")).unwrap();
        let second = apply_template_at(&paths, "ads", None, Some("94.140.14.14")).unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(second.len(), 3);
        let policies = load_policies_at(&paths).unwrap();
        assert_eq!(policies.len(), 3);
        assert!(policies.iter().all(|p| p.target == "94.140.14.14"));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FakeIpFilterFile {
    #[serde(default, rename = "fake-ip-filter")]
    pub filters: Vec<String>,
}

pub fn normalize_fake_ip_filter_domain(domain: &str) -> Result<String> {
    let trimmed = domain.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        anyhow::bail!("domain cannot be empty");
    }
    let bare = trimmed
        .strip_prefix("+.")
        .or_else(|| trimmed.strip_prefix("*."))
        .unwrap_or(trimmed);
    if bare.is_empty() || bare.contains('/') || bare.contains(' ') {
        anyhow::bail!("invalid domain: {domain}");
    }
    Ok(format!("+.{bare}"))
}

pub fn load_fake_ip_filters_at(paths: &AppPaths) -> Result<Vec<String>> {
    let path = paths.dns_fake_ip_filter_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;
    let file: FakeIpFilterFile = serde_yaml::from_str(&content)
        .with_context(|| "Failed to parse dns-fake-ip-filter.yaml")?;
    Ok(file.filters)
}

pub fn save_fake_ip_filters_at(paths: &AppPaths, filters: &[String]) -> Result<()> {
    let path = paths.dns_fake_ip_filter_path();
    std::fs::create_dir_all(paths.config_dir())?;
    let file = FakeIpFilterFile {
        filters: filters.to_vec(),
    };
    let content = serde_yaml::to_string(&file)?;
    let header = "# User-defined DNS fake-ip-filter entries\n# Entries are normalized to +.<domain> suffix match format.\n\n";
    crate::utils::atomic_write_file(
        &path.display().to_string(),
        &format!("{}{}", header, content),
    )?;
    Ok(())
}

pub fn add_fake_ip_filter_at(paths: &AppPaths, domain: &str) -> Result<String> {
    let normalized = normalize_fake_ip_filter_domain(domain)?;
    let mut filters = load_fake_ip_filters_at(paths)?;
    if !filters.iter().any(|f| f == &normalized) {
        filters.push(normalized.clone());
    }
    save_fake_ip_filters_at(paths, &filters)?;
    Ok(normalized)
}

pub fn remove_fake_ip_filter_at(paths: &AppPaths, domain: &str) -> Result<String> {
    let normalized = normalize_fake_ip_filter_domain(domain)?;
    let mut filters = load_fake_ip_filters_at(paths)?;
    let before = filters.len();
    filters.retain(|f| f != &normalized);
    if filters.len() == before {
        anyhow::bail!("No fake-ip-filter entry found for: {domain}");
    }
    save_fake_ip_filters_at(paths, &filters)?;
    Ok(normalized)
}

pub fn list_fake_ip_filters_at(paths: &AppPaths) -> Result<Vec<(usize, String)>> {
    Ok(load_fake_ip_filters_at(paths)?
        .into_iter()
        .enumerate()
        .map(|(i, f)| (i + 1, f))
        .collect())
}

#[cfg(test)]
mod fake_ip_filter_tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, AppPaths) {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        (tmp, paths)
    }

    #[test]
    fn normalizes_fake_ip_filter_domains() {
        assert_eq!(
            normalize_fake_ip_filter_domain("corp.example.com").unwrap(),
            "+.corp.example.com"
        );
        assert_eq!(
            normalize_fake_ip_filter_domain("*.corp.example.com").unwrap(),
            "+.corp.example.com"
        );
        assert_eq!(
            normalize_fake_ip_filter_domain("+.corp.example.com").unwrap(),
            "+.corp.example.com"
        );
    }

    #[test]
    fn add_remove_list_and_dedup_fake_ip_filters() {
        let (_tmp, paths) = setup();
        add_fake_ip_filter_at(&paths, "corp.example.com").unwrap();
        add_fake_ip_filter_at(&paths, "*.corp.example.com").unwrap();
        add_fake_ip_filter_at(&paths, "+.dev.example.com").unwrap();
        let list = list_fake_ip_filters_at(&paths).unwrap();
        assert_eq!(
            list,
            vec![
                (1, "+.corp.example.com".into()),
                (2, "+.dev.example.com".into())
            ]
        );
        let removed = remove_fake_ip_filter_at(&paths, "corp.example.com").unwrap();
        assert_eq!(removed, "+.corp.example.com");
        assert_eq!(
            load_fake_ip_filters_at(&paths).unwrap(),
            vec!["+.dev.example.com"]
        );
    }
}
