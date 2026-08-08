use crate::utils::AppPaths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Rule insertion position
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RulePosition {
    #[default]
    Front,
    Back,
}

impl std::str::FromStr for RulePosition {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "front" => Ok(Self::Front),
            "back" => Ok(Self::Back),
            _ => Err(anyhow::anyhow!(
                "Invalid position: {}. Use 'front' or 'back'",
                s
            )),
        }
    }
}

impl std::fmt::Display for RulePosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Front => write!(f, "front"),
            Self::Back => write!(f, "back"),
        }
    }
}

/// Rules file structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RulesFile {
    #[serde(default)]
    pub rules: Vec<String>,
}

/// Load user rules from rules.yaml using explicit paths.
pub fn load_rules_at(paths: &AppPaths) -> Result<Vec<String>> {
    let path = paths.rules_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read rules file: {}", path.display()))?;

    let rules_file: RulesFile =
        serde_yaml::from_str(&content).with_context(|| "Failed to parse rules.yaml")?;

    Ok(rules_file.rules)
}

/// Save rules to rules.yaml using explicit paths.
pub fn save_rules_at(paths: &AppPaths, rules: &[String]) -> Result<()> {
    let path = paths.rules_path();
    std::fs::create_dir_all(paths.config_dir())?;

    let rules_file = RulesFile {
        rules: rules.to_vec(),
    };

    let content = serde_yaml::to_string(&rules_file)?;
    let header = "# User-defined routing rules\n# Format: TYPE,PARAMETER,POLICY\n# Examples:\n#   - DOMAIN-SUFFIX,example.com,DIRECT\n#   - DOMAIN,google.com,节点选择\n#   - IP-CIDR,192.168.0.0/16,DIRECT\n\n";

    crate::utils::atomic_write_file(
        &path.display().to_string(),
        &format!("{}{}", header, content),
    )?;
    Ok(())
}

/// Add a rule to rules.yaml using explicit paths.
pub fn add_rule_at(paths: &AppPaths, rule: &str, position: Option<RulePosition>) -> Result<()> {
    let mut rules = load_rules_at(paths)?;
    let pos = position.unwrap_or_else(|| get_position_at(paths).unwrap_or_default());

    if rules.iter().any(|existing| existing == rule) {
        return Err(anyhow::anyhow!("Rule already exists: {}", rule));
    }

    match pos {
        RulePosition::Front => rules.insert(0, rule.to_string()),
        RulePosition::Back => rules.push(rule.to_string()),
    }

    save_rules_at(paths, &rules)?;
    Ok(())
}

/// Add a rule to the system rules.yaml.
#[allow(dead_code)]
pub fn add_rule(rule: &str, position: Option<RulePosition>) -> Result<()> {
    add_rule_at(&AppPaths::from_system(), rule, position)
}

/// Remove a rule by index (0-based) using explicit paths.
pub fn remove_rule_at(paths: &AppPaths, index: usize) -> Result<()> {
    let mut rules = load_rules_at(paths)?;
    if index >= rules.len() {
        return Err(anyhow::anyhow!(
            "Rule index {} out of range (0-{})",
            index,
            rules.len().saturating_sub(1)
        ));
    }

    rules.remove(index);
    save_rules_at(paths, &rules)?;
    Ok(())
}

/// Remove a rule by index (0-based) from the system rules.yaml.
#[allow(dead_code)]
pub fn remove_rule(index: usize) -> Result<()> {
    remove_rule_at(&AppPaths::from_system(), index)
}

/// Move a rule from one 0-based index to another using explicit paths.
pub fn move_rule_at(paths: &AppPaths, from: usize, to: usize) -> Result<()> {
    let mut rules = load_rules_at(paths)?;
    if from >= rules.len() || to >= rules.len() {
        anyhow::bail!("Rule index out of range (valid range: 1-{})", rules.len());
    }
    if from == to {
        return Ok(());
    }
    let rule = rules.remove(from);
    rules.insert(to, rule);
    save_rules_at(paths, &rules)?;
    Ok(())
}

