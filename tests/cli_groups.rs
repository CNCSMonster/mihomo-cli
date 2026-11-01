use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

fn setup(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("subscriptions")).unwrap();
    std::fs::write(root.join("subscriptions/active"), "sub-12345678\n").unwrap();
    std::fs::write(
        root.join("subscriptions.yaml"),
        "- id: sub-12345678\n  url: https://example.test/sub\n  updated: 2026-09-02T00:00:00Z\n",
    )
    .unwrap();
    std::fs::write(
        root.join("subscriptions/sub-12345678.yaml"),
        r#"proxies:
  - name: Node-A
    type: direct
  - name: Node-B
    type: direct
proxy-groups:
  - name: Original
    type: select
    proxies:
      - Node-A
rules:
  - MATCH,Original
"#,
    )
    .unwrap();
    std::fs::write(root.join("config.yaml"), "port: 7890\n").unwrap();
}

fn run(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", root)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn group_crud_when_core_stopped_commits_and_returns_pending() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());
    let config_before = std::fs::read(tmp.path().join("config.yaml")).unwrap();

    let created = run(
        tmp.path(),
        &[
            "group", "create", "Custom", "--type", "select", "--member", "Node-A",
        ],
    );
    assert!(created.status.success());
    let stdout = String::from_utf8_lossy(&created.stdout);
    assert!(stdout.contains("pending=true"), "stdout: {stdout}");
    assert!(stdout.contains("mihomo-cli restart"), "stdout: {stdout}");
    assert!(tmp
        .path()
        .join("overrides/sub-12345678/groups.yaml")
        .exists());
    assert_ne!(
        std::fs::read(tmp.path().join("config.yaml")).unwrap(),
        config_before
    );
}

#[test]
fn group_edit_original_when_core_stopped_commits_and_returns_pending() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());
    let file = tmp.path().join("replacement.yaml");
    std::fs::write(
        &file,
        "name: Original\ntype: url-test\nurl: https://example.test\ninterval: 300\nproxies: [Node-B]\n",
    )
    .unwrap();
    let config_before = std::fs::read(tmp.path().join("config.yaml")).unwrap();

    let edited = run(
        tmp.path(),
        &["group", "edit", "Original", file.to_str().unwrap()],
    );
    assert!(edited.status.success());
    let stdout = String::from_utf8_lossy(&edited.stdout);
    assert!(stdout.contains("pending=true"), "stdout: {stdout}");
    assert!(stdout.contains("mihomo-cli restart"), "stdout: {stdout}");
    assert!(tmp
        .path()
        .join("overrides/sub-12345678/groups.yaml")
        .exists());
    assert_ne!(
        std::fs::read(tmp.path().join("config.yaml")).unwrap(),
        config_before
    );
}

#[test]
fn group_rejects_relay_builtin_and_unknown_provider() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let relay = run(
        tmp.path(),
        &[
            "group", "create", "Chain", "--type", "relay", "--member", "DIRECT",
        ],
    );
    assert!(!relay.status.success());
    assert!(
        String::from_utf8_lossy(&relay.stderr).contains("relay"),
        "stderr: {}",
        String::from_utf8_lossy(&relay.stderr)
    );

    let provider_file = tmp.path().join("provider-group.yaml");
    std::fs::write(
        &provider_file,
        "name: ProviderGroup\ntype: select\nuse: [missing]\n",
    )
    .unwrap();
    let provider = run(
        tmp.path(),
        &[
            "group",
            "create",
            "ProviderGroup",
            "--file",
            provider_file.to_str().unwrap(),
        ],
    );
    assert!(!provider.status.success());
    assert!(
        String::from_utf8_lossy(&provider.stderr).contains("unknown proxy provider"),
        "stderr: {}",
        String::from_utf8_lossy(&provider.stderr)
    );
    assert!(!tmp
        .path()
        .join("overrides/sub-12345678/groups.yaml")
        .exists());
}

// ============================================================================
// Phase 3 E2E Contract Tests
// ============================================================================

