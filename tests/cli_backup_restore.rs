use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

#[test]
fn backup_and_restore_use_isolated_config_dir() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("config");
    let backup_dir = tmp.path().join("backup");
    std::fs::create_dir_all(config_dir.join("subscriptions")).unwrap();
    std::fs::write(config_dir.join("config.yaml"), "port: 7890\n").unwrap();
    std::fs::write(config_dir.join("rules.yaml"), "rules:\n  - MATCH,DIRECT\n").unwrap();
    std::fs::write(config_dir.join("subscriptions/active"), "sub-a").unwrap();

    let backup = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", &config_dir)
        .args(["backup", backup_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        backup.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&backup.stderr)
    );
    assert!(backup_dir.join("config.yaml").exists());
    assert!(backup_dir.join("rules.yaml").exists());
    assert!(backup_dir.join("subscriptions/active").exists());

    std::fs::write(config_dir.join("config.yaml"), "port: 9999\n").unwrap();
    let restore = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", &config_dir)
        .args(["restore", backup_dir.to_str().unwrap(), "--yes"])
        .output()
        .unwrap();
    assert!(
        restore.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&restore.stderr)
    );
    let restored_config = std::fs::read_to_string(config_dir.join("config.yaml")).unwrap();
    assert!(restored_config.contains("port: 7890"));
    #[cfg(unix)]
    assert!(
        restored_config.contains("external-controller-unix:"),
        "restore should repair config for the resolved runtime endpoint: {restored_config}"
    );
    #[cfg(windows)]
    assert!(
        restored_config.contains("external-controller-pipe:"),
        "restore should repair config for the resolved runtime endpoint: {restored_config}"
    );
    assert!(
        config_dir.join("backups").exists(),
        "restore should create safety backup dir"
    );
}
