use anyhow::{anyhow, Result};

pub struct YamlEditor {
    source: String,
}

impl YamlEditor {
    pub fn parse(source: &str) -> Result<Self> {
        serde_yaml::from_str::<serde_yaml::Value>(source)
            .map_err(|e| anyhow!("YAML contains syntax errors, cannot safely edit: {e}"))?;
        Ok(Self {
            source: source.to_string(),
        })
    }

    #[cfg(test)]
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn into_source(self) -> String {
        self.source
    }

    pub fn ensure_controller(&mut self, controller_line: &str) -> Result<()> {
        let runtime_keys = [
            "external-controller",
            "external-controller-unix",
            "external-controller-pipe",
            "external-ui",
        ];
        let mut ranges: Vec<(usize, usize)> = runtime_keys
            .iter()
            .filter_map(|key| self.find_top_level_key_range(key))
            .collect();

        if !ranges.is_empty() {
            ranges.sort_by_key(|(start, _)| *start);
            if ranges.len() == 1 {
                let (start, end) = ranges[0];
                if self.source[start..end].trim() == controller_line {
                    return Ok(());
                }
            }

            let insert_pos = ranges[0].0;
            for (start, end) in ranges.iter().rev() {
                self.source.replace_range(*start..*end, "");
            }
            self.source
                .insert_str(insert_pos, &format!("{}\n", controller_line));
            Self::validate(&self.source)?;
            return Ok(());
        }

        let insert_pos = self
            .first_top_level_key_start()
            .unwrap_or(self.source.len());
        if insert_pos == self.source.len() {
            if !self.source.ends_with('\n') && !self.source.is_empty() {
                self.source.push('\n');
            }
            self.source.push_str(controller_line);
            self.source.push('\n');
        } else {
            self.source
                .insert_str(insert_pos, &format!("{}\n", controller_line));
        }
        Self::validate(&self.source)?;
        Ok(())
    }

    pub fn merge_rules(&mut self, user_rules: &[String], position_front: bool) -> Result<()> {
        let marker_start = "# === USER RULES START ===";
        let marker_end = "# === USER RULES END ===";

        if let Some(existing_start) = self.source.find(marker_start) {
            if let Some(existing_end) = self.source.find(marker_end) {
                let end_pos = existing_end + marker_end.len();
                let end_pos = if end_pos < self.source.len()
                    && self.source.as_bytes().get(end_pos) == Some(&b'\n')
                {
                    end_pos + 1
                } else {
                    end_pos
                };

                if user_rules.is_empty() {
                    self.source.replace_range(existing_start..end_pos, "");
                    Self::validate(&self.source)?;
                    return Ok(());
                }

                let indent = self.detect_indent_at(existing_start);
                let rules_block = Self::build_rules_block(user_rules, indent);
                self.source
                    .replace_range(existing_start..end_pos, &rules_block);
                Self::validate(&self.source)?;
                return Ok(());
            }
        }

        if user_rules.is_empty() {
            return Ok(());
        }

        let Some((rules_start, rules_end)) = self.find_top_level_key_range("rules") else {
            eprintln!("  Warning: 'rules' key not found in config, appending new rules section");
            if !self.source.ends_with('\n') && !self.source.is_empty() {
                self.source.push('\n');
            }
            self.source.push_str("rules:\n");
            self.source
                .push_str(&Self::build_rules_block(user_rules, 2));
            Self::validate(&self.source)?;
            return Ok(());
        };

        let indent = self
            .detect_sequence_indent(rules_start, rules_end)
            .unwrap_or(2);
        let rules_block = Self::build_rules_block(user_rules, indent);
        let insert_pos = if position_front {
            self.source[rules_start..]
                .find('\n')
                .map(|p| rules_start + p + 1)
                .unwrap_or(rules_end)
        } else {
            rules_end
        };
        self.source.insert_str(insert_pos, &rules_block);
        Self::validate(&self.source)?;
        Ok(())
    }

