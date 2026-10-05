#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use tempfile::TempDir;

#[cfg(unix)]
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

#[cfg(unix)]
fn validating_mihomo(tmp: &TempDir) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = tmp.path().join("mihomo-validate");
    std::fs::write(
        &path,
        r#"#!/usr/bin/env sh
if [ "$1" != "-t" ] || [ "$2" != "-d" ]; then
  echo "unexpected args: $*" >&2
  exit 2
fi
config="$3/config.yaml"
if [ ! -f "$config" ]; then
  echo "missing config.yaml" >&2
  exit 3
fi
if ! grep -q 'DOMAIN-SUFFIX,e2e.example,DIRECT' "$config"; then
  echo "merged rule missing" >&2
  exit 4
fi
echo "$config" > "$3/mihomo-test-invoked"
exit 0
"#,
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
fn setup_fixture_config(config_dir: &std::path::Path) {
    std::fs::create_dir_all(config_dir.join("subscriptions")).unwrap();
    std::fs::write(config_dir.join("subscriptions/active"), "sub-e2e").unwrap();
    std::fs::write(
        config_dir.join("subscriptions.yaml"),
        "- id: sub-e2e\n  url: file://fixture.yaml\n  updated: 2026-07-17T00:00:00Z\n",
    )
    .unwrap();
    std::fs::write(
        config_dir.join("subscriptions/sub-e2e.yaml"),
        r#"
proxies:
  - name: DirectNode
    type: direct
proxy-groups:
  - name: ProxyGroup
    type: select
    proxies:
      - DirectNode
      - DIRECT
rules:
  - MATCH,DIRECT
"#,
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn fixture_config_rule_merge_runs_mihomo_test_and_leaves_valid_yaml() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("config");
    setup_fixture_config(&config_dir);
    let mihomo = validating_mihomo(&tmp);

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", &config_dir)
        .env("MIHOMO_CLI_MIHOMO_PATH", &mihomo)
        .args(["rule", "add", "DOMAIN-SUFFIX,e2e.example,DIRECT"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let config_path = config_dir.join("config.yaml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    let yaml: serde_yaml::Value = serde_yaml::from_str(&config).unwrap();
    assert!(yaml.get("proxies").is_some(), "config was: {config}");
    assert!(
        config.contains("DOMAIN-SUFFIX,e2e.example,DIRECT"),
        "config was: {config}"
    );
    assert!(
        config_dir.join("mihomo-test-invoked").exists(),
        "fake mihomo -t was not invoked"
    );
}
