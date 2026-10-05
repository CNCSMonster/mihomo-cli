use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

#[cfg(unix)]
fn failing_mihomo(tmp: &TempDir) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = tmp.path().join("mihomo-fail");
    std::fs::write(
        &path,
        "#!/usr/bin/env sh\necho simulated validation failure >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn setup_active_subscription(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("subscriptions")).unwrap();
    std::fs::write(root.join("subscriptions/active"), "sub-a").unwrap();
    std::fs::write(
        root.join("subscriptions/sub-a.yaml"),
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
}

#[test]
#[cfg(unix)]
fn dns_policy_add_rolls_back_policy_file_when_validation_fails() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    setup_active_subscription(&config_dir);
    std::fs::write(
        config_dir.join("dns-policy.yaml"),
        "policies:\n  - domain: +.old.example\n    target: 1.1.1.1\n",
    )
    .unwrap();
    std::fs::write(config_dir.join("config.yaml"), "port: 7890\n").unwrap();
    let mihomo = failing_mihomo(&tmp);

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", &config_dir)
        .env("MIHOMO_CLI_MIHOMO_PATH", &mihomo)
        .args(["dns", "policy", "add", "new.example", "8.8.8.8"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rolled back dns-policy.yaml"),
        "stderr was: {stderr}"
    );
    let dns = std::fs::read_to_string(config_dir.join("dns-policy.yaml")).unwrap();
    assert!(dns.contains("old.example"), "dns-policy was: {dns}");
    assert!(!dns.contains("new.example"), "dns-policy was: {dns}");
    assert_eq!(
        std::fs::read_to_string(config_dir.join("config.yaml")).unwrap(),
        "port: 7890\n"
    );
}

#[test]
fn dns_template_list_is_printable() {
    let tmp = TempDir::new().unwrap();
    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["dns", "template", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("company"), "stdout was: {stdout}");
    assert!(stdout.contains("ads"), "stdout was: {stdout}");
}

#[test]
fn dns_template_apply_company_writes_policy() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(config_dir.join("subscriptions")).unwrap();
    setup_active_subscription(&config_dir);

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", &config_dir)
        .args([
            "dns",
            "template",
            "apply",
            "company",
            "--domain",
            "corp.example",
            "--target",
            "10.0.0.1",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let policy = std::fs::read_to_string(config_dir.join("dns-policy.yaml")).unwrap();
    assert!(policy.contains("+.corp.example"), "policy was: {policy}");
    assert!(policy.contains("10.0.0.1"), "policy was: {policy}");
}

#[test]
fn dns_policy_add_when_core_stopped_commits_intent_and_reports_pending() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(config_dir.join("subscriptions")).unwrap();
    setup_active_subscription(&config_dir);

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", &config_dir)
        .args(["dns", "policy", "add", "custom.example.com", "1.1.1.1"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Policy added: custom.example.com → 1.1.1.1"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("restart"),
        "stdout was expected to contain restart hint: {stdout}"
    );
    let policy = std::fs::read_to_string(config_dir.join("dns-policy.yaml")).unwrap();
    assert!(
        policy.contains("custom.example.com"),
        "policy was: {policy}"
    );
    assert!(policy.contains("1.1.1.1"), "policy was: {policy}");
}

#[test]
fn dns_policy_remove_when_core_stopped_commits_intent_and_reports_pending() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(config_dir.join("subscriptions")).unwrap();
    setup_active_subscription(&config_dir);
    std::fs::write(
        config_dir.join("dns-policy.yaml"),
        "policies:\n  - domain: +.custom.example.com\n    target: 1.1.1.1\n",
    )
    .unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", &config_dir)
        .args(["dns", "policy", "remove", "1"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Policy removed:"), "stdout was: {stdout}");
    let policy = std::fs::read_to_string(config_dir.join("dns-policy.yaml")).unwrap();
    assert!(
        !policy.contains("custom.example.com"),
        "policy was: {policy}"
    );
}
