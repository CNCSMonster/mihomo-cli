use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

fn setup_subscription(root: &std::path::Path, id: &str) {
    let subs_dir = root.join("subscriptions");
    std::fs::create_dir_all(&subs_dir).unwrap();
    let meta_path = root.join("subscriptions.yaml");
    let mut meta = if meta_path.exists() {
        std::fs::read_to_string(&meta_path).unwrap()
    } else {
        String::new()
    };
    meta.push_str(&format!(
        "- id: {id}\n  url: file://{id}.yaml\n  updated: 2026-09-01T00:00:00Z\n"
    ));
    std::fs::write(&meta_path, meta).unwrap();
    std::fs::write(
        subs_dir.join(format!("{id}.yaml")),
        r#"
proxies:
  - name: NodeA
    type: direct
  - name: NodeB
    type: direct
proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - NodeA
      - NodeB
rules:
  - MATCH,DIRECT
"#,
    )
    .unwrap();
}

#[test]
fn select_unpin_fails_fast_without_active_subscription() {
    let tmp = TempDir::new().unwrap();
    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["select", "--unpin", "--group", "Proxy"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No active subscription")
            || stderr.contains("Cannot persist selection without an active subscription"),
        "stderr was: {stderr}"
    );
}

#[test]
fn select_unpin_clears_only_current_subscription() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path();

    setup_subscription(config_dir, "sub-11111111");
    setup_subscription(config_dir, "sub-22222222");

    // 设置 sub-11111111 为当前 active
    std::fs::write(config_dir.join("subscriptions/active"), "sub-11111111").unwrap();

    let sel_dir = config_dir.join("selections");
    std::fs::create_dir_all(&sel_dir).unwrap();

    // 为 sub-11111111 写入两个选择
    std::fs::write(
        sel_dir.join("sub-11111111.yaml"),
        "selections:\n  Proxy: NodeA\n  Region: HK\n",
    )
    .unwrap();

    // 为 sub-22222222 写入一个选择
    std::fs::write(
        sel_dir.join("sub-22222222.yaml"),
        "selections:\n  Proxy: NodeB\n",
    )
    .unwrap();

    // 在 active=sub-11111111 下 unpin Proxy 组
    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", config_dir)
        .args(["select", "--unpin", "--group", "Proxy"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr was: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // sub-11111111 的 Proxy 被移除，Region 保留
    let sel1 = std::fs::read_to_string(sel_dir.join("sub-11111111.yaml")).unwrap();
    assert!(!sel1.contains("Proxy:"), "sel1 was: {sel1}");
    assert!(sel1.contains("Region: HK"), "sel1 was: {sel1}");

    // sub-22222222 完全不受影响
    let sel2 = std::fs::read_to_string(sel_dir.join("sub-22222222.yaml")).unwrap();
    assert!(sel2.contains("Proxy: NodeB"), "sel2 was: {sel2}");

    // 在 active=sub-11111111 下 unpin --all
    let output_all = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", config_dir)
        .args(["select", "--unpin", "--all"])
        .output()
        .unwrap();

    assert!(output_all.status.success());
    let sel1_after_all = std::fs::read_to_string(sel_dir.join("sub-11111111.yaml")).unwrap();
    assert!(
        !sel1_after_all.contains("Region:"),
        "sel1 after all was: {sel1_after_all}"
    );

    // sub-22222222 依然完好
    let sel2_final = std::fs::read_to_string(sel_dir.join("sub-22222222.yaml")).unwrap();
    assert!(
        sel2_final.contains("Proxy: NodeB"),
        "sel2 final was: {sel2_final}"
    );
}

#[test]
fn select_unpin_migrates_legacy_file_on_demand() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path();

    setup_subscription(config_dir, "sub-aaaaaaaa");
    std::fs::write(config_dir.join("subscriptions/active"), "sub-aaaaaaaa").unwrap();

    // 写入旧格式的 selection-state.yaml
    std::fs::write(
        config_dir.join("selection-state.yaml"),
        "selections:\n  Proxy: NodeA\n",
    )
    .unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", config_dir)
        .args(["select", "--unpin", "--group", "Proxy"])
        .output()
        .unwrap();

    assert!(output.status.success());

    // 旧文件已归档为 .legacy
    assert!(!config_dir.join("selection-state.yaml").exists());
    assert!(config_dir.join("selection-state.yaml.legacy").exists());

    // 新文件已生成并完成了 unpin
    let target = config_dir.join("selections/sub-aaaaaaaa.yaml");
    assert!(target.exists());
    let content = std::fs::read_to_string(&target).unwrap();
    assert!(!content.contains("Proxy:"), "target was: {content}");
}