/// Test: `group create --type url-test` injects smart defaults and echoes them
#[test]
fn group_create_url_test_applies_smart_defaults() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let out = run(
        tmp.path(),
        &[
            "group",
            "create",
            "AutoSelect",
            "--type",
            "url-test",
            "--member",
            "Node-A",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Should echo the smart defaults
    assert!(
        stdout.contains("url") || stdout.contains("Smart defaults"),
        "expected smart defaults echo, stdout: {stdout}"
    );
    // Check that url and interval were written into groups.yaml
    let overlay_path = tmp.path().join("overrides/sub-12345678/groups.yaml");
    let overlay_str = std::fs::read_to_string(&overlay_path).unwrap();
    assert!(
        overlay_str.contains("generate_204"),
        "overlay: {overlay_str}"
    );
    assert!(overlay_str.contains("interval"), "overlay: {overlay_str}");
}

/// Test: `group create --type load-balance` injects strategy default
#[test]
fn group_create_load_balance_applies_strategy_default() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let out = run(
        tmp.path(),
        &[
            "group",
            "create",
            "LB",
            "--type",
            "load-balance",
            "--member",
            "Node-A",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let overlay_path = tmp.path().join("overrides/sub-12345678/groups.yaml");
    let overlay_str = std::fs::read_to_string(&overlay_path).unwrap();
    assert!(
        overlay_str.contains("consistent-hashing"),
        "overlay: {overlay_str}"
    );
}

/// Test: `group create --type url-test --url <custom>` uses user-provided url, not default
#[test]
fn group_create_explicit_url_overrides_default() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let out = run(
        tmp.path(),
        &[
            "group",
            "create",
            "AutoSelect",
            "--type",
            "url-test",
            "--member",
            "Node-A",
            "--url",
            "https://my-custom-url.test/probe",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let overlay_path = tmp.path().join("overrides/sub-12345678/groups.yaml");
    let overlay_str = std::fs::read_to_string(&overlay_path).unwrap();
    assert!(
        overlay_str.contains("my-custom-url.test"),
        "overlay: {overlay_str}"
    );
    // Should NOT contain the default URL
    assert!(
        !overlay_str.contains("gstatic"),
        "should not have default url, overlay: {overlay_str}"
    );
}

/// Test: `group add <native>` produces patch in overlay, not a static snapshot in append
#[test]
fn group_add_on_native_group_creates_patch_not_snapshot() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let out = run(
        tmp.path(),
        &["group", "add", "Original", "--member", "Node-B"],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let overlay_path = tmp.path().join("overrides/sub-12345678/groups.yaml");
    let overlay_str = std::fs::read_to_string(&overlay_path).unwrap();

    // Must have patches section
    assert!(
        overlay_str.contains("patches"),
        "expected patches section, overlay: {overlay_str}"
    );
    // Must have add_proxies containing Node-B
    assert!(
        overlay_str.contains("add_proxies"),
        "expected add_proxies, overlay: {overlay_str}"
    );
    assert!(
        overlay_str.contains("Node-B"),
        "expected Node-B in patches, overlay: {overlay_str}"
    );
    // Must NOT have 'append' with Original (the static snapshot anti-pattern)
    // We check that the overlay does NOT contain 'Original' in the append section
    // by verifying patches is used instead of delete+append
    assert!(
        !overlay_str.contains("delete"),
        "should not have delete (no static snapshot), overlay: {overlay_str}"
    );
}

/// Test: `group reset <native>` restores upstream state (clears patch and unhides)
#[test]
fn group_reset_native_group_restores_upstream() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // First add a member to create a patch
    let add_out = run(
        tmp.path(),
        &["group", "add", "Original", "--member", "Node-B"],
    );
    assert!(
        add_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    // Verify patch exists
    let overlay_path = tmp.path().join("overrides/sub-12345678/groups.yaml");
    let overlay_str = std::fs::read_to_string(&overlay_path).unwrap();
    assert!(
        overlay_str.contains("patches"),
        "expected patch before reset"
    );

    // Now reset
    let reset_out = run(tmp.path(), &["group", "reset", "Original"]);
    assert!(
        reset_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&reset_out.stderr)
    );

    // Verify overlay no longer has patches for Original
    let overlay_str_after = std::fs::read_to_string(&overlay_path).unwrap_or_default();
    // Either the file is empty/minimal (no patches key) or patches section exists but Original is gone
    assert!(
        !overlay_str_after.contains("add_proxies"),
        "patch should be cleared after reset, overlay: {overlay_str_after}"
    );
}

/// Test: `group reset <custom>` is rejected with friendly message
#[test]
fn group_reset_custom_group_is_rejected() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // Create a custom group
    let create_out = run(
        tmp.path(),
        &[
            "group", "create", "Custom", "--type", "select", "--member", "Node-A",
        ],
    );
    assert!(create_out.status.success());

    // Try to reset it (should fail with friendly message)
    let reset_out = run(tmp.path(), &["group", "reset", "Custom"]);
    assert!(!reset_out.status.success());
    let stderr = String::from_utf8_lossy(&reset_out.stderr);
    assert!(
        stderr.contains("自建组") || stderr.contains("group delete"),
        "expected friendly rejection message, stderr: {stderr}"
    );
}

/// Test: `group delete` is blocked when rules.yaml references the group
#[test]
fn group_delete_blocked_by_rule_reference() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // Create a custom group
    let create_out = run(
        tmp.path(),
        &[
            "group", "create", "MyGroup", "--type", "select", "--member", "Node-A",
        ],
    );
    assert!(create_out.status.success());

    // Add a rule referencing it
    let rule_out = run(
        tmp.path(),
        &["rule", "add", "DOMAIN-SUFFIX,example.com,MyGroup"],
    );
    assert!(
        rule_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&rule_out.stderr)
    );

    // Attempt to delete should be blocked
    let delete_out = run(tmp.path(), &["group", "delete", "MyGroup"]);
    assert!(!delete_out.status.success());
    let stderr = String::from_utf8_lossy(&delete_out.stderr);
    assert!(
        stderr.contains("Blocked") || stderr.contains("referenced"),
        "expected cascade block message, stderr: {stderr}"
    );
}

