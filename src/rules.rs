use crate::utils::AppPaths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Rule insertion position
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

/// Load user rules from the system rules.yaml.
#[allow(dead_code)]
pub fn load_rules() -> Result<Vec<String>> {
    load_rules_at(&AppPaths::from_system())
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

    std::fs::write(&path, format!("{}{}", header, content))?;
    Ok(())
}

/// Save rules to the system rules.yaml.
#[allow(dead_code)]
pub fn save_rules(rules: &[String]) -> Result<()> {
    save_rules_at(&AppPaths::from_system(), rules)
}

/// Add a rule to rules.yaml using explicit paths.
pub fn add_rule_at(paths: &AppPaths, rule: &str, position: Option<RulePosition>) -> Result<()> {
    let mut rules = load_rules_at(paths)?;
    let pos = position.unwrap_or_else(|| get_position_at(paths).unwrap_or_default());

    match pos {
        RulePosition::Front => rules.insert(0, rule.to_string()),
        RulePosition::Back => rules.push(rule.to_string()),
    }

    save_rules_at(paths, &rules)?;
    Ok(())
}

/// Add a rule to the system rules.yaml.
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
pub fn remove_rule(index: usize) -> Result<()> {
    remove_rule_at(&AppPaths::from_system(), index)
}

/// Clear all rules using explicit paths.
pub fn clear_rules_at(paths: &AppPaths) -> Result<()> {
    save_rules_at(paths, &Vec::new())
}

/// Clear all system rules.
pub fn clear_rules() -> Result<()> {
    clear_rules_at(&AppPaths::from_system())
}

/// List all rules using explicit paths.
pub fn list_rules_at(paths: &AppPaths) -> Result<Vec<String>> {
    load_rules_at(paths)
}

/// List all system rules.
pub fn list_rules() -> Result<Vec<String>> {
    list_rules_at(&AppPaths::from_system())
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

/// Get the system default insertion position.
pub fn get_position() -> Result<RulePosition> {
    get_position_at(&AppPaths::from_system())
}

/// Set the default insertion position using explicit paths.
pub fn set_position_at(paths: &AppPaths, position: RulePosition) -> Result<()> {
    let path = paths.rules_position_path();
    std::fs::create_dir_all(paths.config_dir())?;
    std::fs::write(&path, position.to_string())?;
    Ok(())
}

/// Set the system default insertion position.
pub fn set_position(position: RulePosition) -> Result<()> {
    set_position_at(&AppPaths::from_system(), position)
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
pub fn import_rules(source_path: &str) -> Result<()> {
    import_rules_at(&AppPaths::from_system(), source_path)
}

/// Export rules to an external file using explicit source paths.
pub fn export_rules_at(paths: &AppPaths, dest_path: &str) -> Result<()> {
    let rules = load_rules_at(paths)?;
    let rules_file = RulesFile { rules };
    let content = serde_yaml::to_string(&rules_file)?;
    std::fs::write(dest_path, content)?;
    Ok(())
}

/// Export system rules to an external file.
pub fn export_rules(dest_path: &str) -> Result<()> {
    export_rules_at(&AppPaths::from_system(), dest_path)
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
        save_rules_at(&paths, &vec!["EXISTING,RULE".to_string()]).unwrap();

        add_rule_at(&paths, "NEW,RULE", Some(RulePosition::Front)).unwrap();
        let rules = load_rules_at(&paths).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0], "NEW,RULE");
        assert_eq!(rules[1], "EXISTING,RULE");
    }

    #[test]
    fn test_add_rule_back() {
        let (_tmp, paths) = setup_test_paths();
        save_rules_at(&paths, &vec!["EXISTING,RULE".to_string()]).unwrap();

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
            &vec![
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
        save_rules_at(&paths, &vec!["RULE1".to_string()]).unwrap();
        assert!(remove_rule_at(&paths, 5).is_err());
    }

    #[test]
    fn test_clear_rules() {
        let (_tmp, paths) = setup_test_paths();
        save_rules_at(&paths, &vec!["RULE1".to_string(), "RULE2".to_string()]).unwrap();

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
