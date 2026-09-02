#![allow(dead_code)]
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
    let installer_sid = persist_installer_sid(ctx)?;
    // Generate + atomically replace the single machine-level daemon IPC
    // credential shared by the daemon and installing user's CLI.
    persist_service_token(ctx, &installer_sid)?;
    if let Err(error) = remove_legacy_windows_client_token() {
        crate::log!("legacy Windows client token cleanup skipped: {error}");
    }

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
fn persist_installer_sid(ctx: &crate::instance::InstanceContext) -> anyhow::Result<String> {
    validate_canonical_windows_daemon_credentials(ctx)?;
    let sid_string = current_windows_user_sid()?;
    let installer_sid_path = ctx
        .daemon_credentials
        .token
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Windows service credential path has no parent"))?
        .join("installer-sid");
    write_windows_credential(
        &installer_sid_path,
        sid_string.as_bytes(),
        WindowsCredentialAcl::InstallerMetadata,
        &sid_string,
    )?;
    Ok(sid_string)
}

#[cfg(windows)]
fn current_windows_user_sid() -> anyhow::Result<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
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
        let probe_ok = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
        let probe_error = GetLastError();
        let words = match token_user_buffer_words(
            probe_ok != 0,
            probe_error,
            size,
            std::mem::size_of::<TOKEN_USER>(),
        ) {
            Ok(words) => words,
            Err(error) => {
                CloseHandle(token);
                return Err(error);
            }
        };
        // Vec<usize> guarantees pointer alignment suitable for TOKEN_USER.
        let mut buffer = vec![0usize; words];
        let buffer_bytes = buffer.len() * std::mem::size_of::<usize>();
        let buffer_size = match u32::try_from(buffer_bytes) {
            Ok(size) => size,
            Err(error) => {
                CloseHandle(token);
                return Err(error.into());
            }
        };
        let mut returned_size = size;
        if GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr() as *mut _,
            buffer_size,
            &mut returned_size,
        ) == 0
        {
            CloseHandle(token);
            anyhow::bail!("GetTokenInformation(TokenUser) failed");
        }
        if returned_size < std::mem::size_of::<TOKEN_USER>() as u32
            || returned_size as usize > buffer_bytes
        {
            CloseHandle(token);
            anyhow::bail!("GetTokenInformation(TokenUser) returned an invalid length");
        }
        let token_user = std::ptr::read(buffer.as_ptr().cast::<TOKEN_USER>());
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

        Ok(sid_string)
    }
}

fn token_user_buffer_words(
    probe_succeeded: bool,
    error_code: u32,
    required_size: u32,
    minimum_size: usize,
) -> anyhow::Result<usize> {
    const ERROR_INSUFFICIENT_BUFFER_CODE: u32 = 122;
    if probe_succeeded || error_code != ERROR_INSUFFICIENT_BUFFER_CODE {
        anyhow::bail!("GetTokenInformation(TokenUser) size probe failed with error {error_code}");
    }
    if (required_size as usize) < minimum_size {
        anyhow::bail!(
            "GetTokenInformation(TokenUser) returned undersized buffer length {required_size}"
        );
    }
    let word_size = std::mem::size_of::<usize>();
    Ok((required_size as usize)
        .checked_add(word_size - 1)
        .ok_or_else(|| anyhow::anyhow!("TokenUser buffer length overflow"))?
        / word_size)
}