/// Test: `group delete` is blocked when a logical rule references the group.
#[test]
fn group_delete_blocked_by_logical_rule_reference() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let create_out = run(
        tmp.path(),
        &[
            "group", "create", "MyGroup", "--type", "select", "--member", "Node-A",
        ],
    );
    assert!(create_out.status.success());
    std::fs::write(
        tmp.path().join("rules.yaml"),
        "rules:\n  - \"AND,((NETWORK,udp),(DST-PORT,443)),MyGroup\"\n",
    )
    .unwrap();

    let delete_out = run(tmp.path(), &["group", "delete", "MyGroup"]);
    assert!(!delete_out.status.success());
    let stderr = String::from_utf8_lossy(&delete_out.stderr);
    assert!(
        stderr.contains("Blocked") || stderr.contains("referenced"),
        "expected cascade block message, stderr: {stderr}"
    );
}

/// Test: `group delete` is blocked when another group's proxies reference it
#[test]
fn group_delete_blocked_by_other_group_member_reference() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // Create two custom groups where G2 is a member of G1
    let create_g1 = run(
        tmp.path(),
        &[
            "group", "create", "G1", "--type", "select", "--member", "Node-A",
        ],
    );
    assert!(create_g1.status.success());

    let create_g2 = run(
        tmp.path(),
        &[
            "group", "create", "G2", "--type", "select", "--member", "G1",
        ],
    );
    assert!(create_g2.status.success());

    // Attempt to delete G1 should be blocked because G2 references it
    let delete_out = run(tmp.path(), &["group", "delete", "G1"]);
    assert!(!delete_out.status.success());
    let stderr = String::from_utf8_lossy(&delete_out.stderr);
    assert!(
        stderr.contains("Blocked") || stderr.contains("referenced"),
        "expected cascade block message, stderr: {stderr}"
    );
}

/// Test: `group delete` is blocked when patches[*].add_proxies references the group
#[test]
fn group_delete_blocked_by_patch_add_proxies_reference() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // Create a custom group
    let create_out = run(
        tmp.path(),
        &[
            "group",
            "create",
            "CustomSub",
            "--type",
            "select",
            "--member",
            "Node-A",
        ],
    );
    assert!(create_out.status.success());

    // Add CustomSub as a member of the native group Original (this creates a patch)
    let add_out = run(
        tmp.path(),
        &["group", "add", "Original", "--member", "CustomSub"],
    );
    assert!(
        add_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    // Attempt to delete CustomSub should be blocked because patches[Original].add_proxies references it
    let delete_out = run(tmp.path(), &["group", "delete", "CustomSub"]);
    assert!(!delete_out.status.success());
    let stderr = String::from_utf8_lossy(&delete_out.stderr);
    assert!(
        stderr.contains("Blocked") || stderr.contains("referenced"),
        "expected cascade block due to patch reference, stderr: {stderr}"
    );
}

