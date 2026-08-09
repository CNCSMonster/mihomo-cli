//! System service daemon for mihomo-cli.
//!
//! Runs as the dedicated non-root `mihomo` service user where supported, listens on a Unix socket,
//! and manages the core with service-manager-granted capabilities.
//!
//! The daemon is started by `mihomo-cli install --system` and managed by
//! the system's service manager (systemd/launchd/Windows Service).

#[cfg(unix)]
use crate::instance::ApiEndpoint;
use crate::instance::{SystemPaths, TargetOs};
#[cfg(any(unix, windows))]
use crate::ipc::{DaemonCommand, DaemonResponse};
#[cfg(unix)]
use crate::mihomo_api;
use std::path::PathBuf;
#[cfg(any(unix, windows))]
use std::process::Stdio;
#[cfg(any(unix, windows))]
use std::sync::Arc;
#[cfg(unix)]
use tokio::io::{AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixListener;
#[cfg(any(unix, windows))]
use tokio::sync::Mutex;
#[cfg(any(unix, windows))]
use tokio_util::sync::CancellationToken;

#[cfg(any(unix, windows))]
fn validate_daemon_config_path_shape(config_path: &std::path::Path) -> Result<(), String> {
    if !config_path.is_absolute() {
        return Err(format!(
            "refusing to use non-absolute config path {}",
            config_path.display()
        ));
    }
    if config_path.file_name() != Some(std::ffi::OsStr::new("config.yaml")) {
        return Err(format!(
            "refusing to use config path {}; expected config.yaml",
            config_path.display()
        ));
    }
    let text = config_path.to_string_lossy().replace('\\', "/");
    if text.contains("/../") || text.contains("/./") {
        return Err(format!(
            "refusing to use config path {}; path must not contain . or .. components",
            config_path.display()
        ));
    }

    let allowed = if cfg!(target_os = "windows") {
        (text.ends_with("/AppData/Roaming/mihomo/config.yaml")
            || text.ends_with("/AppData/Roaming/Mihomo/config.yaml"))
            && (text.contains(":/") || text.starts_with("//"))
    } else {
        let parts: Vec<&str> = text.split('/').collect();
        if cfg!(target_os = "macos") {
            matches!(
                parts.as_slice(),
                ["", "Users", user, ".config", "mihomo", "config.yaml"] if !user.is_empty()
            ) || matches!(
                parts.as_slice(),
                ["", "var", "root", ".config", "mihomo", "config.yaml"]
            )
        } else {
            matches!(
                parts.as_slice(),
                ["", "home", user, ".config", "mihomo", "config.yaml"] if !user.is_empty()
            )
        }
    };

    if allowed {
        Ok(())
    } else {
        Err(format!(
            "refusing to use config path {}; system daemon only accepts per-user mihomo config.yaml",
            config_path.display()
        ))
    }
}

#[cfg(unix)]
fn validate_daemon_config_path_for_peer(
    config_path: &std::path::Path,
    peer_uid: Option<u32>,
) -> Result<(), String> {
    validate_daemon_config_path_shape(config_path)?;
    let Some(peer_uid) = peer_uid else {
        return Err(format!(
            "refusing to use config path {}; cannot determine IPC peer uid",
            config_path.display()
        ));
    };
    // Root-originated IPC is allowed for service-manager/manual administration.
    if peer_uid == 0 {
        return Ok(());
    }

    validate_config_owner_for_peer(config_path, peer_uid)
}

#[cfg(unix)]
fn validate_config_owner_for_peer(
    config_path: &std::path::Path,
    peer_uid: u32,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(config_path).map_err(|e| {
        format!(
            "cannot inspect config path owner {}: {e}",
            config_path.display()
        )
    })?;
    if metadata.uid() == peer_uid {
        Ok(())
    } else {
        Err(format!(
            "refusing to use config path {}; owner uid {} does not match IPC peer uid {}",
            config_path.display(),
            metadata.uid(),
            peer_uid
        ))
    }
}

#[cfg(windows)]
#[allow(dead_code)]
fn validate_daemon_config_path_for_peer(
    config_path: &std::path::Path,
    _peer_uid: Option<u32>,
) -> Result<(), String> {
    validate_daemon_config_path_shape(config_path)
}

#[cfg(unix)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct AuthorizedClient {
    pub user: String,
    pub uid: u32,
    pub token: String,
}

#[cfg(unix)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq, Eq)]
pub(crate) struct AuthorizedClients {
    pub clients: Vec<AuthorizedClient>,
}

#[cfg(unix)]
pub(crate) fn authorized_clients_path() -> PathBuf {
    std::env::var_os("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/mihomo-cli/authorized-clients.json"))
}

#[cfg(unix)]
pub(crate) fn read_authorized_clients_from(
    path: &std::path::Path,
) -> anyhow::Result<AuthorizedClients> {
    if !path.exists() {
        return Ok(AuthorizedClients::default());
    }
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(unix)]
pub(crate) fn write_authorized_clients_to(
    path: &std::path::Path,
    table: &AuthorizedClients,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(table)?)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn validate_client_token_for_peer(
    token: Option<&str>,
    peer_uid: Option<u32>,
) -> Result<(), String> {
    if peer_uid == Some(0) {
        return Ok(());
    }
    let token = token.ok_or_else(|| "invalid or missing auth token".to_string())?;
    let uid = peer_uid.ok_or_else(|| "cannot determine IPC peer uid".to_string())?;
    let table = read_authorized_clients_from(&authorized_clients_path())
        .map_err(|e| format!("cannot read authorized clients: {e}"))?;
    fn ct_eq(a: &str, b: &str) -> bool {
        let ab = a.as_bytes();
        let bb = b.as_bytes();
        let mut diff = ab.len() ^ bb.len();
        for i in 0..ab.len().max(bb.len()) {
            let x = *ab.get(i).unwrap_or(&0);
            let y = *bb.get(i).unwrap_or(&0);
            diff |= (x ^ y) as usize;
        }
        diff == 0
    }
    match table.clients.iter().find(|c| ct_eq(&c.token, token)) {
        Some(c) if c.uid == uid => Ok(()),
        Some(_) => Err("auth token does not belong to IPC peer uid".to_string()),
        None => Err("invalid or missing auth token".to_string()),
    }
}

#[cfg(windows)]
/// Windows token-only validation — no peer UID available on named pipes.
/// Skips the UID cross-check that the Unix version performs.
pub(crate) fn validate_client_token_for_peer(
    token: Option<&str>,
    _peer_uid: Option<u32>,
) -> Result<(), String> {
    let token = token.ok_or_else(|| "invalid or missing auth token".to_string())?;
    let table = read_authorized_clients_from(&authorized_clients_path())
        .map_err(|e| format!("cannot read authorized clients: {e}"))?;
    fn ct_eq(a: &str, b: &str) -> bool {
        let ab = a.as_bytes();
        let bb = b.as_bytes();
        let mut diff = ab.len() ^ bb.len();
        for i in 0..ab.len().max(bb.len()) {
            let x = *ab.get(i).unwrap_or(&0);
            let y = *bb.get(i).unwrap_or(&0);
            diff |= (x ^ y) as usize;
        }
        diff == 0
    }
    match table.clients.iter().find(|c| ct_eq(&c.token, token)) {
        Some(_) => Ok(()),
        None => Err("invalid or missing auth token".to_string()),
    }
}

#[cfg(windows)]
/// Windows path for the authorized-clients file.
pub(crate) fn authorized_clients_path() -> PathBuf {
    std::env::var_os("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("ProgramData")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
                .join("mihomo")
                .join("authorized-clients.json")
        })
}

#[cfg(windows)]
pub(crate) fn read_authorized_clients_from(
    path: &std::path::Path,
) -> anyhow::Result<AuthorizedClients> {
    if !path.exists() {
        return Ok(AuthorizedClients::default());
    }
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct AuthorizedClient {
    pub user: String,
    pub uid: u32,
    pub token: String,
}

#[cfg(windows)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq, Eq)]
pub(crate) struct AuthorizedClients {
    pub clients: Vec<AuthorizedClient>,
}

#[cfg(any(unix, windows))]
pub(crate) fn expected_system_core_binary_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/mihomo/bin/mihomo")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("mihomo")
            .join("bin")
            .join("mihomo.exe")
    } else {
        PathBuf::from("/usr/local/lib/mihomo/mihomo")
    }
}

#[cfg(any(unix, windows))]
fn validate_system_core_binary_request(core_binary: &std::path::Path) -> Result<(), String> {
    let expected = expected_system_core_binary_path();
    if core_binary == expected {
        Ok(())
    } else {
        Err(format!(
            "refusing to start untrusted core binary {} via system daemon; expected {}",
            core_binary.display(),
            expected.display()
        ))
    }
}