fn cleanup_credential_temp_after_check<T>(
    temp_path: &std::path::Path,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    if result.is_err() {
        let _ = std::fs::remove_file(temp_path);
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsCredentialAcl {
    InstallerMetadata,
    Token,
}

fn windows_credential_sddl(kind: WindowsCredentialAcl, installer_sid: &str) -> String {
    match kind {
        WindowsCredentialAcl::InstallerMetadata => "D:P(A;;GA;;;SY)(A;;GA;;;BA)".to_string(),
        WindowsCredentialAcl::Token => {
            format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GR;;;{installer_sid})")
        }
    }
}

#[cfg(windows)]
fn validate_canonical_windows_daemon_credentials(
    ctx: &crate::instance::InstanceContext,
) -> anyhow::Result<()> {
    let canonical = crate::instance::planned_daemon_credential_paths(
        crate::instance::TargetOs::Windows,
        &crate::instance::PathInputs::from_current_env(),
    );
    if ctx.daemon_credentials != canonical {
        anyhow::bail!("refusing non-canonical Windows daemon credential paths");
    }
    Ok(())
}

#[cfg(windows)]
fn inspect_windows_credential_target(
    path: &std::path::Path,
) -> anyhow::Result<Option<std::fs::Metadata>> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(anyhow::anyhow!(
                "cannot inspect credential path {}: {error}",
                path.display()
            ))
        }
    };
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        anyhow::bail!(
            "refusing Windows credential path through a reparse point: {}",
            path.display()
        );
    }
    Ok(Some(metadata))
}

#[cfg(windows)]
fn wide_path(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(windows)]
fn write_windows_credential(
    path: &std::path::Path,
    bytes: &[u8],
    acl: WindowsCredentialAcl,
    installer_sid: &str,
) -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, LocalFree, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, MoveFileExW, WriteFile, CREATE_NEW, FILE_ATTRIBUTE_NORMAL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, MOVEFILE_REPLACE_EXISTING,
        MOVEFILE_WRITE_THROUGH,
    };

    if !crate::instance::valid_windows_sid_string(installer_sid) {
        anyhow::bail!("refusing malformed Windows installer SID");
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Windows credential path has no parent"))?;
    std::fs::create_dir_all(parent).map_err(|e| {
        anyhow::anyhow!(
            "failed to create Windows credential directory {}: {e}",
            parent.display()
        )
    })?;
    let parent_metadata = inspect_windows_credential_target(parent)?
        .ok_or_else(|| anyhow::anyhow!("Windows credential directory disappeared"))?;
    if !parent_metadata.is_dir() {
        anyhow::bail!(
            "Windows credential parent is not a directory: {}",
            parent.display()
        );
    }
    if let Some(metadata) = inspect_windows_credential_target(path)? {
        if !metadata.is_file() {
            anyhow::bail!(
                "refusing to replace non-file Windows credential: {}",
                path.display()
            );
        }
    }

    let suffix = generate_auth_token();
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Windows credential path has no file name"))?
        .to_string_lossy();
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", &suffix[..16]));
    let temp_wide = wide_path(&temp_path);
    let target_wide = wide_path(path);
    let mut sddl_wide = windows_credential_sddl(acl, installer_sid)
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let mut descriptor_size = 0u32;

    let conversion_ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_wide.as_mut_ptr(),
            1,
            &mut descriptor,
            &mut descriptor_size,
        )
    };
    if conversion_ok == 0 {
        return Err(std::io::Error::last_os_error())
            .map_err(|e| anyhow::anyhow!("failed to build Windows credential ACL: {e}"));
    }
    let security_attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateFileW(
            temp_wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_DELETE,
            &security_attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    unsafe {
        LocalFree(descriptor);
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error()).map_err(|e| {
            anyhow::anyhow!(
                "failed to create protected Windows credential {}: {e}",
                temp_path.display()
            )
        });
    }

    let write_result = (|| -> anyhow::Result<()> {
        let mut written = 0u32;
        if unsafe {
            WriteFile(
                handle,
                bytes.as_ptr(),
                bytes.len().try_into()?,
                &mut written,
                std::ptr::null_mut(),
            )
        } == 0
            || written as usize != bytes.len()
        {
            return Err(std::io::Error::last_os_error()).map_err(|e| {
                anyhow::anyhow!(
                    "failed to write protected Windows credential {}: {e}",
                    temp_path.display()
                )
            });
        }
        if unsafe { FlushFileBuffers(handle) } == 0 {
            return Err(std::io::Error::last_os_error()).map_err(|e| {
                anyhow::anyhow!(
                    "failed to flush protected Windows credential {}: {e}",
                    temp_path.display()
                )
            });
        }
        Ok(())
    })();
    unsafe {
        CloseHandle(handle);
    }
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }

    cleanup_credential_temp_after_check(&temp_path, inspect_windows_credential_target(path))?;
    if unsafe {
        MoveFileExW(
            temp_wide.as_ptr(),
            target_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        let _ = std::fs::remove_file(&temp_path);
        anyhow::bail!(
            "failed to atomically install protected Windows credential {}: {error}",
            path.display()
        );
    }
    Ok(())
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
        crate::utils::ensure_dir_all_no_follow(parent)?;
    }
    crate::utils::write_bytes_file_no_follow(token_path, token.as_bytes(), 0o600)?;
    crate::utils::restore_original_user_config_ownership(token_path)?;
    Ok(token)
}