/// Move a system rule from one 0-based index to another.
#[allow(dead_code)]
pub fn move_rule(from: usize, to: usize) -> Result<()> {
    move_rule_at(&AppPaths::from_system(), from, to)
}

/// Clear all rules using explicit paths.
pub fn clear_rules_at(paths: &AppPaths) -> Result<()> {
    save_rules_at(paths, &Vec::new())
}

/// Clear all system rules.
#[allow(dead_code)]
pub fn clear_rules() -> Result<()> {
    clear_rules_at(&AppPaths::from_system())
}

/// List all rules using explicit paths.
pub fn list_rules_at(paths: &AppPaths) -> Result<Vec<String>> {
    load_rules_at(paths)
}

/// Get the default insertion position using explicit paths.
pub fn get_position_at(paths: &AppPaths) -> Result<RulePosition> {
    let path = paths.rules_position_path();
    if !Path::new(&path).exists() {
        return Ok(RulePosition::default());
    }

    let content = std::fs::read_to_string(&path).with_context(|| "Failed to read position file")?;

    content.trim().parse()
}

/// Set the default insertion position using explicit paths.
pub fn set_position_at(paths: &AppPaths, position: RulePosition) -> Result<()> {
    let path = paths.rules_position_path();
    std::fs::create_dir_all(paths.config_dir())?;
    crate::utils::atomic_write_file(&path.display().to_string(), &position.to_string())?;
    Ok(())
}

/// Import rules from an external file using explicit target paths.
pub fn import_rules_at(paths: &AppPaths, source_path: &str) -> Result<()> {
    let content = std::fs::read_to_string(source_path)
        .with_context(|| format!("Failed to read source file: {}", source_path))?;

    let rules_file: RulesFile =
        serde_yaml::from_str(&content).with_context(|| "Failed to parse source rules file")?;

    save_rules_at(paths, &rules_file.rules)?;
    Ok(())
}

/// Import rules from an external file into the system rules.yaml.
#[allow(dead_code)]
pub fn import_rules(source_path: &str) -> Result<()> {
    import_rules_at(&AppPaths::from_system(), source_path)
}

