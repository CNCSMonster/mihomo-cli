#![allow(dead_code)]
use crate::utils;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceOs {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceMode {
    System,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedCommand {
    program: String,
    args: Vec<String>,
    privileged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TimedCommandExecution {
    Sudo { args: Vec<String> },
    UserSystemctl { args: Vec<String> },
    Direct(PlannedCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SudoDispatch {
    DirectAsRoot,
    NonInteractiveSudo,
    PromptPassword,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildWaitDecision {
    Finished(bool),
    KeepWaiting,
    TimedOut,
    PollError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceDetection {
    Installed(ServiceMode),
    NotInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SudoCommandPlan {
    program: String,
    args: Vec<String>,
    stdin: Option<String>,
}

/// Unified privilege executor — single entry point for all privileged operations.
///
/// Consolidates scattered sudo/root dispatch logic. All privileged file writes,
/// command executions, and path removals go through this struct.
///
/// Smart dispatch:
///   1. Already root → run directly, no sudo, no prompt.
///   2. Sudo credentials cached → `sudo -n`, no prompt.
///   3. Otherwise → prompt for password via dialoguer, pipe to `sudo -S`.
pub(crate) struct PrivilegeExecutor;

impl PrivilegeExecutor {
    /// Check if currently running as root.
    pub(crate) fn is_root() -> bool {
        is_root()
    }

    /// Execute a command that may require elevated privileges.
    pub(crate) fn run(args: &[&str]) -> anyhow::Result<()> {
        run_privileged(args)
    }

    /// Write file content to a privileged path atomically (stage → install).
    pub(crate) fn write_file(
        path: &std::path::Path,
        content: &[u8],
        mode: u16,
    ) -> anyhow::Result<()> {
        install_staged_file_privileged(path, content, mode)
    }

    /// Write string content to a privileged path (convenience wrapper).
    pub(crate) fn write_file_str(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
        write_file_privileged(path, content)
    }

    /// Remove a file or directory at a privileged path.
    pub(crate) fn remove_path(path: &std::path::Path) -> anyhow::Result<()> {
        remove_path_privileged(&path.display().to_string())
    }

    /// Create a directory at a privileged path.
    pub(crate) fn ensure_dir(path: &std::path::Path, mode: u16) -> anyhow::Result<()> {
        let mode_str = format!("{mode:o}");
        run_privileged(&[
            "install",
            "-d",
            "-m",
            &mode_str,
            &path.display().to_string(),
        ])
    }
}

impl PlannedCommand {
    fn new(program: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            privileged: false,
        }
    }

    fn privileged(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            privileged: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceFilePlan {
    path: String,
    content: String,
    privileged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceFileRemoval {
    path: String,
    privileged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrivilegedFileWritePlan {
    program: String,
    args: Vec<String>,
    stdin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceInstallPlan {
    cleanup_commands: Vec<PlannedCommand>,
    cleanup_files: Vec<ServiceFileRemoval>,
    pre_commands: Vec<PlannedCommand>,
    files: Vec<ServiceFilePlan>,
    commands: Vec<PlannedCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceUninstallPlan {
    commands: Vec<PlannedCommand>,
    files: Vec<ServiceFileRemoval>,
    legacy_service_mode_marker: Option<ServiceFileRemoval>,
}

fn linux_system_unit_path() -> &'static str {
    "/etc/systemd/system/mihomo.service"
}

fn linux_user_unit_path(home: &str) -> String {
    format!("{home}/.config/systemd/user/mihomo.service")
}

fn macos_daemon_plist_path() -> &'static str {
    "/Library/LaunchDaemons/io.mihomo.plist"
}

fn macos_agent_plist_path(home: &str) -> String {
    format!("{home}/Library/LaunchAgents/io.mihomo.plist")
}

/// Returns the launchd GUI domain for the current user (e.g., "gui/501").
/// Used for LaunchAgent Modern API commands (bootstrap/bootout/kickstart/kill).
fn macos_gui_domain() -> String {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    format!("gui/{uid}")
}

fn current_service_os() -> anyhow::Result<ServiceOs> {
    if cfg!(target_os = "linux") {
        Ok(ServiceOs::Linux)
    } else if cfg!(target_os = "macos") {
        Ok(ServiceOs::Macos)
    } else if cfg!(target_os = "windows") {
        Ok(ServiceOs::Windows)
    } else {
        anyhow::bail!("Unsupported OS")
    }
}

fn service_start_command(os: ServiceOs, mode: ServiceMode, _home: &str) -> PlannedCommand {
    match (os, mode) {
        (ServiceOs::Linux, ServiceMode::User) => {
            PlannedCommand::new("systemctl", ["--user", "start", "mihomo"])
        }
        (ServiceOs::Linux, ServiceMode::System) => {
            PlannedCommand::privileged("systemctl", ["start", "mihomo"])
        }
        (ServiceOs::Macos, ServiceMode::User) => {
            let domain = macos_gui_domain();
            PlannedCommand::new(
                "launchctl",
                ["kickstart", "-k", &format!("{domain}/io.mihomo")],
            )
        }
        (ServiceOs::Macos, ServiceMode::System) => {
            PlannedCommand::privileged("launchctl", ["kickstart", "-k", "system/io.mihomo"])
        }
        (ServiceOs::Windows, _) => PlannedCommand::new("sc.exe", ["start", "mihomo"]),
    }
}

fn service_stop_command(os: ServiceOs, mode: ServiceMode, _home: &str) -> PlannedCommand {
    match (os, mode) {
        (ServiceOs::Linux, ServiceMode::User) => {
            PlannedCommand::new("systemctl", ["--user", "stop", "mihomo"])
        }
        (ServiceOs::Linux, ServiceMode::System) => {
            PlannedCommand::privileged("systemctl", ["stop", "mihomo"])
        }
        (ServiceOs::Macos, ServiceMode::User) => {
            let domain = macos_gui_domain();
            PlannedCommand::new(
                "launchctl",
                ["kill", "SIGTERM", &format!("{domain}/io.mihomo")],
            )
        }
        (ServiceOs::Macos, ServiceMode::System) => {
            PlannedCommand::privileged("launchctl", ["kill", "SIGTERM", "system/io.mihomo"])
        }
        (ServiceOs::Windows, _) => PlannedCommand::new("sc.exe", ["stop", "mihomo"]),
    }
}

fn service_start_message(os: ServiceOs, mode: ServiceMode) -> &'static str {
    match (os, mode) {
        (ServiceOs::Linux, ServiceMode::User) => "Starting mihomo via user service...",
        (ServiceOs::Macos, ServiceMode::User) => "Starting mihomo via LaunchAgent...",
        _ => "Starting mihomo via service...",
    }
}

fn direct_start_missing_binary_error(path: &str) -> String {
    format!(
        "mihomo binary not found at {path}
  Run: mihomo-cli install"
    )
}

fn direct_start_missing_config_error() -> &'static str {
    "No config found.
  Run: mihomo-cli config"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MihomoProcessCheck {
    WindowsTasklist,
    Pgrep,
}

fn current_mihomo_process_check() -> MihomoProcessCheck {
    if cfg!(target_os = "windows") {
        MihomoProcessCheck::WindowsTasklist
    } else {
        MihomoProcessCheck::Pgrep
    }
}

fn mihomo_process_command(check: MihomoProcessCheck) -> PlannedCommand {
    match check {
        MihomoProcessCheck::WindowsTasklist => {
            PlannedCommand::new("tasklist", ["/FI", "IMAGENAME eq mihomo.exe"])
        }
        MihomoProcessCheck::Pgrep => PlannedCommand::new("pgrep", ["-x", "mihomo"]),
    }
}

fn parse_mihomo_process_running(
    check: MihomoProcessCheck,
    status_success: bool,
    stdout: &str,
) -> bool {
    match check {
        MihomoProcessCheck::WindowsTasklist => stdout
            .lines()
            .any(|line| line.to_ascii_lowercase().contains("mihomo.exe")),
        MihomoProcessCheck::Pgrep => status_success,
    }
}

fn is_mihomo_process_running_with(check: MihomoProcessCheck) -> bool {
    let command = mihomo_process_command(check);
    Command::new(&command.program)
        .args(&command.args)
        .output()
        .map(|o| {
            parse_mihomo_process_running(
                check,
                o.status.success(),
                &String::from_utf8_lossy(&o.stdout),
            )
        })
        .unwrap_or(false)
}

fn is_mihomo_process_running() -> bool {
    is_mihomo_process_running_with(current_mihomo_process_check())
}

fn config_test_crash_error() -> &'static str {
    "mihomo crashed during config test — binary may be corrupted.
  Run: mihomo-cli update"
}

fn config_syntax_error(output: &str) -> String {
    format!(
        "Config syntax error — mihomo cannot start.
{}",
        output.lines().take(5).collect::<Vec<_>>().join(
            "
"
        )
    )
}

fn direct_start_message() -> &'static str {
    "No service installed, starting mihomo directly..."
}

fn start_failure_with_log_error(log: &str) -> String {
    format!(
        "mihomo failed to start.
  Check logs: tail -20 {log}"
    )
}

fn config_test_no_output_diag(exit_code: i32) -> String {
    format!("Config test failed (exit {exit_code}) with no output")
}

fn config_syntax_diag(output: &str) -> String {
    format!(
        "Config syntax error:
{}",
        output.lines().take(5).collect::<Vec<_>>().join(
            "
"
        )
    )
}

fn cannot_run_config_test_diag(error: &str) -> String {
    format!("Cannot run mihomo -t: {error}")
}

fn missing_binary_diag(mihomo: &str) -> String {
    format!("mihomo binary not found at {mihomo}")
}

fn start_failure_no_log_error(diag: &str) -> String {
    format!(
        "mihomo failed to start (no log file).
  {diag}
  Try: mihomo-cli uninstall --all && mihomo-cli install"
    )
}

fn restart_with_fixed_config_message() -> &'static str {
    "  Restarting with fixed config..."
}

fn restart_after_fix_failed_error(log_path: &str) -> String {
    format!(
        "Failed to restart after config fix.
  Check logs: tail -20 {log_path}"
    )
}

fn socket_unreachable_with_controller_error(log_path: &str) -> String {
    format!(
        "Socket unreachable and config already has controller.
  Check logs: tail -20 {log_path}"
    )
}

fn start_result_lines(api_ready: bool) -> Vec<String> {
    if api_ready {
        vec!["Done.".to_string(), "Run: mihomo-cli status".to_string()]
    } else {
        vec!["  Check: mihomo-cli status".to_string()]
    }
}

fn api_not_ready_warning_lines() -> Vec<String> {
    vec![
        "  ⚠ API not responding after 15s — mihomo may still be initializing.".to_string(),
        crate::mihomo_api::socket_fix_suggestion(),
    ]
}

fn socket_fix_applied_message() -> &'static str {
    "  ⚠ Config was missing Unix socket controller — fixed."
}

#[cfg(unix)]
fn stale_socket_cleanup_message() -> &'static str {
    "  Cleaned up stale socket"
}

#[cfg(unix)]
fn stale_socket_cleanup_failed_message(error: &str, sock: &str) -> String {
    format!(
        "  ⚠ Cannot remove stale socket: {error}
      Run: sudo rm -f {sock}"
    )
}

fn service_restart_commands(os: ServiceOs, mode: ServiceMode, _home: &str) -> Vec<PlannedCommand> {
    match (os, mode) {
        (ServiceOs::Linux, ServiceMode::User) => {
            vec![PlannedCommand::new(
                "systemctl",
                ["--user", "restart", "mihomo"],
            )]
        }
        (ServiceOs::Linux, ServiceMode::System) => {
            vec![PlannedCommand::privileged(
                "systemctl",
                ["restart", "mihomo"],
            )]
        }
        (ServiceOs::Macos, ServiceMode::System) => vec![PlannedCommand::privileged(
            "launchctl",
            ["kickstart", "-k", "system/io.mihomo"],
        )],
        (ServiceOs::Macos, ServiceMode::User) => {
            let domain = macos_gui_domain();
            vec![PlannedCommand::new(
                "launchctl",
                ["kickstart", "-k", &format!("{domain}/io.mihomo")],
            )]
        }
        (ServiceOs::Windows, _) => vec![
            PlannedCommand::new("sc.exe", ["stop", "mihomo"]),
            PlannedCommand::new("sc.exe", ["start", "mihomo"]),
        ],
    }
}

/// Check if a launchd job with the given label is currently loaded.
fn macos_service_loaded(label: &str) -> bool {
    Command::new("launchctl")
        .args(["list", label])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn launchd_plist(home: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>io.mihomo</string>
<key>ProgramArguments</key><array><string>{home}/.config/mihomo/start.sh</string></array>
<key>RunAtLoad</key><false/><key>KeepAlive</key><dict><key>Crashed</key><true/></dict>
<key>WorkingDirectory</key><string>{home}/.config/mihomo</string>
<key>StandardOutPath</key><string>{home}/.config/mihomo/mihomo.log</string>
<key>StandardErrorPath</key><string>{home}/.config/mihomo/mihomo.log</string>
</dict></plist>
"#
    )
}

fn systemd_system_prepare_commands() -> Vec<PlannedCommand> {
    vec![
        PlannedCommand::privileged("sh", ["-c", "getent group mihomo >/dev/null || groupadd --system mihomo"]),
        PlannedCommand::privileged("sh", ["-c", "id -u mihomo >/dev/null 2>&1 || useradd --system --gid mihomo --home-dir /var/lib/mihomo-cli --no-create-home --shell /usr/sbin/nologin mihomo"]),
        PlannedCommand::privileged("install", ["-d", "-o", "mihomo", "-g", "mihomo", "-m", "0750", "/var/run/mihomo"]),
        PlannedCommand::privileged("install", ["-d", "-o", "mihomo", "-g", "mihomo", "-m", "0750", "/var/log/mihomo"]),
        PlannedCommand::privileged("install", ["-d", "-o", "root", "-g", "mihomo", "-m", "0750", "/var/lib/mihomo-cli"]),
    ]
}

fn systemd_install_plan(
    mode: ServiceMode,
    home: &str,
    user: Option<&str>,
    old_other_mode_exists: bool,
) -> ServiceInstallPlan {
    match mode {
        ServiceMode::System => {
            let mut cleanup_commands = Vec::new();
            if old_other_mode_exists {
                cleanup_commands.extend([
                    PlannedCommand::new("systemctl", ["--user", "stop", "mihomo"]),
                    PlannedCommand::new("systemctl", ["--user", "disable", "mihomo"]),
                    PlannedCommand::new("systemctl", ["--user", "daemon-reload"]),
                ]);
            }
            ServiceInstallPlan {
                cleanup_commands,
                cleanup_files: if old_other_mode_exists {
                    vec![ServiceFileRemoval {
                        path: linux_user_unit_path(home),
                        privileged: false,
                    }]
                } else {
                    vec![]
                },
                pre_commands: systemd_system_prepare_commands(),
                files: vec![ServiceFilePlan {
                    path: linux_system_unit_path().to_string(),
                    content: systemd_system_unit(home),
                    privileged: true,
                }],
                commands: vec![
                    PlannedCommand::privileged("systemctl", ["daemon-reload"]),
                    PlannedCommand::privileged("systemctl", ["enable", "--now", "mihomo"]),
                ],
            }
        }
        ServiceMode::User => {
            let mut cleanup_commands = Vec::new();
            if old_other_mode_exists {
                cleanup_commands.extend([
                    PlannedCommand::privileged("systemctl", ["stop", "mihomo"]),
                    PlannedCommand::privileged("systemctl", ["disable", "mihomo"]),
                    PlannedCommand::privileged("rm", [linux_system_unit_path()]),
                    PlannedCommand::privileged("systemctl", ["daemon-reload"]),
                ]);
            }
            let mut commands = vec![
                PlannedCommand::new("systemctl", ["--user", "daemon-reload"]),
                PlannedCommand::new("systemctl", ["--user", "enable", "--now", "mihomo"]),
            ];
            if let Some(user) = user.filter(|u| !u.is_empty()) {
                commands.push(PlannedCommand::privileged(
                    "loginctl",
                    ["enable-linger", user],
                ));
            }
            ServiceInstallPlan {
                cleanup_commands,
                cleanup_files: if old_other_mode_exists {
                    vec![ServiceFileRemoval {
                        path: linux_system_unit_path().to_string(),
                        privileged: true,
                    }]
                } else {
                    vec![]
                },
                pre_commands: vec![],
                files: vec![ServiceFilePlan {
                    path: linux_user_unit_path(home),
                    content: systemd_user_unit(home),
                    privileged: false,
                }],
                commands,
            }
        }
    }
}

fn launchd_install_plan(mode: ServiceMode, home: &str) -> ServiceInstallPlan {
    match mode {
        ServiceMode::System => ServiceInstallPlan {
            cleanup_commands: vec![],
            cleanup_files: vec![],
            pre_commands: vec![],
            files: vec![ServiceFilePlan {
                path: macos_daemon_plist_path().to_string(),
                content: launchd_plist(home),
                privileged: true,
            }],
            commands: vec![
                PlannedCommand::privileged("chmod", ["644", macos_daemon_plist_path()]),
                PlannedCommand::privileged(
                    "launchctl",
                    ["bootstrap", "system", macos_daemon_plist_path()],
                ),
            ],
        },
        ServiceMode::User => ServiceInstallPlan {
            cleanup_commands: vec![],
            cleanup_files: vec![],
            pre_commands: vec![],
            files: vec![ServiceFilePlan {
                path: macos_agent_plist_path(home),
                content: launchd_plist(home),
                privileged: false,
            }],
            commands: vec![PlannedCommand::new(
                "launchctl",
                [
                    "bootstrap",
                    &macos_gui_domain(),
                    &macos_agent_plist_path(home),
                ],
            )],
        },
    }
}

fn systemd_uninstall_plan(
    mode: ServiceMode,
    home: &str,
    marker_path: &str,
) -> ServiceUninstallPlan {
    match mode {
        ServiceMode::System => ServiceUninstallPlan {
            commands: vec![
                PlannedCommand::privileged("systemctl", ["stop", "mihomo"]),
                PlannedCommand::privileged("systemctl", ["disable", "mihomo"]),
                PlannedCommand::privileged("systemctl", ["daemon-reload"]),
            ],
            files: vec![ServiceFileRemoval {
                path: linux_system_unit_path().to_string(),
                privileged: true,
            }],
            legacy_service_mode_marker: Some(ServiceFileRemoval {
                path: marker_path.to_string(),
                privileged: false,
            }),
        },
        ServiceMode::User => ServiceUninstallPlan {
            commands: vec![
                PlannedCommand::new("systemctl", ["--user", "stop", "mihomo"]),
                PlannedCommand::new("systemctl", ["--user", "disable", "mihomo"]),
                PlannedCommand::new("systemctl", ["--user", "daemon-reload"]),
            ],
            files: vec![ServiceFileRemoval {
                path: linux_user_unit_path(home),
                privileged: false,
            }],
            legacy_service_mode_marker: Some(ServiceFileRemoval {
                path: marker_path.to_string(),
                privileged: false,
            }),
        },
    }
}

fn launchd_uninstall_plan(
    mode: ServiceMode,
    home: &str,
    marker_path: &str,
) -> ServiceUninstallPlan {
    match mode {
        ServiceMode::System => ServiceUninstallPlan {
            commands: vec![PlannedCommand::privileged(
                "launchctl",
                ["bootout", "system/io.mihomo"],
            )],
            files: vec![ServiceFileRemoval {
                path: macos_daemon_plist_path().to_string(),
                privileged: true,
            }],
            legacy_service_mode_marker: Some(ServiceFileRemoval {
                path: marker_path.to_string(),
                privileged: false,
            }),
        },
        ServiceMode::User => ServiceUninstallPlan {
            commands: vec![PlannedCommand::new(
                "launchctl",
                ["bootout", &format!("{}/io.mihomo", macos_gui_domain())],
            )],
            files: vec![ServiceFileRemoval {
                path: macos_agent_plist_path(home),
                privileged: false,
            }],
            legacy_service_mode_marker: Some(ServiceFileRemoval {
                path: marker_path.to_string(),
                privileged: false,
            }),
        },
    }
}

fn windows_uninstall_plan(marker_path: &str) -> ServiceUninstallPlan {
    ServiceUninstallPlan {
        commands: vec![
            PlannedCommand::new("sc.exe", ["stop", "mihomo"]),
            PlannedCommand::new("sc.exe", ["delete", "mihomo"]),
        ],
        files: vec![],
        legacy_service_mode_marker: Some(ServiceFileRemoval {
            path: marker_path.to_string(),
            privileged: false,
        }),
    }
}

fn windows_install_commands(config_dir: &str, bin_path: &str) -> Vec<PlannedCommand> {
    vec![
        PlannedCommand::new(
            "sc.exe",
            [
                "create",
                "mihomo",
                "binPath=",
                &format!("\"{bin_path}\" -d \"{config_dir}\""),
                "start=",
                "auto",
                "DisplayName=",
                "Mihomo Proxy Service",
            ],
        ),
        PlannedCommand::new("sc.exe", ["start", "mihomo"]),
    ]
}

fn config_test_command(mihomo: &str, config_dir: &str) -> PlannedCommand {
    PlannedCommand::new(mihomo, ["-t", "-d", config_dir])
}

fn direct_start_command(mihomo: &str, config_dir: &str) -> PlannedCommand {
    #[cfg(unix)]
    {
        PlannedCommand::new("nohup", [mihomo, "-d", config_dir])
    }
    #[cfg(not(unix))]
    {
        PlannedCommand::new(mihomo, ["-d", config_dir])
    }
}

fn windows_service_query_command() -> PlannedCommand {
    PlannedCommand::new("sc.exe", ["query", "mihomo"])
}

fn windows_kill_command() -> PlannedCommand {
    PlannedCommand::new("taskkill", ["/F", "/IM", "mihomo.exe"])
}

fn pkill_mihomo_command() -> PlannedCommand {
    PlannedCommand::new("pkill", ["-9", "-x", "mihomo"])
}

fn sudo_pkill_mihomo_command() -> PlannedCommand {
    PlannedCommand::new("sudo", ["pkill", "-9", "-x", "mihomo"])
}

pub(crate) fn windows_service_query_indicates_installed(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout).contains("STATE")
}

#[cfg(windows)]
/// Install the mihomo Windows service via the service-manager crate.
///
/// service-manager's `sc create` path escapes the binPath properly (shell_escape,
/// lifted from mullvad/windows-service-rs) — fixing the StartService 87 caused by
/// manual binPath quoting through PowerShell.
pub fn windows_install_service(ctx: &crate::instance::InstanceContext) -> anyhow::Result<()> {
    use service_manager::{ServiceInstallCtx, ServiceLabel, ServiceManager, ServiceStartCtx};

    // Persist the installing user's SID — the daemon (running as SYSTEM) reads
    // it at startup to build the pipe SDDL (it cannot query its own token for
    // the installer's identity).
    persist_installer_sid()?;
    // Generate + persist the daemon IPC auth token (N1a: double copy —
    // server %ProgramData%\mihomo\service-token, client <config_dir>\service-client-token).
    persist_service_token(ctx)?;

    let manager = <dyn ServiceManager>::native()
        .map_err(|e| anyhow::anyhow!("failed to get service manager: {e}"))?;
    let label: ServiceLabel = "mihomo"
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid service label: {e}"))?;

    // Remove any stale service first (idempotent install).
    let _ = manager.uninstall(service_manager::ServiceUninstallCtx {
        label: label.clone(),
    });

    manager
        .install(ServiceInstallCtx {
            label: label.clone(),
            program: ctx.paths.cli_binary.clone(),
            args: vec![std::ffi::OsString::from("daemon")],
            contents: None,
            username: None,
            working_directory: None,
            environment: None,
            // ADR-17: default NO autostart; user opts in via `autostart on`.
            autostart: false,
            restart_policy: service_manager::RestartPolicy::OnFailure {
                delay_secs: Some(2),
                max_retries: None,
                reset_after_secs: Some(60),
            },
        })
        .map_err(|e| anyhow::anyhow!("failed to install mihomo service: {e}"))?;

    manager
        .start(ServiceStartCtx { label })
        .map_err(|e| anyhow::anyhow!("failed to start mihomo service: {e}"))?;
    Ok(())
}

#[cfg(windows)]
/// Persist the installing user's SID to `%ProgramData%\mihomo\installer-sid`.
///
/// The daemon (SYSTEM) reads this at startup to build the pipe SDDL — it cannot
/// query its own token for the installer's identity.
fn persist_installer_sid() -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: windows_sys::Win32::Foundation::HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            anyhow::bail!("OpenProcessToken failed");
        }

        // First call gets required size.
        let mut size: u32 = 0;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0u8; size as usize];
        if GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr() as *mut _,
            size,
            &mut size,
        ) == 0
        {
            CloseHandle(token);
            anyhow::bail!("GetTokenInformation(TokenUser) failed");
        }
        let token_user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let user_sid = token_user.User.Sid;

        let mut sid_string_ptr: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user_sid, &mut sid_string_ptr) == 0 {
            CloseHandle(token);
            anyhow::bail!("ConvertSidToStringSidW failed");
        }
        let sid_string = {
            let mut len = 0;
            while *sid_string_ptr.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(sid_string_ptr, len))
        };
        windows_sys::Win32::Foundation::LocalFree(sid_string_ptr as *mut _);
        CloseHandle(token);

        // Write to %ProgramData%\mihomo\installer-sid
        let program_data = std::env::var_os("ProgramData")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"));
        let dir = program_data.join("mihomo");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("installer-sid"), sid_string.as_bytes())?;
        Ok(())
    }
}