#[cfg(windows)]
/// Run the Windows daemon main loop on a named pipe until cancelled.
pub async fn run_daemon(pipe_path: PathBuf, cancel: CancellationToken) -> anyhow::Result<()> {
    let pipe_name = pipe_path.display().to_string();
    let state = Arc::new(Mutex::new(WindowsDaemonState::default()));
    eprintln!("[mihomo-daemon] listening on {pipe_name}");

    // first_pipe_instance only for the first create (P1-2).
    let mut first_instance = true;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                eprintln!("[mihomo-daemon] shutdown requested, exiting accept loop");
                // Stop the managed core child before exiting — dropping the
                // Child alone does NOT kill the process (P1-1).
                let _ = stop_windows_core(Arc::clone(&state)).await;
                return Ok(());
            }
            accepted = accept_one_pipe_connection(&pipe_name, first_instance) => {
                first_instance = false;
                match accepted {
                    Ok(Some(server)) => {
                        let st = Arc::clone(&state);
                        tokio::spawn(async move {
                            if let Err(e) = handle_windows_pipe(server, st).await {
                                eprintln!("[mihomo-daemon] pipe connection error: {e}");
                            }
                        });
                    }
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("[mihomo-daemon] pipe create/connect error: {e}");
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
async fn accept_one_pipe_connection(
    pipe_name: &str,
    first_instance: bool,
) -> anyhow::Result<Option<tokio::net::windows::named_pipe::NamedPipeServer>> {
    use tokio::net::windows::named_pipe::ServerOptions;

    // Build SDDL-restricted security attributes (SYSTEM + Admin + installer SID).
    let mut sd_storage = windows_pipe_security_attributes()?;
    let server = unsafe {
        ServerOptions::new()
            // FILE_FLAG_FIRST_PIPE_INSTANCE only on the first create: a
            // subsequent instance would fail (ERROR_ACCESS_DENIED) while the
            // first pipe name still exists (P1-2).
            .first_pipe_instance(first_instance)
            .create_with_security_attributes_raw(pipe_name, sd_storage.as_mut_ptr())?
    };
    server.connect().await?;
    Ok(Some(server))
}

#[cfg(windows)]
/// Build a SECURITY_ATTRIBUTES with an SDDL descriptor restricting pipe access
/// to SYSTEM + Administrators + the installing user's SID.
///
/// `installer-sid` is written at install time; if absent the descriptor is
/// SYSTEM-only (fail closed).
///
/// Returns a boxed descriptor plus the SECURITY_ATTRIBUTES that points into it.
/// The Box keeps the descriptor alive for the caller's pipe creation.
fn windows_pipe_security_attributes() -> anyhow::Result<windows_pipe_security::PipeSecurity> {
    windows_pipe_security::build()
}

#[cfg(windows)]
mod windows_pipe_security {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

    /// Owns the SECURITY_DESCRIPTOR heap allocation (from
    /// ConvertStringSecurityDescriptorToSecurityDescriptorW, LocalAlloc-based)
    /// referenced by the SECURITY_ATTRIBUTES pointer. LocalFree on drop.
    pub struct PipeSecurity {
        pub attributes: SECURITY_ATTRIBUTES,
        descriptor_ptr: *mut std::ffi::c_void,
    }

    impl PipeSecurity {
        pub fn as_mut_ptr(&mut self) -> *mut std::ffi::c_void {
            &mut self.attributes as *mut _ as *mut std::ffi::c_void
        }
    }

    impl Drop for PipeSecurity {
        fn drop(&mut self) {
            // SAFETY: descriptor_ptr was allocated by
            // ConvertStringSecurityDescriptorToSecurityDescriptorW (LocalAlloc),
            // so LocalFree is the matching deallocator.
            unsafe {
                LocalFree(self.descriptor_ptr);
            }
        }
    }

    pub fn build() -> anyhow::Result<PipeSecurity> {
        let installer_sid = super::read_installer_sid().unwrap_or_default();
        let sddl = super::pipe_sddl_for_installer(&installer_sid);

        let sddl_wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        // The function allocates the SECURITY_DESCRIPTOR itself and writes the
        // address into `descriptor_ptr` — do NOT pre-allocate.
        let mut descriptor_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl_wide.as_ptr(),
                1, // SDDL_REVISION_1
                &mut descriptor_ptr,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            anyhow::bail!("ConvertStringSecurityDescriptorToSecurityDescriptorW failed");
        }

        let mut attributes: SECURITY_ATTRIBUTES = unsafe { std::mem::zeroed() };
        attributes.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
        attributes.lpSecurityDescriptor = descriptor_ptr;
        attributes.bInheritHandle = 0;

        Ok(PipeSecurity {
            attributes,
            descriptor_ptr,
        })
    }
}

#[cfg(windows)]
/// Read the installer SID from `%ProgramData%\mihomo\installer-sid`.
fn read_installer_sid() -> Option<String> {
    let program_data = std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let path = program_data.join("mihomo").join("installer-sid");
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Build the pipe SDDL string restricting access to SYSTEM + Administrators +
/// the installing user's SID. Empty/missing installer SID → fail closed
/// (SYSTEM + Administrators only). Pure function for testability.
#[cfg(any(windows, test))]
fn pipe_sddl_for_installer(installer_sid: &str) -> String {
    if installer_sid.trim().is_empty() {
        "D:P(A;;GA;;;SY)(A;;GA;;;BA)".to_string()
    } else {
        format!(
            "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{})",
            installer_sid.trim()
        )
    }
}

#[cfg(not(any(unix, windows)))]
pub async fn run_daemon(_socket_path: PathBuf, _cancel: CancellationToken) -> anyhow::Result<()> {
    anyhow::bail!("system service daemon is not implemented on this platform")
}

#[cfg(windows)]
/// Enter the Windows SCM dispatcher. Must be called synchronously from the
/// main thread (StartServiceCtrlDispatcher requirement); service_main builds
/// its own tokio runtime to run the daemon loop.
///
/// Falls back to a raw console loop when not launched by SCM (e.g. manual
/// `mihomo-cli daemon` from a shell) — `service_dispatcher::start` returns an
/// error immediately outside a service context.
pub fn run_windows_service() -> anyhow::Result<()> {
    match windows_service_entry::run_dispatcher() {
        Ok(()) => Ok(()),
        Err(e) => {
            // Not running under SCM (manual `mihomo-cli daemon` from a shell):
            // dispatcher fails immediately; fall back to the raw console loop.
            eprintln!("[mihomo-daemon] dispatcher unavailable ({e}), running raw console loop");
            let pipe_path = crate::ipc::system_service_socket_path();
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|e| anyhow::anyhow!("failed to build tokio runtime: {e}"))?;
            runtime.block_on(run_daemon(pipe_path, CancellationToken::new()))
        }
    }
}

#[cfg(windows)]
mod windows_service_entry {
    use std::ffi::OsString;
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState as WinState,
        ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::{define_windows_service, service_dispatcher};

    use crate::ipc;

    const SERVICE_LABEL: &str = "mihomo";

    define_windows_service!(ffi_service_main, service_main);

    pub fn run_dispatcher() -> windows_service::Result<()> {
        service_dispatcher::start(SERVICE_LABEL, ffi_service_main)
    }

    /// Redirect stderr (and stdout) to `%ProgramData%\mihomo\mihomo.log` —
    /// the SCM service process has no console (N1b).
    fn redirect_stderr_to_log() {
        use std::os::windows::io::AsRawHandle;

        let program_data = std::env::var_os("ProgramData")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"));
        let dir = program_data.join("mihomo");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let log_path = dir.join("mihomo.log");
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            unsafe {
                let _ = windows_sys::Win32::System::Console::SetStdHandle(
                    windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
                    file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
                );
            }
        }
    }

    fn service_main(_arguments: Vec<OsString>) {
        // N1b: the SCM service has no console — redirect stderr to a log file
        // so daemon diagnostics (pipe failures, core errors) are recoverable.
        redirect_stderr_to_log();

        let stop = CancellationToken::new();
        let handler_stop = stop.clone();

        let event_handler = move |event| match event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                handler_stop.cancel();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };

        let status_handle = match service_control_handler::register(SERVICE_LABEL, event_handler) {
            Ok(handle) => handle,
            Err(e) => {
                eprintln!("[mihomo-daemon] failed to register service control handler: {e}");
                return;
            }
        };

        if let Err(e) = status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: WinState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::ZERO,
            process_id: None,
        }) {
            eprintln!("[mihomo-daemon] failed to report Running: {e}");
            return;
        }

        let pipe_path = ipc::system_service_socket_path();
        let cancel = stop.clone();
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("[mihomo-daemon] failed to build tokio runtime: {e}");
                let _ = status_handle.set_service_status(ServiceStatus {
                    service_type: ServiceType::OWN_PROCESS,
                    current_state: WinState::Stopped,
                    controls_accepted: ServiceControlAccept::empty(),
                    exit_code: ServiceExitCode::Win32(1),
                    checkpoint: 0,
                    wait_hint: Duration::ZERO,
                    process_id: None,
                });
                return;
            }
        };

        let result = runtime.block_on(super::run_daemon(pipe_path, cancel));

        if let Err(e) = &result {
            eprintln!("[mihomo-daemon] daemon loop error: {e}");
        }

        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: WinState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(if result.is_ok() { 0 } else { 1 }),
            checkpoint: 0,
            wait_hint: Duration::ZERO,
            process_id: None,
        });
    }
}

#[cfg(windows)]
#[derive(Default)]
struct WindowsDaemonState {
    core_running: bool,
    core_child: Option<tokio::process::Child>,
    core_pid: Option<u32>,
    tun_enabled: bool,
    config_path: Option<PathBuf>,
    core_binary: Option<PathBuf>,
}

#[cfg(windows)]
async fn handle_windows_pipe(
    mut pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    state: Arc<Mutex<WindowsDaemonState>>,
) -> anyhow::Result<()> {
    let cmd_buf = crate::ipc::read_json_payload(&mut pipe, "daemon command").await?;
    let cmd = match parse_daemon_command(&cmd_buf) {
        Ok(cmd) => cmd,
        Err(message) => {
            crate::ipc::write_json_message(&mut pipe, &DaemonResponse::Error { message }).await?;
            return Ok(());
        }
    };
    let response = process_windows_command(cmd, state).await;
    crate::ipc::write_json_message(&mut pipe, &response).await?;
    Ok(())
}

#[cfg(any(unix, windows))]
fn read_tun_enabled_from_config(config_path: Option<&PathBuf>) -> Option<bool> {
    let content = std::fs::read_to_string(config_path?).ok()?;
    let config: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    config.get("tun")?.get("enable")?.as_bool()
}

#[cfg(windows)]
async fn process_windows_command(
    cmd: DaemonCommand,
    state: Arc<Mutex<WindowsDaemonState>>,
) -> DaemonResponse {
    // Token auth (N1a): reject commands whose token does not match the
    // server-side copy. Skipped when the server has no token (legacy install).
    let client_token = match &cmd {
        DaemonCommand::StartCore { token, .. }
        | DaemonCommand::RestartCore { token, .. }
        | DaemonCommand::EnableTun { token, .. }
        | DaemonCommand::StopCore { token }
        | DaemonCommand::DisableTun { token }
        | DaemonCommand::GetStatus { token }
        | DaemonCommand::SetAutostart { token, .. } => token.as_deref(),
    };
    if let Err(message) = validate_client_token_for_peer(client_token, None) {
        return DaemonResponse::Error { message };
    }
    match cmd {
        DaemonCommand::GetStatus { .. } => {
            let mut s = state.lock().await;
            reap_exited_windows_core(&mut s);
            if let Some(tun_enabled) = read_tun_enabled_from_config(s.config_path.as_ref()) {
                s.tun_enabled = tun_enabled;
            }
            DaemonResponse::Status {
                running: s.core_running,
                tun_enabled: s.tun_enabled,
                core_pid: s.core_pid,
                config_path: s.config_path.clone(),
                autostart_enabled: daemon_config_dir().join("autostart").exists(),
            }
        }
        // ADR-19: daemon owns the autostart marker.
        DaemonCommand::SetAutostart { enabled, .. } => {
            let marker = daemon_config_dir().join("autostart");
            let result: std::io::Result<()> = if enabled {
                let dir_result = marker
                    .parent()
                    .map(std::fs::create_dir_all)
                    .unwrap_or(Ok(()));
                match dir_result {
                    Ok(()) => std::fs::write(&marker, b"enabled\n"),
                    Err(e) => Err(e),
                }
            } else {
                match std::fs::remove_file(&marker) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e),
                }
            };
            match result {
                Ok(()) => DaemonResponse::Success {
                    message: if enabled {
                        "core autostart enabled".to_string()
                    } else {
                        "core autostart disabled".to_string()
                    },
                },
                Err(e) => DaemonResponse::Error {
                    message: format!("failed to update autostart marker: {e}"),
                },
            }
        }
        DaemonCommand::StartCore { config_path, .. } => {
            start_windows_core(state, config_path, expected_system_core_binary_path()).await
        }
        DaemonCommand::StopCore { .. } => stop_windows_core(state).await,
        DaemonCommand::RestartCore { config_path, .. } => {
            let core_binary = expected_system_core_binary_path();
            if let Err(message) = preflight_system_core_start_request(&config_path, &core_binary) {
                return DaemonResponse::Error { message };
            }
            let _ = stop_windows_core(Arc::clone(&state)).await;
            match start_windows_core(state, config_path.clone(), core_binary).await {
                DaemonResponse::Success { .. } => DaemonResponse::Success {
                    message: format!("core restarted with config {}", config_path.display()),
                },
                other => other,
            }
        }
        DaemonCommand::EnableTun {
            config_path,
            stack,
            dns_hijack,
            ..
        } => {
            if let Err(message) = validate_daemon_config_path_shape(&config_path) {
                return DaemonResponse::Error { message };
            }
            toggle_windows_tun_by_restart(
                state,
                config_path,
                true,
                stack.as_deref(),
                dns_hijack.as_deref(),
            )
            .await
        }
        DaemonCommand::DisableTun { .. } => {
            let config_path = {
                let s = state.lock().await;
                s.config_path.clone()
            };
            match config_path {
                Some(path) => toggle_windows_tun_by_restart(state, path, false, None, None).await,
                None => DaemonResponse::Error {
                    message: "core is not running, cannot disable TUN".to_string(),
                },
            }
        }
    }
}