/// Test: `group list` shows active subscription and source labels
#[test]
fn group_list_shows_active_subscription_and_labels() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let out = run(tmp.path(), &["group", "list"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Should show the active subscription
    assert!(
        stdout.contains("Active Subscription"),
        "expected active subscription header, stdout: {stdout}"
    );
    assert!(
        stdout.contains("sub-12345678"),
        "expected subscription ID, stdout: {stdout}"
    );
    // Should show source label for Original (native group)
    assert!(
        stdout.contains("[native]"),
        "expected [native] label, stdout: {stdout}"
    );
}

#[test]
fn concurrent_group_create_both_preserved_under_config_lock() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let tmp_a = tmp.path().to_path_buf();
    let tmp_b = tmp.path().to_path_buf();
    let bin_a = bin().to_string();
    let bin_b = bin().to_string();

    let handle_a = std::thread::spawn(move || {
        Command::new(&bin_a)
            .env("MIHOMO_CLI_CONFIG_DIR", &tmp_a)
            .args([
                "group", "create", "GroupA", "--type", "select", "--member", "Node-A",
            ])
            .output()
            .unwrap()
    });
    let handle_b = std::thread::spawn(move || {
        Command::new(&bin_b)
            .env("MIHOMO_CLI_CONFIG_DIR", &tmp_b)
            .args([
                "group", "create", "GroupB", "--type", "select", "--member", "Node-B",
            ])
            .output()
            .unwrap()
    });

    let out_a = handle_a.join().unwrap();
    let out_b = handle_b.join().unwrap();

    assert!(
        out_a.status.success(),
        "stderr a: {}",
        String::from_utf8_lossy(&out_a.stderr)
    );
    assert!(
        out_b.status.success(),
        "stderr b: {}",
        String::from_utf8_lossy(&out_b.stderr)
    );

    // Verify both groups exist in the list
    let list_out = run(tmp.path(), &["group", "list"]);
    let stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        stdout.contains("GroupA"),
        "GroupA missing from stdout: {stdout}"
    );
    assert!(
        stdout.contains("GroupB"),
        "GroupB missing from stdout: {stdout}"
    );
}

#[test]
fn native_group_remove_then_add_restores_without_add_proxies() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());
    // Give Original two nodes so removing one does not leave it empty
    std::fs::write(
        tmp.path().join("subscriptions/sub-12345678.yaml"),
        r#"proxies:
  - name: Node-A
    type: direct
  - name: Node-B
    type: direct
proxy-groups:
  - name: Original
    type: select
    proxies:
      - Node-A
      - Node-B
rules:
  - MATCH,Original
"#,
    )
    .unwrap();

    // 1. Remove native node Node-A
    let rm_out = run(
        tmp.path(),
        &["group", "remove", "Original", "--member", "Node-A"],
    );
    assert!(
        rm_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&rm_out.stderr)
    );

    // Check patch in groups.yaml
    let overlay_path = tmp.path().join("overrides/sub-12345678/groups.yaml");
    let content = std::fs::read_to_string(&overlay_path).unwrap();
    assert!(content.contains("remove_proxies:\n    - Node-A"));

    // Verify it is marked as [patched]
    let list_patched = run(tmp.path(), &["group", "list"]);
    assert!(String::from_utf8_lossy(&list_patched.stdout).contains("[patched]"));

    // 2. Add Node-A back to Original
    let add_out = run(
        tmp.path(),
        &["group", "add", "Original", "--member", "Node-A"],
    );
    assert!(
        add_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    // Check that remove_proxies no longer has Node-A, and the empty patch is completely cleared
    let content_after = std::fs::read_to_string(&overlay_path).unwrap();
    assert!(
        !content_after.contains("- Node-A"),
        "Node-A should be cleared from patch, but found in: {content_after}"
    );

    // Verify it is restored to [native] label (not left as ghost [patched])
    let list_restored = run(tmp.path(), &["group", "list"]);
    let stdout = String::from_utf8_lossy(&list_restored.stdout);
    assert!(
        stdout.contains("[native]"),
        "Original should be [native] after member restored, got: {stdout}"
    );
    assert!(
        !stdout.contains("[patched]"),
        "Original should not be [patched] after member restored, got: {stdout}"
    );
}

#[test]
fn group_remove_only_member_fails_active_validation() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // Original in setup only has Node-A. Removing it should fail fast because proxy groups cannot be empty.
    let rm_out = run(
        tmp.path(),
        &["group", "remove", "Original", "--member", "Node-A"],
    );
    assert!(
        !rm_out.status.success(),
        "removing the only member must fail active validation"
    );
    let stderr = String::from_utf8_lossy(&rm_out.stderr);
    assert!(
        stderr.contains("must have at least one proxy"),
        "expected empty group error, got: {stderr}"
    );
}

#[test]
fn group_create_empty_proxy_group_fails_active_validation() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let create_out = run(
        tmp.path(),
        &["group", "create", "EmptyGroup", "--type", "select"],
    );
    assert!(
        !create_out.status.success(),
        "creating empty group must fail active validation"
    );
    let stderr = String::from_utf8_lossy(&create_out.stderr);
    assert!(
        stderr.contains("must have at least one proxy"),
        "expected empty group error, got: {stderr}"
    );
}