/// Generate a 32-byte random token as 64 lowercase hex chars — the daemon IPC
/// auth token. Pure function (cross-platform, testable).
pub(crate) fn generate_auth_token() -> String {
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn generate_and_write_token(token_path: &std::path::Path) -> anyhow::Result<String> {
    let token = generate_auth_token();
    if let Some(parent) = token_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(token_path, token.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(token_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(token)
}

#[cfg(unix)]
pub(crate) fn client_token_path_for_home(home: &std::path::Path) -> std::path::PathBuf {
    home.join(".config").join("mihomo").join("service-token")
}

#[cfg(unix)]
pub(crate) fn generate_client_token_for_home(home: &std::path::Path) -> anyhow::Result<String> {
    generate_and_write_token(&client_token_path_for_home(home))
}

#[cfg(windows)]
/// Generate a 32-byte random token and persist it in two copies (N1a):
/// - server: `%ProgramData%\mihomo\service-token` (daemon reads for validation)
/// - client: `<config_dir>\service-client-token` (CLI reads to attach)
fn persist_service_token(ctx: &crate::instance::InstanceContext) -> anyhow::Result<()> {
    let program_data = std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"));
    let token = generate_and_write_token(&program_data.join("mihomo").join("service-token"))?;

    std::fs::create_dir_all(&ctx.paths.config_dir)?;
    std::fs::write(
        ctx.paths.config_dir.join("service-client-token"),
        token.as_bytes(),
    )?;
    Ok(())
}

#[cfg(windows)]
/// Uninstall the mihomo Windows service via the service-manager crate.
pub fn windows_uninstall_service() -> anyhow::Result<()> {
    use service_manager::{ServiceManager, ServiceUninstallCtx};

    let manager = <dyn ServiceManager>::native()
        .map_err(|e| anyhow::anyhow!("failed to get service manager: {e}"))?;
    let label: service_manager::ServiceLabel = "mihomo"
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid service label: {e}"))?;
    let _ = manager.uninstall(ServiceUninstallCtx { label });
    Ok(())
}

fn linux_service_detection(system_unit_exists: bool, user_unit_exists: bool) -> ServiceDetection {
    if system_unit_exists {
        ServiceDetection::Installed(ServiceMode::System)
    } else if user_unit_exists {
        ServiceDetection::Installed(ServiceMode::User)
    } else {
        ServiceDetection::NotInstalled
    }
}

fn macos_service_detection(daemon_exists: bool, agent_exists: bool) -> ServiceDetection {
    if daemon_exists {
        ServiceDetection::Installed(ServiceMode::System)
    } else if agent_exists {
        ServiceDetection::Installed(ServiceMode::User)
    } else {
        ServiceDetection::NotInstalled
    }
}

fn service_detection_result(detection: ServiceDetection) -> bool {
    // v3 mode resolution is runtime/service-artifact driven. Probing whether a
    // legacy service exists must not recreate the deprecated .service-mode
    // marker as a side effect.
    matches!(detection, ServiceDetection::Installed(_))
}

fn run_command_output(command: &PlannedCommand) -> std::io::Result<std::process::Output> {
    Command::new(&command.program).args(&command.args).output()
}

fn spawn_detached_command(command: &PlannedCommand) -> std::io::Result<std::process::Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        Command::new(&command.program)
            .args(&command.args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
    }
    #[cfg(not(windows))]
    {
        Command::new(&command.program)
            .args(&command.args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    }
}

fn run_planned_command(command: &PlannedCommand) -> anyhow::Result<()> {
    if command.privileged {
        let mut args: Vec<&str> = Vec::with_capacity(command.args.len() + 1);
        args.push(command.program.as_str());
        args.extend(command.args.iter().map(String::as_str));
        run_privileged(&args)
    } else {
        let status = Command::new(&command.program)
            .args(&command.args)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!(
                "Command failed: {} {} (exit: {})",
                command.program,
                command.args.join(" "),
                status
            )
        }
    }
}

fn planned_timed_command_execution(command: &PlannedCommand) -> TimedCommandExecution {
    if command.privileged {
        let mut args = Vec::with_capacity(command.args.len() + 1);
        args.push(command.program.clone());
        args.extend(command.args.clone());
        TimedCommandExecution::Sudo { args }
    } else if command.program == "systemctl"
        && command.args.first().map(String::as_str) == Some("--user")
    {
        TimedCommandExecution::UserSystemctl {
            args: command.args.iter().skip(1).cloned().collect(),
        }
    } else {
        TimedCommandExecution::Direct(command.clone())
    }
}

fn run_planned_command_with_timeout(
    command: &PlannedCommand,
    timeout: std::time::Duration,
) -> bool {
    match planned_timed_command_execution(command) {
        TimedCommandExecution::Sudo { args } => {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run_sudo_with_timeout(&args, timeout)
        }
        TimedCommandExecution::UserSystemctl { args } => {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run_userctl(&args, timeout)
        }
        TimedCommandExecution::Direct(command) => Command::new(&command.program)
            .args(&command.args)
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
    }
}

fn run_planned_command_best_effort(command: &PlannedCommand) {
    let _ = run_planned_command(command);
}

fn remove_planned_file_best_effort(removal: &ServiceFileRemoval) {
    if removal.privileged {
        let _ = run_privileged(&["rm", &removal.path]);
    } else {
        let _ = std::fs::remove_file(&removal.path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsElevatedCommandPlan {
    program: String,
    args: Vec<String>,
}

fn powershell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn windows_elevated_powershell_plan(script: &str) -> WindowsElevatedCommandPlan {
    WindowsElevatedCommandPlan {
        program: "powershell.exe".to_string(),
        args: vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-Command".to_string(),
            format!(
                "$p=Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-Command',{}) -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
                powershell_single_quote(script)
            ),
        ],
    }
}

fn windows_elevated_native_command_script(program: &str, args: &[String]) -> String {
    // Use the PowerShell call operator `&` so each argument is passed verbatim.
    // Start-Process -ArgumentList would re-parse embedded quotes in args like
    // `binPath= "C:\...\mihomo-cli.exe" daemon` and break sc.exe's binPath.
    let mut parts = vec![format!("& '{}'", program.replace('\'', "''"))];
    for arg in args {
        parts.push(format!("'{}'", arg.replace('\'', "''")));
    }
    format!("{}; exit $LASTEXITCODE", parts.join(" "))
}

fn windows_copy_file_elevated_script(source: &std::path::Path, target: &std::path::Path) -> String {
    let parent = target.parent().unwrap_or_else(|| std::path::Path::new("."));
    format!(
        "New-Item -ItemType Directory -Force -Path {} | Out-Null; Copy-Item -Force -LiteralPath {} -Destination {}",
        powershell_single_quote(&parent.display().to_string()),
        powershell_single_quote(&source.display().to_string()),
        powershell_single_quote(&target.display().to_string())
    )
}

fn windows_remove_path_elevated_script(path: &str) -> String {
    format!(
        "if (Test-Path -LiteralPath {}) {{ Remove-Item -LiteralPath {} -Recurse -Force }}",
        powershell_single_quote(path),
        powershell_single_quote(path)
    )
}

fn windows_create_dir_elevated_script(path: &std::path::Path) -> String {
    format!(
        "New-Item -ItemType Directory -Force -Path {} | Out-Null",
        powershell_single_quote(&path.display().to_string())
    )
}

/// Windows: is the current process running with elevated (Administrator) token?
/// Used to skip the UAC prompt when already elevated (e.g. CI runners, admin shells).
///
/// Queries TokenElevation via GetTokenInformation (windows-sys) — more reliable
/// than spawning `net session` (localization-independent, no subprocess).
#[cfg(windows)]
pub(crate) fn is_process_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut size: u32 = 0;
        let ok = GetTokenInformation(
            token,
            windows_sys::Win32::Security::TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        ) != 0;
        CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(windows)]
fn run_windows_elevated_powershell(script: &str) -> anyhow::Result<()> {
    // Already elevated (CI runner / admin shell)? Run directly — UAC prompt
    // would hang in non-interactive environments.
    if is_process_elevated() {
        let status = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .map_err(|e| anyhow::anyhow!("failed to run elevated command: {e}"))?;
        if status.success() {
            return Ok(());
        }
        anyhow::bail!("elevated command failed")
    }
    let plan = windows_elevated_powershell_plan(script);
    let status = std::process::Command::new(&plan.program)
        .args(&plan.args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| anyhow::anyhow!("failed to request Administrator privileges via UAC: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("elevated command failed or was cancelled by the user")
    }
}

pub fn create_dir_privileged(path: &std::path::Path, mode: u16) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mode = format!("{mode:o}");
        run_privileged(&["install", "-d", "-m", &mode, &path.display().to_string()])
    }
    #[cfg(windows)]
    {
        let _ = mode;
        run_windows_elevated_powershell(&windows_create_dir_elevated_script(path))
    }
}

#[allow(dead_code)]
pub(crate) fn write_file_privileged(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    install_staged_file_privileged(path, content.as_bytes(), 0o644)
}

pub fn install_staged_file_privileged(
    path: &std::path::Path,
    bytes: &[u8],
    mode: u16,
) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let temp_path = std::env::temp_dir().join(format!(
            "mihomo-cli-privileged-write-{}-{}",
            std::process::id(),
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("payload")
        ));
        std::fs::write(&temp_path, bytes)?;
        let cleanup = TempFileCleanup(temp_path.clone());
        let script = windows_copy_file_elevated_script(&temp_path, path);
        let _ = mode;
        run_windows_elevated_powershell(&script)?;
        drop(cleanup);
        Ok(())
    }

    #[cfg(unix)]
    {
        // 1. L6: Reject symlinks to prevent symlink attacks
        if let Ok(metadata) = path.symlink_metadata() {
            if metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "refusing to write to symlink: {}\n  \
                     This is a security measure to prevent symlink attacks.",
                    path.display()
                );
            }
        }

        // 2. Already root → write directly (O_NOFOLLOW + explicit permissions).
        if is_root() {
            return write_installed_file_direct(path, bytes, mode);
        }

        // 3. Non-root → stage in an O_NOFOLLOW-protected temp file, then
        //    `sudo install` to the target (restores privilege escalation).
        install_staged_file_privileged_non_root(path, bytes, mode)
    }
}