/// Export rules to an external file using explicit source paths.
pub fn export_rules_at(paths: &AppPaths, dest_path: &str) -> Result<()> {
    let rules = load_rules_at(paths)?;
    let rules_file = RulesFile { rules };
    let content = serde_yaml::to_string(&rules_file)?;
    crate::utils::atomic_write_file(dest_path, &content)?;
    Ok(())
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
    fn test_rule_position_default() {
        assert_eq!(RulePosition::default(), RulePosition::Front);
    }

    #[test]
    fn test_rule_position_parse() {
        assert_eq!(
            "front".parse::<RulePosition>().unwrap(),
            RulePosition::Front
        );
        assert_eq!("back".parse::<RulePosition>().unwrap(), RulePosition::Back);
        assert!("invalid".parse::<RulePosition>().is_err());
    }

    #[test]
    fn test_save_and_load_rules() {
        let (_tmp, paths) = setup_test_paths();
        let rules = vec![
            "DOMAIN-SUFFIX,example.com,DIRECT".to_string(),
            "DOMAIN,google.com,节点选择".to_string(),
        ];

        save_rules_at(&paths, &rules).unwrap();
        let loaded = load_rules_at(&paths).unwrap();
        assert_eq!(loaded, rules);
    }

    #[test]
    fn test_add_rule_front() {
        let (_tmp, paths) = setup_test_paths();
        save_rules_at(&paths, &["EXISTING,RULE".to_string()]).unwrap();

        add_rule_at(&paths, "NEW,RULE", Some(RulePosition::Front)).unwrap();
        let rules = load_rules_at(&paths).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0], "NEW,RULE");
        assert_eq!(rules[1], "EXISTING,RULE");
    }

    #[test]
    fn test_add_rule_back() {
        let (_tmp, paths) = setup_test_paths();
        save_rules_at(&paths, &["EXISTING,RULE".to_string()]).unwrap();

        add_rule_at(&paths, "NEW,RULE", Some(RulePosition::Back)).unwrap();
        let rules = load_rules_at(&paths).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0], "EXISTING,RULE");
        assert_eq!(rules[1], "NEW,RULE");
    }

    #[test]
    fn test_remove_rule() {
        let (_tmp, paths) = setup_test_paths();
        save_rules_at(
            &paths,
            &[
                "RULE1".to_string(),
                "RULE2".to_string(),
                "RULE3".to_string(),
            ],
        )
        .unwrap();

        remove_rule_at(&paths, 1).unwrap();
        let rules = load_rules_at(&paths).unwrap();
        assert_eq!(rules, vec!["RULE1".to_string(), "RULE3".to_string()]);
    }

    #[test]
    fn test_remove_rule_out_of_range() {
        let (_tmp, paths) = setup_test_paths();
        save_rules_at(&paths, &["RULE1".to_string()]).unwrap();
        assert!(remove_rule_at(&paths, 5).is_err());
    }

    #[test]
    fn test_clear_rules() {
        let (_tmp, paths) = setup_test_paths();
        save_rules_at(&paths, &["RULE1".to_string(), "RULE2".to_string()]).unwrap();

        clear_rules_at(&paths).unwrap();
        let rules = load_rules_at(&paths).unwrap();
        assert!(rules.is_empty());
    }

    #[test]
    fn test_position_persistence() {
        let (_tmp, paths) = setup_test_paths();

        set_position_at(&paths, RulePosition::Back).unwrap();
        assert_eq!(get_position_at(&paths).unwrap(), RulePosition::Back);

        set_position_at(&paths, RulePosition::Front).unwrap();
        assert_eq!(get_position_at(&paths).unwrap(), RulePosition::Front);
    }
}

#[derive(Debug, Clone, Copy)]
struct RuleTypeSpec {
    name: &'static str,
    example_param: &'static str,
    supports_no_resolve: bool,
    description: &'static str,
}

const RULE_TYPES: &[RuleTypeSpec] = &[
    RuleTypeSpec {
        name: "DOMAIN",
        example_param: "example.com",
        supports_no_resolve: false,
        description: "exact domain",
    },
    RuleTypeSpec {
        name: "DOMAIN-SUFFIX",
        example_param: "example.com",
        supports_no_resolve: false,
        description: "domain suffix",
    },
    RuleTypeSpec {
        name: "DOMAIN-KEYWORD",
        example_param: "google",
        supports_no_resolve: false,
        description: "domain keyword",
    },
    RuleTypeSpec {
        name: "GEOSITE",
        example_param: "cn",
        supports_no_resolve: false,
        description: "GeoSite set",
    },
    RuleTypeSpec {
        name: "IP-CIDR",
        example_param: "192.168.0.0/16",
        supports_no_resolve: true,
        description: "IPv4 CIDR",
    },
    RuleTypeSpec {
        name: "IP-CIDR6",
        example_param: "2001:db8::/32",
        supports_no_resolve: true,
        description: "IPv6 CIDR",
    },
    RuleTypeSpec {
        name: "GEOIP",
        example_param: "CN",
        supports_no_resolve: true,
        description: "GeoIP country code",
    },
    RuleTypeSpec {
        name: "SRC-IP-CIDR",
        example_param: "10.0.0.0/8",
        supports_no_resolve: false,
        description: "source IP CIDR",
    },
    RuleTypeSpec {
        name: "SRC-PORT",
        example_param: "443",
        supports_no_resolve: false,
        description: "source port",
    },
    RuleTypeSpec {
        name: "DST-PORT",
        example_param: "443",
        supports_no_resolve: false,
        description: "destination port",
    },
    RuleTypeSpec {
        name: "PROCESS-NAME",
        example_param: "curl",
        supports_no_resolve: false,
        description: "process name",
    },
    RuleTypeSpec {
        name: "PROCESS-PATH",
        example_param: "/usr/bin/curl",
        supports_no_resolve: false,
        description: "process path",
    },
    RuleTypeSpec {
        name: "NETWORK",
        example_param: "tcp",
        supports_no_resolve: false,
        description: "tcp or udp",
    },
    RuleTypeSpec {
        name: "MATCH",
        example_param: "",
        supports_no_resolve: false,
        description: "fallback rule",
    },
];