#[test]
fn group_delete_unpins_selection_persistence() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // 1. Create a custom group
    let create_out = run(
        tmp.path(),
        &[
            "group",
            "create",
            "CustomSel",
            "--type",
            "select",
            "--member",
            "Node-A",
        ],
    );
    assert!(create_out.status.success());

    // 2. Set up selection intent in selections/sub-12345678.yaml
    let sel_dir = tmp.path().join("selections");
    std::fs::create_dir_all(&sel_dir).unwrap();
    let sel_path = sel_dir.join("sub-12345678.yaml");
    std::fs::write(
        &sel_path,
        "selections:\n  CustomSel: Node-A\n  Original: Node-A\n",
    )
    .unwrap();

    // 3. Delete CustomSel
    let del_out = run(tmp.path(), &["group", "delete", "CustomSel"]);
    assert!(del_out.status.success());

    // 4. Verify selections file has CustomSel removed but Original kept
    let sel_content = std::fs::read_to_string(&sel_path).unwrap();
    assert!(
        !sel_content.contains("CustomSel"),
        "CustomSel selection should be unpinned, found: {sel_content}"
    );
    assert!(
        sel_content.contains("Original: Node-A"),
        "Original selection should be preserved, found: {sel_content}"
    );
}

#[test]
fn cannot_create_custom_group_with_same_name_as_hidden_native_group() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // 1. Delete (hide) native group Original
    let del_out = run(tmp.path(), &["group", "delete", "Original"]);
    assert!(del_out.status.success());

    // 2. Try to create custom group with same name Original
    let create_out = run(
        tmp.path(),
        &[
            "group", "create", "Original", "--type", "select", "--member", "Node-B",
        ],
    );
    assert!(
        !create_out.status.success(),
        "Creating custom group with same name as hidden native group must fail"
    );
    let stderr = String::from_utf8_lossy(&create_out.stderr);
    assert!(
        stderr.contains("already exists"),
        "Expected error message about already exists, got: {stderr}"
    );
}

#[test]
fn native_group_edit_then_reset_restores_without_duplicate_error() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // 1. Edit native group via file
    let edit_file = tmp.path().join("replacement.yaml");
    std::fs::write(
        &edit_file,
        "name: Original\ntype: select\nproxies:\n  - Node-B\n",
    )
    .unwrap();

    let edit_out = run(
        tmp.path(),
        &["group", "edit", "Original", edit_file.to_str().unwrap()],
    );
    assert!(
        edit_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&edit_out.stderr)
    );

    // 2. Reset native group
    let reset_out = run(tmp.path(), &["group", "reset", "Original"]);
    assert!(reset_out.status.success());

    // 3. Show group and verify list works without duplicate name error
    let show_out = run(tmp.path(), &["group", "show", "Original"]);
    assert!(show_out.status.success());
    let stdout = String::from_utf8_lossy(&show_out.stdout);
    assert!(
        stdout.contains("Node-A"),
        "Original upstream proxies should be restored: {stdout}"
    );

    let list_out = run(tmp.path(), &["group", "list"]);
    assert!(
        list_out.status.success(),
        "group list must succeed without duplicate error"
    );
}

#[test]
fn upstream_subscription_with_proxy_and_bracketed_names_works_with_overlay() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());
    // Upstream has groups named "PROXY" and "节点选择 [自动]" (common in airport configs)
    std::fs::write(
        tmp.path().join("subscriptions/sub-12345678.yaml"),
        r#"proxies:
  - name: Node-A
    type: direct
  - name: Node-B
    type: direct
proxy-groups:
  - name: PROXY
    type: select
    proxies:
      - Node-A
  - name: 节点选择 [自动]
    type: url-test
    url: http://example.test
    interval: 300
    proxies:
      - Node-B
rules:
  - MATCH,PROXY
"#,
    )
    .unwrap();

    // 1. Verify group list works without error and labels native groups correctly
    let list_out = run(tmp.path(), &["group", "list"]);
    assert!(
        list_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    let stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(
        stdout.contains("PROXY\tselect\t[native]"),
        "PROXY should be [native], got: {stdout}"
    );
    assert!(
        stdout.contains("节点选择 [自动]\turl-test\t[native]"),
        "bracketed group should be [native], got: {stdout}"
    );

    // 2. Create a custom group alongside them
    let create_out = run(
        tmp.path(),
        &[
            "group", "create", "MyCustom", "--type", "select", "--member", "Node-A",
        ],
    );
    assert!(
        create_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&create_out.stderr)
    );

    // 3. Patch the PROXY group and bracketed group
    let add_proxy = run(tmp.path(), &["group", "add", "PROXY", "--member", "Node-B"]);
    assert!(
        add_proxy.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_proxy.stderr)
    );

    let add_out = run(
        tmp.path(),
        &["group", "add", "节点选择 [自动]", "--member", "Node-A"],
    );
    assert!(
        add_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    // 4. Verify PROXY and bracketed group are now labeled [patched]
    let list_patched = run(tmp.path(), &["group", "list"]);
    let stdout_patched = String::from_utf8_lossy(&list_patched.stdout);
    assert!(
        stdout_patched.contains("PROXY\tselect\t[patched]"),
        "PROXY should be [patched], got: {stdout_patched}"
    );
    assert!(
        stdout_patched.contains("节点选择 [自动]\turl-test\t[patched]"),
        "bracketed group should be [patched], got: {stdout_patched}"
    );

    // 5. Verify group show PROXY displays [patched]
    let show_proxy = run(tmp.path(), &["group", "show", "PROXY"]);
    assert!(show_proxy.status.success());
    let show_stdout = String::from_utf8_lossy(&show_proxy.stdout);
    assert!(
        show_stdout.contains("Source:   [patched]"),
        "show PROXY must be [patched], got: {show_stdout}"
    );
}