#[cfg(unix)]
/// Direct write with O_NOFOLLOW, used when running as root. Also exercised
/// directly by tests, which must never trigger a real sudo invocation.
fn write_installed_file_direct(
    path: &std::path::Path,
    bytes: &[u8],
    mode: u16,
) -> anyhow::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(mode as u32)
        .open(path)?;

    file.write_all(bytes)?;
    drop(file);

    // Explicitly set permissions (mode in OpenOptions only applies to new files)
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode as u32))?;

    Ok(())
}

#[cfg(unix)]
/// Non-root fallback: stage `bytes` in an O_NOFOLLOW-protected temp file
/// (0o600), then install it at `path` via `sudo install -m <mode>`.
fn install_staged_file_privileged_non_root(
    path: &std::path::Path,
    bytes: &[u8],
    mode: u16,
) -> anyhow::Result<()> {
    let temp_path = temp_payload_path(path);
    write_temp_payload(&temp_path, bytes)?;
    let cleanup = TempFileCleanup(temp_path.clone());
    let args: Vec<String> = privileged_install_args(&temp_path, mode, path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = run_privileged(&arg_refs);
    drop(cleanup);
    result
}

#[cfg(unix)]
/// Name of the staged temp file for a privileged write to `target`.
fn temp_payload_path(target: &std::path::Path) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "mihomo-cli-privileged-write-{}-{}",
        std::process::id(),
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("payload")
    ))
}

#[cfg(unix)]
/// Write the staged payload with O_NOFOLLOW protection and owner-only mode.
fn write_temp_payload(temp_path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(temp_path)?;
    file.write_all(bytes)
}