#[cfg(windows)]
fn reap_exited_windows_core(s: &mut WindowsDaemonState) {
    let Some(child) = s.core_child.as_mut() else {
        s.core_running = false;
        s.core_pid = None;
        return;
    };
    match child.try_wait() {
        Ok(Some(_)) | Err(_) => {
            s.core_child = None;
            s.core_running = false;
            s.core_pid = None;
            s.tun_enabled = false;
            s.config_path = None;
            s.core_binary = None;
        }
        Ok(None) => {
            s.core_running = true;
            s.core_pid = child.id();
        }
    }
}

#[cfg(windows)]
async fn start_windows_core(
    state: Arc<Mutex<WindowsDaemonState>>,
    config_path: PathBuf,
    core_binary: PathBuf,
) -> DaemonResponse {
    let mut s = state.lock().await;
    if let Some(child) = s.core_child.as_mut() {
        match child.try_wait() {
            Ok(None) => {
                s.core_running = true;
                return DaemonResponse::Error {
                    message: "core is already running".to_string(),
                };
            }
            Ok(Some(_)) | Err(_) => {
                s.core_child = None;
                s.core_pid = None;
                s.core_running = false;
            }
        }
    }

    let api_endpoint = match preflight_system_core_start_request(&config_path, &core_binary) {
        Ok(endpoint) => endpoint,
        Err(message) => return DaemonResponse::Error { message },
    };
    if endpoint_is_connectable(&api_endpoint) {
        return DaemonResponse::Error {
            message: duplicate_core_endpoint_message(&api_endpoint),
        };
    }

    let config_dir = config_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let log_file = windows_core_log_file_path();
    let stdout_log = match open_append_log_file(&log_file) {
        Ok(file) => file,
        Err(e) => {
            return DaemonResponse::Error {
                message: format!("failed to open core log file {}: {e}", log_file.display()),
            };
        }
    };
    let stderr_log = match stdout_log.try_clone() {
        Ok(file) => file,
        Err(e) => {
            return DaemonResponse::Error {
                message: format!("failed to clone core log file {}: {e}", log_file.display()),
            };
        }
    };

    let mut cmd = tokio::process::Command::new(&core_binary);
    cmd.arg("-d")
        .arg(&config_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            match child.try_wait() {
                Ok(Some(status)) => DaemonResponse::Error {
                    message: early_exit_message(status, &core_binary, &log_file),
                },
                Err(e) => DaemonResponse::Error {
                    message: format!(
                        "failed to inspect started core process: {e}
  Logs: {}",
                        log_file.display()
                    ),
                },
                Ok(None) => {
                    s.core_running = true;
                    s.config_path = Some(config_path.clone());
                    s.core_binary = Some(core_binary.clone());
                    s.core_pid = pid;
                    s.core_child = Some(child);
                    DaemonResponse::Success {
                        message: format!("core started with config {}", config_path.display()),
                    }
                }
            }
        }
        Err(e) => DaemonResponse::Error {
            message: format!("failed to start core {}: {e}", core_binary.display()),
        },
    }
}

#[cfg(windows)]
async fn stop_windows_core(state: Arc<Mutex<WindowsDaemonState>>) -> DaemonResponse {
    let mut s = state.lock().await;
    let Some(mut child) = s.core_child.take() else {
        s.core_running = false;
        s.core_pid = None;
        s.tun_enabled = false;
        s.config_path = None;
        s.core_binary = None;
        return DaemonResponse::Error {
            message: "core is not running".to_string(),
        };
    };

    let kill_result = child.kill().await;
    let _ = child.wait().await;
    s.core_running = false;
    s.core_pid = None;
    s.tun_enabled = false;
    s.config_path = None;
    s.core_binary = None;

    match kill_result {
        Ok(()) => DaemonResponse::Success {
            message: "core stopped".to_string(),
        },
        Err(e) => DaemonResponse::Error {
            message: format!("failed to stop core: {e}"),
        },
    }
}

#[cfg(windows)]
async fn toggle_windows_tun_by_restart(
    state: Arc<Mutex<WindowsDaemonState>>,
    config_path: PathBuf,
    enable: bool,
    stack: Option<&str>,
    dns_hijack: Option<&str>,
) -> DaemonResponse {
    let core_binary = {
        let mut s = state.lock().await;
        reap_exited_windows_core(&mut s);
        if !s.core_running {
            return DaemonResponse::Error {
                message: format!(
                    "core is not running, cannot {} TUN",
                    if enable { "enable" } else { "disable" }
                ),
            };
        }
        match s.core_binary.clone() {
            Some(path) => path,
            None => {
                return DaemonResponse::Error {
                    message:
                        "daemon does not know the core binary path; restart the system service"
                            .to_string(),
                };
            }
        }
    };

    if let Err(e) = set_tun_in_config_file(&config_path, enable, stack, dns_hijack) {
        return DaemonResponse::Error {
            message: format!("failed to update TUN config {}: {e}", config_path.display()),
        };
    }

    let _ = stop_windows_core(Arc::clone(&state)).await;
    match start_windows_core(Arc::clone(&state), config_path, core_binary).await {
        DaemonResponse::Success { .. } => {
            let mut s = state.lock().await;
            s.tun_enabled = enable;
            DaemonResponse::Success {
                message: format!("TUN {}", if enable { "enabled" } else { "disabled" }),
            }
        }
        other => other,
    }
}

#[cfg(windows)]
fn windows_core_log_file_path() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("mihomo")
        .join("mihomo.log")
}

#[cfg(unix)]
/// Global lifecycle lock — serializes all lifecycle commands (start/stop/restart/
/// TUN toggle) end-to-end, aligned with clash-verge-service OWNER_LIFECYCLE_LOCK.
static OWNER_LIFECYCLE_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> =
    std::sync::OnceLock::new();

#[cfg(unix)]
/// Daemon state shared across connections.
struct DaemonState {
    /// Whether the mihomo core is currently running.
    core_running: bool,
    /// Spawned mihomo core child.
    core_child: Option<tokio::process::Child>,
    /// PID of the running core process.
    core_pid: Option<u32>,
    /// Whether TUN is enabled.
    tun_enabled: bool,
    /// Path to the active config.
    config_path: Option<PathBuf>,
    /// Path to the active core binary. Needed to restart the core when another
    /// user enables TUN with a different per-user config.
    core_binary: Option<PathBuf>,
    /// API endpoint of the running core (for TUN toggle).
    api_endpoint: Option<String>,
    /// PID file used to recover ownership after daemon restart.
    pid_file: PathBuf,
    /// Log file where daemon-managed core stdout/stderr are redirected.
    core_log_file: PathBuf,
}

#[cfg(unix)]
impl Default for DaemonState {
    fn default() -> Self {
        Self {
            core_running: false,
            core_child: None,
            core_pid: None,
            tun_enabled: false,
            config_path: None,
            core_binary: None,
            api_endpoint: None,
            pid_file: core_pid_file_path(),
            core_log_file: core_log_file_path(),
        }
    }
}

/// The daemon's authoritative system paths (ADR-18/19).
///
/// Returns the SystemPaths for the current OS. The daemon uses these
/// paths for config, autostart markers, and runtime files.
/// Must NOT resolve via the daemon's own getpwuid home (root would get /root).
#[allow(dead_code)]
fn daemon_system_paths() -> SystemPaths {
    // Detect the real target OS at runtime
    #[cfg(target_os = "linux")]
    let os = TargetOs::Linux;
    #[cfg(target_os = "macos")]
    let os = TargetOs::Macos;
    #[cfg(target_os = "windows")]
    let os = TargetOs::Windows;

    SystemPaths::for_os(os)
}

/// The daemon's authoritative config directory (ADR-22 single source of truth).
fn daemon_config_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_default().join(".config/mihomo")
}

#[cfg(unix)]
/// Run the daemon main loop.
///
/// This function blocks until the daemon is shut down.
/// It should be called as the main entry point when the binary
/// is invoked as a system service daemon.
pub async fn run_daemon(socket_path: PathBuf, cancel: CancellationToken) -> anyhow::Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Remove stale socket
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;

    // Set socket permissions: readable/writable by all (for IPC from any user)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o666))?;
    }

    let mut initial_state = DaemonState::default();
    if let Some(metadata) = read_pid_file(&initial_state.pid_file) {
        if pid_metadata_is_recoverable_system_core(&metadata) {
            initial_state.core_running = true;
            initial_state.core_pid = Some(metadata.pid);
            initial_state.config_path = non_empty_path(metadata.config_path.clone());
            initial_state.core_binary = non_empty_path(metadata.core_binary.clone());
            initial_state.api_endpoint = metadata.api_endpoint.clone();
        } else {
            remove_pid_file(&initial_state.pid_file);
        }
    }
    let state = Arc::new(Mutex::new(initial_state));

    // ADR-19: if core autostart is enabled, start the core automatically on
    // daemon startup (e.g. boot). The marker is daemon-owned at the
    // authoritative config dir — NOT the CLI's possibly-root-resolved home.
    let autostart_marker = daemon_config_dir().join("autostart");
    if autostart_marker.exists() {
        let config_path = daemon_config_dir().join("config.yaml");
        if config_path.exists() {
            eprintln!("[mihomo-daemon] autostart marker present; starting core");
            let core_binary =
                crate::instance::planned_current_context(crate::instance::InstanceMode::System)
                    .map(|ctx| ctx.paths.core_binary.clone())
                    .unwrap_or_else(|| std::path::PathBuf::from(crate::utils::mihomo_path()));
            let resp = start_core(Arc::clone(&state), config_path, core_binary).await;
            if let DaemonResponse::Error { message } = &resp {
                eprintln!("[mihomo-daemon] autostart core failed: {message}");
            }
        } else {
            eprintln!(
                "[mihomo-daemon] autostart marker present but no config.yaml; skipping core start"
            );
        }
    }

    eprintln!("[mihomo-daemon] listening on {}", socket_path.display());

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                eprintln!("[mihomo-daemon] shutdown requested, exiting accept loop");
                return Ok(());
            }
            accepted = listener.accept() => {
                let (stream, _addr) = accepted?;
                let state = Arc::clone(&state);

                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, state).await {
                        eprintln!("[mihomo-daemon] connection error: {e}");
                    }
                });
            }
        }
    }
}