#[test]
fn group_list_and_show_json_output() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // 1. group list --json
    let list_out = run(tmp.path(), &["--json", "group", "list"]);
    assert!(
        list_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    let list_val: serde_json::Value =
        serde_json::from_slice(&list_out.stdout).expect("valid JSON for group list");
    assert_eq!(
        list_val["active_subscription"].as_str(),
        Some("sub-12345678")
    );
    assert!(!list_val["groups"].as_array().unwrap().is_empty());
    let original = &list_val["groups"][0];
    assert_eq!(original["name"].as_str(), Some("Original"));
    assert_eq!(original["source"].as_str(), Some("native"));

    // 2. group show --json
    let show_out = run(tmp.path(), &["--json", "group", "show", "Original"]);
    assert!(
        show_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&show_out.stderr)
    );
    let show_val: serde_json::Value =
        serde_json::from_slice(&show_out.stdout).expect("valid JSON for group show");
    assert_eq!(show_val["name"].as_str(), Some("Original"));
    assert_eq!(show_val["source"].as_str(), Some("native"));
    assert!(show_val["definition"]["proxies"].is_array());
}

#[test]
fn group_show_outputs_rich_details_and_selection() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // 1. Set up selection intent in selections/sub-12345678.yaml
    let sel_dir = tmp.path().join("selections");
    std::fs::create_dir_all(&sel_dir).unwrap();
    std::fs::write(
        sel_dir.join("sub-12345678.yaml"),
        "selections:\n  Original: Node-A\n",
    )
    .unwrap();

    // 2. Add patch to Original
    let add_out = run(
        tmp.path(),
        &["group", "add", "Original", "--member", "Node-B"],
    );
    assert!(add_out.status.success());

    // 3. Show Original
    let show_out = run(tmp.path(), &["group", "show", "Original"]);
    assert!(show_out.status.success());
    let stdout = String::from_utf8_lossy(&show_out.stdout);

    // Must show Source, Selected node, and Patch details
    assert!(stdout.contains("Source:   [patched]"), "stdout: {stdout}");
    assert!(
        stdout.contains("Selected: Node-A (persisted)"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("Patch:"), "stdout: {stdout}");
    assert!(stdout.contains("Node-B"), "stdout: {stdout}");
}

#[test]
fn group_delete_json_stdout_pure_for_custom_group() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let create_out = run(
        tmp.path(),
        &[
            "group",
            "create",
            "CustomSel",
            "--type",
            "select",
            "--member",
            "Node-A",
        ],
    );
    assert!(create_out.status.success());

    let del_out = run(tmp.path(), &["--json", "group", "delete", "CustomSel"]);
    assert!(
        del_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&del_out.stderr)
    );
    let val: serde_json::Value =
        serde_json::from_slice(&del_out.stdout).expect("valid JSON envelope on stdout");
    assert_eq!(val["status"].as_str(), Some("success"));
    assert_eq!(val["action"].as_str(), Some("delete"));
    assert_eq!(val["group"].as_str(), Some("CustomSel"));
    let stdout_str = String::from_utf8_lossy(&del_out.stdout);
    assert!(stdout_str.trim().starts_with('{'));
}

#[test]
fn group_delete_json_stdout_pure_for_native_group_and_notice_to_stderr() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let del_out = run(tmp.path(), &["--json", "group", "delete", "Original"]);
    assert!(
        del_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&del_out.stderr)
    );
    let val: serde_json::Value =
        serde_json::from_slice(&del_out.stdout).expect("valid JSON envelope on stdout");
    assert_eq!(val["status"].as_str(), Some("success"));
    assert_eq!(val["action"].as_str(), Some("delete"));
    assert_eq!(val["group"].as_str(), Some("Original"));

    let stdout_str = String::from_utf8_lossy(&del_out.stdout);
    assert!(
        !stdout_str.contains("原生组"),
        "stdout must not contain text notice: {stdout_str}"
    );

    let stderr_str = String::from_utf8_lossy(&del_out.stderr);
    assert!(
        stderr_str.contains("原生组 `Original` 已标记隐藏"),
        "stderr must contain hide notice: {stderr_str}"
    );

    // Non-json mode check: notice remains on stdout
    let tmp2 = TempDir::new().unwrap();
    setup(tmp2.path());
    let plain_del = run(tmp2.path(), &["group", "delete", "Original"]);
    assert!(plain_del.status.success());
    let plain_stdout = String::from_utf8_lossy(&plain_del.stdout);
    assert!(
        plain_stdout.contains("原生组 `Original` 已标记隐藏"),
        "plain mode stdout must keep hide notice: {plain_stdout}"
    );
}