    pub fn merge_dns_policies(&mut self, policies: &[crate::dns::DnsPolicy]) -> Result<()> {
        let mut value: serde_yaml::Value = serde_yaml::from_str(&self.source)
            .map_err(|e| anyhow!("Failed to parse YAML before DNS merge: {e}"))?;
        let root = value
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("YAML root must be a mapping"))?;
        let dns_key = serde_yaml::Value::String("dns".to_string());
        let dns = root
            .get_mut(&dns_key)
            .ok_or_else(|| anyhow!("'dns' key not found in config, cannot merge DNS policies"))?;
        let dns_map = dns
            .as_mapping_mut()
            .ok_or_else(|| anyhow!("'dns' must be a mapping to merge DNS policies"))?;

        dns_map.remove(serde_yaml::Value::String("nameserver-policy".to_string()));
        if !policies.is_empty() {
            let mut policy_map = serde_yaml::Mapping::new();
            for p in policies {
                let ips: Vec<&str> = p.target.split(',').map(|s| s.trim()).collect();
                let target_value = if ips.len() == 1 {
                    serde_yaml::Value::String(ips[0].to_string())
                } else {
                    serde_yaml::Value::Sequence(
                        ips.into_iter()
                            .map(|ip| serde_yaml::Value::String(ip.to_string()))
                            .collect(),
                    )
                };
                policy_map.insert(
                    serde_yaml::Value::String(p.match_pattern.clone()),
                    target_value,
                );
            }
            dns_map.insert(
                serde_yaml::Value::String("nameserver-policy".to_string()),
                serde_yaml::Value::Mapping(policy_map),
            );
        }

