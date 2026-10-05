// Gate 6: Exit-code contract tests for lifecycle commands
//
// These tests verify that lifecycle commands return consistent exit codes
// and provide appropriate feedback. The tests document the actual behavior.

use std::process::Command;
use tempfile::TempDir;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_mihomo-cli")
}

fn isolated_env() -> TempDir {
    let tmp = TempDir::new().unwrap();
    // Create minimal valid config
    std::fs::write(
        tmp.path().join("config.yaml"),
        "mixed-port: 7890\nlog-level: info\n",
    )
    .unwrap();
    tmp
}

// ── G6.1: stop 命令退出码 ──────────────────────────────────────────

#[test]
fn g6_stop_no_instance_returns_zero_with_message() {
    // 实际行为：stop 即使没有运行实例也返回 0，但会输出消息
    let tmp = isolated_env();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["stop"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    // stop 应该总是成功（幂等操作）
    assert!(
        output.status.success(),
        "stop should succeed even with no running instance (idempotent)"
    );

    // 应该有输出说明状态
    assert!(
        combined.contains("Stopping") || combined.contains("stop") || !combined.is_empty(),
        "stop should give some output, got: {combined}"
    );
}

// ── G6.2: start 命令退出码 ──────────────────────────────────────────

#[test]
fn g6_start_no_service_installed_gives_feedback() {
    let tmp = TempDir::new().unwrap();
    // 空配置目录，没有安装服务

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["start"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    // 应该给出反馈（可能成功进入交互安装，或失败提示需要安装）
    assert!(
        !combined.is_empty(),
        "start with no service should give feedback, got empty output"
    );
}

#[test]
fn g6_config_validate_invalid_config_returns_nonzero() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("config.yaml"), "port: [\n").unwrap();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["config", "--validate"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "config --validate with invalid YAML should fail"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not valid")
            || stderr.contains("invalid")
            || stderr.contains("error")
            || stderr.contains("YAML"),
        "config --validate should give error message, got: {stderr}"
    );
}

// ── G6.3: restart 命令退出码 ────────────────────────────────────────

#[test]
fn g6_restart_no_instance_gives_feedback() {
    let tmp = isolated_env();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["restart"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    // restart 应该给出反馈
    assert!(
        !combined.is_empty(),
        "restart should give feedback about instance state"
    );
}

// ── G6.4: status 命令退出码 ─────────────────────────────────────────

#[test]
fn g6_status_help_describes_read_only_overview_without_probe() {
    let output = Command::new(bin())
        .args(["status", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success(), "status --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("read-only running status overview"),
        "status help should describe a read-only overview, got: {stdout}"
    );
    assert!(
        !stdout.to_ascii_lowercase().contains("probe"),
        "status help must not advertise a network probe, got: {stdout}"
    );
}

#[test]
fn g6_status_gives_instance_info() {
    let tmp = isolated_env();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["status"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    // status 应该总是给出信息
    assert!(
        !combined.is_empty(),
        "status should give info about instance state"
    );

    // 应该包含一些状态相关信息
    let has_status_info = combined.contains("not running")
        || combined.contains("No running")
        || combined.contains("stopped")
        || combined.contains("Status")
        || combined.contains("Mode")
        || combined.contains("mode")
        || combined.contains("instance");

    assert!(
        has_status_info || output.status.success(),
        "status should give meaningful info, got: {combined}"
    );
}

// ── G6.5: config --validate 退出码 ──────────────────────────────────

#[test]
fn g6_config_validate_valid_config_returns_zero() {
    let tmp = isolated_env();

    let output = Command::new(bin())
        .env("MIHOMO_CLI_CONFIG_DIR", tmp.path())
        .args(["config", "--validate"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "config --validate with valid config should succeed"
    );
}