#[test]
fn group_delete_json_cleanup_failure_routes_warning_to_stderr_and_stdout_is_pure_json() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let create_out = run(
        tmp.path(),
        &[
            "group",
            "create",
            "CustomSel",
            "--type",
            "select",
            "--member",
            "Node-A",
        ],
    );
    assert!(create_out.status.success());

    // Corrupt selections file to induce unpin failure
    let sel_dir = tmp.path().join("selections");
    std::fs::create_dir_all(&sel_dir).unwrap();
    std::fs::write(sel_dir.join("sub-12345678.yaml"), "not: valid: [yaml").unwrap();

    let del_out = run(tmp.path(), &["--json", "group", "delete", "CustomSel"]);
    assert!(
        del_out.status.success(),
        "delete commits even if unpin fails: {}",
        String::from_utf8_lossy(&del_out.stderr)
    );
    let val: serde_json::Value =
        serde_json::from_slice(&del_out.stdout).expect("valid JSON envelope on stdout");
    assert_eq!(val["status"].as_str(), Some("success"));
    assert_eq!(val["group"].as_str(), Some("CustomSel"));

    let stderr_str = String::from_utf8_lossy(&del_out.stderr);
    assert!(
        stderr_str.contains('⚠') && stderr_str.contains("was not cleaned up"),
        "stderr must contain cleanup failure warning: {stderr_str}"
    );

    let stdout_str = String::from_utf8_lossy(&del_out.stdout);
    assert!(
        !stdout_str.contains('⚠'),
        "stdout must not be polluted with warning: {stdout_str}"
    );
}

#[test]
fn group_reset_json_noop_routes_notice_to_stderr_and_stdout_is_pure_json() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    // Original is at upstream state (no patch, not deleted)
    let reset_out = run(tmp.path(), &["--json", "group", "reset", "Original"]);
    assert!(
        reset_out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&reset_out.stderr)
    );
    let val: serde_json::Value =
        serde_json::from_slice(&reset_out.stdout).expect("valid JSON envelope on stdout");
    assert_eq!(val["status"].as_str(), Some("success"));
    assert_eq!(val["action"].as_str(), Some("reset"));
    assert_eq!(val["group"].as_str(), Some("Original"));

    let stderr_str = String::from_utf8_lossy(&reset_out.stderr);
    assert!(
        stderr_str.contains("proxy group `Original` is already at upstream state (no-op)"),
        "stderr must contain no-op notice: {stderr_str}"
    );

    let stdout_str = String::from_utf8_lossy(&reset_out.stdout);
    assert!(
        !stdout_str.contains("upstream state"),
        "stdout must not contain no-op notice text: {stdout_str}"
    );

    // Non-json mode check: notice remains on stdout
    let tmp2 = TempDir::new().unwrap();
    setup(tmp2.path());
    let plain_reset = run(tmp2.path(), &["group", "reset", "Original"]);
    assert!(plain_reset.status.success());
    let plain_stdout = String::from_utf8_lossy(&plain_reset.stdout);
    assert!(
        plain_stdout.contains("proxy group `Original` is already at upstream state (no-op)"),
        "plain mode stdout must keep no-op notice: {plain_stdout}"
    );
}

#[test]
fn group_edit_with_provider_from_override_succeeds() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let override_src = tmp.path().join("my-override.yaml");
    std::fs::write(
        &override_src,
        r#"
proxy-providers:
  corp-host:
    type: inline
    payload:
      - name: SG-corp-host
        type: socks5
        server: 127.0.0.1
        port: 11080
"#,
    )
    .unwrap();
    let import_out = run(
        tmp.path(),
        &["override", "import", override_src.to_str().unwrap()],
    );
    assert!(
        import_out.status.success(),
        "override import must succeed: {}",
        String::from_utf8_lossy(&import_out.stderr)
    );

    let edit_file = tmp.path().join("group-edit.yaml");
    std::fs::write(
        &edit_file,
        r#"
name: Original
type: select
proxies:
  - Node-A
use:
  - corp-host
"#,
    )
    .unwrap();

    let edit_out = run(
        tmp.path(),
        &["group", "edit", "Original", edit_file.to_str().unwrap()],
    );
    assert!(
        edit_out.status.success(),
        "group edit referencing override provider must succeed: {}",
        String::from_utf8_lossy(&edit_out.stderr)
    );

    let config_content = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
    assert!(
        config_content.contains("corp-host"),
        "config.yaml must contain corp-host provider reference"
    );
}

