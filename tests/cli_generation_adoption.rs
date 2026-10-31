use std::process::{Command, Stdio};

fn invoke(args: &[&str]) -> std::process::Output {
    let temp = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_mihomo-cli"))
        .args(args)
        .env("MIHOMO_CLI_CONFIG_DIR", temp.path())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn unconfirmed_noninteractive_adoption_fails_before_any_privileged_access() {
    let output = invoke(&["install", "--system", "--adopt-current"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("--yes"));
    assert!(!message.contains("sudo:"));
}

#[test]
fn json_unconfirmed_adoption_is_exactly_one_error_envelope() {
    let output = invoke(&["--json", "install", "--system", "--adopt-current"]);
    assert!(!output.status.success());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["ok"], false);
    assert!(String::from_utf8_lossy(&output.stdout).contains("--yes"));
}

#[test]
fn incompatible_or_unscoped_flags_fail_during_argument_parsing() {
    for args in [
        vec!["install", "--adopt-current"],
        vec!["install", "--system", "--dry-run"],
        vec!["install", "--system", "--adopt-current", "--force"],
        vec!["install", "--system", "--adopt-current", "--version=v1"],
        vec!["doctor", "--privileged"],
        vec!["doctor", "--user", "--privileged"],
    ] {
        let output = invoke(&args);
        assert_eq!(output.status.code(), Some(2), "args={args:?}");
    }
}

#[test]
fn help_explains_explicit_trust_preview_and_read_only_authorization() {
    let install = invoke(&["install", "--help"]);
    let help = String::from_utf8_lossy(&install.stdout);
    assert!(help.contains("--adopt-current") && help.contains("--dry-run"));
    let doctor = invoke(&["doctor", "--help"]);
    let help = String::from_utf8_lossy(&doctor.stdout);
    assert!(help.contains("--privileged") && help.contains("read-only"));
}
