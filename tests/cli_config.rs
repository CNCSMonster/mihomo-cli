use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

#[test]
fn config_validate_rejects_invalid_yaml_in_isolated_config_dir() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("config.yaml"), "port: [\n").unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["config", "--validate"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Config is not valid YAML"),
        "stderr was: {stderr}"
    );
}

#[test]
fn config_dry_run_fix_does_not_create_or_modify_config() {
    let tmp = TempDir::new().unwrap();
    let config = tmp.path().join("config.yaml");

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["config", "--dry-run", "--fix"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!config.exists(), "dry-run unexpectedly created config.yaml");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Dry run"), "stdout was: {stdout}");
    assert!(
        stdout.contains("Would ensure config"),
        "stdout was: {stdout}"
    );
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

#[test]
#[cfg(unix)]
fn config_import_rolls_back_subscription_metadata_when_validation_fails() {
    use std::io::Write;
    use std::process::Stdio;

    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(config_dir.join("subscriptions")).unwrap();
    std::fs::write(config_dir.join("config.yaml"), "port: 7890\n").unwrap();
    std::fs::write(config_dir.join("subscriptions/active"), "sub-old").unwrap();
    std::fs::write(
        config_dir.join("subscriptions.yaml"),
        "- id: sub-old\n  url: file://old.yaml\n  updated: 2026-07-17T00:00:00Z\n",
    )
    .unwrap();
    std::fs::write(
        config_dir.join("subscriptions/sub-old.yaml"),
        "proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n",
    )
    .unwrap();
    let import_file = tmp.path().join("import.yaml");
    std::fs::write(
        &import_file,
        "proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n",
    )
    .unwrap();
    let mihomo = failing_mihomo(&tmp);

    let mut child = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", &config_dir)
        .env("MIHOMO_CLI_MIHOMO_PATH", &mihomo)
        .args([
            "config",
            "--import",
            import_file.to_str().unwrap(),
            "--activate",
            "--yes",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"y\n").unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rolled back subscription file and metadata"),
        "stderr was: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(config_dir.join("subscriptions/active")).unwrap(),
        "sub-old"
    );
    let meta = std::fs::read_to_string(config_dir.join("subscriptions.yaml")).unwrap();
    assert!(meta.contains("sub-old"), "metadata was: {meta}");
    assert!(!meta.contains("import.yaml"), "metadata was: {meta}");

    let sub_files: Vec<_> = std::fs::read_dir(config_dir.join("subscriptions"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        sub_files.contains(&"active".to_string()),
        "files were: {sub_files:?}"
    );
    assert!(
        sub_files.contains(&"sub-old.yaml".to_string()),
        "files were: {sub_files:?}"
    );
    assert_eq!(
        sub_files.len(),
        2,
        "unexpected subscription files: {sub_files:?}"
    );
    assert_eq!(
        std::fs::read_to_string(config_dir.join("config.yaml")).unwrap(),
        "port: 7890\n"
    );
}

#[test]
fn config_info_prints_subscription_summary() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path();
    std::fs::create_dir_all(config_dir.join("subscriptions")).unwrap();
    std::fs::write(
        config_dir.join("subscriptions.yaml"),
        "- id: sub-a\n  url: file://sub-a.yaml\n  updated: 2026-07-17T00:00:00Z\n",
    )
    .unwrap();
    std::fs::write(config_dir.join("subscriptions/active"), "sub-a").unwrap();
    std::fs::write(
        config_dir.join("subscriptions/sub-a.yaml"),
        "proxies:\n  - name: A\n    type: direct\n  - name: B\n    type: direct\n",
    )
    .unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", config_dir)
        .args(["config", "--info"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Subscription: sub-a"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("Proxies: 2"), "stdout was: {stdout}");
}

#[test]
fn logs_tail_and_level_filter_log_file() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("mihomo.log"),
        "INFO one\nDEBUG two\nERROR three\nINFO four\n",
    )
    .unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["logs", "--tail", "2", "--level", "info"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("INFO one"), "stdout was: {stdout}");
    assert!(stdout.contains("INFO four"), "stdout was: {stdout}");
    assert!(!stdout.contains("ERROR three"), "stdout was: {stdout}");
}

#[test]
fn override_yaml_is_applied_when_rule_change_regenerates_config() {
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
  - name: Original
    type: select
    proxies:
      - Proxy
rules:
  - MATCH,DIRECT
"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("override.yaml"),
        r#"
proxy-groups:
  - name: Custom
    type: select
    proxies:
      - DIRECT
dns:
  enhanced-mode: redir-host
"#,
    )
    .unwrap();

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
    assert!(config.contains("name: Custom"), "config was: {config}");
    assert!(
        config.contains("enhanced-mode: redir-host"),
        "config was: {config}"
    );
}

#[test]
fn config_set_ua_updates_subscription_metadata_and_info() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path();
    std::fs::create_dir_all(config_dir.join("subscriptions")).unwrap();
    std::fs::write(
        config_dir.join("subscriptions.yaml"),
        "- id: sub-a\n  url: file://sub-a.yaml\n  updated: 2026-07-17T00:00:00Z\n",
    )
    .unwrap();
    std::fs::write(config_dir.join("subscriptions/active"), "sub-a").unwrap();
    std::fs::write(
        config_dir.join("subscriptions/sub-a.yaml"),
        "proxies:\n  - name: direct\n    type: direct\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n",
    )
    .unwrap();

    let set = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", config_dir)
        .args(["config", "--set-ua", "sub-a", "clash-verge/v2.0.4"])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&set.stderr)
    );

    let meta = std::fs::read_to_string(config_dir.join("subscriptions.yaml")).unwrap();
    assert!(
        meta.contains("user_agent: clash-verge/v2.0.4"),
        "meta was: {meta}"
    );
    assert!(meta.contains("user_agent_mode: fixed"), "meta was: {meta}");

    let info = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", config_dir)
        .args(["config", "--info", "sub-a"])
        .output()
        .unwrap();
    assert!(info.status.success());
    let stdout = String::from_utf8_lossy(&info.stdout);
    assert!(
        stdout.contains("User-Agent: clash-verge/v2.0.4"),
        "stdout was: {stdout}"
    );

    let auto = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", config_dir)
        .args(["config", "--set-ua", "sub-a", "auto"])
        .output()
        .unwrap();
    assert!(auto.status.success());
    let meta = std::fs::read_to_string(config_dir.join("subscriptions.yaml")).unwrap();
    assert!(meta.contains("user_agent_mode: auto"), "meta was: {meta}");
    assert!(!meta.contains("user_agent:"), "meta was: {meta}");
}