#[test]
fn group_add_use_provider_succeeds() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let override_src = tmp.path().join("my-override.yaml");
    std::fs::write(
        &override_src,
        r#"
proxy-providers:
  corp-host:
    type: inline
    payload:
      - name: SG-corp-host
        type: socks5
        server: 127.0.0.1
        port: 11080
"#,
    )
    .unwrap();
    let import_out = run(
        tmp.path(),
        &["override", "import", override_src.to_str().unwrap()],
    );
    assert!(
        import_out.status.success(),
        "override import must succeed: {}",
        String::from_utf8_lossy(&import_out.stderr)
    );

    let add_out = run(
        tmp.path(),
        &["group", "add", "Original", "--use", "corp-host"],
    );
    assert!(
        add_out.status.success(),
        "group add --use corp-host must succeed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    let show_out = run(tmp.path(), &["group", "show", "Original"]);
    let stdout = String::from_utf8_lossy(&show_out.stdout);
    assert!(
        stdout.contains("corp-host"),
        "show output must show add_use: {stdout}"
    );

    let config_content = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
    assert!(
        config_content.contains("corp-host"),
        "config.yaml must contain corp-host"
    );
}

#[test]
fn group_add_member_from_proxies_append_succeeds() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let override_src = tmp.path().join("my-override.yaml");
    std::fs::write(
        &override_src,
        r#"
proxies-append:
  - name: SG-corp-host
    type: socks5
    server: 127.0.0.1
    port: 11080
"#,
    )
    .unwrap();
    let import_out = run(
        tmp.path(),
        &["override", "import", override_src.to_str().unwrap()],
    );
    assert!(
        import_out.status.success(),
        "override import must succeed: {}",
        String::from_utf8_lossy(&import_out.stderr)
    );

    let add_out = run(
        tmp.path(),
        &["group", "add", "Original", "--member", "SG-corp-host"],
    );
    assert!(
        add_out.status.success(),
        "group add --member SG-corp-host must succeed: {}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    let config_content = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
    let parsed: serde_yaml::Value = serde_yaml::from_str(&config_content).unwrap();
    let proxies = parsed["proxies"].as_sequence().unwrap();
    assert!(
        proxies.iter().any(|p| p["name"] == "SG-corp-host"),
        "config.yaml proxies must contain SG-corp-host"
    );
}

#[test]
fn group_add_provider_payload_node_rejected_with_helpful_error() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let override_src = tmp.path().join("my-override.yaml");
    std::fs::write(
        &override_src,
        r#"
proxy-providers:
  corp-host:
    type: inline
    payload:
      - name: SG-corp-host
        type: socks5
        server: 127.0.0.1
        port: 11080
"#,
    )
    .unwrap();
    let import_out = run(
        tmp.path(),
        &["override", "import", override_src.to_str().unwrap()],
    );
    assert!(import_out.status.success());

    let add_out = run(
        tmp.path(),
        &["group", "add", "Original", "--member", "SG-corp-host"],
    );
    assert!(
        !add_out.status.success(),
        "adding provider payload node as member must fail"
    );
    let stderr = String::from_utf8_lossy(&add_out.stderr);
    assert!(
        stderr.contains("defined inside proxy-provider `corp-host`"),
        "stderr must contain helpful provider guidance: {stderr}"
    );
}

#[test]
fn group_add_provider_name_as_member_rejected_with_helpful_error() {
    let tmp = TempDir::new().unwrap();
    setup(tmp.path());

    let override_src = tmp.path().join("my-override.yaml");
    std::fs::write(
        &override_src,
        r#"
proxy-providers:
  corp-host:
    type: inline
    payload:
      - name: SG-corp-host
        type: socks5
        server: 127.0.0.1
        port: 11080
"#,
    )
    .unwrap();
    let import_out = run(
        tmp.path(),
        &["override", "import", override_src.to_str().unwrap()],
    );
    assert!(import_out.status.success());

    let add_out = run(
        tmp.path(),
        &["group", "add", "Original", "--member", "corp-host"],
    );
    assert!(
        !add_out.status.success(),
        "adding provider name as member must fail"
    );
    let stderr = String::from_utf8_lossy(&add_out.stderr);
    assert!(
        stderr.contains("is a proxy-provider, not a proxy node"),
        "stderr must guide user to --use: {stderr}"
    );
}