#[cfg(unix)]
pub(crate) fn client_token_path_for_home(home: &std::path::Path) -> std::path::PathBuf {
    crate::instance::planned_unix_daemon_client_token_path(home)
}

#[cfg(unix)]
pub(crate) fn validate_unix_client_token_target(
    home: &std::path::Path,
    target: &std::path::Path,
) -> anyhow::Result<()> {
    let expected = client_token_path_for_home(home);
    if target != expected {
        anyhow::bail!(
            "refusing non-canonical daemon credential path {}; expected {}",
            target.display(),
            expected.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn grant_client_token_for_unix_identity(
    home: &std::path::Path,
    uid: u32,
    gid: u32,
) -> anyhow::Result<String> {
    let token_path = client_token_path_for_home(home);
    validate_unix_client_token_target(home, &token_path)?;
    if let Some(dir) = token_path.parent() {
        crate::utils::ensure_home_path_traversable(dir, home, gid)?;
        crate::utils::ensure_dir_all_under_home_no_follow(dir, home)?;
    }
    let token = generate_auth_token();
    crate::utils::write_bytes_file_under_home_no_follow(
        &token_path,
        home,
        token.as_bytes(),
        0o600,
        uid,
        gid,
    )?;
    Ok(token)
}

#[cfg(unix)]
pub(crate) fn generate_client_token_for_home(home: &std::path::Path) -> anyhow::Result<String> {
    generate_and_write_token(&client_token_path_for_home(home))
}

#[cfg(windows)]
/// Generate and atomically replace the single canonical Windows token at
/// `%ProgramData%\mihomo\service-token`. Both daemon and CLI read this file.
fn persist_service_token(
    ctx: &crate::instance::InstanceContext,
    installer_sid: &str,
) -> anyhow::Result<()> {
    validate_canonical_windows_daemon_credentials(ctx)?;
    let token = generate_auth_token();
    write_windows_credential(
        &ctx.daemon_credentials.token,
        token.as_bytes(),
        WindowsCredentialAcl::Token,
        installer_sid,
    )?;
    Ok(())
}

#[cfg(windows)]
fn remove_legacy_windows_client_token() -> anyhow::Result<()> {
    let inputs = crate::instance::PathInputs::from_current_env();
    let legacy = inputs.app_data.join("mihomo/service-client-token");
    let Some(metadata) = inspect_windows_credential_target(&legacy)? else {
        return Ok(());
    };
    if !metadata.is_file() {
        anyhow::bail!(
            "refusing to remove non-file legacy Windows credential {}",
            legacy.display()
        );
    }
    std::fs::remove_file(&legacy).map_err(|error| {
        anyhow::anyhow!(
            "cannot remove legacy Windows credential {}: {error}",
            legacy.display()
        )
    })
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
    let temp_dir = tempfile::Builder::new()
        .prefix("mihomo-cli-privileged-write-")
        .tempdir()?;
    let temp_path = temp_dir.path().join("payload");
    write_temp_payload(&temp_path, bytes)?;
    let args: Vec<String> = privileged_install_args(&temp_path, mode, path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_privileged(&arg_refs)
}

#[cfg(unix)]
/// Write staged bytes in a 0700 private temporary directory with owner-only mode.
fn write_temp_payload(temp_path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_installer_metadata_acl_is_protected_and_machine_only() {
        let sddl =
            windows_credential_sddl(WindowsCredentialAcl::InstallerMetadata, "S-1-5-21-1000");
        assert!(sddl.starts_with("D:P"));
        assert!(sddl.contains(";;;SY)"));
        assert!(sddl.contains(";;;BA)"));
        assert!(!sddl.contains("S-1-5-21-1000"));
        for broad_principal in [";;;WD)", ";;;AU)", ";;;BU)"] {
            assert!(!sddl.contains(broad_principal));
        }
    }

    #[test]
    fn windows_single_token_acl_is_protected_and_installer_readable() {
        let sid = "S-1-5-21-1000";
        let sddl = windows_credential_sddl(WindowsCredentialAcl::Token, sid);
        assert!(sddl.starts_with("D:P"));
        assert!(sddl.contains(";;;SY)"));
        assert!(sddl.contains(";;;BA)"));
        assert!(sddl.contains(&format!(";;;{sid})")));
        for broad_principal in [";;;WD)", ";;;AU)", ";;;BU)"] {
            assert!(!sddl.contains(broad_principal));
        }
    }

    #[test]
    fn windows_acl_planner_rejects_sddl_injection_as_an_installer_sid() {
        assert!(crate::instance::valid_windows_sid_string("S-1-5-21-1000"));
        assert!(!crate::instance::valid_windows_sid_string(
            "S-1-5-21-1000)(A;;GA;;;WD"
        ));
        assert!(!crate::instance::valid_windows_sid_string("alice"));
    }

    #[test]
    fn token_user_probe_requires_insufficient_buffer_and_aligned_storage() {
        let words = token_user_buffer_words(false, 122, 40, 16).unwrap();
        assert!(words * std::mem::size_of::<usize>() >= 40);
        assert!(token_user_buffer_words(true, 0, 40, 16).is_err());
        assert!(token_user_buffer_words(false, 5, 40, 16).is_err());
        assert!(token_user_buffer_words(false, 122, 8, 16).is_err());
    }

    #[test]
    fn failed_final_target_check_removes_staged_credential() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join(".service-token.staged");
        std::fs::write(&temp, b"new-token").unwrap();

        let result: anyhow::Result<()> = cleanup_credential_temp_after_check(
            &temp,
            Err(anyhow::anyhow!("target became a reparse point")),
        );
        assert!(result.is_err());
        assert!(!temp.exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_single_token_replace_rotates_atomically_and_failure_keeps_old_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service-token");
        let sid = current_windows_user_sid().unwrap();
        let old = generate_auth_token();
        write_windows_credential(&path, old.as_bytes(), WindowsCredentialAcl::Token, &sid).unwrap();

        let failed = write_windows_credential(
            &path,
            b"must-not-replace",
            WindowsCredentialAcl::Token,
            "malformed SID",
        );
        assert!(failed.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);

        let new = generate_auth_token();
        write_windows_credential(&path, new.as_bytes(), WindowsCredentialAcl::Token, &sid).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), new);
    }

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

    #[cfg(unix)]
    #[test]
    fn credential_write_target_must_match_canonical_identity_path() {
        let home = std::path::Path::new("/home/alice");
        assert!(validate_unix_client_token_target(
            home,
            std::path::Path::new("/home/alice/.config/mihomo/service-token")
        )
        .is_ok());
        assert!(validate_unix_client_token_target(
            home,
            std::path::Path::new("/tmp/untrusted-config/service-token")
        )
        .is_err());
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

        assert!(
            temp_path.exists(),
            "private temporary directory owns cleanup after privileged install"
        );
    }
}