        self.source = serde_yaml::to_string(&value)?;
        Ok(())
    }

    fn validate(source: &str) -> Result<()> {
        serde_yaml::from_str::<serde_yaml::Value>(source)
            .map(|_| ())
            .map_err(|e| anyhow!("Edit produced invalid YAML syntax: {e}"))
    }

    fn find_top_level_key_range(&self, key_name: &str) -> Option<(usize, usize)> {
        let mut lines = self.line_spans().peekable();
        while let Some((start, end, line)) = lines.next() {
            if Self::is_top_level_key(line, key_name) {
                let mut range_end = end;
                while let Some((next_start, next_end, next_line)) = lines.peek().copied() {
                    if !next_line.trim().is_empty()
                        && !next_line.trim_start().starts_with('#')
                        && Self::looks_like_top_level_key(next_line)
                    {
                        let _ = next_start;
                        break;
                    }
                    range_end = next_end;
                    lines.next();
                }
                return Some((start, range_end));
            }
        }
        None
    }

    fn first_top_level_key_start(&self) -> Option<usize> {
        self.line_spans()
            .find(|(_, _, line)| Self::looks_like_top_level_key(line))
            .map(|(start, _, _)| start)
    }

    fn line_spans(&self) -> impl Iterator<Item = (usize, usize, &str)> {
        let mut start = 0;
        self.source.split_inclusive('\n').map(move |line| {
            let end = start + line.len();
            let span = (start, end, line);
            start = end;
            span
        })
    }

    fn looks_like_top_level_key(line: &str) -> bool {
        !line.starts_with(char::is_whitespace)
            && !line.starts_with('-')
            && !line.trim_start().starts_with('#')
            && line
                .split_once(':')
                .map(|(key, _)| !key.trim().is_empty())
                .unwrap_or(false)
    }

    fn is_top_level_key(line: &str, key_name: &str) -> bool {
        Self::looks_like_top_level_key(line)
            && line
                .split_once(':')
                .map(|(key, _)| key.trim() == key_name)
                .unwrap_or(false)
    }

    fn detect_sequence_indent(&self, start: usize, end: usize) -> Option<usize> {
        self.source[start..end].lines().skip(1).find_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('-') {
                Some(line.len() - trimmed.len())
            } else {
                None
            }
        })
    }

    fn detect_indent_at(&self, pos: usize) -> usize {
        let line_start = self.source[..pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
        pos - line_start
    }

    fn build_rules_block(rules: &[String], rule_indent: usize) -> String {
        let ri = " ".repeat(rule_indent);
        let mut block = String::new();
        block.push_str(&format!("{}# === USER RULES START ===\n", ri));
        for rule in rules {
            block.push_str(&format!("{}- {}\n", ri, rule));
        }
        block.push_str(&format!("{}# === USER RULES END ===\n", ri));
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_controller_inserts() {
        let yaml = "mixed-port: 7890\nproxies:\n  - name: test\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        editor
            .ensure_controller("external-controller-unix: /tmp/test.sock")
            .unwrap();
        assert!(editor
            .source()
            .contains("external-controller-unix: /tmp/test.sock"));
    }

    #[test]
    fn test_ensure_controller_already_exists() {
        let yaml = "external-controller-unix: /tmp/existing.sock\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        editor
            .ensure_controller("external-controller-unix: /tmp/test.sock")
            .unwrap();
        assert_eq!(
            editor.source(),
            "external-controller-unix: /tmp/test.sock\n"
        );
    }

    #[test]
    fn test_ensure_controller_keeps_matching_existing_controller() {
        let yaml = "external-controller-unix: /tmp/test.sock\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        let original = editor.source().to_string();
        editor
            .ensure_controller("external-controller-unix: /tmp/test.sock")
            .unwrap();
        assert_eq!(editor.source(), original.as_str());
    }

    #[test]
    fn test_merge_rules_front() {
        let yaml = "rules:\n  - MATCH,Proxy\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        editor
            .merge_rules(&["DOMAIN,test.com,DIRECT".to_string()], true)
            .unwrap();
        assert!(editor.source().contains("# === USER RULES START ==="));
        assert!(editor.source().contains("  - DOMAIN,test.com,DIRECT"));
        assert!(editor.source().contains("  - MATCH,Proxy"));
    }

    #[test]
    fn test_merge_rules_back() {
        let yaml = "rules:\n  - MATCH,Proxy\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        editor
            .merge_rules(&["DOMAIN,test.com,DIRECT".to_string()], false)
            .unwrap();
        let source = editor.source();
        let match_pos = source.find("MATCH,Proxy").unwrap();
        let user_pos = source.find("DOMAIN,test.com").unwrap();
        assert!(match_pos < user_pos);
    }

    #[test]
    fn test_merge_empty_rules_removes_markers() {
        let yaml = "rules:\n# === USER RULES START ===\n  - OLD,DIRECT\n# === USER RULES END ===\n  - MATCH,Proxy\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        editor.merge_rules(&[], true).unwrap();
        assert!(!editor.source().contains("USER RULES START"));
        assert!(!editor.source().contains("OLD,DIRECT"));
        assert!(editor.source().contains("MATCH,Proxy"));
    }

    #[test]
    fn test_parse_invalid_yaml_fails() {
        let yaml = "invalid: yaml: with: multiple: colons\n  - bad indent\n    - worse\n";
        let result = YamlEditor::parse(yaml);
        assert!(result.is_err(), "should fail on YAML with syntax errors");
    }

    #[test]
    fn test_merge_dns_policies_no_dns_key_fails() {
        let yaml = "rules:\n  - MATCH,Proxy\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        let policy = crate::dns::DnsPolicy {
            match_pattern: "+.example.com".to_string(),
            target: "8.8.8.8".to_string(),
        };
        let result = editor.merge_dns_policies(&[policy]);
        assert!(result.is_err(), "should fail when dns key not found");
    }

    #[test]
    fn test_merge_rules_no_rules_key_appends() {
        let yaml = "mixed-port: 7890\nproxies:\n  - name: test\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        editor
            .merge_rules(&["DOMAIN,test.com,DIRECT".to_string()], true)
            .unwrap();
        assert!(editor.source().contains("rules:"));
        assert!(editor.source().contains("  - DOMAIN,test.com,DIRECT"));
    }

    #[test]
    fn test_merge_rules_zero_indent_existing_rules() {
        let yaml = "rules:\n- MATCH,Proxy\n";
        let mut editor = YamlEditor::parse(yaml).unwrap();
        editor
            .merge_rules(&["DOMAIN,test.com,DIRECT".to_string()], true)
            .unwrap();
        let source = editor.source();
        assert!(source.contains("# === USER RULES START ==="));
        assert!(source.contains("- DOMAIN,test.com,DIRECT"));
        assert!(source.contains("- MATCH,Proxy"));
        serde_yaml::from_str::<serde_yaml::Value>(source).unwrap();
    }
}