pub fn print_rule_types() {
    println!("  Supported rule types:");
    for spec in RULE_TYPES {
        let example = if spec.name == "MATCH" {
            "MATCH,DIRECT".to_string()
        } else {
            format!(
                "{},{},DIRECT{}",
                spec.name,
                spec.example_param,
                if spec.supports_no_resolve {
                    ",no-resolve"
                } else {
                    ""
                }
            )
        };
        println!(
            "  - {:15} no-resolve: {:3}  {:20}  e.g. {}",
            spec.name,
            if spec.supports_no_resolve {
                "yes"
            } else {
                "no"
            },
            spec.description,
            example
        );
    }
}

pub fn validate_rules_file(path: &str) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read source file: {}", path))?;
    let rules_file: RulesFile =
        serde_yaml::from_str(&content).with_context(|| "Failed to parse source rules file")?;
    for rule in &rules_file.rules {
        validate_rule(rule)?;
    }
    Ok(())
}

pub fn validate_rule(rule: &str) -> Result<()> {
    let parts: Vec<&str> = rule.split(',').map(str::trim).collect();
    if parts.len() < 2 {
        anyhow::bail!(
            "Invalid rule `{}`: expected TYPE,PARAMETER,POLICY or MATCH,POLICY",
            rule
        );
    }
    let typ = parts[0].to_ascii_uppercase();
    let spec = RULE_TYPES.iter().find(|s| s.name == typ).ok_or_else(|| {
        anyhow::anyhow!(
            "Unsupported rule type `{}`. Run: mihomo-cli rule types",
            parts[0]
        )
    })?;

    if typ == "MATCH" {
        if parts.len() != 2 {
            anyhow::bail!("MATCH rule format is MATCH,POLICY");
        }
        validate_policy_token(parts[1])?;
        return Ok(());
    }
    if parts.len() < 3 || parts.len() > 4 {
        anyhow::bail!(
            "Invalid rule `{}`: expected TYPE,PARAMETER,POLICY[,no-resolve]",
            rule
        );
    }
    let param = parts[1];
    if param.is_empty() {
        anyhow::bail!("Rule parameter cannot be empty");
    }
    validate_policy_token(parts[2])?;
    if parts.len() == 4 {
        if parts[3] != "no-resolve" {
            anyhow::bail!(
                "Unknown rule option `{}`; only `no-resolve` is supported",
                parts[3]
            );
        }
        if !spec.supports_no_resolve {
            anyhow::bail!("{} does not support no-resolve", typ);
        }
    }
    match typ.as_str() {
        "IP-CIDR" | "SRC-IP-CIDR" => validate_cidr(param, false)?,
        "IP-CIDR6" => validate_cidr(param, true)?,
        "SRC-PORT" | "DST-PORT" => validate_port(param)?,
        "NETWORK" => {
            if param != "tcp" && param != "udp" {
                anyhow::bail!("NETWORK must be `tcp` or `udp`");
            }
        }
        "GEOIP" if param.len() != 2 => {
            anyhow::bail!("GEOIP expects a 2-letter country code, e.g. CN");
        }
        _ => {}
    }
    Ok(())
}

fn validate_policy_token(policy: &str) -> Result<()> {
    if policy.trim().is_empty() {
        anyhow::bail!("Rule policy cannot be empty");
    }
    Ok(())
}