#[cfg(unix)]
/// Construct the `sudo install` argv for a staged privileged write.
fn privileged_install_args(
    temp_path: &std::path::Path,
    mode: u16,
    target: &std::path::Path,
) -> Vec<String> {
    vec![
        "install".to_string(),
        "-m".to_string(),
        format!("{mode:o}"),
        temp_path.display().to_string(),
        target.display().to_string(),
    ]
}
struct TempFileCleanup(std::path::PathBuf);

impl Drop for TempFileCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn remove_path_privileged(path: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        run_privileged(&["rm", "-rf", path])
    }
    #[cfg(windows)]
    {
        run_windows_elevated_powershell(&windows_remove_path_elevated_script(path))
    }
}

fn privileged_file_write_plan(file: &ServiceFilePlan) -> PrivilegedFileWritePlan {
    PrivilegedFileWritePlan {
        program: "sudo".to_string(),
        args: vec!["tee".to_string(), file.path.clone()],
        stdin: file.content.clone(),
    }
}

fn write_planned_file(file: &ServiceFilePlan) -> anyhow::Result<()> {
    if file.privileged {
        println!("Creating {} (sudo required)...", file.path);
        let plan = privileged_file_write_plan(file);
        let mut child = Command::new(&plan.program)
            .args(&plan.args)
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(plan.stdin.as_bytes())?;
        let status = child.wait()?;
        if !status.success() {
            anyhow::bail!("Failed to write {} (exit: {})", file.path, status);
        }
        Ok(())
    } else {
        if let Some(parent) = std::path::Path::new(&file.path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file.path, &file.content)?;
        Ok(())
    }
}

trait ServicePlanExecutor {
    fn run(&mut self, command: &PlannedCommand) -> anyhow::Result<()>;
    fn run_best_effort(&mut self, command: &PlannedCommand);
    fn remove_best_effort(&mut self, removal: &ServiceFileRemoval);
    fn write_file(&mut self, file: &ServiceFilePlan) -> anyhow::Result<()>;
}

struct SystemServicePlanExecutor;

impl ServicePlanExecutor for SystemServicePlanExecutor {
    fn run(&mut self, command: &PlannedCommand) -> anyhow::Result<()> {
        run_planned_command(command)
    }

    fn run_best_effort(&mut self, command: &PlannedCommand) {
        run_planned_command_best_effort(command);
    }

    fn remove_best_effort(&mut self, removal: &ServiceFileRemoval) {
        remove_planned_file_best_effort(removal);
    }

    fn write_file(&mut self, file: &ServiceFilePlan) -> anyhow::Result<()> {
        write_planned_file(file)
    }
}

fn apply_service_install_plan(plan: &ServiceInstallPlan) -> anyhow::Result<()> {
    let mut executor = SystemServicePlanExecutor;
    apply_service_install_plan_with(plan, &mut executor)
}

fn apply_service_install_plan_with(
    plan: &ServiceInstallPlan,
    executor: &mut dyn ServicePlanExecutor,
) -> anyhow::Result<()> {
    for command in &plan.cleanup_commands {
        executor.run_best_effort(command);
    }
    for removal in &plan.cleanup_files {
        executor.remove_best_effort(removal);
    }
    for command in &plan.pre_commands {
        executor.run(command)?;
    }
    for file in &plan.files {
        executor.write_file(file)?;
    }
    for command in &plan.commands {
        executor.run(command)?;
    }
    Ok(())
}

fn apply_service_uninstall_plan(plan: &ServiceUninstallPlan) {
    let mut executor = SystemServicePlanExecutor;
    apply_service_uninstall_plan_with(plan, &mut executor);
}

fn apply_service_uninstall_plan_with(
    plan: &ServiceUninstallPlan,
    executor: &mut dyn ServicePlanExecutor,
) {
    for command in &plan.commands {
        executor.run_best_effort(command);
    }
    for file in &plan.files {
        executor.remove_best_effort(file);
    }
    if let Some(marker) = &plan.legacy_service_mode_marker {
        executor.remove_best_effort(marker);
    }
}

pub fn run_instance_command(command: &crate::instance::PlannedCommand) -> anyhow::Result<()> {
    if command.privileged {
        #[cfg(unix)]
        {
            let mut args: Vec<&str> = Vec::with_capacity(command.args.len() + 1);
            args.push(command.program.as_str());
            args.extend(command.args.iter().map(String::as_str));
            return run_privileged(&args);
        }
        #[cfg(windows)]
        {
            let script = windows_elevated_native_command_script(&command.program, &command.args);
            return run_windows_elevated_powershell(&script);
        }
    }
    let status = std::process::Command::new(&command.program)
        .args(&command.args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run {}: {e}", command.program))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "command failed: {} {}",
            command.program,
            command.args.join(" ")
        )
    }
}

/// Run `sudo systemctl restart mihomo` (or `--user` variant) with a 30-second timeout.
/// Returns true if the command succeeded within the timeout.
#[allow(dead_code)]
fn restart_service(mode: ServiceMode) -> bool {
    let Ok(os) = current_service_os() else {
        return false;
    };
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let commands = service_restart_commands(os, mode, &home);
    for (idx, command) in commands.iter().enumerate() {
        if idx > 0 {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        if !run_planned_command_with_timeout(command, std::time::Duration::from_secs(30)) {
            return false;
        }
    }
    true
}

/// Run a user-level systemctl command with timeout.
fn run_userctl(args: &[&str], timeout: std::time::Duration) -> bool {
    let mut full_args = vec!["--user"];
    full_args.extend_from_slice(args);
    let status = Command::new("systemctl")
        .args(&full_args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn();
    match status {
        Ok(child) => wait_child(child, timeout),
        Err(_) => false,
    }
}

/// Run a privileged command with smart dispatch (root/cached/prompt).
/// Returns `Ok(())` on success, or a descriptive error on failure.
fn run_privileged(args: &[&str]) -> anyhow::Result<()> {
    if run_sudo_with_timeout(args, std::time::Duration::from_secs(30)) {
        Ok(())
    } else {
        anyhow::bail!("Command failed. Try manually: sudo {}", args.join(" "))
    }
}

/// Run a command that may need root privileges.
///
/// Smart dispatch:
///   1. Already root → run directly, no sudo, no prompt.
///   2. Sudo credentials cached (e.g. `sudo mihomo-cli ...`) → `sudo -n`, no prompt.
///   3. Otherwise → prompt for password via dialoguer, pipe to `sudo -S`.
///
/// The user never interacts with sudo directly — only with mihomo-cli.
fn plan_sudo_dispatch(is_root: bool, sudo_credentials_cached: bool) -> SudoDispatch {
    if is_root {
        SudoDispatch::DirectAsRoot
    } else if sudo_credentials_cached {
        SudoDispatch::NonInteractiveSudo
    } else {
        SudoDispatch::PromptPassword
    }
}

fn sudo_command_plan(
    dispatch: SudoDispatch,
    args: &[&str],
    password: Option<&str>,
) -> SudoCommandPlan {
    match dispatch {
        SudoDispatch::DirectAsRoot => SudoCommandPlan {
            program: args[0].to_string(),
            args: args[1..].iter().map(|arg| (*arg).to_string()).collect(),
            stdin: None,
        },
        SudoDispatch::NonInteractiveSudo => {
            let mut command_args = vec!["-n".to_string()];
            command_args.extend(args.iter().map(|arg| (*arg).to_string()));
            SudoCommandPlan {
                program: "sudo".to_string(),
                args: command_args,
                stdin: None,
            }
        }
        SudoDispatch::PromptPassword => {
            let mut command_args = vec!["-S".to_string()];
            command_args.extend(args.iter().map(|arg| (*arg).to_string()));
            SudoCommandPlan {
                program: "sudo".to_string(),
                args: command_args,
                stdin: password.map(|password| format!("{password}\n")),
            }
        }
    }
}

fn run_sudo_status_plan(plan: &SudoCommandPlan) -> bool {
    use std::process::{Command, Stdio};

    Command::new(&plan.program)
        .args(&plan.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PasswordProviderResult {
    Provided(String),
    Cancelled,
    IoError(String),
}

fn password_provider_result(outcome: Result<Option<String>, String>) -> PasswordProviderResult {
    match outcome {
        Ok(Some(p)) => PasswordProviderResult::Provided(p),
        Ok(None) => PasswordProviderResult::Cancelled,
        Err(e) => PasswordProviderResult::IoError(e),
    }
}

fn run_sudo_with_timeout(args: &[&str], timeout: std::time::Duration) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    match plan_sudo_dispatch(is_root(), sudo_credentials_cached()) {
        SudoDispatch::DirectAsRoot => {
            crate::log!("already root, running directly");
            return run_sudo_status_plan(&sudo_command_plan(
                SudoDispatch::DirectAsRoot,
                args,
                None,
            ));
        }
        SudoDispatch::NonInteractiveSudo => {
            crate::log!("sudo credentials cached, using sudo -n");
            return run_sudo_status_plan(&sudo_command_plan(
                SudoDispatch::NonInteractiveSudo,
                args,
                None,
            ));
        }
        SudoDispatch::PromptPassword => {}
    }

    // ── Case 3: Need password ──
    eprintln!("  The mihomo service runs as root.");
    eprintln!("  Restarting it requires admin privileges.");

    let password: Result<Option<String>, String> = dialoguer::Password::new()
        .with_prompt("sudo password")
        .allow_empty_password(false)
        .interact()
        .map(Some)
        .map_err(|e| e.to_string());

    let password = match password_provider_result(password) {
        PasswordProviderResult::Provided(p) if !p.is_empty() => p,
        PasswordProviderResult::Provided(_) | PasswordProviderResult::Cancelled => {
            eprintln!("  No password provided — aborting.");
            eprintln!("  Run manually: sudo systemctl restart mihomo");
            return false;
        }
        PasswordProviderResult::IoError(e) => {
            eprintln!("  Failed to read password: {e}");
            eprintln!("  Run manually: sudo systemctl restart mihomo");
            return false;
        }
    };

    // Spawn sudo -S (reads password from stdin, no TTY dependency)
    let plan = sudo_command_plan(SudoDispatch::PromptPassword, args, Some(&password));
    let mut child = match Command::new(&plan.program)
        .args(&plan.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  Failed to run sudo: {e}");
            eprintln!("  Try: sudo systemctl restart mihomo");
            return false;
        }
    };

    // Pipe password to sudo's stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(plan.stdin.as_deref().unwrap_or_default().as_bytes());
    }

    wait_child(child, timeout)
}

/// Check if we're already running as root.
fn is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| o.stdout.starts_with(b"0"))
        .unwrap_or(false)
}