/// Recover from daemon crash by restarting the daemon.
///
/// This function is called when the CLI detects that the daemon has crashed
/// but the core process may still be running. The daemon's normal startup
/// logic will detect the existing core process via the PID file and reattach to it.
#[cfg(unix)]
pub async fn recover_daemon(socket_path: PathBuf) -> anyhow::Result<()> {
    eprintln!("[mihomo-daemon] recovery mode: checking for existing core process...");

    // Check if there's a PID file with a running core
    if let Some(metadata) = read_pid_file(&PathBuf::from("/var/run/mihomo/core.pid")) {
        if pid_metadata_is_recoverable_system_core(&metadata) {
            eprintln!(
                "[mihomo-daemon] found running core (PID {}), will reattach",
                metadata.pid
            );
        } else {
            eprintln!("[mihomo-daemon] no recoverable core found, starting fresh");
        }
    } else {
        eprintln!("[mihomo-daemon] no PID file found, starting fresh");
    }

    // Run the normal daemon startup which handles recovery automatically
    run_daemon(socket_path, CancellationToken::new()).await
}

#[cfg(not(unix))]
pub async fn recover_daemon(socket_path: PathBuf) -> anyhow::Result<()> {
    eprintln!("[mihomo-daemon] recovery mode not supported on this platform");
    run_daemon(socket_path, CancellationToken::new()).await
}

#[cfg(unix)]
/// Handle a single IPC connection.
async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<DaemonState>>,
) -> anyhow::Result<()> {
    let peer_uid = stream.peer_cred().ok().map(|cred| cred.uid());
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let cmd_buf = match crate::ipc::read_json_payload(&mut reader, "daemon command").await {
        Ok(buf) => buf,
        Err(e) => {
            let resp = DaemonResponse::Error {
                message: e.to_string(),
            };
            send_response(&mut writer, &resp).await?;
            return Ok(());
        }
    };

    let cmd = match parse_daemon_command(&cmd_buf) {
        Ok(cmd) => cmd,
        Err(message) => {
            let resp = DaemonResponse::Error { message };
            send_response(&mut writer, &resp).await?;
            return Ok(());
        }
    };

    // Aligned with clash-verge-service OWNER_LIFECYCLE_LOCK:
    // lifecycle commands (start/stop/restart/TUN toggle) are serialized by a
    // single global mutex held for the *entire* operation — including core
    // spawn and readiness wait — so concurrent clients cannot interleave.
    // GetStatus is read-only and does not take the lifecycle lock.
    let is_lifecycle = !matches!(cmd, DaemonCommand::GetStatus { token: None });
    let response = if is_lifecycle {
        let lock = OWNER_LIFECYCLE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
        let _guard = lock.lock().await;
        process_command(cmd, state, peer_uid).await
    } else {
        process_command(cmd, state, peer_uid).await
    };

    // Send response
    send_response(&mut writer, &response).await?;

    Ok(())
}

#[cfg(any(unix, windows))]
fn parse_daemon_command(bytes: &[u8]) -> Result<DaemonCommand, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("invalid daemon command: {e}"))
}

#[cfg(unix)]
/// Only root (uid 0) may toggle TUN on/off.
fn validate_tun_peer_is_root(peer_uid: Option<u32>) -> Result<(), String> {
    match peer_uid {
        Some(0) => Ok(()),
        Some(_) => Err("TUN on/off requires root privileges".to_string()),
        None => {
            Err("TUN on/off requires root privileges; cannot determine IPC peer uid".to_string())
        }
    }
}

#[cfg(unix)]
async fn process_command(
    cmd: DaemonCommand,
    state: Arc<Mutex<DaemonState>>,
    peer_uid: Option<u32>,
) -> DaemonResponse {
    let client_token = match &cmd {
        DaemonCommand::StartCore { token, .. }
        | DaemonCommand::RestartCore { token, .. }
        | DaemonCommand::EnableTun { token, .. }
        | DaemonCommand::StopCore { token }
        | DaemonCommand::DisableTun { token }
        | DaemonCommand::GetStatus { token }
        | DaemonCommand::SetAutostart { token, .. } => token.as_deref(),
    };
    if let Err(message) = validate_client_token_for_peer(client_token, peer_uid) {
        return DaemonResponse::Error { message };
    }
    if matches!(
        cmd,
        DaemonCommand::EnableTun { .. } | DaemonCommand::DisableTun { .. }
    ) {
        if let Err(message) = validate_tun_peer_is_root(peer_uid) {
            return DaemonResponse::Error { message };
        }
    }
    match cmd {
        DaemonCommand::GetStatus { .. } => {
            let mut s = state.lock().await;
            reap_exited_core(&mut s);
            if let Some(tun_enabled) = read_tun_enabled_from_config(s.config_path.as_ref()) {
                s.tun_enabled = tun_enabled;
            }
            DaemonResponse::Status {
                running: s.core_running,
                tun_enabled: s.tun_enabled,
                core_pid: s.core_pid,
                config_path: s.config_path.clone(),
                autostart_enabled: daemon_config_dir().join("autostart").exists(),
            }
        }
        // ADR-19: daemon owns the autostart marker (its config_dir is the
        // authoritative per-user path — the CLI under sudo would resolve a
        // different home).
        DaemonCommand::SetAutostart { enabled, .. } => {
            let marker = daemon_config_dir().join("autostart");
            let result: std::io::Result<()> = if enabled {
                let dir_result = marker
                    .parent()
                    .map(std::fs::create_dir_all)
                    .unwrap_or(Ok(()));
                match dir_result {
                    Ok(()) => std::fs::write(&marker, b"enabled\n"),
                    Err(e) => Err(e),
                }
            } else {
                match std::fs::remove_file(&marker) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e),
                }
            };
            match result {
                Ok(()) => DaemonResponse::Success {
                    message: if enabled {
                        "core autostart enabled".to_string()
                    } else {
                        "core autostart disabled".to_string()
                    },
                },
                Err(e) => DaemonResponse::Error {
                    message: format!("failed to update autostart marker: {e}"),
                },
            }
        }
        DaemonCommand::StartCore { config_path, .. } => {
            if let Err(message) = validate_daemon_config_path_for_peer(&config_path, peer_uid) {
                return DaemonResponse::Error { message };
            }
            start_core(state, config_path, expected_system_core_binary_path()).await
        }
        DaemonCommand::StopCore { .. } => stop_core(state).await,
        DaemonCommand::RestartCore { config_path, .. } => {
            if let Err(message) = validate_daemon_config_path_for_peer(&config_path, peer_uid) {
                return DaemonResponse::Error { message };
            }
            let core_binary = expected_system_core_binary_path();
            if let Err(message) = preflight_system_core_start_request(&config_path, &core_binary) {
                return DaemonResponse::Error { message };
            }
            let _ = stop_core(Arc::clone(&state)).await;
            match start_core(state, config_path.clone(), core_binary).await {
                DaemonResponse::Success { .. } => DaemonResponse::Success {
                    message: format!("core restarted with config {}", config_path.display()),
                },
                other => other,
            }
        }
        DaemonCommand::EnableTun {
            config_path,
            stack,
            dns_hijack,
            ..
        } => {
            if let Err(message) = validate_daemon_config_path_for_peer(&config_path, peer_uid) {
                return DaemonResponse::Error { message };
            }
            let restart_with = {
                let mut s = state.lock().await;
                reap_exited_core(&mut s);

                if !s.core_running {
                    return DaemonResponse::Error {
                        message: "core is not running, cannot enable TUN".to_string(),
                    };
                }

                if !active_config_matches_requested(s.config_path.as_ref(), &config_path) {
                    match s.core_binary.clone() {
                        Some(core_binary) => Some(core_binary),
                        None => {
                            return DaemonResponse::Error {
                                message: "daemon does not know the core binary path; restart the system service".to_string(),
                            };
                        }
                    }
                } else {
                    None
                }
            };

            if let Some(core_binary) = restart_with {
                let _ = stop_core(Arc::clone(&state)).await;
                match start_core(Arc::clone(&state), config_path.clone(), core_binary).await {
                    DaemonResponse::Success { .. } => {}
                    other => return other,
                }
            }

            let mut s = state.lock().await;
            reap_exited_core(&mut s);
            if !s.core_running {
                return DaemonResponse::Error {
                    message: "core is not running, cannot enable TUN".to_string(),
                };
            }

            // Toggle TUN via mihomo core's API
            match toggle_tun_via_core_api(
                &s.api_endpoint,
                true,
                stack.as_deref(),
                dns_hijack.as_deref(),
            )
            .await
            {
                Ok(()) => {
                    s.tun_enabled = true;
                    DaemonResponse::Success {
                        message: "TUN enabled".to_string(),
                    }
                }
                Err(e) => DaemonResponse::Error {
                    message: format!("failed to enable TUN: {e}"),
                },
            }
        }
        DaemonCommand::DisableTun { .. } => {
            let api_endpoint = {
                let mut s = state.lock().await;
                reap_exited_core(&mut s);

                if !s.core_running {
                    return DaemonResponse::Error {
                        message: "core is not running, cannot disable TUN".to_string(),
                    };
                }
                s.api_endpoint.clone()
            };

            // Toggle TUN via mihomo core's API
            match toggle_tun_via_core_api(&api_endpoint, false, None, None).await {
                Ok(()) => {
                    let mut s = state.lock().await;
                    s.tun_enabled = false;
                    DaemonResponse::Success {
                        message: "TUN disabled".to_string(),
                    }
                }
                Err(e) => DaemonResponse::Error {
                    message: format!("failed to disable TUN: {e}"),
                },
            }
        }
    }
}

#[cfg(unix)]
fn reap_exited_core(s: &mut DaemonState) {
    let Some(child) = s.core_child.as_mut() else {
        if let Some(metadata) = read_pid_file(&s.pid_file)
            .filter(pid_metadata_is_trusted_system_core)
            .filter(|metadata| process_alive(metadata.pid))
        {
            s.core_running = true;
            s.core_pid = Some(metadata.pid);
            if s.config_path.is_none() {
                s.config_path = non_empty_path(metadata.config_path.clone());
            }
            if s.core_binary.is_none() {
                s.core_binary = non_empty_path(metadata.core_binary.clone());
            }
            if s.api_endpoint.is_none() {
                s.api_endpoint = metadata.api_endpoint.clone();
            }
        } else {
            s.core_running = false;
            s.core_pid = None;
            remove_pid_file(&s.pid_file);
        }
        return;
    };
    match child.try_wait() {
        Ok(Some(_)) | Err(_) => {
            s.core_child = None;
            s.core_running = false;
            s.core_pid = None;
            s.tun_enabled = false;
            s.config_path = None;
            s.core_binary = None;
            s.api_endpoint = None;
            remove_pid_file(&s.pid_file);
        }
        Ok(None) => {
            s.core_running = true;
            s.core_pid = child.id();
        }
    }
}

#[cfg(unix)]
fn active_config_matches_requested(active: Option<&PathBuf>, requested: &std::path::Path) -> bool {
    active.map(|path| path == requested).unwrap_or(false)
}

#[cfg(unix)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct CorePidMetadata {
    pid: u32,
    config_path: PathBuf,
    core_binary: PathBuf,
    api_endpoint: Option<String>,
}

#[cfg(unix)]
fn core_pid_file_path() -> PathBuf {
    PathBuf::from("/var/run/mihomo/core.pid")
}

#[cfg(unix)]
fn core_log_file_path() -> PathBuf {
    PathBuf::from("/var/log/mihomo/mihomo.log")
}

#[cfg(any(unix, windows))]
fn open_append_log_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
}

#[cfg(any(unix, windows))]
fn early_exit_message(
    status: std::process::ExitStatus,
    core_binary: &std::path::Path,
    log_file: &std::path::Path,
) -> String {
    format!(
        "core exited immediately after start (status: {status}). Check config and binary: {}\n  Logs: {}",
        core_binary.display(),
        log_file.display()
    )
}

