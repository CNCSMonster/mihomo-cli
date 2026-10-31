// `mihomo-cli manual` contract: embedded manual is rendered from compile-time
// content only (no repository checkout, no disk reads, no network, no writes).

use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

fn run_in_isolation(args: &[&str]) -> (std::process::Output, TempDir) {
    let tmp = TempDir::new().unwrap();
    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(args)
        .output()
        .unwrap();
    (output, tmp)
}

fn assert_untouched(tmp: &TempDir) {
    assert_eq!(
        std::fs::read_dir(tmp.path()).unwrap().count(),
        0,
        "manual must not create files"
    );
}

#[test]
fn manual_default_and_index_are_identical() {
    let (default, default_tmp) = run_in_isolation(&["manual"]);
    let (index, index_tmp) = run_in_isolation(&["manual", "index"]);

    assert!(default.status.success());
    assert!(index.status.success());
    assert!(default.stderr.is_empty(), "stderr must stay empty");
    let stdout = String::from_utf8_lossy(&default.stdout);
    assert!(stdout.starts_with("# mihomo-cli 用户手册\n"));
    assert_eq!(default.stdout, index.stdout);
    assert_untouched(&default_tmp);
    assert_untouched(&index_tmp);
}

#[test]
fn doc_alias_is_equivalent_to_manual() {
    let (manual_topic, manual_tmp) = run_in_isolation(&["manual", "tun"]);
    let (doc_topic, doc_tmp) = run_in_isolation(&["doc", "tun"]);
    assert!(manual_topic.status.success());
    assert!(doc_topic.status.success());
    assert_eq!(manual_topic.stdout, doc_topic.stdout);
    assert_untouched(&manual_tmp);
    assert_untouched(&doc_tmp);

    let (manual_list, manual_list_tmp) = run_in_isolation(&["manual", "--list"]);
    let (doc_list, doc_list_tmp) = run_in_isolation(&["doc", "--list"]);
    assert!(manual_list.status.success());
    assert!(doc_list.status.success());
    assert_eq!(manual_list.stdout, doc_list.stdout);
    assert_untouched(&manual_list_tmp);
    assert_untouched(&doc_list_tmp);
}

#[test]
fn manual_list_has_twelve_slug_tab_title_lines() {
    let (output, tmp) = run_in_isolation(&["manual", "--list"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 12, "stdout was:\n{stdout}");
    assert_eq!(lines[0], "index\tmihomo-cli 用户手册");
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 2, "line must be `<slug>\\t<title>`: {line}");
        assert!(!fields[0].is_empty() && !fields[1].is_empty());
    }
    assert_untouched(&tmp);
}

#[test]
fn manual_renders_topic_content_verbatim() {
    let (output, tmp) = run_in_isolation(&["manual", "tun"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("# TUN 透明代理\n"), "stdout:\n{stdout}");
    assert_untouched(&tmp);
}

#[test]
fn manual_json_topic_is_a_single_parseable_envelope() {
    let (output, tmp) = run_in_isolation(&["--json", "manual", "tun"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parsing the whole stdout proves there is no decoration around the envelope.
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["command"], "manual");
    assert_eq!(value["data"]["topic"], "tun");
    assert!(value["data"]["content"]
        .as_str()
        .unwrap()
        .starts_with("# TUN 透明代理"));
    assert_untouched(&tmp);
}

#[test]
fn manual_json_default_is_the_index_topic() {
    let (output, _tmp) = run_in_isolation(&["--json", "manual"]);
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout))
        .expect("stdout must be one JSON document");
    assert_eq!(value["data"]["topic"], "index");
    assert!(value["data"]["content"]
        .as_str()
        .unwrap()
        .starts_with("# mihomo-cli 用户手册"));
}

#[test]
fn manual_json_list_is_parseable_and_complete() {
    let (output, tmp) = run_in_isolation(&["manual", "--json", "--list"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout))
        .expect("stdout must be one JSON document");
    let topics = value["data"]["topics"].as_array().unwrap();
    assert_eq!(topics.len(), 12);
    assert_eq!(topics[0]["slug"], "index");
    assert_eq!(topics[7]["slug"], "tun");
    assert_untouched(&tmp);
}

#[test]
fn manual_unknown_topic_exits_2_with_stderr_hint() {
    let (output, tmp) = run_in_isolation(&["manual", "nope"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "unknown topic is a usage error; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "stdout must stay clean");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown manual topic: nope"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("Available topics:"), "stderr: {stderr}");
    assert!(stderr.contains("manual --list"), "stderr: {stderr}");
    assert_untouched(&tmp);
}

#[test]
fn manual_json_unknown_topic_keeps_one_error_envelope_and_exits_2() {
    let (output, _tmp) = run_in_isolation(&["--json", "manual", "nope"]);
    assert_eq!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(&output.stdout))
        .expect("stdout must be one JSON document");
    assert_eq!(value["ok"], false);
    assert_eq!(value["command"], "manual");
    assert!(value["error"]
        .as_str()
        .unwrap()
        .contains("unknown manual topic: nope"));
}

#[test]
fn manual_list_and_topic_are_mutually_exclusive() {
    let (output, _tmp) = run_in_isolation(&["manual", "--list", "tun"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn help_of_main_commands_points_at_the_matching_manual_topic() {
    for (command, topic) in [
        ("install", "install"),
        ("upgrade", "upgrade"),
        ("select", "nodes"),
        ("tun", "tun"),
        ("status", "install"),
        ("group", "nodes"),
        ("rule", "subscription"),
    ] {
        let (output, _tmp) = run_in_isolation(&[command, "--help"]);
        assert!(output.status.success(), "{command} --help failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let expected = format!("See: mihomo-cli manual {topic}");
        assert!(
            stdout.contains(&expected),
            "{command} --help must contain `{expected}`:\n{stdout}"
        );
    }
}