/// Check if sudo credentials are already cached (non-interactive check).
fn sudo_credentials_cached() -> bool {
    std::process::Command::new("sudo")
        .args(["-n", "true"]) // -n: non-interactive, exits immediately if password needed
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn child_wait_decision(
    poll_result: std::io::Result<Option<std::process::ExitStatus>>,
    timed_out: bool,
) -> ChildWaitDecision {
    match poll_result {
        Ok(Some(status)) => ChildWaitDecision::Finished(status.success()),
        Ok(None) if timed_out => ChildWaitDecision::TimedOut,
        Ok(None) => ChildWaitDecision::KeepWaiting,
        Err(_) => ChildWaitDecision::PollError,
    }
}

/// Wait for a child process with timeout.
fn wait_child(mut child: std::process::Child, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        match child_wait_decision(child.try_wait(), start.elapsed() >= timeout) {
            ChildWaitDecision::Finished(success) => return success,
            ChildWaitDecision::KeepWaiting => {}
            ChildWaitDecision::PollError => return false,
            ChildWaitDecision::TimedOut => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("  Command timed out after {}s.", timeout.as_secs());
                return false;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

fn current_service_detection() -> ServiceDetection {
    if cfg!(target_os = "macos") {
        let home = dirs::home_dir().unwrap_or_default();
        macos_service_detection(
            std::path::Path::new("/Library/LaunchDaemons/io.mihomo.plist").exists(),
            std::path::Path::new(&home.join("Library/LaunchAgents/io.mihomo.plist")).exists(),
        )
    } else if cfg!(target_os = "linux") {
        let home = dirs::home_dir().unwrap_or_default();
        let user_path = format!("{}/.config/systemd/user/mihomo.service", home.display());
        linux_service_detection(
            std::path::Path::new("/etc/systemd/system/mihomo.service").exists(),
            std::path::Path::new(&user_path).exists(),
        )
    } else if cfg!(target_os = "windows") {
        match run_command_output(&windows_service_query_command()) {
            Ok(o) if windows_service_query_indicates_installed(&o.stdout) => {
                ServiceDetection::Installed(ServiceMode::System)
            }
            _ => ServiceDetection::NotInstalled,
        }
    } else {
        ServiceDetection::NotInstalled
    }
}

pub fn installed_service_mode_label() -> Option<&'static str> {
    match current_service_detection() {
        ServiceDetection::Installed(ServiceMode::System) => Some("system"),
        ServiceDetection::Installed(ServiceMode::User) => Some("user"),
        ServiceDetection::NotInstalled => None,
    }
}

pub fn service_installed() -> bool {
    service_detection_result(current_service_detection())
}

#[allow(dead_code)]
pub(crate) fn kill_mihomo() {
    if cfg!(target_os = "windows") {
        let command = windows_kill_command();
        let _ = Command::new(&command.program).args(&command.args).status();
        return;
    }

    // Try direct kill first (works if same user)
    let command = pkill_mihomo_command();
    let direct_ok = Command::new(&command.program)
        .args(&command.args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if direct_ok {
        return;
    }

    // If direct failed, try with sudo for legacy/system-owned processes.
    // Print to stderr so the user sees the sudo password prompt.
    eprintln!("  mihomo appears to be system-owned — sudo required to stop it.");
    let command = sudo_pkill_mihomo_command();
    let _ = Command::new(&command.program).args(&command.args).status();
}

/// Ensure the socket directory exists AND is writable by the current user.
/// Catches the case where the socket directory has wrong permissions — current
/// user can see it but cannot create files (socket bind) inside.
#[cfg(unix)]
fn ensure_socket_dir_writable(dir: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;

    let probe = format!("{dir}/.write_test");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(_) => {
            anyhow::bail!(
                "Socket directory {dir} is not writable.\n\
                 Fix: sudo chown -R $(whoami) {dir}"
            )
        }
    }
}

/// Remove a stale socket file left over from a previous run.
/// Safety: only deletes if the file is actually a socket AND no process is listening on it.
#[cfg(unix)]
fn cleanup_stale_socket() {
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::UnixStream;

    let sock = format!("{}/mihomo.sock", crate::utils::socket_dir());

    let Ok(meta) = std::fs::metadata(&sock) else {
        return;
    };
    if !meta.file_type().is_socket() {
        return;
    }

    match UnixStream::connect(&sock) {
        Ok(_) => {} // active listener — leave it alone
        Err(_) => match std::fs::remove_file(&sock) {
            Ok(()) => eprintln!("  Cleaned up stale socket"),
            Err(e) => eprintln!(
                "  ⚠ Cannot remove stale socket: {e}\n\
                     \x20 Run: sudo rm -f {sock}"
            ),
        },
    }
}

fn current_uid_gid() -> (String, String) {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let gid = std::process::Command::new("id")
        .arg("-g")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    (uid, gid)
}

fn start_script_content_for(
    home: &str,
    socket_dir: &str,
    owner_uid: &str,
    owner_gid: &str,
) -> String {
    format!(
        "#!/bin/bash\nset -e\nmkdir -p \"{socket_dir}\"\nif [ \"$(id -u)\" = \"0\" ] && [ -n \"{owner_uid}\" ] && [ -n \"{owner_gid}\" ]; then\n  chown {owner_uid}:{owner_gid} \"{socket_dir}\" || true\nfi\nchmod 700 \"{socket_dir}\"\nexec \"{home}/.local/bin/mihomo\" -d \"{home}/.config/mihomo\"\n"
    )
}

#[allow(dead_code)]
#[cfg(windows)]
pub(crate) fn start_script_content() -> String {
    "".to_string()
}

#[allow(dead_code)]
#[cfg(not(windows))]
pub(crate) fn start_script_content() -> String {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let socket_dir = crate::utils::socket_dir();
    let (uid, gid) = current_uid_gid();
    start_script_content_for(&home, &socket_dir, &uid, &gid)
}

fn launchd_install_result_lines(mode: ServiceMode, running: bool, home: &str) -> Vec<String> {
    match (mode, running) {
        (ServiceMode::System, true) => {
            vec!["LaunchDaemon installed and started — mihomo is running".to_string()]
        }
        (ServiceMode::User, true) => {
            vec!["LaunchAgent installed and started — mihomo is running (user mode)".to_string()]
        }
        (ServiceMode::System, false) => vec![
            "LaunchDaemon installed but mihomo may not be running.".to_string(),
            format!("Check logs: tail -f {home}/.config/mihomo/mihomo.log"),
        ],
        (ServiceMode::User, false) => vec![
            "LaunchAgent installed but mihomo may not be running.".to_string(),
            format!("Check logs: tail -f {home}/.config/mihomo/mihomo.log"),
        ],
    }
}

fn launchd_removed_message(mode: ServiceMode) -> &'static str {
    match mode {
        ServiceMode::System => "LaunchDaemon removed",
        ServiceMode::User => "LaunchAgent removed",
    }
}

fn systemd_old_service_removal_message(mode: ServiceMode) -> &'static str {
    match mode {
        ServiceMode::System => "Removing old system service...",
        ServiceMode::User => "Removing old user service...",
    }
}

fn systemd_linger_message(user: &str) -> String {
    format!("  Enabling linger for user '{user}' (service survives logout)...")
}

fn systemd_install_success_message(mode: ServiceMode) -> &'static str {
    match mode {
        ServiceMode::System => "systemd system service installed and started (system mode)",
        ServiceMode::User => "systemd user service installed and started",
    }
}

fn systemd_missing_service_message(mode: ServiceMode) -> &'static str {
    match mode {
        ServiceMode::System => "No system service found",
        ServiceMode::User => "No user service found",
    }
}

fn systemd_removed_message(mode: ServiceMode) -> &'static str {
    match mode {
        ServiceMode::System => "systemd system service removed",
        ServiceMode::User => "systemd user service removed",
    }
}

fn windows_install_success_message() -> &'static str {
    "Windows service installed and started"
}

fn windows_removed_message() -> &'static str {
    "Windows service removed"
}

// --- macOS ---

fn install_launchdaemon() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let plan = launchd_install_plan(ServiceMode::System, &home);
    apply_service_install_plan(&plan)?;

    std::thread::sleep(std::time::Duration::from_secs(3));
    let running = is_mihomo_process_running_with(MihomoProcessCheck::Pgrep);

    for line in launchd_install_result_lines(ServiceMode::System, running, &home) {
        println!("{line}");
    }
    Ok(())
}

#[allow(deprecated)]
fn uninstall_launchdaemon() -> bool {
    let p = macos_daemon_plist_path();
    if !std::path::Path::new(p).exists() {
        return false;
    }
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let marker = utils::legacy_service_mode_path();
    let plan = launchd_uninstall_plan(ServiceMode::System, &home, &marker);
    apply_service_uninstall_plan(&plan);
    println!("{}", launchd_removed_message(ServiceMode::System));
    true
}

fn install_launchagent() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let plan = launchd_install_plan(ServiceMode::User, &home);
    apply_service_install_plan(&plan)?;

    std::thread::sleep(std::time::Duration::from_secs(3));
    let running = is_mihomo_process_running_with(MihomoProcessCheck::Pgrep);

    for line in launchd_install_result_lines(ServiceMode::User, running, &home) {
        println!("{line}");
    }
    Ok(())
}

#[allow(deprecated)]
fn uninstall_launchagent() -> bool {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let p = macos_agent_plist_path(&home);
    if !std::path::Path::new(&p).exists() {
        return false;
    }
    let marker = utils::legacy_service_mode_path();
    let plan = launchd_uninstall_plan(ServiceMode::User, &home, &marker);
    apply_service_uninstall_plan(&plan);
    println!("{}", launchd_removed_message(ServiceMode::User));
    true
}

// --- Linux ---

/// Generate systemd unit content for system service mode.
fn systemd_system_unit(home: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Mihomo proxy\n\
         After=network.target\n\n\
         [Service]\n\
         Type=simple\n\
         User=mihomo\n\
         Group=mihomo\n\
         UMask=0027\n\
         RuntimeDirectory=mihomo\n\
         RuntimeDirectoryMode=0750\n\
         CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE\n\
         AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE\n\
         NoNewPrivileges=true\n\
         PrivateTmp=true\n\
         ProtectHome=read-only\n\
         ProtectSystem=strict\n\
         ReadWritePaths=/var/run/mihomo /run/mihomo /var/log/mihomo {home}/.config/mihomo /var/lib/mihomo-cli\n\
         ExecStart={home}/.local/bin/mihomo -d {home}/.config/mihomo\n\
         StandardOutput=append:/var/log/mihomo/mihomo.log\n\
         StandardError=append:/var/log/mihomo/mihomo.log\n\
         Restart=on-failure\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// Generate systemd unit content for user mode.
fn systemd_user_unit(home: &str) -> String {
    let log_path = format!("{home}/.config/mihomo/mihomo.log");
    format!(
        "[Unit]\n\
         Description=Mihomo proxy\n\
         After=network.target\n\n\
         [Service]\n\
         RuntimeDirectory=mihomo\n\
         RuntimeDirectoryMode=0700\n\
         ExecStart={home}/.local/bin/mihomo -d {home}/.config/mihomo\n\
         StandardOutput=append:{log_path}\n\
         StandardError=append:{log_path}\n\
         Restart=on-failure\n\n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

fn install_systemd_system() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let old_user_unit = std::path::Path::new(&linux_user_unit_path(&home)).exists();
    if old_user_unit {
        println!("{}", systemd_old_service_removal_message(ServiceMode::User));
    }
    let plan = systemd_install_plan(ServiceMode::System, &home, None, old_user_unit);
    apply_service_install_plan(&plan)?;
    println!("{}", systemd_install_success_message(ServiceMode::System));
    Ok(())
}

fn install_systemd_user() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let old_system_unit = std::path::Path::new(linux_system_unit_path()).exists();
    if old_system_unit {
        println!(
            "{}",
            systemd_old_service_removal_message(ServiceMode::System)
        );
    }

    let user = std::env::var("USER").unwrap_or_default();
    let needs_linger = if user.is_empty() {
        false
    } else {
        let linger_path = format!("/var/lib/systemd/linger/{user}");
        !std::path::Path::new(&linger_path).exists()
    };
    if needs_linger {
        println!("{}", systemd_linger_message(&user));
    }

    let plan = systemd_install_plan(
        ServiceMode::User,
        &home,
        needs_linger.then_some(user.as_str()),
        old_system_unit,
    );
    apply_service_install_plan(&plan)?;
    println!("{}", systemd_install_success_message(ServiceMode::User));
    Ok(())
}

#[allow(deprecated)]
fn uninstall_systemd() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let mode = match current_service_detection() {
        ServiceDetection::Installed(mode) => mode,
        ServiceDetection::NotInstalled => ServiceMode::User,
    };
    let unit_path = match mode {
        ServiceMode::System => linux_system_unit_path().to_string(),
        ServiceMode::User => linux_user_unit_path(&home),
    };

    if !std::path::Path::new(&unit_path).exists() {
        println!("{}", systemd_missing_service_message(mode));
        return Ok(());
    }

    let marker = utils::legacy_service_mode_path();
    let plan = systemd_uninstall_plan(mode, &home, &marker);
    apply_service_uninstall_plan(&plan);
    println!("{}", systemd_removed_message(mode));
    Ok(())
}

// --- Windows ---

fn mihomo_dirs() -> (String, String, String) {
    let local =
        dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
    let config_dir = format!("{}\\mihomo", local.display());
    let bin_path = format!("{}\\mihomo.exe", config_dir);
    let config_path = format!("{}\\config.yaml", config_dir);
    (config_dir, bin_path, config_path)
}

fn install_windows() -> anyhow::Result<()> {
    let (config_dir, bin_path, _) = mihomo_dirs();
    std::fs::create_dir_all(&config_dir)?;

    for command in windows_install_commands(&config_dir, &bin_path) {
        run_planned_command(&command)?;
    }

    println!("{}", windows_install_success_message());
    Ok(())
}