fn validate_port(s: &str) -> Result<()> {
    let p: u16 = s
        .parse()
        .map_err(|_| anyhow::anyhow!("Port must be an integer from 1 to 65535"))?;
    if p == 0 {
        anyhow::bail!("Port must be from 1 to 65535");
    }
    Ok(())
}

fn validate_cidr(s: &str, ipv6: bool) -> Result<()> {
    let (ip, prefix) = s
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("CIDR must contain `/`, e.g. 192.168.0.0/16"))?;
    let prefix: u8 = prefix
        .parse()
        .map_err(|_| anyhow::anyhow!("CIDR prefix must be a number"))?;
    if ipv6 {
        let _: std::net::Ipv6Addr = ip
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid IPv6 address in CIDR"))?;
        if prefix > 128 {
            anyhow::bail!("IPv6 CIDR prefix must be 0..128");
        }
    } else {
        let _: std::net::Ipv4Addr = ip
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid IPv4 address in CIDR"))?;
        if prefix > 32 {
            anyhow::bail!("IPv4 CIDR prefix must be 0..32");
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SelectionStateFile {
    #[serde(default)]
    pub selections: std::collections::BTreeMap<String, String>,
}

pub fn load_selection_state_at(
    paths: &AppPaths,
) -> Result<std::collections::BTreeMap<String, String>> {
    let path = paths.selection_state_path();
    if !path.exists() {
        return Ok(std::collections::BTreeMap::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read selection state: {}", path.display()))?;
    let state: SelectionStateFile =
        serde_yaml::from_str(&content).with_context(|| "Failed to parse selection-state.yaml")?;
    Ok(state.selections)
}

pub fn save_selection_state_at(
    paths: &AppPaths,
    selections: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    std::fs::create_dir_all(paths.config_dir())?;
    let state = SelectionStateFile {
        selections: selections.clone(),
    };
    let content = serde_yaml::to_string(&state)?;
    crate::utils::atomic_write_file(
        &paths.selection_state_path().display().to_string(),
        &format!("# Last selections made by mihomo-cli; used only for drift warnings.\n{content}"),
    )?;
    Ok(())
}

pub fn remember_selection_at(paths: &AppPaths, group: &str, node: &str) -> Result<()> {
    let mut selections = load_selection_state_at(paths)?;
    selections.insert(group.to_string(), node.to_string());
    save_selection_state_at(paths, &selections)
}

pub fn selection_drift_warnings_at(paths: &AppPaths) -> Result<Vec<String>> {
    let selections = load_selection_state_at(paths)?;
    if selections.is_empty() {
        return Ok(Vec::new());
    }
    let config_path = paths.config_path();
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
    selection_drift_warnings_in_config(&content, &selections)
}

pub fn selection_drift_warnings_in_config(
    config_content: &str,
    selections: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<String>> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(config_content).with_context(|| "Failed to parse config.yaml")?;
    let groups = yaml
        .get("proxy-groups")
        .and_then(|v| v.as_sequence())
        .ok_or_else(|| anyhow::anyhow!("config.yaml does not contain proxy-groups"))?;
    let mut members =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for group in groups {
        let Some(name) = group.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let set = group
            .get("proxies")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(ToString::to_string))
                    .collect::<std::collections::BTreeSet<_>>()
            })
            .unwrap_or_default();
        members.insert(name.to_string(), set);
    }
    let mut warnings = Vec::new();
    for (group, selected) in selections {
        match members.get(group) {
            None => warnings.push(format!(
                "Warning: selected group `{group}` is not available in current proxy groups."
            )),
            Some(nodes) if !nodes.contains(selected) => warnings.push(format!(
                "Warning: selected node `{selected}` is not available in group `{group}`."
            )),
            Some(_) => {}
        }
    }
    Ok(warnings)
}

/// Extract the policy/target token from a Clash rule string.
pub fn rule_policy(rule: &str) -> Option<&str> {
    let parts: Vec<&str> = rule.split(',').map(str::trim).collect();
    if parts.first()?.eq_ignore_ascii_case("MATCH") {
        parts.get(1).copied()
    } else {
        parts.get(2).copied()
    }
}

/// Warn when a rule points at a policy/group not present in the current config.
/// This intentionally reports facts only; it never rewrites user rules.
pub fn rule_policy_warnings_at(paths: &AppPaths, rules: &[String]) -> Result<Vec<String>> {
    let policies = available_policies_at(paths)?;
    let mut warnings = Vec::new();
    for rule in rules {
        if let Some(policy) = rule_policy(rule) {
            if !policies.iter().any(|p| p == policy) {
                warnings.push(format!(
                    "Warning: rule target `{policy}` is not available in current proxy groups/policies. Rule: {rule}"
                ));
            }
        }
    }
    Ok(warnings)
}

pub fn available_policies_at(paths: &AppPaths) -> Result<Vec<String>> {
    let mut policies = vec![
        "DIRECT".to_string(),
        "REJECT".to_string(),
        "REJECT-DROP".to_string(),
        "PASS".to_string(),
    ];
    let path = paths.config_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            if let Some(groups) = yaml.get("proxy-groups").and_then(|v| v.as_sequence()) {
                for g in groups {
                    if let Some(name) = g.get("name").and_then(|v| v.as_str()) {
                        if !policies.iter().any(|p| p == name) {
                            policies.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(policies)
}

pub fn available_policies() -> Result<Vec<String>> {
    available_policies_at(&AppPaths::from_system())
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validate_rule_accepts_common_valid_rules() {
        for rule in [
            "DOMAIN-SUFFIX,example.com,DIRECT",
            "DOMAIN,example.com,Proxy",
            "DOMAIN-KEYWORD,google,节点选择",
            "IP-CIDR,192.168.0.0/16,DIRECT,no-resolve",
            "IP-CIDR6,2001:db8::/32,DIRECT,no-resolve",
            "SRC-IP-CIDR,10.0.0.0/8,DIRECT",
            "SRC-PORT,443,DIRECT",
            "DST-PORT,65535,DIRECT",
            "NETWORK,tcp,DIRECT",
            "NETWORK,udp,DIRECT",
            "GEOIP,CN,DIRECT,no-resolve",
            "MATCH,DIRECT",
        ] {
            validate_rule(rule).unwrap_or_else(|e| panic!("{rule} should be valid: {e}"));
        }
    }

    #[test]
    fn validate_rule_rejects_bad_cidr() {
        assert!(validate_rule("IP-CIDR,999.0.0.0/8,DIRECT").is_err());
        assert!(validate_rule("IP-CIDR,192.168.0.0/33,DIRECT").is_err());
        assert!(validate_rule("IP-CIDR6,2001:db8::/129,DIRECT").is_err());
        assert!(validate_rule("IP-CIDR,192.168.0.0,DIRECT").is_err());
    }

    #[test]
    fn validate_rule_rejects_bad_ports_and_networks() {
        assert!(validate_rule("DST-PORT,0,DIRECT").is_err());
        assert!(validate_rule("DST-PORT,65536,DIRECT").is_err());
        assert!(validate_rule("DST-PORT,abc,DIRECT").is_err());
        assert!(validate_rule("NETWORK,http,DIRECT").is_err());
    }

    #[test]
    fn validate_rule_rejects_bad_shape_and_options() {
        assert!(validate_rule("UNKNOWN,x,DIRECT").is_err());
        assert!(validate_rule("DOMAIN,,DIRECT").is_err());
        assert!(validate_rule("DOMAIN,example.com,").is_err());
        assert!(validate_rule("DOMAIN,example.com,DIRECT,no-resolve").is_err());
        assert!(validate_rule("IP-CIDR,192.168.0.0/16,DIRECT,extra").is_err());
        assert!(validate_rule("MATCH,example.com,DIRECT").is_err());
    }

    #[test]
    fn validate_rules_file_checks_each_rule() {
        let tmp = TempDir::new().unwrap();
        let ok = tmp.path().join("ok.yaml");
        std::fs::write(
            &ok,
            "rules:\n  - DOMAIN-SUFFIX,example.com,DIRECT\n  - MATCH,DIRECT\n",
        )
        .unwrap();
        validate_rules_file(ok.to_str().unwrap()).unwrap();

        let bad = tmp.path().join("bad.yaml");
        std::fs::write(&bad, "rules:\n  - DST-PORT,70000,DIRECT\n").unwrap();
        assert!(validate_rules_file(bad.to_str().unwrap()).is_err());
    }

    #[test]
    fn selection_drift_reports_missing_group_and_node() {
        let mut selections = std::collections::BTreeMap::new();
        selections.insert("OpenAI".to_string(), "US-01".to_string());
        selections.insert("Netflix".to_string(), "JP-01".to_string());
        let config = r#"
proxy-groups:
  - name: OpenAI
    type: select
    proxies:
      - US-02
"#;
        let warnings = selection_drift_warnings_in_config(config, &selections).unwrap();
        assert!(warnings.contains(
            &"Warning: selected node `US-01` is not available in group `OpenAI`.".to_string()
        ));
        assert!(warnings.contains(
            &"Warning: selected group `Netflix` is not available in current proxy groups."
                .to_string()
        ));
    }

    #[test]
    fn available_policies_includes_builtins_and_proxy_groups_without_duplicates() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.config_path(),
            r#"
proxy-groups:
  - name: Proxy
    type: select
  - name: DIRECT
    type: select
  - name: Final
    type: select
"#,
        )
        .unwrap();

        let policies = available_policies_at(&paths).unwrap();
        assert_eq!(policies[0], "DIRECT");
        assert!(policies.contains(&"REJECT".to_string()));
        assert!(policies.contains(&"Proxy".to_string()));
        assert!(policies.contains(&"Final".to_string()));
        assert_eq!(
            policies.iter().filter(|p| p.as_str() == "DIRECT").count(),
            1
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatch {
    pub index: usize,
    pub rule: String,
    pub policy: String,
}

pub fn test_rule_match_at(paths: &AppPaths, target: &str) -> Result<Option<RuleMatch>> {
    let config_path = paths.config_path();
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
    test_rule_match_in_config(&content, target)
}

pub fn test_rule_match_in_config(config_content: &str, target: &str) -> Result<Option<RuleMatch>> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(config_content).with_context(|| "Failed to parse config.yaml")?;
    let rules = yaml
        .get("rules")
        .and_then(|v| v.as_sequence())
        .ok_or_else(|| anyhow::anyhow!("config.yaml does not contain a rules array"))?;

    for (idx, rule_value) in rules.iter().enumerate() {
        let Some(rule) = rule_value.as_str() else {
            continue;
        };
        if rule_matches_target(rule, target)? {
            return Ok(Some(RuleMatch {
                index: idx + 1,
                rule: rule.to_string(),
                policy: rule_policy(rule).unwrap_or_default().to_string(),
            }));
        }
    }

    Ok(None)
}

fn rule_matches_target(rule: &str, target: &str) -> Result<bool> {
    let parts: Vec<&str> = rule.split(',').map(str::trim).collect();
    if parts.is_empty() {
        return Ok(false);
    }
    let typ = parts[0].to_ascii_uppercase();
    if typ == "MATCH" {
        return Ok(true);
    }
    if parts.len() < 3 {
        return Ok(false);
    }
    let param = parts[1];
    let target_lc = target.trim().trim_end_matches('.').to_ascii_lowercase();
    let param_lc = param
        .trim()
        .trim_start_matches("+.")
        .trim_start_matches('.')
        .trim_end_matches('.')
        .to_ascii_lowercase();

    match typ.as_str() {
        "DOMAIN" => Ok(target_lc == param_lc),
        "DOMAIN-SUFFIX" => {
            Ok(target_lc == param_lc || target_lc.ends_with(&format!(".{param_lc}")))
        }
        "DOMAIN-KEYWORD" => Ok(target_lc.contains(&param_lc)),
        "IP-CIDR" | "SRC-IP-CIDR" => match target.parse::<std::net::Ipv4Addr>() {
            Ok(ip) => ipv4_in_cidr(ip, param),
            Err(_) => Ok(false),
        },
        "IP-CIDR6" => match target.parse::<std::net::Ipv6Addr>() {
            Ok(ip) => ipv6_in_cidr(ip, param),
            Err(_) => Ok(false),
        },
        // Geo databases, process rules, ports, and NETWORK need runtime context.
        // Offline `rule test` skips them and continues to later rules.
        _ => Ok(false),
    }
}

fn ipv4_in_cidr(ip: std::net::Ipv4Addr, cidr: &str) -> Result<bool> {
    let (network, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("Invalid IPv4 CIDR in rule: {cidr}"))?;
    let network: std::net::Ipv4Addr = network
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid IPv4 CIDR address in rule: {cidr}"))?;
    let prefix: u32 = prefix
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid IPv4 CIDR prefix in rule: {cidr}"))?;
    if prefix > 32 {
        anyhow::bail!("Invalid IPv4 CIDR prefix in rule: {cidr}");
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Ok((u32::from(ip) & mask) == (u32::from(network) & mask))
}

fn ipv6_in_cidr(ip: std::net::Ipv6Addr, cidr: &str) -> Result<bool> {
    let (network, prefix) = cidr
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("Invalid IPv6 CIDR in rule: {cidr}"))?;
    let network: std::net::Ipv6Addr = network
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid IPv6 CIDR address in rule: {cidr}"))?;
    let prefix: u32 = prefix
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid IPv6 CIDR prefix in rule: {cidr}"))?;
    if prefix > 128 {
        anyhow::bail!("Invalid IPv6 CIDR prefix in rule: {cidr}");
    }
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    Ok((u128::from(ip) & mask) == (u128::from(network) & mask))
}

#[cfg(test)]
mod rule_test_tests {
    use super::*;

    #[test]
    fn test_rule_match_uses_first_matching_rule() {
        let config = r#"
rules:
  - DOMAIN,example.com,DIRECT
  - DOMAIN-SUFFIX,example.com,Proxy
  - MATCH,Final
"#;
        let matched = test_rule_match_in_config(config, "example.com")
            .unwrap()
            .unwrap();
        assert_eq!(matched.index, 1);
        assert_eq!(matched.policy, "DIRECT");
    }

    #[test]
    fn test_rule_match_supports_suffix_keyword_and_match() {
        let config = r#"
rules:
  - DOMAIN-SUFFIX,example.com,DIRECT
  - DOMAIN-KEYWORD,google,Proxy
  - MATCH,Final
"#;
        assert_eq!(
            test_rule_match_in_config(config, "api.example.com")
                .unwrap()
                .unwrap()
                .policy,
            "DIRECT"
        );
        assert_eq!(
            test_rule_match_in_config(config, "www.google.com")
                .unwrap()
                .unwrap()
                .policy,
            "Proxy"
        );
        assert_eq!(
            test_rule_match_in_config(config, "unmatched.test")
                .unwrap()
                .unwrap()
                .policy,
            "Final"
        );
    }

    #[test]
    fn test_rule_match_supports_ip_cidr() {
        let config = r#"
rules:
  - IP-CIDR,10.0.0.0/8,DIRECT
  - IP-CIDR6,2001:db8::/32,Proxy
  - MATCH,Final
"#;
        assert_eq!(
            test_rule_match_in_config(config, "10.1.2.3")
                .unwrap()
                .unwrap()
                .policy,
            "DIRECT"
        );
        assert_eq!(
            test_rule_match_in_config(config, "2001:db8::1")
                .unwrap()
                .unwrap()
                .policy,
            "Proxy"
        );
        assert_eq!(
            test_rule_match_in_config(config, "192.168.1.1")
                .unwrap()
                .unwrap()
                .policy,
            "Final"
        );
    }
}
