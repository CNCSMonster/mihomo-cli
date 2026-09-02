#![allow(dead_code)]

//! Pre-flight checks for commands
//!
//! This module provides pre-flight checks that verify dependencies and state
//! before executing commands. If checks fail, clear error messages with hints
//! are returned.

use std::path::Path;

/// Pre-flight check result
pub struct PreflightResult {
    pub passed: bool,
    pub message: String,
    pub hint: Option<String>,
}

impl PreflightResult {
    pub fn pass() -> Self {
        Self::pass_named("")
    }

    pub fn pass_named(message: impl Into<String>) -> Self {
        Self {
            passed: true,
            message: message.into(),
            hint: None,
        }
    }

    pub fn fail(message: impl Into<String>, hint: Option<String>) -> Self {
        Self {
            passed: false,
            message: message.into(),
            hint,
        }
    }

    pub fn format(&self) -> Option<String> {
        if self.passed {
            return None;
        }
        let mut result = format!("❌ {}", self.message);
        if let Some(hint) = &self.hint {
            result.push_str(&format!("\n💡 {}", hint));
        }
        Some(result)
    }
}

/// Format a complete preflight result without hiding passed checks.
pub fn format_report(checks: &[PreflightResult]) -> String {
    let mut output = vec!["Passed:".to_string()];
    for check in checks
        .iter()
        .filter(|check| check.passed && !check.message.is_empty())
    {
        output.push(format!("  ✓ {}", check.message));
    }

    let blocked: Vec<_> = checks.iter().filter(|check| !check.passed).collect();
    if blocked.is_empty() {
        output.push("Blocked: none".to_string());
        output.push("Next: none".to_string());
        return output.join("\n");
    }

    output.push("Blocked:".to_string());
    for check in blocked {
        output.push(format!("  ✗ {}", check.message));
    }
    output.push("Next:".to_string());
    for check in checks.iter().filter(|check| !check.passed) {
        if let Some(hint) = &check.hint {
            for line in hint.lines() {
                output.push(format!("  {}", line));
            }
        }
    }
    output.join("\n")
}

/// Check if config file exists
pub fn check_config_exists(config_path: &Path) -> PreflightResult {
    if config_path.exists() {
        return PreflightResult::pass();
    }
    PreflightResult::fail(
        format!("配置文件不存在: {}", config_path.display()),
        Some("请先添加订阅:\n   mihomo-cli config --import <subscription-url>\n   或\n   mihomo-cli config -u <subscription-url>".to_string())
    )
}

/// Check if config file is readable (Unix only)
///
/// On non-Unix platforms, this check always passes.
pub fn check_config_readable(config_path: &Path) -> PreflightResult {
    match std::fs::metadata(config_path) {
        Ok(metadata) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let permissions = metadata.permissions();
                let mode = permissions.mode();
                // Check if file is readable by owner
                if mode & 0o400 == 0 {
                    return PreflightResult::fail(
                        format!("配置文件不可读: {}", config_path.display()),
                        Some(format!(
                            "请修复文件权限:\n   sudo chmod 644 {}",
                            config_path.display()
                        )),
                    );
                }
            }
            #[cfg(not(unix))]
            {
                let _ = metadata; // Suppress unused warning on non-Unix
            }
            PreflightResult::pass()
        }
        Err(e) => PreflightResult::fail(
            format!("无法读取配置文件: {}", e),
            Some("请检查文件权限和路径".to_string()),
        ),
    }
}

/// Run multiple preflight checks
pub fn run_preflight_checks(checks: Vec<PreflightResult>) -> Result<(), String> {
    let errors: Vec<String> = checks
        .into_iter()
        .filter_map(|check| check.format())
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preflight_result_pass() {
        let result = PreflightResult::pass();
        assert!(result.passed);
        assert!(result.format().is_none());
    }

    #[test]
    fn test_preflight_result_fail_with_hint() {
        let result = PreflightResult::fail("配置文件不存在", Some("请先添加订阅".to_string()));
        assert!(!result.passed);
        let formatted = result.format().unwrap();
        assert!(formatted.contains("❌"));
        assert!(formatted.contains("💡"));
    }

    #[test]
    fn test_preflight_result_fail_without_hint() {
        let result = PreflightResult::fail("错误", None);
        assert!(!result.passed);
        let formatted = result.format().unwrap();
        assert!(formatted.contains("❌"));
        assert!(!formatted.contains("💡"));
    }

    #[test]
    fn test_format_report_preserves_passed_blocked_next_sections() {
        let checks = vec![
            PreflightResult::pass_named("daemon IPC authorized"),
            PreflightResult::fail(
                "user Core is still running",
                Some("Run `mihomo-cli stop`, then retry `mihomo-cli tun on`.".to_string()),
            ),
        ];
        let report = format_report(&checks);
        assert!(report.contains("Passed:\n  ✓ daemon IPC authorized"));
        assert!(report.contains("Blocked:\n  ✗ user Core is still running"));
        assert!(report.contains("Next:\n  Run `mihomo-cli stop`"));
    }

    #[test]
    fn test_format_report_all_pass_has_no_blocked_reason() {
        let report = format_report(&[PreflightResult::pass_named("/dev/net/tun available")]);
        assert!(report.contains("Passed:\n  ✓ /dev/net/tun available"));
        assert!(report.contains("Blocked: none"));
        assert!(report.contains("Next: none"));
    }

    #[test]
    fn test_check_config_exists_missing() {
        let result = check_config_exists(Path::new("/nonexistent/config.yaml"));
        assert!(!result.passed);
        assert!(result.hint.is_some());
    }

    #[test]
    fn test_check_config_exists_present() {
        // Use a file that definitely exists
        let result = check_config_exists(Path::new("/etc/passwd"));
        assert!(result.passed);
    }

    #[test]
    fn test_check_config_readable_missing() {
        let result = check_config_readable(Path::new("/nonexistent/config.yaml"));
        assert!(!result.passed);
        assert!(result.message.contains("无法读取配置文件"));
    }

    #[test]
    fn test_check_config_readable_present() {
        // /etc/passwd should be readable
        let result = check_config_readable(Path::new("/etc/passwd"));
        assert!(result.passed);
    }

    #[test]
    fn test_run_preflight_checks_all_pass() {
        let checks = vec![PreflightResult::pass(), PreflightResult::pass()];
        assert!(run_preflight_checks(checks).is_ok());
    }

    #[test]
    fn test_run_preflight_checks_mixed() {
        let checks = vec![
            PreflightResult::pass(),
            PreflightResult::fail("错误1", None),
            PreflightResult::pass(),
            PreflightResult::fail("错误2", Some("提示".to_string())),
        ];
        let result = run_preflight_checks(checks);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("错误1"));
        assert!(err.contains("错误2"));
        assert!(err.contains("💡"));
    }

    #[test]
    fn test_run_preflight_checks_all_fail() {
        let checks = vec![
            PreflightResult::fail("错误1", None),
            PreflightResult::fail("错误2", None),
        ];
        let result = run_preflight_checks(checks);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("错误1"));
        assert!(err.contains("错误2"));
    }
}
