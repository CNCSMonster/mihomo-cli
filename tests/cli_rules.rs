use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

#[test]
fn rule_add_rejects_invalid_rule_without_writing_rules_file() {
    let tmp = TempDir::new().unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "add", "IP-CIDR,999.0.0.0/8,DIRECT"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!tmp.path().join("rules.yaml").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid IPv4 address"),
        "stderr was: {stderr}"
    );
}

#[test]
fn rule_add_valid_rule_writes_to_isolated_config_dir() {
    let tmp = TempDir::new().unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "add", "DOMAIN-SUFFIX,example.com,DIRECT"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rules = std::fs::read_to_string(tmp.path().join("rules.yaml")).unwrap();
    assert!(
        rules.contains("DOMAIN-SUFFIX,example.com,DIRECT"),
        "rules were: {rules}"
    );
}

#[test]
fn rule_types_and_policies_are_printable() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.yaml"),
        "proxy-groups:\n  - name: Proxy\n    type: select\n",
    )
    .unwrap();

    let types = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "types"])
        .output()
        .unwrap();
    assert!(types.status.success());
    assert!(String::from_utf8_lossy(&types.stdout).contains("DOMAIN-SUFFIX"));

    let policies = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "policies"])
        .output()
        .unwrap();
    assert!(policies.status.success());
    let stdout = String::from_utf8_lossy(&policies.stdout);
    assert!(stdout.contains("DIRECT"), "stdout was: {stdout}");
    assert!(stdout.contains("Proxy"), "stdout was: {stdout}");
}

#[test]
fn rule_add_merges_rule_into_config_when_subscription_is_active() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("subscriptions")).unwrap();
    std::fs::write(tmp.path().join("subscriptions/active"), "sub-a").unwrap();
    std::fs::write(
        tmp.path().join("subscriptions/sub-a.yaml"),
        r#"
proxies:
  - name: Proxy
    type: direct
proxy-groups:
  - name: ProxyGroup
    type: select
    proxies:
      - Proxy
rules:
  - MATCH,DIRECT
"#,
    )
    .unwrap();
    std::fs::write(tmp.path().join("subscriptions.yaml"), "[]\n").unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "add", "DOMAIN-SUFFIX,example.com,DIRECT"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
    assert!(
        config.contains("DOMAIN-SUFFIX,example.com,DIRECT"),
        "config was: {config}"
    );
}

#[test]
fn rule_test_reports_first_matching_rule_and_policy() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.yaml"),
        r#"
rules:
  - DOMAIN-SUFFIX,example.com,DIRECT
  - DOMAIN-KEYWORD,google,Proxy
  - MATCH,Final
"#,
    )
    .unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "test", "api.example.com"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Matched rule #1"), "stdout was: {stdout}");
    assert!(stdout.contains("DIRECT"), "stdout was: {stdout}");
}

#[test]
fn rule_add_rejects_duplicate_rule() {
    let tmp = TempDir::new().unwrap();
    let rule = "DOMAIN-SUFFIX,example.com,DIRECT";
    let first = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "add", rule])
        .output()
        .unwrap();
    assert!(first.status.success());

    let second = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "add", rule])
        .output()
        .unwrap();
    assert!(!second.status.success());
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("Rule already exists"),
        "stderr was: {stderr}"
    );
}

#[test]
fn rule_move_reorders_rules() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("rules.yaml"),
        "rules:\n  - DOMAIN,a.example,DIRECT\n  - DOMAIN,b.example,DIRECT\n  - DOMAIN,c.example,DIRECT\n",
    )
    .unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["rule", "move", "3", "1"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rules = std::fs::read_to_string(tmp.path().join("rules.yaml")).unwrap();
    let c = rules.find("DOMAIN,c.example,DIRECT").unwrap();
    let a = rules.find("DOMAIN,a.example,DIRECT").unwrap();
    assert!(c < a, "rules were: {rules}");
}

#[test]
fn concurrent_rule_add_both_rules_preserved() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("subscriptions")).unwrap();
    std::fs::write(tmp.path().join("subscriptions/active"), "sub-a").unwrap();
    std::fs::write(
        tmp.path().join("subscriptions/sub-a.yaml"),
        "proxies:\n  - name: p\n    type: direct\nrules:\n  - MATCH,DIRECT\n",
    )
    .unwrap();

    let tmp_a = tmp.path().to_path_buf();
    let tmp_b = tmp.path().to_path_buf();
    let bin_a = bin().to_string();
    let bin_b = bin().to_string();

    let handle_a = std::thread::spawn(move || {
        Command::new(&bin_a)
            .env("MIHOMO_CLI_CONFIG_DIR", &tmp_a)
            .args(["rule", "add", "DOMAIN-SUFFIX,a.com,DIRECT"])
            .output()
            .unwrap()
    });
    let handle_b = std::thread::spawn(move || {
        Command::new(&bin_b)
            .env("MIHOMO_CLI_CONFIG_DIR", &tmp_b)
            .args(["rule", "add", "DOMAIN-SUFFIX,b.com,DIRECT"])
            .output()
            .unwrap()
    });

    let out_a = handle_a.join().unwrap();
    let out_b = handle_b.join().unwrap();

    assert!(
        out_a.status.success(),
        "thread A failed: {}",
        String::from_utf8_lossy(&out_a.stderr)
    );
    assert!(
        out_b.status.success(),
        "thread B failed: {}",
        String::from_utf8_lossy(&out_b.stderr)
    );

    let rules = std::fs::read_to_string(tmp.path().join("rules.yaml")).unwrap();
    assert!(
        rules.contains("DOMAIN-SUFFIX,a.com,DIRECT"),
        "rule A missing. rules: {rules}"
    );
    assert!(
        rules.contains("DOMAIN-SUFFIX,b.com,DIRECT"),
        "rule B missing. rules: {rules}"
    );
}