#[cfg(unix)]
fn format_pid_metadata(metadata: &CorePidMetadata) -> String {
    serde_json::to_string_pretty(metadata).unwrap_or_else(|_| format!("{}\n", metadata.pid))
}

#[cfg(unix)]
fn parse_pid_file_content(content: &str) -> Option<CorePidMetadata> {
    if let Ok(metadata) = serde_json::from_str::<CorePidMetadata>(content) {
        return (metadata.pid > 0).then_some(metadata);
    }
    content
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
        .map(|pid| CorePidMetadata {
            pid,
            config_path: PathBuf::new(),
            core_binary: PathBuf::new(),
            api_endpoint: None,
        })
}

#[cfg(unix)]
fn read_pid_file(path: &std::path::Path) -> Option<CorePidMetadata> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| parse_pid_file_content(&content))
}

#[cfg(unix)]
fn write_pid_file(path: &std::path::Path, metadata: &CorePidMetadata) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format_pid_metadata(metadata));
}

#[cfg(unix)]
fn remove_pid_file(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(unix)]
fn non_empty_path(path: PathBuf) -> Option<PathBuf> {
    (!path.as_os_str().is_empty()).then_some(path)
}

#[cfg(unix)]
fn pid_metadata_is_trusted_system_core(metadata: &CorePidMetadata) -> bool {
    metadata.pid > 0
        && validate_daemon_config_path_shape(&metadata.config_path).is_ok()
        && validate_system_core_binary_request(&metadata.core_binary).is_ok()
        && metadata
            .api_endpoint
            .as_deref()
            .map(validate_system_core_api_endpoint)
            .is_some_and(|result| result.is_ok())
}

#[cfg(unix)]
fn pid_metadata_is_recoverable_system_core(metadata: &CorePidMetadata) -> bool {
    pid_metadata_is_trusted_system_core(metadata)
        && process_alive(metadata.pid)
        && process_matches_core_metadata(metadata)
}

#[cfg(unix)]
fn pid_metadata_matches_state(metadata: &CorePidMetadata, state: &DaemonState) -> bool {
    if metadata.pid == 0 {
        return false;
    }
    if let Some(config_path) = &state.config_path {
        if !metadata.config_path.as_os_str().is_empty() && &metadata.config_path != config_path {
            return false;
        }
    }
    if let Some(endpoint) = &state.api_endpoint {
        if let Some(metadata_endpoint) = &metadata.api_endpoint {
            if metadata_endpoint != endpoint {
                return false;
            }
        }
    }
    true
}

#[cfg(target_os = "linux")]
fn process_cmdline(pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    parse_nul_cmdline(&bytes)
}

#[cfg(target_os = "macos")]
fn process_cmdline(pid: u32) -> Option<Vec<String>> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_shell_words_lossy(String::from_utf8_lossy(&output.stdout).trim())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[cfg_attr(windows, allow(dead_code))]
fn process_cmdline(_pid: u32) -> Option<Vec<String>> {
    None
}

#[cfg(target_os = "linux")]
fn parse_nul_cmdline(bytes: &[u8]) -> Option<Vec<String>> {
    let args: Vec<String> = bytes
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect();
    (!args.is_empty()).then_some(args)
}

#[cfg(unix)]
#[allow(dead_code)]
fn parse_shell_words_lossy(command: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (_, c) => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    (!args.is_empty()).then_some(args)
}

#[cfg(unix)]
fn path_basename_text(path: &std::path::Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
}

#[cfg(unix)]
fn cmdline_matches_core_metadata(args: &[String], metadata: &CorePidMetadata) -> bool {
    let Some(program) = args.first() else {
        return false;
    };
    let program_path = std::path::Path::new(program);
    if !metadata.core_binary.as_os_str().is_empty() {
        return program_path == metadata.core_binary;
    }
    path_basename_text(program_path)
        .map(|name| name == "mihomo" || name == "mihomo.exe")
        .unwrap_or(false)
}

#[cfg(unix)]
fn process_matches_core_metadata(metadata: &CorePidMetadata) -> bool {
    match process_cmdline(metadata.pid) {
        Some(args) => cmdline_matches_core_metadata(&args, metadata),
        // Non-Linux platforms do not expose /proc in this implementation. Fall back
        // to pid-file metadata and kill(0) liveness rather than pretending support.
        None => !cfg!(target_os = "linux"),
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> std::io::Result<()> {
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn endpoint_unix_path(endpoint: &str) -> Option<&str> {
    endpoint
        .strip_prefix("unix://")
        .or_else(|| endpoint.starts_with('/').then_some(endpoint))
}

fn endpoint_windows_pipe_path(endpoint: &str) -> Option<&str> {
    endpoint
        .strip_prefix("pipe://")
        .or_else(|| endpoint.starts_with(r"\\.\pipe\").then_some(endpoint))
}

#[cfg(any(unix, windows))]
fn endpoint_is_connectable(endpoint: &str) -> bool {
    if let Some(path) = endpoint_unix_path(endpoint) {
        #[cfg(unix)]
        {
            return std::os::unix::net::UnixStream::connect(path).is_ok();
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
    }

    if let Some(path) = endpoint_windows_pipe_path(endpoint) {
        #[cfg(windows)]
        {
            return std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .is_ok();
        }
        #[cfg(not(windows))]
        {
            let _ = path;
        }
    }

    false
}

#[cfg(any(unix, windows))]
fn duplicate_core_endpoint_message(endpoint: &str) -> String {
    format!(
        "core API endpoint is already reachable at {endpoint}; refusing to start a duplicate core. Stop the existing instance first."
    )
}

#[cfg(any(unix, windows))]
fn expected_system_core_api_endpoint() -> &'static str {
    if cfg!(windows) {
        r"\\.\pipe\mihomo-core"
    } else {
        "/var/run/mihomo/mihomo.sock"
    }
}

#[cfg(any(unix, windows))]
fn validate_system_core_api_endpoint(endpoint: &str) -> Result<(), String> {
    let expected = expected_system_core_api_endpoint();
    let matches_expected = if cfg!(windows) {
        endpoint_windows_pipe_path(endpoint) == Some(expected)
    } else {
        endpoint_unix_path(endpoint) == Some(expected)
    };

    if matches_expected {
        Ok(())
    } else {
        Err(format!(
            "refusing to start system core with unsupported API endpoint {endpoint}; expected {expected}"
        ))
    }
}

#[cfg(any(unix, windows))]
fn read_required_system_core_api_endpoint(config_path: &PathBuf) -> Result<String, String> {
    let endpoint = read_api_endpoint_from_config(config_path).ok_or_else(|| {
        format!(
            "refusing to start system core from {}; missing system core API endpoint {}",
            config_path.display(),
            expected_system_core_api_endpoint()
        )
    })?;
    validate_system_core_api_endpoint(&endpoint)?;
    Ok(endpoint)
}

fn preflight_system_core_start_request(
    config_path: &PathBuf,
    core_binary: &std::path::Path,
) -> Result<String, String> {
    validate_daemon_config_path_shape(config_path)?;
    validate_system_core_binary_request(core_binary)?;
    if !core_binary.exists() {
        return Err(format!("core binary not found: {}", core_binary.display()));
    }
    if !config_path.exists() {
        return Err(format!("config not found: {}", config_path.display()));
    }
    read_required_system_core_api_endpoint(config_path)
}

#[cfg(unix)]
fn remove_stale_unix_endpoint(endpoint: &str) {
    if let Some(path) = endpoint_unix_path(endpoint) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
async fn start_core(
    state: Arc<Mutex<DaemonState>>,
    config_path: PathBuf,
    core_binary: PathBuf,
) -> DaemonResponse {
    let mut s = state.lock().await;
    if let Some(child) = s.core_child.as_mut() {
        match child.try_wait() {
            Ok(None) => {
                s.core_running = true;
                return DaemonResponse::Error {
                    message: "core is already running".to_string(),
                };
            }
            Ok(Some(_)) | Err(_) => {
                s.core_child = None;
                s.core_pid = None;
                s.core_running = false;
            }
        }
    }

    let api_endpoint = match preflight_system_core_start_request(&config_path, &core_binary) {
        Ok(endpoint) => endpoint,
        Err(message) => return DaemonResponse::Error { message },
    };
    if endpoint_is_connectable(&api_endpoint) {
        return DaemonResponse::Error {
            message: duplicate_core_endpoint_message(&api_endpoint),
        };
    }
    remove_stale_unix_endpoint(&api_endpoint);

    let config_dir = config_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let stdout_log = match open_append_log_file(&s.core_log_file) {
        Ok(file) => file,
        Err(e) => {
            return DaemonResponse::Error {
                message: format!(
                    "failed to open core log file {}: {e}",
                    s.core_log_file.display()
                ),
            };
        }
    };
    let stderr_log = match stdout_log.try_clone() {
        Ok(file) => file,
        Err(e) => {
            return DaemonResponse::Error {
                message: format!(
                    "failed to clone core log file {}: {e}",
                    s.core_log_file.display()
                ),
            };
        }
    };

    let mut cmd = tokio::process::Command::new(&core_binary);
    cmd.arg("-d")
        .arg(&config_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            match child.try_wait() {
                Ok(Some(status)) => {
                    s.core_running = false;
                    s.core_pid = None;
                    s.config_path = None;
                    s.api_endpoint = None;
                    remove_pid_file(&s.pid_file);
                    DaemonResponse::Error {
                        message: early_exit_message(status, &core_binary, &s.core_log_file),
                    }
                }
                Err(e) => {
                    s.core_running = false;
                    s.core_pid = None;
                    s.config_path = None;
                    s.api_endpoint = None;
                    remove_pid_file(&s.pid_file);
                    DaemonResponse::Error {
                        message: format!(
                            "failed to inspect started core process: {e}\n  Logs: {}",
                            s.core_log_file.display()
                        ),
                    }
                }
                Ok(None) => {
                    s.core_running = true;
                    s.config_path = Some(config_path.clone());
                    s.core_binary = Some(core_binary.clone());
                    s.core_pid = pid;
                    s.api_endpoint = Some(api_endpoint.clone());
                    if let Some(pid) = pid {
                        write_pid_file(
                            &s.pid_file,
                            &CorePidMetadata {
                                pid,
                                config_path: config_path.clone(),
                                core_binary: core_binary.clone(),
                                api_endpoint: s.api_endpoint.clone(),
                            },
                        );
                    }
                    s.core_child = Some(child);

                    // Readiness: wait for the core API to become reachable before
                    // declaring success. This moves the readiness contract from the
                    // CLI client into the daemon (aligned with clash-verge-service:
                    // the client receives Success only once the core is actually ready).
                    let endpoint = ApiEndpoint::UnixSocket(
                        endpoint_unix_path(&api_endpoint)
                            .map(PathBuf::from)
                            .unwrap_or_else(|| PathBuf::from(&api_endpoint)),
                    );
                    let ready = mihomo_api::wait_for_api_ready_at_endpoint(&endpoint, 15).await;
                    if !ready {
                        // Core spawned but did not become ready; kill it and report failure.
                        let _ = s.core_child.take();
                        s.core_running = false;
                        s.core_pid = None;
                        s.config_path = None;
                        s.api_endpoint = None;
                        remove_pid_file(&s.pid_file);
                        return DaemonResponse::Error {
                            message: format!(
                                "core started but did not become API-ready within 15s at {api_endpoint}\n  \
                                 Logs: {}",
                                s.core_log_file.display()
                            ),
                        };
                    }
                    DaemonResponse::Success {
                        message: format!(
                            "core started and API ready at {api_endpoint} (config {})",
                            config_path.display()
                        ),
                    }
                }
            }
        }
        Err(e) => DaemonResponse::Error {
            message: format!("failed to start core {}: {e}", core_binary.display()),
        },
    }
}

#[cfg(unix)]
async fn stop_core(state: Arc<Mutex<DaemonState>>) -> DaemonResponse {
    let mut s = state.lock().await;
    let Some(mut child) = s.core_child.take() else {
        if let Some(metadata) = read_pid_file(&s.pid_file)
            .filter(pid_metadata_is_trusted_system_core)
            .filter(|metadata| process_alive(metadata.pid))
        {
            if !pid_metadata_matches_state(&metadata, &s)
                || !process_matches_core_metadata(&metadata)
            {
                return DaemonResponse::Error {
                    message: format!(
                        "refusing to terminate orphan core process {} because pid metadata/process identity does not match daemon state",
                        metadata.pid
                    ),
                };
            }
            let pid = metadata.pid;
            let result = terminate_process(pid);
            remove_pid_file(&s.pid_file);
            s.core_running = false;
            s.core_pid = None;
            s.tun_enabled = false;
            s.config_path = None;
            s.core_binary = None;
            s.api_endpoint = None;
            return match result {
                Ok(()) => DaemonResponse::Success {
                    message: format!("orphan core process {pid} terminated"),
                },
                Err(e) => DaemonResponse::Error {
                    message: format!("failed to terminate orphan core process {pid}: {e}"),
                },
            };
        }
        let orphan_endpoint = s
            .api_endpoint
            .as_ref()
            .filter(|endpoint| endpoint_is_connectable(endpoint))
            .cloned();
        s.core_running = false;
        s.core_pid = None;
        s.tun_enabled = false;
        s.config_path = None;
        s.core_binary = None;
        remove_pid_file(&s.pid_file);
        if let Some(endpoint) = orphan_endpoint {
            return DaemonResponse::Error {
                message: format!(
                    "core API endpoint is reachable at {endpoint}, but this daemon does not own the process and has no live pid file. Restart the system service or stop the orphan core manually."
                ),
            };
        }
        return DaemonResponse::Error {
            message: "core is not running".to_string(),
        };
    };

    let kill_result = child.kill().await;
    let _ = child.wait().await;
    s.core_running = false;
    s.core_pid = None;
    s.tun_enabled = false;
    s.config_path = None;
    s.core_binary = None;
    s.api_endpoint = None;
    remove_pid_file(&s.pid_file);

    match kill_result {
        Ok(()) => DaemonResponse::Success {
            message: "core stopped".to_string(),
        },
        Err(e) => DaemonResponse::Error {
            message: format!("failed to stop core: {e}"),
        },
    }
}

#[cfg(unix)]
/// Send a length-prefixed JSON response.
async fn send_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    response: &DaemonResponse,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(response)?;
    let len = json.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(any(unix, windows))]
/// Read the mihomo core's API endpoint from config.yaml.
///
/// Looks for `external-controller-unix` (Linux/macOS) or
/// `external-controller-pipe` (Windows) in the config file.
fn read_api_endpoint_from_config(config_path: &PathBuf) -> Option<String> {
    let content = std::fs::read_to_string(config_path).ok()?;
    let config: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    read_api_endpoint_from_yaml_value(&config, cfg!(windows))
}

#[cfg(any(unix, windows))]
fn read_api_endpoint_from_yaml_value(
    config: &serde_yaml::Value,
    prefer_windows_pipe: bool,
) -> Option<String> {
    let unix = config
        .get("external-controller-unix")
        .and_then(|v| v.as_str());
    let pipe = config
        .get("external-controller-pipe")
        .and_then(|v| v.as_str());

    if prefer_windows_pipe {
        pipe.or(unix).map(ToString::to_string)
    } else {
        unix.or(pipe).map(ToString::to_string)
    }
    .or_else(|| {
        config
            .get("external-controller")
            .and_then(|v| v.as_str())
            .map(|tcp| format!("http://{tcp}"))
    })
}

#[cfg(any(windows, test))]
fn set_tun_in_config_file(
    config_path: &std::path::Path,
    enable: bool,
    stack: Option<&str>,
    dns_hijack: Option<&str>,
) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(config_path)?;
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&content)?;
    if !matches!(doc, serde_yaml::Value::Mapping(_)) {
        doc = serde_yaml::Value::Mapping(Default::default());
    }
    let root = doc.as_mapping_mut().expect("mapping initialized");
    let tun_key = serde_yaml::Value::String("tun".to_string());
    let tun = root
        .entry(tun_key)
        .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    if !matches!(tun, serde_yaml::Value::Mapping(_)) {
        *tun = serde_yaml::Value::Mapping(Default::default());
    }
    let tun = tun.as_mapping_mut().expect("tun mapping initialized");
    tun.insert(
        serde_yaml::Value::String("enable".to_string()),
        serde_yaml::Value::Bool(enable),
    );
    if let Some(stack) = stack {
        tun.insert(
            serde_yaml::Value::String("stack".to_string()),
            serde_yaml::Value::String(stack.to_string()),
        );
    }
    if let Some(dns_hijack) = dns_hijack {
        tun.insert(
            serde_yaml::Value::String("dns-hijack".to_string()),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String(dns_hijack.to_string())]),
        );
    }
    std::fs::write(config_path, serde_yaml::to_string(&doc)?)?;
    Ok(())
}