#[allow(deprecated)]
fn uninstall_windows() -> anyhow::Result<()> {
    let marker = utils::legacy_service_mode_path();
    let plan = windows_uninstall_plan(&marker);
    apply_service_uninstall_plan(&plan);
    println!("{}", windows_removed_message());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_token_is_64_hex_chars() {
        let token = generate_auth_token();
        assert_eq!(token.len(), 64);
        assert!(token
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(token, generate_auth_token());
    }

    #[test]
    fn generate_and_write_token_writes_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service-token");
        let token = generate_and_write_token(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), token);
        assert_eq!(token.len(), 64);
    }

    #[cfg(unix)]
    #[test]
    fn generate_and_write_token_sets_unix_permissions_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service-token");
        generate_and_write_token(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn generate_client_token_for_home_writes_0600_client_token() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let token = generate_client_token_for_home(dir.path()).unwrap();
        let path = client_token_path_for_home(dir.path());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), token);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[derive(Default)]
    struct RecordingExecutor {
        events: Vec<String>,
        fail_write_path: Option<String>,
        fail_run_program: Option<String>,
    }

    impl RecordingExecutor {
        fn command_label(command: &PlannedCommand) -> String {
            format!(
                "{}{} {}",
                if command.privileged { "sudo " } else { "" },
                command.program,
                command.args.join(" ")
            )
        }
    }

    impl ServicePlanExecutor for RecordingExecutor {
        fn run(&mut self, command: &PlannedCommand) -> anyhow::Result<()> {
            self.events
                .push(format!("run:{}", Self::command_label(command)));
            if self.fail_run_program.as_deref() == Some(command.program.as_str()) {
                anyhow::bail!("simulated command failure");
            }
            Ok(())
        }

        fn run_best_effort(&mut self, command: &PlannedCommand) {
            self.events
                .push(format!("best-effort:{}", Self::command_label(command)));
        }

        fn remove_best_effort(&mut self, removal: &ServiceFileRemoval) {
            self.events.push(format!(
                "remove:{}{}",
                if removal.privileged { "sudo " } else { "" },
                removal.path
            ));
        }

        fn write_file(&mut self, file: &ServiceFilePlan) -> anyhow::Result<()> {
            self.events.push(format!(
                "write:{}{}={}",
                if file.privileged { "sudo " } else { "" },
                file.path,
                file.content
            ));
            if self.fail_write_path.as_deref() == Some(file.path.as_str()) {
                anyhow::bail!("simulated write failure");
            }
            Ok(())
        }
    }

    #[test]
    fn start_script_chowns_socket_dir_back_to_installing_user_when_run_as_root() {
        let script = start_script_content_for("/Users/alice", "/tmp/mihomo-501", "501", "20");

        assert!(script.contains("mkdir -p \"/tmp/mihomo-501\""));
        assert!(script.contains("chown 501:20 \"/tmp/mihomo-501\" || true"));
        assert!(script.contains("chmod 700 \"/tmp/mihomo-501\""));
        assert!(script.contains(
            "exec \"/Users/alice/.local/bin/mihomo\" -d \"/Users/alice/.config/mihomo\""
        ));
    }

    #[test]
    fn windows_elevated_plans_use_uac_runas_and_quote_arguments() {
        let script = windows_elevated_native_command_script(
            "sc.exe",
            &[
                "create".to_string(),
                "mihomo".to_string(),
                "binPath= C:\\Program Files\\mihomo\\mihomo-cli.exe daemon".to_string(),
            ],
        );
        // Call operator preserves embedded quotes verbatim (Start-Process
        // would re-parse them and break sc.exe binPath).
        assert!(script.starts_with("& 'sc.exe'"));
        assert!(script.contains("'binPath= C:\\Program Files\\mihomo\\mihomo-cli.exe daemon'"));
        assert!(script.contains("exit $LASTEXITCODE"));

        let plan = windows_elevated_powershell_plan(&script);
        assert_eq!(plan.program, "powershell.exe");
        assert!(plan.args.iter().any(|arg| arg == "-NoProfile"));
        let command = plan.args.last().unwrap();
        assert!(command.contains("-Verb RunAs -Wait -PassThru"));
        assert!(command.contains("Start-Process -FilePath 'powershell.exe'"));
        assert!(command.contains("exit $p.ExitCode"));
    }

    #[test]
    fn windows_create_dir_script_uses_elevated_new_item() {
        let script =
            windows_create_dir_elevated_script(std::path::Path::new(r"C:\ProgramData\mihomo\bin"));
        assert!(script.contains("New-Item -ItemType Directory -Force"));
        assert!(script.contains(r"C:\ProgramData\mihomo\bin"));
    }

    #[test]
    fn windows_privileged_file_scripts_escape_single_quotes() {
        let copy = windows_copy_file_elevated_script(
            std::path::Path::new(r"C:\Temp\mihomo'src.exe"),
            std::path::Path::new(r"C:\ProgramData\mihomo\bin\mihomo.exe"),
        );
        assert!(copy.contains("New-Item -ItemType Directory -Force"));
        assert!(copy.contains(r"C:\Temp\mihomo''src.exe"));

        let remove = windows_remove_path_elevated_script(r"C:\ProgramData\mihomo");
        assert!(remove.contains("Remove-Item"));
        assert!(remove.contains(r"C:\ProgramData\mihomo"));
    }

    #[test]
    fn privileged_file_write_plan_uses_sudo_tee_and_stdin_content() {
        let file = ServiceFilePlan {
            path: "/etc/systemd/system/mihomo.service".to_string(),
            content: "[Unit]\nDescription=Mihomo\n".to_string(),
            privileged: true,
        };

        assert_eq!(
            privileged_file_write_plan(&file),
            PrivilegedFileWritePlan {
                program: "sudo".to_string(),
                args: vec![
                    "tee".to_string(),
                    "/etc/systemd/system/mihomo.service".to_string(),
                ],
                stdin: "[Unit]\nDescription=Mihomo\n".to_string(),
            }
        );
    }

    #[cfg(unix)]
    fn exit_status(code: i32) -> std::process::ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(code << 8)
    }

    #[test]
    #[cfg(unix)]
    fn child_wait_decision_maps_poll_results_and_timeout() {
        assert_eq!(
            child_wait_decision(Ok(Some(exit_status(0))), false),
            ChildWaitDecision::Finished(true)
        );
        assert_eq!(
            child_wait_decision(Ok(Some(exit_status(1))), false),
            ChildWaitDecision::Finished(false)
        );
        assert_eq!(
            child_wait_decision(Ok(None), false),
            ChildWaitDecision::KeepWaiting
        );
        assert_eq!(
            child_wait_decision(Ok(None), true),
            ChildWaitDecision::TimedOut
        );
        assert_eq!(
            child_wait_decision(Err(std::io::Error::other("poll failed")), false),
            ChildWaitDecision::PollError
        );
    }

    #[test]
    fn service_file_detection_prefers_root_then_user_then_not_installed() {
        assert_eq!(
            linux_service_detection(true, true),
            ServiceDetection::Installed(ServiceMode::System)
        );
        assert_eq!(
            linux_service_detection(false, true),
            ServiceDetection::Installed(ServiceMode::User)
        );
        assert_eq!(
            linux_service_detection(false, false),
            ServiceDetection::NotInstalled
        );

        assert_eq!(
            macos_service_detection(true, true),
            ServiceDetection::Installed(ServiceMode::System)
        );
        assert_eq!(
            macos_service_detection(false, true),
            ServiceDetection::Installed(ServiceMode::User)
        );
        assert_eq!(
            macos_service_detection(false, false),
            ServiceDetection::NotInstalled
        );
    }

    #[test]
    fn kill_and_windows_service_query_commands_are_planned_and_parsed() {
        assert_eq!(
            windows_service_query_command(),
            PlannedCommand::new("sc.exe", ["query", "mihomo"])
        );
        assert!(windows_service_query_indicates_installed(
            b"SERVICE_NAME: mihomo\r\n        STATE              : 4  RUNNING\r\n"
        ));
        assert!(!windows_service_query_indicates_installed(
            b"[SC] EnumQueryServicesStatus:OpenService FAILED 1060"
        ));
        assert_eq!(
            windows_kill_command(),
            PlannedCommand::new("taskkill", ["/F", "/IM", "mihomo.exe"])
        );
        assert_eq!(
            pkill_mihomo_command(),
            PlannedCommand::new("pkill", ["-9", "-x", "mihomo"])
        );
        assert_eq!(
            sudo_pkill_mihomo_command(),
            PlannedCommand::new("sudo", ["pkill", "-9", "-x", "mihomo"])
        );
    }

    #[test]
    fn direct_start_and_config_test_commands_are_planned() {
        assert_eq!(
            config_test_command("/opt/mihomo", "/home/u/.config/mihomo"),
            PlannedCommand::new("/opt/mihomo", ["-t", "-d", "/home/u/.config/mihomo"])
        );
        #[cfg(unix)]
        assert_eq!(
            direct_start_command("/opt/mihomo", "/home/u/.config/mihomo"),
            PlannedCommand::new("nohup", ["/opt/mihomo", "-d", "/home/u/.config/mihomo"])
        );
        #[cfg(not(unix))]
        assert_eq!(
            direct_start_command("/opt/mihomo", "/home/u/.config/mihomo"),
            PlannedCommand::new("/opt/mihomo", ["-d", "/home/u/.config/mihomo"])
        );
    }

    #[test]
    fn sudo_command_plans_cover_direct_cached_and_password_modes() {
        let args = ["systemctl", "restart", "mihomo"];

        assert_eq!(
            sudo_command_plan(SudoDispatch::DirectAsRoot, &args, None),
            SudoCommandPlan {
                program: "systemctl".to_string(),
                args: vec!["restart".to_string(), "mihomo".to_string()],
                stdin: None,
            }
        );

        assert_eq!(
            sudo_command_plan(SudoDispatch::NonInteractiveSudo, &args, None),
            SudoCommandPlan {
                program: "sudo".to_string(),
                args: vec![
                    "-n".to_string(),
                    "systemctl".to_string(),
                    "restart".to_string(),
                    "mihomo".to_string(),
                ],
                stdin: None,
            }
        );

        assert_eq!(
            sudo_command_plan(SudoDispatch::PromptPassword, &args, Some("secret")),
            SudoCommandPlan {
                program: "sudo".to_string(),
                args: vec![
                    "-S".to_string(),
                    "systemctl".to_string(),
                    "restart".to_string(),
                    "mihomo".to_string(),
                ],
                stdin: Some("secret\n".to_string()),
            }
        );
    }

    #[test]
    fn sudo_dispatch_prefers_root_then_cached_credentials_then_prompt() {
        assert_eq!(plan_sudo_dispatch(true, false), SudoDispatch::DirectAsRoot);
        assert_eq!(
            plan_sudo_dispatch(true, true),
            SudoDispatch::DirectAsRoot,
            "root should not shell through sudo even if sudo credentials also appear cached"
        );
        assert_eq!(
            plan_sudo_dispatch(false, true),
            SudoDispatch::NonInteractiveSudo
        );
        assert_eq!(
            plan_sudo_dispatch(false, false),
            SudoDispatch::PromptPassword
        );
    }

    #[test]
    fn timed_command_execution_plans_privileged_user_systemctl_and_direct_paths() {
        assert_eq!(
            planned_timed_command_execution(&PlannedCommand::privileged(
                "systemctl",
                ["restart", "mihomo"],
            )),
            TimedCommandExecution::Sudo {
                args: vec![
                    "systemctl".to_string(),
                    "restart".to_string(),
                    "mihomo".to_string(),
                ],
            }
        );

        assert_eq!(
            planned_timed_command_execution(&PlannedCommand::new(
                "systemctl",
                ["--user", "restart", "mihomo"],
            )),
            TimedCommandExecution::UserSystemctl {
                args: vec!["restart".to_string(), "mihomo".to_string()],
            }
        );

        let direct = PlannedCommand::new("sc.exe", ["start", "mihomo"]);
        assert_eq!(
            planned_timed_command_execution(&direct),
            TimedCommandExecution::Direct(direct)
        );
    }

    #[test]
    fn systemd_system_unit_contains_log_directives() {
        let unit = systemd_system_unit("/home/testuser");
        assert!(
            unit.contains("UMask=0027"),
            "system mode should set UMask for readable logs"
        );
        assert!(unit.contains("StandardOutput=append:/var/log/mihomo/mihomo.log"));
        assert!(unit.contains("StandardError=append:/var/log/mihomo/mihomo.log"));
        assert!(unit.contains("User=mihomo"));
        assert!(unit.contains("Group=mihomo"));
        assert!(
            unit.contains("CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE")
        );
        assert!(unit.contains("AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW CAP_NET_BIND_SERVICE"));
        assert!(unit.contains("NoNewPrivileges=true"));
        assert!(unit.contains("PrivateTmp=true"));
        assert!(unit.contains("ProtectHome=read-only"));
        assert!(unit.contains("ProtectSystem=strict"));
        assert!(unit.contains("ReadWritePaths=/var/run/mihomo /run/mihomo /var/log/mihomo /home/testuser/.config/mihomo /var/lib/mihomo-cli"));
    }

    #[test]
    fn systemd_user_unit_contains_log_directives_no_umask() {
        let unit = systemd_user_unit("/home/testuser");
        assert!(!unit.contains("UMask="), "user mode should not set UMask");
        assert!(unit.contains("StandardOutput=append:/home/testuser/.config/mihomo/mihomo.log"));
        assert!(unit.contains("StandardError=append:/home/testuser/.config/mihomo/mihomo.log"));
        assert!(
            !unit.contains("User=root"),
            "user mode should not have User=root"
        );
    }

    #[test]
    fn systemd_units_use_expanded_home_not_specifier() {
        let unit = systemd_system_unit("/home/alice");
        assert!(
            !unit.contains("%h"),
            "should use expanded home, not %h specifier"
        );
        assert!(unit.contains("/home/alice/.local/bin/mihomo"));
    }

    #[test]
    fn service_start_messages_are_planned_from_os_and_mode() {
        assert_eq!(
            service_start_message(ServiceOs::Linux, ServiceMode::User),
            "Starting mihomo via user service..."
        );
        assert_eq!(
            service_start_message(ServiceOs::Macos, ServiceMode::User),
            "Starting mihomo via LaunchAgent..."
        );
        assert_eq!(
            service_start_message(ServiceOs::Linux, ServiceMode::System),
            "Starting mihomo via service..."
        );
        assert_eq!(
            service_start_message(ServiceOs::Windows, ServiceMode::User),
            "Starting mihomo via service..."
        );
    }

    #[test]
    fn direct_start_precondition_errors_are_stable() {
        assert_eq!(
            direct_start_missing_binary_error("/opt/mihomo"),
            "mihomo binary not found at /opt/mihomo
  Run: mihomo-cli install"
        );
        assert_eq!(
            direct_start_missing_config_error(),
            "No config found.
  Run: mihomo-cli config"
        );
    }

    #[test]
    fn mihomo_process_checks_are_planned_and_parsed() {
        assert_eq!(
            mihomo_process_command(MihomoProcessCheck::WindowsTasklist),
            PlannedCommand::new("tasklist", ["/FI", "IMAGENAME eq mihomo.exe"])
        );
        assert_eq!(
            mihomo_process_command(MihomoProcessCheck::Pgrep),
            PlannedCommand::new("pgrep", ["-x", "mihomo"])
        );
        assert!(parse_mihomo_process_running(
            MihomoProcessCheck::WindowsTasklist,
            false,
            "Image Name                     PID Session Name\nmihomo.exe                    42 Console"
        ));
        assert!(parse_mihomo_process_running(
            MihomoProcessCheck::WindowsTasklist,
            true,
            "MIHOMO.EXE                    42 Console"
        ));
        assert!(!parse_mihomo_process_running(
            MihomoProcessCheck::WindowsTasklist,
            true,
            "INFO: No tasks are running which match the specified criteria."
        ));
        assert!(parse_mihomo_process_running(
            MihomoProcessCheck::Pgrep,
            true,
            "123"
        ));
        assert!(!parse_mihomo_process_running(
            MihomoProcessCheck::Pgrep,
            false,
            ""
        ));
    }

    #[test]
    fn platform_service_messages_are_planned() {
        assert_eq!(
            launchd_install_result_lines(ServiceMode::System, true, "/Users/alice"),
            vec!["LaunchDaemon installed and started — mihomo is running".to_string()]
        );
        assert_eq!(
            launchd_install_result_lines(ServiceMode::User, true, "/Users/alice"),
            vec!["LaunchAgent installed and started — mihomo is running (user mode)".to_string()]
        );
        assert_eq!(
            launchd_install_result_lines(ServiceMode::System, false, "/Users/alice"),
            vec![
                "LaunchDaemon installed but mihomo may not be running.".to_string(),
                "Check logs: tail -f /Users/alice/.config/mihomo/mihomo.log".to_string(),
            ]
        );
        assert_eq!(
            launchd_removed_message(ServiceMode::System),
            "LaunchDaemon removed"
        );
        assert_eq!(
            launchd_removed_message(ServiceMode::User),
            "LaunchAgent removed"
        );
        assert_eq!(
            systemd_old_service_removal_message(ServiceMode::User),
            "Removing old user service..."
        );
        assert_eq!(
            systemd_old_service_removal_message(ServiceMode::System),
            "Removing old system service..."
        );
        assert_eq!(
            systemd_linger_message("alice"),
            "  Enabling linger for user 'alice' (service survives logout)..."
        );
        assert_eq!(
            systemd_install_success_message(ServiceMode::System),
            "systemd system service installed and started (system mode)"
        );
        assert_eq!(
            systemd_install_success_message(ServiceMode::User),
            "systemd user service installed and started"
        );
        assert_eq!(
            systemd_missing_service_message(ServiceMode::System),
            "No system service found"
        );
        assert_eq!(
            systemd_missing_service_message(ServiceMode::User),
            "No user service found"
        );
        assert_eq!(
            systemd_removed_message(ServiceMode::System),
            "systemd system service removed"
        );
        assert_eq!(
            systemd_removed_message(ServiceMode::User),
            "systemd user service removed"
        );
        assert_eq!(
            windows_install_success_message(),
            "Windows service installed and started"
        );
        assert_eq!(windows_removed_message(), "Windows service removed");
    }

    #[test]
    fn direct_start_diagnostics_are_planned() {
        assert_eq!(
            config_test_crash_error(),
            "mihomo crashed during config test — binary may be corrupted.
  Run: mihomo-cli update"
        );
        assert_eq!(
            config_syntax_error(
                "line1
line2
line3
line4
line5
line6"
            ),
            "Config syntax error — mihomo cannot start.
line1
line2
line3
line4
line5"
        );
        assert_eq!(
            direct_start_message(),
            "No service installed, starting mihomo directly..."
        );
        assert_eq!(
            start_failure_with_log_error("/tmp/mihomo.log"),
            "mihomo failed to start.
  Check logs: tail -20 /tmp/mihomo.log"
        );
        assert_eq!(
            config_test_no_output_diag(2),
            "Config test failed (exit 2) with no output"
        );
        assert_eq!(
            config_syntax_diag(
                "a
b
c
d
e
f"
            ),
            "Config syntax error:
a
b
c
d
e"
        );
        assert_eq!(
            cannot_run_config_test_diag("permission denied"),
            "Cannot run mihomo -t: permission denied"
        );
        assert_eq!(
            missing_binary_diag("/opt/mihomo"),
            "mihomo binary not found at /opt/mihomo"
        );
        assert_eq!(
            start_failure_no_log_error("Config syntax: valid"),
            "mihomo failed to start (no log file).
  Config syntax: valid
  Try: mihomo-cli uninstall --all && mihomo-cli install"
        );
    }

    #[test]
    fn socket_fix_and_start_result_messages_are_planned() {
        assert_eq!(
            restart_with_fixed_config_message(),
            "  Restarting with fixed config..."
        );
        assert_eq!(
            restart_after_fix_failed_error("/tmp/mihomo.log"),
            "Failed to restart after config fix.
  Check logs: tail -20 /tmp/mihomo.log"
        );
        assert_eq!(
            socket_unreachable_with_controller_error("/tmp/mihomo.log"),
            "Socket unreachable and config already has controller.
  Check logs: tail -20 /tmp/mihomo.log"
        );
        assert_eq!(
            start_result_lines(true),
            vec!["Done.".to_string(), "Run: mihomo-cli status".to_string()]
        );
        assert_eq!(
            start_result_lines(false),
            vec!["  Check: mihomo-cli status".to_string()]
        );
        assert_eq!(
            api_not_ready_warning_lines()[0],
            "  ⚠ API not responding after 15s — mihomo may still be initializing."
        );
        assert_eq!(
            socket_fix_applied_message(),
            "  ⚠ Config was missing Unix socket controller — fixed."
        );
        #[cfg(unix)]
        {
            assert_eq!(stale_socket_cleanup_message(), "  Cleaned up stale socket");
            assert_eq!(
                stale_socket_cleanup_failed_message("denied", "/tmp/mihomo.sock"),
                "  ⚠ Cannot remove stale socket: denied
      Run: sudo rm -f /tmp/mihomo.sock"
            );
        }
    }

    #[test]
    fn service_start_stop_commands_respect_os_and_mode() {
        assert_eq!(
            service_start_command(ServiceOs::Linux, ServiceMode::User, "/home/alice"),
            PlannedCommand::new("systemctl", ["--user", "start", "mihomo"])
        );
        assert_eq!(
            service_stop_command(ServiceOs::Linux, ServiceMode::System, "/home/alice"),
            PlannedCommand::privileged("systemctl", ["stop", "mihomo"])
        );
        assert_eq!(
            service_start_command(ServiceOs::Macos, ServiceMode::User, "/Users/alice"),
            PlannedCommand::new(
                "launchctl",
                [
                    "kickstart",
                    "-k",
                    &format!("{}/io.mihomo", macos_gui_domain())
                ]
            )
        );
        assert_eq!(
            service_start_command(ServiceOs::Macos, ServiceMode::System, "/Users/alice"),
            PlannedCommand::privileged("launchctl", ["kickstart", "-k", "system/io.mihomo"])
        );
        assert_eq!(
            service_stop_command(ServiceOs::Macos, ServiceMode::System, "/Users/alice"),
            PlannedCommand::privileged("launchctl", ["kill", "SIGTERM", "system/io.mihomo"])
        );
        assert_eq!(
            service_stop_command(ServiceOs::Windows, ServiceMode::System, "C:/Users/Alice"),
            PlannedCommand::new("sc.exe", ["stop", "mihomo"])
        );
    }

    #[test]
    fn restart_command_plan_matches_current_platform_behavior() {
        let home = "/home/alice";
        assert_eq!(
            service_restart_commands(ServiceOs::Linux, ServiceMode::User, home),
            vec![PlannedCommand::new(
                "systemctl",
                ["--user", "restart", "mihomo"]
            )]
        );
        assert_eq!(
            service_restart_commands(ServiceOs::Macos, ServiceMode::User, home),
            vec![PlannedCommand::new(
                "launchctl",
                [
                    "kickstart",
                    "-k",
                    &format!("{}/io.mihomo", macos_gui_domain())
                ]
            )]
        );
        assert_eq!(
            service_restart_commands(ServiceOs::Macos, ServiceMode::System, home),
            vec![PlannedCommand::privileged(
                "launchctl",
                ["kickstart", "-k", "system/io.mihomo"],
            )]
        );
        assert_eq!(
            service_restart_commands(ServiceOs::Windows, ServiceMode::System, home),
            vec![
                PlannedCommand::new("sc.exe", ["stop", "mihomo"]),
                PlannedCommand::new("sc.exe", ["start", "mihomo"]),
            ]
        );
    }

    #[test]
    fn systemd_system_install_plan_writes_privileged_unit_and_cleans_user_service() {
        let plan = systemd_install_plan(ServiceMode::System, "/home/alice", Some("alice"), true);

        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, "/etc/systemd/system/mihomo.service");
        assert!(plan.files[0].privileged);
        assert!(plan.files[0].content.contains("User=mihomo"));
        assert!(plan.files[0]
            .content
            .contains("ExecStart=/home/alice/.local/bin/mihomo -d /home/alice/.config/mihomo"));
        assert_eq!(
            plan.cleanup_commands,
            vec![
                PlannedCommand::new("systemctl", ["--user", "stop", "mihomo"]),
                PlannedCommand::new("systemctl", ["--user", "disable", "mihomo"]),
                PlannedCommand::new("systemctl", ["--user", "daemon-reload"]),
            ]
        );
        assert_eq!(
            plan.cleanup_files,
            vec![ServiceFileRemoval {
                path: "/home/alice/.config/systemd/user/mihomo.service".to_string(),
                privileged: false,
            }]
        );
        assert_eq!(
            plan.commands,
            vec![
                PlannedCommand::privileged("systemctl", ["daemon-reload"]),
                PlannedCommand::privileged("systemctl", ["enable", "--now", "mihomo"]),
            ]
        );
    }

    #[test]
    fn systemd_user_install_plan_writes_user_unit_and_can_enable_linger() {
        let plan = systemd_install_plan(ServiceMode::User, "/home/alice", Some("alice"), true);

        assert_eq!(
            plan.files[0].path,
            "/home/alice/.config/systemd/user/mihomo.service"
        );
        assert!(!plan.files[0].privileged);
        assert!(!plan.files[0].content.contains("User=root"));
        assert_eq!(
            plan.cleanup_commands,
            vec![
                PlannedCommand::privileged("systemctl", ["stop", "mihomo"]),
                PlannedCommand::privileged("systemctl", ["disable", "mihomo"]),
                PlannedCommand::privileged("rm", ["/etc/systemd/system/mihomo.service"]),
                PlannedCommand::privileged("systemctl", ["daemon-reload"]),
            ]
        );
        assert_eq!(
            plan.cleanup_files,
            vec![ServiceFileRemoval {
                path: "/etc/systemd/system/mihomo.service".to_string(),
                privileged: true,
            }]
        );
        assert_eq!(
            plan.commands,
            vec![
                PlannedCommand::new("systemctl", ["--user", "daemon-reload"]),
                PlannedCommand::new("systemctl", ["--user", "enable", "--now", "mihomo"]),
                PlannedCommand::privileged("loginctl", ["enable-linger", "alice"]),
            ]
        );
    }

    #[test]
    fn launchd_install_plans_capture_daemon_and_agent_differences() {
        let daemon = launchd_install_plan(ServiceMode::System, "/Users/alice");
        assert_eq!(
            daemon.files[0].path,
            "/Library/LaunchDaemons/io.mihomo.plist"
        );
        assert!(daemon.files[0].privileged);
        assert!(daemon.files[0]
            .content
            .contains("/Users/alice/.config/mihomo/start.sh"));
        assert_eq!(
            daemon.commands,
            vec![
                PlannedCommand::privileged(
                    "chmod",
                    ["644", "/Library/LaunchDaemons/io.mihomo.plist"]
                ),
                PlannedCommand::privileged(
                    "launchctl",
                    [
                        "bootstrap",
                        "system",
                        "/Library/LaunchDaemons/io.mihomo.plist"
                    ]
                ),
            ]
        );

        let agent = launchd_install_plan(ServiceMode::User, "/Users/alice");
        assert_eq!(
            agent.files[0].path,
            "/Users/alice/Library/LaunchAgents/io.mihomo.plist"
        );
        assert!(!agent.files[0].privileged);
        assert_eq!(
            agent.commands,
            vec![PlannedCommand::new(
                "launchctl",
                [
                    "bootstrap",
                    &macos_gui_domain(),
                    "/Users/alice/Library/LaunchAgents/io.mihomo.plist"
                ]
            )]
        );
    }

    #[test]
    fn windows_install_commands_quote_paths_with_spaces() {
        let commands = windows_install_commands(
            r"C:\Users\Alice Smith\AppData\Local\mihomo",
            r"C:\Users\Alice Smith\AppData\Local\mihomo\mihomo.exe",
        );

        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].program, "sc.exe");
        assert_eq!(commands[0].args[0], "create");
        assert!(commands[0].args[3]
            .contains(r#""C:\Users\Alice Smith\AppData\Local\mihomo\mihomo.exe""#));
        assert!(commands[0].args[3].contains(r#"-d "C:\Users\Alice Smith\AppData\Local\mihomo""#));
        assert_eq!(
            commands[1],
            PlannedCommand::new("sc.exe", ["start", "mihomo"])
        );
    }

    #[test]
    fn service_install_apply_runs_cleanup_writes_files_then_commands() {
        let plan = ServiceInstallPlan {
            cleanup_commands: vec![PlannedCommand::new(
                "systemctl",
                ["--user", "stop", "mihomo"],
            )],
            cleanup_files: vec![ServiceFileRemoval {
                path: "/home/alice/old.service".to_string(),
                privileged: false,
            }],
            pre_commands: vec![],
            files: vec![ServiceFilePlan {
                path: "/etc/systemd/system/mihomo.service".to_string(),
                content: "unit".to_string(),
                privileged: true,
            }],
            commands: vec![
                PlannedCommand::privileged("systemctl", ["daemon-reload"]),
                PlannedCommand::privileged("systemctl", ["enable", "--now", "mihomo"]),
            ],
        };
        let mut executor = RecordingExecutor::default();

        apply_service_install_plan_with(&plan, &mut executor).unwrap();

        assert_eq!(
            executor.events,
            vec![
                "best-effort:systemctl --user stop mihomo",
                "remove:/home/alice/old.service",
                "write:sudo /etc/systemd/system/mihomo.service=unit",
                "run:sudo systemctl daemon-reload",
                "run:sudo systemctl enable --now mihomo",
            ]
        );
    }

    #[test]
    fn service_install_apply_stops_before_commands_when_file_write_fails() {
        let plan = ServiceInstallPlan {
            cleanup_commands: vec![PlannedCommand::new(
                "systemctl",
                ["--user", "stop", "mihomo"],
            )],
            cleanup_files: vec![],
            pre_commands: vec![],
            files: vec![ServiceFilePlan {
                path: "/tmp/mihomo.service".to_string(),
                content: "unit".to_string(),
                privileged: false,
            }],
            commands: vec![PlannedCommand::new(
                "systemctl",
                ["--user", "enable", "mihomo"],
            )],
        };
        let mut executor = RecordingExecutor {
            fail_write_path: Some("/tmp/mihomo.service".to_string()),
            ..Default::default()
        };

        let err = apply_service_install_plan_with(&plan, &mut executor).unwrap_err();

        assert!(
            err.to_string().contains("simulated write failure"),
            "error was: {err}"
        );
        assert_eq!(
            executor.events,
            vec![
                "best-effort:systemctl --user stop mihomo",
                "write:/tmp/mihomo.service=unit",
            ],
            "commands and marker must not run after failed file write"
        );
    }

    #[test]
    fn service_uninstall_apply_is_best_effort_for_commands_files_and_marker() {
        let plan = ServiceUninstallPlan {
            commands: vec![
                PlannedCommand::new("systemctl", ["--user", "stop", "mihomo"]),
                PlannedCommand::new("systemctl", ["--user", "disable", "mihomo"]),
            ],
            files: vec![ServiceFileRemoval {
                path: "/home/alice/.config/systemd/user/mihomo.service".to_string(),
                privileged: false,
            }],
            legacy_service_mode_marker: Some(ServiceFileRemoval {
                path: "/home/alice/.config/mihomo/service-mode".to_string(),
                privileged: false,
            }),
        };
        let mut executor = RecordingExecutor::default();

        apply_service_uninstall_plan_with(&plan, &mut executor);

        assert_eq!(
            executor.events,
            vec![
                "best-effort:systemctl --user stop mihomo",
                "best-effort:systemctl --user disable mihomo",
                "remove:/home/alice/.config/systemd/user/mihomo.service",
                "remove:/home/alice/.config/mihomo/service-mode",
            ]
        );
    }

    #[test]
    fn systemd_uninstall_plans_capture_root_and_user_cleanup() {
        let root = systemd_uninstall_plan(ServiceMode::System, "/home/alice", "/tmp/mode");
        assert_eq!(
            root.commands,
            vec![
                PlannedCommand::privileged("systemctl", ["stop", "mihomo"]),
                PlannedCommand::privileged("systemctl", ["disable", "mihomo"]),
                PlannedCommand::privileged("systemctl", ["daemon-reload"]),
            ]
        );
        assert_eq!(
            root.files,
            vec![ServiceFileRemoval {
                path: "/etc/systemd/system/mihomo.service".to_string(),
                privileged: true,
            }]
        );
        assert_eq!(
            root.legacy_service_mode_marker,
            Some(ServiceFileRemoval {
                path: "/tmp/mode".to_string(),
                privileged: false,
            })
        );

        let user = systemd_uninstall_plan(ServiceMode::User, "/home/alice", "/tmp/mode");
        assert_eq!(
            user.commands,
            vec![
                PlannedCommand::new("systemctl", ["--user", "stop", "mihomo"]),
                PlannedCommand::new("systemctl", ["--user", "disable", "mihomo"]),
                PlannedCommand::new("systemctl", ["--user", "daemon-reload"]),
            ]
        );
        assert_eq!(
            user.files,
            vec![ServiceFileRemoval {
                path: "/home/alice/.config/systemd/user/mihomo.service".to_string(),
                privileged: false,
            }]
        );
    }

    #[test]
    fn launchd_and_windows_uninstall_plans_capture_commands_and_files() {
        let daemon = launchd_uninstall_plan(ServiceMode::System, "/Users/alice", "/tmp/mode");
        assert_eq!(
            daemon.commands,
            vec![PlannedCommand::privileged(
                "launchctl",
                ["bootout", "system/io.mihomo"]
            )]
        );
        assert_eq!(
            daemon.files,
            vec![ServiceFileRemoval {
                path: "/Library/LaunchDaemons/io.mihomo.plist".to_string(),
                privileged: true,
            }]
        );

        let agent = launchd_uninstall_plan(ServiceMode::User, "/Users/alice", "/tmp/mode");
        assert_eq!(
            agent.commands,
            vec![PlannedCommand::new(
                "launchctl",
                ["bootout", &format!("{}/io.mihomo", macos_gui_domain())]
            )]
        );
        assert_eq!(
            agent.files,
            vec![ServiceFileRemoval {
                path: "/Users/alice/Library/LaunchAgents/io.mihomo.plist".to_string(),
                privileged: false,
            }]
        );

        let windows = windows_uninstall_plan(r"C:\tmp\service-mode");
        assert_eq!(
            windows.commands,
            vec![
                PlannedCommand::new("sc.exe", ["stop", "mihomo"]),
                PlannedCommand::new("sc.exe", ["delete", "mihomo"]),
            ]
        );
        assert_eq!(
            windows.legacy_service_mode_marker,
            Some(ServiceFileRemoval {
                path: r"C:\tmp\service-mode".to_string(),
                privileged: false,
            })
        );
    }
    #[test]
    fn password_provider_result_classifies_dialoguer_outcomes() {
        assert_eq!(
            password_provider_result(Ok(Some("secret".to_string()))),
            PasswordProviderResult::Provided("secret".to_string())
        );
        assert_eq!(
            password_provider_result(Ok(Some("".to_string()))),
            PasswordProviderResult::Provided("".to_string()),
            "empty password is still Provided; caller decides policy"
        );
        assert_eq!(
            password_provider_result(Ok(None)),
            PasswordProviderResult::Cancelled
        );
        assert_eq!(
            password_provider_result(Err("tty".to_string())),
            PasswordProviderResult::IoError("tty".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_staged_file_rejects_symlink_target() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real-file");
        std::fs::write(&real, b"original").unwrap();
        let link = dir.path().join("symlink-to-real");
        symlink(&real, &link).unwrap();

        let result = install_staged_file_privileged(&link, b"pwned", 0o644);
        assert!(result.is_err(), "write to symlink must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("refusing to write to symlink"),
            "error must mention symlink: {msg}"
        );
        // Original file must be untouched
        assert_eq!(std::fs::read(&real).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn install_staged_file_writes_new_file_with_correct_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("file.bin");

        if is_root() {
            // Root: exercise the full privileged path (direct-write branch).
            install_staged_file_privileged(&path, b"hello world", 0o755).unwrap();
        } else {
            // Non-root: the function would dispatch through sudo (tests must
            // never trigger a real sudo invocation), so exercise the
            // direct-write branch body via its helper instead.
            write_installed_file_direct(&path, b"hello world", 0o755).unwrap();
        }

        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_staged_file_overwrites_existing_regular_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing");
        std::fs::write(&path, b"old content").unwrap();

        if is_root() {
            install_staged_file_privileged(&path, b"new content", 0o600).unwrap();
        } else {
            write_installed_file_direct(&path, b"new content", 0o600).unwrap();
        }

        assert_eq!(std::fs::read(&path).unwrap(), b"new content");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn privileged_install_args_form_sudo_install_command() {
        let temp = std::path::Path::new("/tmp/mihomo-cli-privileged-write-1234-mihomo");
        let target = std::path::Path::new("/usr/local/bin/mihomo-cli");
        assert_eq!(
            privileged_install_args(temp, 0o755, target),
            vec![
                "install".to_string(),
                "-m".to_string(),
                "755".to_string(),
                "/tmp/mihomo-cli-privileged-write-1234-mihomo".to_string(),
                "/usr/local/bin/mihomo-cli".to_string(),
            ],
            "non-root branch must build: sudo install -m <mode> <temp> <target>"
        );
        assert_eq!(
            privileged_install_args(temp, 0o644, target)[2],
            "644",
            "mode must be formatted as a plain octal string"
        );
    }

    #[cfg(unix)]
    #[test]
    fn temp_payload_file_is_0600_and_cleanup_removes_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let temp_path = dir.path().join("mihomo-cli-privileged-write-1234-payload");

        write_temp_payload(&temp_path, b"payload bytes").unwrap();

        assert_eq!(std::fs::read(&temp_path).unwrap(), b"payload bytes");
        assert_eq!(
            std::fs::metadata(&temp_path).unwrap().permissions().mode() & 0o777,
            0o600,
            "staged temp payload must be owner-only readable"
        );

        // Simulate the install step finishing: cleanup must remove the file.
        let cleanup = TempFileCleanup(temp_path.clone());
        drop(cleanup);
        assert!(
            !temp_path.exists(),
            "staged temp payload must be cleaned up after install"
        );
    }
}