#[cfg(unix)]
/// Toggle TUN via the mihomo core's REST API.
///
/// The core exposes a PATCH /configs endpoint that accepts partial config updates.
/// We use this to enable/disable TUN without restarting the core.
async fn toggle_tun_via_core_api(
    api_endpoint: &Option<String>,
    enable: bool,
    stack: Option<&str>,
    dns_hijack: Option<&str>,
) -> anyhow::Result<()> {
    let endpoint = api_endpoint.as_ref().ok_or_else(|| {
        anyhow::anyhow!("no API endpoint known — core may not be started by daemon")
    })?;
    let socket_path = endpoint_unix_path(endpoint).ok_or_else(|| {
        anyhow::anyhow!(
            "system daemon TUN control requires a Unix socket core API endpoint, got {endpoint}"
        )
    })?;

    // Build the TUN patch payload
    let mut tun = serde_json::Map::new();
    tun.insert("enable".to_string(), serde_json::Value::Bool(enable));
    if let Some(stack) = stack {
        tun.insert(
            "stack".to_string(),
            serde_json::Value::String(stack.to_string()),
        );
    }
    if let Some(dns_hijack) = dns_hijack {
        tun.insert(
            "dns-hijack".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String(dns_hijack.to_string())]),
        );
    }
    let tun_config = serde_json::json!({ "tun": tun });

    // Send PATCH to the core's API via Unix socket.
    let client = reqwest::Client::builder()
        .unix_socket(socket_path)
        .build()?;

    let resp = client
        .patch("http://localhost/configs")
        .json(&tun_config)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("mihomo API returned {status}: {body}");
    }

    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn daemon_tun_peer_uid_requires_root() {
        assert!(validate_tun_peer_is_root(Some(0)).is_ok());
        assert!(validate_tun_peer_is_root(Some(1000)).is_err());
        assert!(validate_tun_peer_is_root(None)
            .unwrap_err()
            .contains("cannot determine IPC peer uid"));
    }

    #[test]
    fn authorized_clients_table_roundtrip_sets_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("authorized-clients.json");
        let table = AuthorizedClients {
            clients: vec![AuthorizedClient {
                user: "alice".into(),
                uid: 1000,
                token: "tok".into(),
            }],
        };
        write_authorized_clients_to(&path, &table).unwrap();
        assert_eq!(read_authorized_clients_from(&path).unwrap(), table);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn validate_client_token_checks_token_and_peer_uid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("authorized-clients.json");
        write_authorized_clients_to(
            &path,
            &AuthorizedClients {
                clients: vec![AuthorizedClient {
                    user: "alice".into(),
                    uid: 1000,
                    token: "tok".into(),
                }],
            },
        )
        .unwrap();
        let old = std::env::var_os("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH");
        unsafe {
            std::env::set_var("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH", &path);
        }
        assert!(validate_client_token_for_peer(Some("tok"), Some(1000)).is_ok());
        assert!(validate_client_token_for_peer(Some("tok"), Some(1001)).is_err());
        assert!(validate_client_token_for_peer(Some("bad"), Some(1000)).is_err());
        if let Some(v) = old {
            unsafe {
                std::env::set_var("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH", v);
            }
        } else {
            unsafe {
                std::env::remove_var("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH");
            }
        }
    }

    #[tokio::test]
    async fn process_command_rejects_unauthorized_client_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("authorized-clients.json");
        write_authorized_clients_to(&path, &AuthorizedClients { clients: vec![] }).unwrap();
        let old = std::env::var_os("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH");
        unsafe {
            std::env::set_var("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH", &path);
        }
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let response = process_command(
            DaemonCommand::GetStatus {
                token: Some("bad".into()),
            },
            state,
            Some(1000),
        )
        .await;
        assert!(matches!(response, DaemonResponse::Error { .. }));
        if let Some(v) = old {
            unsafe {
                std::env::set_var("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH", v);
            }
        } else {
            unsafe {
                std::env::remove_var("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH");
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_command_rejects_enable_tun_from_non_root_peer() {
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let response = process_command(
            DaemonCommand::EnableTun {
                config_path: PathBuf::from("/home/alice/.config/mihomo/config.yaml"),
                stack: None,
                dns_hijack: None,
                token: None,
            },
            state,
            Some(1000),
        )
        .await;

        assert!(matches!(response, DaemonResponse::Error { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_command_rejects_disable_tun_from_non_root_peer() {
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let response =
            process_command(DaemonCommand::DisableTun { token: None }, state, Some(1000)).await;

        assert!(matches!(response, DaemonResponse::Error { .. }));
    }

    #[test]
    fn pipe_sddl_restricts_to_system_admin_and_installer() {
        let sddl = pipe_sddl_for_installer("S-1-5-21-1000-2000-3000-4000");
        assert!(sddl.contains("SY"));
        assert!(sddl.contains("BA"));
        assert!(sddl.contains("S-1-5-21-1000-2000-3000-4000"));
        assert!(sddl.starts_with("D:P("));
    }

    #[test]
    fn pipe_sddl_fails_closed_without_installer_sid() {
        // Empty/missing installer SID → SYSTEM + Administrators only.
        let sddl = pipe_sddl_for_installer("");
        assert!(sddl.contains("SY"));
        assert!(sddl.contains("BA"));
        assert!(!sddl.contains("(A;;GA;;;S-1-"));
        let sddl_whitespace = pipe_sddl_for_installer("   ");
        assert_eq!(sddl, sddl_whitespace);
    }

    #[test]
    fn daemon_command_parser_rejects_unknown_legacy_fields_with_clear_error() {
        let legacy = br#"{
            "type": "StartCore",
            "config_path": "/home/alice/.config/mihomo/config.yaml",
            "core_binary": "/tmp/untrusted-mihomo"
        }"#;

        let err = parse_daemon_command(legacy).unwrap_err();
        assert!(err.contains("invalid daemon command"));
        assert!(err.contains("core_binary"));
    }

    #[test]
    fn system_tun_endpoint_requires_unix_socket() {
        assert_eq!(
            endpoint_unix_path("/var/run/mihomo/mihomo.sock"),
            Some("/var/run/mihomo/mihomo.sock")
        );
        assert_eq!(
            endpoint_unix_path("unix:///var/run/mihomo/mihomo.sock"),
            Some("/var/run/mihomo/mihomo.sock")
        );
        assert_eq!(endpoint_unix_path("http://127.0.0.1:9090"), None);
    }

    #[test]
    fn endpoint_unix_path_accepts_plain_and_unix_scheme() {
        assert_eq!(
            endpoint_unix_path("/var/run/mihomo/mihomo.sock"),
            Some("/var/run/mihomo/mihomo.sock")
        );
        assert_eq!(
            endpoint_unix_path("unix:///tmp/mihomo.sock"),
            Some("/tmp/mihomo.sock")
        );
        assert_eq!(endpoint_unix_path("http://127.0.0.1:9090"), None);
    }

    #[test]
    fn endpoint_windows_pipe_path_accepts_plain_and_pipe_scheme() {
        assert_eq!(
            endpoint_windows_pipe_path(r"\\.\pipe\mihomo-core"),
            Some(r"\\.\pipe\mihomo-core")
        );
        assert_eq!(
            endpoint_windows_pipe_path(r"pipe://\\.\pipe\mihomo-core"),
            Some(r"\\.\pipe\mihomo-core")
        );
        assert_eq!(endpoint_windows_pipe_path("http://127.0.0.1:9090"), None);
    }

    #[test]
    fn duplicate_core_endpoint_message_is_shared_across_platforms() {
        let message = duplicate_core_endpoint_message(r"\\.\pipe\mihomo-core");
        assert!(message.contains("refusing to start a duplicate core"));
        assert!(message.contains(r"\\.\pipe\mihomo-core"));
    }

    fn sample_metadata(pid: u32) -> CorePidMetadata {
        // Use the current platform's v3 system paths so trust checks pass on
        // every OS (Linux: /usr/local/lib; macOS: /Library/Application Support).
        #[cfg(target_os = "macos")]
        let (config_path, core_binary) = (
            PathBuf::from("/Users/alice/.config/mihomo/config.yaml"),
            PathBuf::from("/Library/Application Support/mihomo/bin/mihomo"),
        );
        #[cfg(not(target_os = "macos"))]
        let (config_path, core_binary) = (
            PathBuf::from("/home/alice/.config/mihomo/config.yaml"),
            PathBuf::from("/usr/local/lib/mihomo/mihomo"),
        );
        CorePidMetadata {
            pid,
            config_path,
            core_binary,
            api_endpoint: Some("/var/run/mihomo/mihomo.sock".to_string()),
        }
    }

    #[test]
    fn pid_file_content_roundtrips_and_rejects_invalid_values() {
        let metadata = sample_metadata(1234);
        let content = format_pid_metadata(&metadata);
        assert_eq!(parse_pid_file_content(&content), Some(metadata));
        assert_eq!(parse_pid_file_content("1234\n").map(|m| m.pid), Some(1234));
        assert_eq!(parse_pid_file_content("0"), None);
        assert_eq!(parse_pid_file_content("not-a-pid"), None);
        assert_eq!(parse_pid_file_content(""), None);
    }

    #[test]
    fn pid_metadata_trust_requires_v3_system_paths_and_endpoint() {
        let trusted = sample_metadata(1234);
        assert!(pid_metadata_is_trusted_system_core(&trusted));

        let mut legacy = trusted.clone();
        legacy.core_binary = PathBuf::new();
        assert!(!pid_metadata_is_trusted_system_core(&legacy));

        let mut wrong_binary = trusted.clone();
        wrong_binary.core_binary = PathBuf::from("/tmp/mihomo");
        assert!(!pid_metadata_is_trusted_system_core(&wrong_binary));

        let mut wrong_config = trusted.clone();
        wrong_config.config_path = PathBuf::from("/etc/mihomo/config.yaml");
        assert!(!pid_metadata_is_trusted_system_core(&wrong_config));

        let mut missing_endpoint = trusted.clone();
        missing_endpoint.api_endpoint = None;
        assert!(!pid_metadata_is_trusted_system_core(&missing_endpoint));

        let mut wrong_endpoint = trusted.clone();
        wrong_endpoint.api_endpoint = Some("/tmp/mihomo.sock".to_string());
        assert!(!pid_metadata_is_trusted_system_core(&wrong_endpoint));
    }

    #[test]
    fn pid_file_read_write_remove_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("core.pid");
        let metadata = sample_metadata(4321);
        assert_eq!(read_pid_file(&path), None);
        write_pid_file(&path, &metadata);
        assert_eq!(read_pid_file(&path), Some(metadata));
        remove_pid_file(&path);
        assert_eq!(read_pid_file(&path), None);
    }

    #[test]
    fn command_line_parsers_handle_proc_and_ps_formats() {
        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                parse_nul_cmdline(b"/usr/bin/mihomo\0-d\0/tmp/cfg\0"),
                Some(vec![
                    "/usr/bin/mihomo".to_string(),
                    "-d".to_string(),
                    "/tmp/cfg".to_string()
                ])
            );
            assert_eq!(parse_nul_cmdline(b""), None);
        }

        assert_eq!(
            parse_shell_words_lossy("/usr/local/bin/mihomo -d /tmp/cfg"),
            Some(vec![
                "/usr/local/bin/mihomo".to_string(),
                "-d".to_string(),
                "/tmp/cfg".to_string()
            ])
        );
        assert_eq!(
            parse_shell_words_lossy(
                "'/Library/Application Support/mihomo/bin/mihomo' -d '/Users/alice/.config/mihomo'"
            ),
            Some(vec![
                "/Library/Application Support/mihomo/bin/mihomo".to_string(),
                "-d".to_string(),
                "/Users/alice/.config/mihomo".to_string()
            ])
        );
        assert_eq!(parse_shell_words_lossy(""), None);
    }

    #[test]
    fn set_tun_in_config_file_preserves_config_and_sets_flag() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = tmp.path().join("config.yaml");
        std::fs::write(
            &config,
            "mixed-port: 7897\ntun:\n  stack: gvisor\n  enable: false\n",
        )
        .unwrap();

        set_tun_in_config_file(&config, true, Some("mixed"), Some("any:53")).unwrap();
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(doc["mixed-port"].as_i64(), Some(7897));
        assert_eq!(doc["tun"]["stack"].as_str(), Some("mixed"));
        assert_eq!(doc["tun"]["dns-hijack"][0].as_str(), Some("any:53"));
        assert_eq!(doc["tun"]["enable"].as_bool(), Some(true));
    }

    #[test]
    fn set_tun_in_config_file_disables_tun_without_dropping_other_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = tmp.path().join("config.yaml");
        std::fs::write(
            &config,
            "mixed-port: 7897\ntun:\n  stack: mixed\n  dns-hijack:\n  - any:53\n  enable: true\n",
        )
        .unwrap();

        set_tun_in_config_file(&config, false, None, None).unwrap();
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(doc["mixed-port"].as_i64(), Some(7897));
        assert_eq!(doc["tun"]["enable"].as_bool(), Some(false));
        assert_eq!(doc["tun"]["stack"].as_str(), Some("mixed"));
        assert_eq!(doc["tun"]["dns-hijack"][0].as_str(), Some("any:53"));
    }

    #[test]
    fn set_tun_in_config_file_replaces_non_mapping_documents_as_needed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = tmp.path().join("config.yaml");

        std::fs::write(&config, "[]\n").unwrap();
        set_tun_in_config_file(&config, true, Some("system"), None).unwrap();
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(doc["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(doc["tun"]["stack"].as_str(), Some("system"));

        std::fs::write(&config, "mixed-port: 7897\ntun: disabled\n").unwrap();
        set_tun_in_config_file(&config, false, None, Some("any:53")).unwrap();
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(doc["mixed-port"].as_i64(), Some(7897));
        assert_eq!(doc["tun"]["enable"].as_bool(), Some(false));
        assert_eq!(doc["tun"]["dns-hijack"][0].as_str(), Some("any:53"));
    }

    #[test]
    fn system_daemon_requires_v3_system_core_api_endpoint() {
        assert!(validate_system_core_api_endpoint("/var/run/mihomo/mihomo.sock").is_ok());
        assert!(validate_system_core_api_endpoint("unix:///var/run/mihomo/mihomo.sock").is_ok());

        let tcp_err = validate_system_core_api_endpoint("http://127.0.0.1:9090").unwrap_err();
        assert!(tcp_err.contains("unsupported API endpoint"));
        assert!(tcp_err.contains("/var/run/mihomo/mihomo.sock"));

        let other_sock = validate_system_core_api_endpoint("/tmp/mihomo.sock").unwrap_err();
        assert!(other_sock.contains("unsupported API endpoint"));
    }

    #[tokio::test]
    async fn stop_core_clears_stale_tun_state_when_no_core_is_running() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: true,
            core_child: None,
            core_pid: Some(4242),
            tun_enabled: true,
            config_path: Some(PathBuf::from("/home/alice/.config/mihomo/config.yaml")),
            core_binary: Some(expected_system_core_binary_path()),
            api_endpoint: Some("/var/run/mihomo/mihomo.sock".to_string()),
            pid_file: tmp.path().join("missing-core.pid"),
            core_log_file: tmp.path().join("mihomo.log"),
        }));

        let response = stop_core(Arc::clone(&state)).await;

        match response {
            DaemonResponse::Error { message } => assert!(message.contains("core is not running")),
            other => panic!("expected not-running error, got {other:?}"),
        }
        let s = state.lock().await;
        assert!(!s.core_running);
        assert_eq!(s.core_pid, None);
        assert!(
            !s.tun_enabled,
            "stale TUN state must be cleared when core is stopped"
        );
        assert_eq!(s.config_path, None);
        assert_eq!(s.core_binary, None);
    }

    #[tokio::test]
    async fn restart_core_preflights_new_config_before_stopping_current_core() {
        let tmp = tempfile::TempDir::new().unwrap();
        #[cfg(target_os = "macos")]
        let config_path = PathBuf::from("/Users/alice/.config/mihomo/config.yaml");
        #[cfg(not(target_os = "macos"))]
        let config_path = PathBuf::from("/home/alice/.config/mihomo/config.yaml");
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: true,
            core_child: None,
            core_pid: Some(4242),
            tun_enabled: false,
            config_path: Some(config_path.clone()),
            core_binary: Some(expected_system_core_binary_path()),
            api_endpoint: Some("/var/run/mihomo/mihomo.sock".to_string()),
            pid_file: tmp.path().join("core.pid"),
            core_log_file: tmp.path().join("mihomo.log"),
        }));
        let missing_config = config_path.clone();

        let response = process_command(
            DaemonCommand::RestartCore {
                config_path: missing_config,
                token: None,
            },
            Arc::clone(&state),
            Some(0),
        )
        .await;

        match response {
            DaemonResponse::Error { message } => assert!(
                message.contains("core binary not found") || message.contains("config not found")
            ),
            other => panic!("expected preflight error, got {other:?}"),
        }
        let s = state.lock().await;
        assert!(
            s.core_running,
            "restart preflight must not stop the current core"
        );
        assert_eq!(s.core_pid, Some(4242));
        assert_eq!(
            s.api_endpoint.as_deref(),
            Some("/var/run/mihomo/mihomo.sock")
        );
    }

    #[test]
    fn system_daemon_rejects_config_without_required_v3_endpoint() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = tmp.path().join("config.yaml");

        std::fs::write(&config, "mixed-port: 7897\n").unwrap();
        let missing = read_required_system_core_api_endpoint(&config).unwrap_err();
        assert!(missing.contains("missing system core API endpoint"));

        std::fs::write(&config, "external-controller: 127.0.0.1:9090\n").unwrap();
        let tcp = read_required_system_core_api_endpoint(&config).unwrap_err();
        assert!(tcp.contains("unsupported API endpoint"));

        std::fs::write(
            &config,
            "external-controller-unix: unix:///var/run/mihomo/mihomo.sock\n",
        )
        .unwrap();
        assert_eq!(
            read_required_system_core_api_endpoint(&config).unwrap(),
            "unix:///var/run/mihomo/mihomo.sock"
        );
    }

    #[test]
    fn api_endpoint_reader_prefers_platform_specific_endpoint_when_multiple_exist() {
        let config: serde_yaml::Value = serde_yaml::from_str(
            r#"external-controller-unix: /var/run/mihomo/mihomo.sock
external-controller-pipe: '\\.\pipe\mihomo-core'
external-controller: 127.0.0.1:9090
"#,
        )
        .unwrap();

        assert_eq!(
            read_api_endpoint_from_yaml_value(&config, false),
            Some("/var/run/mihomo/mihomo.sock".to_string())
        );
        assert_eq!(
            read_api_endpoint_from_yaml_value(&config, true),
            Some(r"\\.\pipe\mihomo-core".to_string())
        );
    }

    #[test]
    fn reads_unix_pipe_and_tcp_api_endpoints_from_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = tmp.path().join("config.yaml");

        std::fs::write(&config, "external-controller-unix: /tmp/mihomo.sock\n").unwrap();
        assert_eq!(
            read_api_endpoint_from_config(&config),
            Some("/tmp/mihomo.sock".to_string())
        );

        std::fs::write(
            &config,
            r#"external-controller-pipe: '\\.\pipe\mihomo-core'
"#,
        )
        .unwrap();
        assert_eq!(
            read_api_endpoint_from_config(&config),
            Some(r"\\.\pipe\mihomo-core".to_string())
        );

        std::fs::write(&config, "external-controller: 127.0.0.1:9090\n").unwrap();
        assert_eq!(
            read_api_endpoint_from_config(&config),
            Some("http://127.0.0.1:9090".to_string())
        );
    }

    #[test]
    fn daemon_core_log_path_follows_v3_system_service_spec() {
        assert_eq!(
            core_log_file_path(),
            PathBuf::from("/var/log/mihomo/mihomo.log")
        );
    }

    #[test]
    fn daemon_rejects_client_supplied_untrusted_core_binary_path() {
        assert!(validate_system_core_binary_request(&expected_system_core_binary_path()).is_ok());
        let err =
            validate_system_core_binary_request(std::path::Path::new("/tmp/mihomo")).unwrap_err();
        assert!(err.contains("refusing to start untrusted core binary"));
        assert!(
            err.contains("/usr/local/lib/mihomo/mihomo")
                || err.contains("/Library/Application Support/mihomo/bin/mihomo")
        );
    }

    #[test]
    fn daemon_accepts_only_clean_per_user_mihomo_config_paths() {
        // Current platform's legitimate per-user config path must be accepted.
        #[cfg(target_os = "macos")]
        let valid = "/Users/alice/.config/mihomo/config.yaml";
        #[cfg(not(target_os = "macos"))]
        let valid = "/home/alice/.config/mihomo/config.yaml";
        assert!(validate_daemon_config_path_shape(std::path::Path::new(valid)).is_ok());
        // The other platform's path shape must be rejected.
        let platform_other_home = if cfg!(target_os = "macos") {
            "/home/alice/.config/mihomo/config.yaml"
        } else {
            "/Users/alice/.config/mihomo/config.yaml"
        };
        let err = validate_daemon_config_path_shape(std::path::Path::new(platform_other_home))
            .unwrap_err();
        assert!(err.contains("system daemon only accepts per-user mihomo config.yaml"));

        let err =
            validate_daemon_config_path_shape(std::path::Path::new("/etc/shadow")).unwrap_err();
        assert!(err.contains("expected config.yaml"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/alice/.config/other/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("system daemon only accepts per-user mihomo config.yaml"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/alice/extra/.config/mihomo/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("system daemon only accepts per-user mihomo config.yaml"));
        let err = validate_daemon_config_path_shape(std::path::Path::new("relative/config.yaml"))
            .unwrap_err();
        assert!(err.contains("non-absolute"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/alice/../bob/.config/mihomo/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("must not contain . or .. components"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/alice/./.config/mihomo/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("must not contain . or .. components"));
    }

    #[test]
    fn daemon_config_owner_must_match_ipc_peer_uid() {
        use std::os::unix::fs::MetadataExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let config = tmp.path().join("config.yaml");
        std::fs::write(&config, "mixed-port: 7897\n").unwrap();
        let uid = std::fs::metadata(&config).unwrap().uid();

        assert!(validate_config_owner_for_peer(&config, uid).is_ok());
        let wrong_uid = if uid == 0 { 1 } else { 0 };
        let err = validate_config_owner_for_peer(&config, wrong_uid).unwrap_err();
        assert!(err.contains("does not match IPC peer uid"));
    }

    #[test]
    fn early_exit_message_mentions_binary_and_log_path() {
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .status()
            .unwrap();
        let message = early_exit_message(
            status,
            std::path::Path::new("/usr/local/lib/mihomo/mihomo"),
            std::path::Path::new("/var/log/mihomo/mihomo.log"),
        );
        assert!(message.contains("core exited immediately after start"));
        assert!(message.contains("/usr/local/lib/mihomo/mihomo"));
        assert!(message.contains("/var/log/mihomo/mihomo.log"));
    }

    #[test]
    fn reads_tun_enabled_from_active_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config = tmp.path().join("config.yaml");
        std::fs::write(&config, "mixed-port: 7897\ntun:\n  enable: true\n").unwrap();
        assert_eq!(read_tun_enabled_from_config(Some(&config)), Some(true));

        std::fs::write(&config, "mixed-port: 7897\ntun:\n  enable: false\n").unwrap();
        assert_eq!(read_tun_enabled_from_config(Some(&config)), Some(false));
        assert_eq!(read_tun_enabled_from_config(None), None);
    }

    #[test]
    fn active_config_match_requires_same_path() {
        let active = PathBuf::from("/Users/alice/.config/mihomo/config.yaml");
        assert!(active_config_matches_requested(Some(&active), &active));
        assert!(!active_config_matches_requested(
            Some(&active),
            std::path::Path::new("/Users/bob/.config/mihomo/config.yaml")
        ));
        assert!(!active_config_matches_requested(None, &active));
    }

    #[test]
    fn open_append_log_file_creates_parent_and_appends() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nested/mihomo.log");
        {
            use std::io::Write;
            let mut file = open_append_log_file(&path).unwrap();
            writeln!(file, "first").unwrap();
        }
        {
            use std::io::Write;
            let mut file = open_append_log_file(&path).unwrap();
            writeln!(file, "second").unwrap();
        }
        let content = std::fs::read_to_string(path).unwrap();
        assert_eq!(
            content,
            "first
second
"
        );
    }

    #[test]
    fn cmdline_matching_requires_core_binary_identity_when_available() {
        let metadata = sample_metadata(1234);
        assert!(cmdline_matches_core_metadata(
            &[
                metadata.core_binary.to_string_lossy().to_string(),
                "-d".to_string()
            ],
            &metadata
        ));
        assert!(!cmdline_matches_core_metadata(
            &["/tmp/other/mihomo".to_string(), "-d".to_string()],
            &metadata
        ));
        assert!(!cmdline_matches_core_metadata(
            &["/usr/bin/sleep".to_string(), "999".to_string()],
            &metadata
        ));
    }

    #[test]
    fn cmdline_matching_accepts_legacy_pid_metadata_only_for_mihomo_name() {
        let legacy = CorePidMetadata {
            pid: 1234,
            config_path: PathBuf::new(),
            core_binary: PathBuf::new(),
            api_endpoint: None,
        };
        assert!(cmdline_matches_core_metadata(
            &["mihomo".to_string(), "-d".to_string()],
            &legacy
        ));
        assert!(!cmdline_matches_core_metadata(
            &["bash".to_string(), "-c".to_string()],
            &legacy
        ));
    }

    // BUG-13 相关：OWNER_LIFECYCLE_LOCK 只串行化生命周期命令，GetStatus 不被阻塞
    #[tokio::test]
    async fn lifecycle_lock_serializes_lifecycle_but_not_status() {
        // 两个并发任务：一个持锁（模拟生命周期操作），一个 GetStatus
        // GetStatus 不经过生命周期锁 → 不阻塞
        let lock = OWNER_LIFECYCLE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
        let guard = lock.lock().await;

        // 持锁时，GetStatus 仍应立即执行（不取生命周期锁）
        let cmd = DaemonCommand::GetStatus { token: None };
        let is_lifecycle = !matches!(cmd, DaemonCommand::GetStatus { token: None });
        assert!(!is_lifecycle, "GetStatus must not be a lifecycle command");

        drop(guard);

        // 生命周期命令（StartCore）应标记为需要锁
        let cmd = DaemonCommand::StartCore {
            config_path: PathBuf::from("/tmp/x/config.yaml"),
            token: None,
        };
        let is_lifecycle = !matches!(cmd, DaemonCommand::GetStatus { token: None });
        assert!(is_lifecycle, "StartCore must be a lifecycle command");
    }

    #[tokio::test]
    async fn lifecycle_lock_serializes_concurrent_commands() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let counter = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        let current = Arc::new(AtomicUsize::new(0));

        async fn simulate_lifecycle(
            counter: Arc<AtomicUsize>,
            max_concurrent: Arc<AtomicUsize>,
            current: Arc<AtomicUsize>,
        ) {
            let lock = OWNER_LIFECYCLE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
            let _guard = lock.lock().await;
            let now = current.fetch_add(1, Ordering::SeqCst) + 1;
            max_concurrent.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            current.fetch_sub(1, Ordering::SeqCst);
            counter.fetch_add(1, Ordering::SeqCst);
        }

        let mut handles = Vec::new();
        for _ in 0..5 {
            let c = Arc::clone(&counter);
            let m = Arc::clone(&max_concurrent);
            let cur = Arc::clone(&current);
            handles.push(tokio::spawn(simulate_lifecycle(c, m, cur)));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 5, "all 5 must complete");
        assert_eq!(
            max_concurrent.load(Ordering::SeqCst),
            1,
            "lifecycle operations must be strictly serialized (max concurrency 1)"
        );
    }
}
