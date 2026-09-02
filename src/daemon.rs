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
#[cfg(unix)]
use crate::ipc::CoreApiMethod;
#[cfg(any(unix, windows))]
use crate::ipc::{DaemonCommand, DaemonResponse};
#[cfg(unix)]
use crate::mihomo_api;
#[cfg(unix)]
use crate::mihomo_api::MihomoApiClient;
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

#[cfg(unix)]
fn validate_selection_intent_dir(path: &std::path::Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!(
            "refusing to use non-absolute selection intent directory {}",
            path.display()
        ));
    }
    let text = path.to_string_lossy().replace('\\', "/");
    if text.contains("/../") || text.contains("/./") {
        return Err(format!(
            "refusing to use selection intent directory {}; path must not contain . or .. components",
            path.display()
        ));
    }
    let parts: Vec<&str> = text.split('/').collect();
    let allowed = if cfg!(target_os = "macos") {
        matches!(
            parts.as_slice(),
            ["", "Users", user, ".config", "mihomo"] if !user.is_empty()
        ) || matches!(parts.as_slice(), ["", "var", "root", ".config", "mihomo"])
    } else {
        matches!(
            parts.as_slice(),
            ["", "home", user, ".config", "mihomo"] if !user.is_empty()
        )
    };
    if !allowed {
        return Err(format!(
            "refusing to use selection intent directory {}; expected a per-user .config/mihomo directory",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_optional_selection_intent_dir(path: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    validate_selection_intent_dir(&path)?;
    Ok(Some(path))
}

#[cfg(any(unix, windows))]
fn validate_daemon_config_path_shape(config_path: &std::path::Path) -> Result<(), String> {
    if !config_path.is_absolute() {
        return Err(format!(
            "refusing to use non-absolute config path {}",
            config_path.display()
        ));
    }
    let file_name = config_path.file_name().and_then(|n| n.to_str());
    let is_tun_config = file_name == Some("tun-config.yaml");
    let is_user_config = file_name == Some("config.yaml");
    let is_managed_recovery_config = matches!(
        file_name,
        Some("active-config.yaml") | Some("recovery-target.yaml")
    );
    if !is_tun_config && !is_user_config && !is_managed_recovery_config {
        return Err(format!(
            "refusing to use config path {}; expected config.yaml or tun-config.yaml",
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
        // User config: %APPDATA%/mihomo/config.yaml
        let user_config_ok = (text.ends_with("/AppData/Roaming/mihomo/config.yaml")
            || text.ends_with("/AppData/Roaming/Mihomo/config.yaml"))
            && (text.contains(":/") || text.starts_with("//"));
        // TUN config: %ProgramData%/mihomo-cli/tun-config.yaml
        let tun_config_ok =
            is_tun_config && text.ends_with("/ProgramData/mihomo-cli/tun-config.yaml");
        user_config_ok || tun_config_ok
    } else {
        let parts: Vec<&str> = text.split('/').collect();
        if cfg!(target_os = "macos") {
            // User config: /Users/<user>/.config/mihomo/config.yaml
            let user_config_ok = matches!(
                parts.as_slice(),
                ["", "Users", user, ".config", "mihomo", "config.yaml"] if !user.is_empty()
            ) || matches!(
                parts.as_slice(),
                ["", "var", "root", ".config", "mihomo", "config.yaml"]
            );
            // TUN config: /Library/Application Support/mihomo-cli/tun-config.yaml
            let tun_config_ok = is_tun_config
                && matches!(
                    parts.as_slice(),
                    [
                        "",
                        "Library",
                        "Application Support",
                        "mihomo-cli",
                        "tun-config.yaml"
                    ]
                );
            user_config_ok || tun_config_ok
        } else {
            // Linux
            // User config: /home/<user>/.config/mihomo/config.yaml
            let user_config_ok = matches!(
                parts.as_slice(),
                ["", "home", user, ".config", "mihomo", "config.yaml"] if !user.is_empty()
            );
            // TUN config: /var/lib/mihomo-cli/tun-config.yaml
            let tun_config_ok = is_tun_config
                && matches!(
                    parts.as_slice(),
                    ["", "var", "lib", "mihomo-cli", "tun-config.yaml"]
                );
            let managed_recovery_config_ok = is_managed_recovery_config
                && matches!(
                    parts.as_slice(),
                    ["", "var", "lib", "mihomo-cli", "active-config.yaml"]
                        | [
                            "",
                            "var",
                            "lib",
                            "mihomo-cli",
                            "transactions",
                            "active",
                            "recovery-target.yaml"
                        ]
                );
            user_config_ok || tun_config_ok || managed_recovery_config_ok
        }
    };

    if allowed {
        Ok(())
    } else {
        Err(format!(
            "refusing to use config path {}; system daemon only accepts per-user config.yaml or system-level tun-config.yaml",
            config_path.display()
        ))
    }
}

#[cfg(unix)]
fn validate_daemon_config_path_for_peer(
    config_path: &std::path::Path,
    peer_uid: Option<u32>,
) -> Result<(), String> {
    if matches!(
        config_path.file_name().and_then(|name| name.to_str()),
        Some("active-config.yaml") | Some("recovery-target.yaml")
    ) {
        return Err(format!(
            "refusing to use internal recovery config path {} through daemon IPC",
            config_path.display()
        ));
    }
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
    // 提权上下文（root）下忽略可伪造的环境变量覆盖，避免 root 借它写到任意路径
    if unsafe { libc::geteuid() } == 0 {
        return PathBuf::from("/var/lib/mihomo-cli/authorized-clients.json");
    }
    std::env::var_os("MIHOMO_CLI_AUTHORIZED_CLIENTS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/mihomo-cli/authorized-clients.json"))
}

#[cfg(unix)]
pub(crate) fn revoke_authorized_client(
    table: &mut AuthorizedClients,
    uid: u32,
    token: &str,
) -> anyhow::Result<bool> {
    if token.is_empty() {
        anyhow::bail!("refusing to revoke an empty client token");
    }
    let matches: Vec<usize> = table
        .clients
        .iter()
        .enumerate()
        .filter(|(_, client)| client.uid == uid && constant_time_token_eq(&client.token, token))
        .map(|(index, _)| index)
        .collect();
    if matches.len() > 1 {
        anyhow::bail!("authorized-client table has duplicate UID/token entries");
    }
    if matches.is_empty() {
        return Ok(false);
    }
    table.clients.remove(matches[0]);
    Ok(true)
}

#[cfg(unix)]
pub(crate) fn read_client_token_for_home(home: &std::path::Path) -> anyhow::Result<String> {
    use std::io::Read;
    let path = crate::service::client_token_path_for_home(home);
    let mut file = crate::utils::open_regular_file_no_follow(&path)?;
    let mut token = String::new();
    file.read_to_string(&mut token)?;
    let token = token.trim().to_string();
    if token.is_empty() {
        anyhow::bail!("client token is empty: {}", path.display());
    }
    Ok(token)
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
fn write_root_authorized_clients_file(
    path: &std::path::Path,
    bytes: &[u8],
    mode: u16,
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    if unsafe { libc::geteuid() } != 0 || path != authorized_clients_path() {
        anyhow::bail!(
            "refusing privileged authorized-client write outside {}",
            authorized_clients_path().display()
        );
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("authorized-clients path has no parent"))?;
    if !parent.is_dir() {
        anyhow::bail!(
            "authorized-client state directory is missing: {}. Reinstall the system service",
            parent.display()
        );
    }
    let dir = crate::utils::open_directory_no_follow(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("authorized-clients path has no file name"))?;
    let name = std::ffi::CString::new(name.as_bytes())?;
    let temp_name =
        std::ffi::CString::new(format!(".authorized-clients.{}.tmp", std::process::id()))?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            temp_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let write_result = (|| -> anyhow::Result<()> {
        if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        unsafe {
            libc::unlinkat(dir.as_raw_fd(), temp_name.as_ptr(), 0);
        }
        return Err(error);
    }
    if unsafe {
        libc::renameat(
            dir.as_raw_fd(),
            temp_name.as_ptr(),
            dir.as_raw_fd(),
            name.as_ptr(),
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::unlinkat(dir.as_raw_fd(), temp_name.as_ptr(), 0);
        }
        return Err(error.into());
    }
    if unsafe { libc::fsync(dir.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn write_authorized_clients_to(
    path: &std::path::Path,
    table: &AuthorizedClients,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    let mode = 0o640;
    #[cfg(not(target_os = "linux"))]
    let mode = 0o600;
    let bytes = serde_json::to_vec_pretty(table)?;
    let privileged_system_path =
        unsafe { libc::geteuid() } == 0 && path == authorized_clients_path();
    if privileged_system_path {
        write_root_authorized_clients_file(path, &bytes, mode)?;
    } else {
        if let Some(parent) = path.parent() {
            crate::utils::ensure_dir_all_no_follow(parent)?;
        }
        crate::utils::write_bytes_file_no_follow(path, &bytes, mode)?;
    }
    #[cfg(target_os = "linux")]
    if privileged_system_path {
        let group = unsafe { libc::getgrnam(c"mihomo".as_ptr()) };
        if group.is_null() {
            anyhow::bail!("mihomo group not found; reinstall the system service");
        }
        let gid = unsafe { (*group).gr_gid };
        let path_c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())?;
        if unsafe { libc::chown(path_c.as_ptr(), 0, gid) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(not(target_os = "linux"))]
    if privileged_system_path {
        use std::os::unix::ffi::OsStrExt;
        let path_c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        if unsafe { libc::chown(path_c.as_ptr(), 0, 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
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
    match table
        .clients
        .iter()
        .find(|c| constant_time_token_eq(&c.token, token))
    {
        Some(c) if c.uid == uid => Ok(()),
        Some(_) => Err("auth token does not belong to IPC peer uid".to_string()),
        None => Err("invalid or missing auth token".to_string()),
    }
}

fn constant_time_token_eq(a: &str, b: &str) -> bool {
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

#[cfg(any(test, windows))]
fn validate_windows_client_token_value(
    request_token: Option<&str>,
    service_token: Option<&str>,
) -> Result<(), String> {
    match (request_token, service_token) {
        (Some(request), Some(stored))
            if !request.is_empty()
                && !stored.is_empty()
                && constant_time_token_eq(request, stored) =>
        {
            Ok(())
        }
        _ => Err("invalid or missing auth token".to_string()),
    }
}

#[cfg(windows)]
/// Windows token-only validation — no peer UID available on named pipes.
/// The single credential in `%ProgramData%\mihomo\service-token` is shared by
/// the daemon and the installing user's CLI.
pub(crate) fn validate_client_token_for_peer(
    token: Option<&str>,
    _peer_uid: Option<u32>,
) -> Result<(), String> {
    use std::io::Read;

    let inputs = crate::instance::PathInputs::from_current_env();
    let service_token_path = crate::instance::planned_daemon_credential_paths(
        crate::instance::TargetOs::Windows,
        &inputs,
    )
    .token;
    let mut file = crate::utils::open_regular_file_no_follow(&service_token_path)
        .map_err(|_| "invalid or missing auth token".to_string())?;
    let mut stored = String::new();
    file.read_to_string(&mut stored)
        .map_err(|_| "invalid or missing auth token".to_string())?;
    validate_windows_client_token_value(token, Some(stored.trim()))
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
    let _ = daemon_executable_revision();
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
    use std::io::Read;

    let inputs = crate::instance::PathInputs::from_current_env();
    let service_token = crate::instance::planned_daemon_credential_paths(
        crate::instance::TargetOs::Windows,
        &inputs,
    )
    .token;
    let path = service_token.parent()?.join("installer-sid");
    let mut file = crate::utils::open_regular_file_no_follow(&path).ok()?;
    let mut sid = String::new();
    file.read_to_string(&mut sid).ok()?;
    let sid = sid.trim();
    crate::instance::valid_windows_sid_string(sid).then(|| sid.to_string())
}

/// Build the pipe SDDL string restricting access to SYSTEM + Administrators +
/// the installing user's SID. Empty/missing installer SID → fail closed
/// (SYSTEM + Administrators only). Pure function for testability.
#[cfg(any(windows, test))]
fn pipe_sddl_for_installer(installer_sid: &str) -> String {
    let installer_sid = installer_sid.trim();
    if !crate::instance::valid_windows_sid_string(installer_sid) {
        "D:P(A;;GA;;;SY)(A;;GA;;;BA)".to_string()
    } else {
        format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{})", installer_sid)
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
    config_path: Option<PathBuf>,
    launched_config_revision: Option<String>,
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

#[cfg(windows)]
async fn promote_system_config_windows(
    _state: Arc<Mutex<WindowsDaemonState>>,
    _config_content: String,
    _config_revision: String,
    _selection_intent_dir: Option<String>,
) -> DaemonResponse {
    DaemonResponse::Error {
        message: "system configuration promotion is not supported by the Windows service yet"
            .to_string(),
    }
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
        | DaemonCommand::ApplySystemTunSnapshot { token, .. }
        | DaemonCommand::PromoteSystemConfig { token, .. }
        | DaemonCommand::SelectSystemProxy { token, .. }
        | DaemonCommand::StopCore { token }
        | DaemonCommand::DisableTun { token }
        | DaemonCommand::GetStatus { token }
        | DaemonCommand::CoreApiRequest { token, .. }
        | DaemonCommand::SetAutostart { token, .. }
        | DaemonCommand::ValidatePreparedRuntime { token, .. }
        | DaemonCommand::ApplyPromotedSnapshot { token, .. }
        | DaemonCommand::QuiesceCandidateRuntime { token, .. }
        | DaemonCommand::RestoreOldRuntime { token, .. }
        | DaemonCommand::AttestCurrentTransaction { token, .. }
        | DaemonCommand::ApplyLegacyRecoveryTarget { token, .. }
        | DaemonCommand::GetTransactionStatus { token } => token.as_deref(),
    };
    if let Err(message) = validate_client_token_for_peer(client_token, None) {
        return DaemonResponse::Error { message };
    }
    match cmd {
        DaemonCommand::GetStatus { .. } => {
            let mut s = state.lock().await;
            reap_exited_windows_core(&mut s);
            let (tun_journal_state, tun_journal_error) = active_journal_status();
            DaemonResponse::Status {
                running: s.core_running,
                core_pid: s.core_pid,
                config_path: s.config_path.clone(),
                tun_snapshot_revision: managed_system_tun_snapshot_path()
                    .ok()
                    .and_then(|path| crate::ipc::managed_snapshot_revision(&path).ok()),
                launched_config_revision: s.launched_config_revision.clone(),
                autostart_enabled: daemon_config_dir().join("autostart").exists(),
                daemon_executable_revision: daemon_executable_revision(),
                tun_journal_state,
                tun_journal_error,
            }
        }
        DaemonCommand::PromoteSystemConfig {
            config_content,
            config_revision,
            selection_intent_dir,
            ..
        } => {
            promote_system_config_windows(
                state,
                config_content,
                config_revision,
                selection_intent_dir,
            )
            .await
        }
        DaemonCommand::SelectSystemProxy { group, node, .. } => {
            let _ = (group, node);
            DaemonResponse::Error {
                message: "system proxy selection is not supported by the Windows service yet"
                    .to_string(),
            }
        }
        DaemonCommand::CoreApiRequest { .. } => DaemonResponse::Error {
            message: "Core API forwarding is not supported by the Windows service yet".to_string(),
        },
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
        DaemonCommand::StartCore {
            config_content,
            config_revision,
            selection_intent_dir,
            ..
        } => {
            promote_system_config_windows(
                state,
                config_content,
                config_revision,
                selection_intent_dir,
            )
            .await
        }
        DaemonCommand::StopCore { .. } => stop_windows_core(state).await,
        DaemonCommand::RestartCore {
            config_content,
            config_revision,
            selection_intent_dir,
            ..
        } => {
            promote_system_config_windows(
                state,
                config_content,
                config_revision,
                selection_intent_dir,
            )
            .await
        }
        DaemonCommand::ApplySystemTunSnapshot { .. } | DaemonCommand::DisableTun { .. } => {
            DaemonResponse::Error {
                message:
                    "legacy TUN commands are deprecated; please use transaction-based tun on/off"
                        .to_string(),
            }
        }
        DaemonCommand::ValidatePreparedRuntime { .. }
        | DaemonCommand::ApplyPromotedSnapshot { .. }
        | DaemonCommand::QuiesceCandidateRuntime { .. }
        | DaemonCommand::RestoreOldRuntime { .. }
        | DaemonCommand::AttestCurrentTransaction { .. }
        | DaemonCommand::ApplyLegacyRecoveryTarget { .. }
        | DaemonCommand::GetTransactionStatus { .. } => DaemonResponse::Error {
            message: "TUN transaction recovery is not implemented on Windows yet".to_string(),
        },
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
    let config_path = match system_runtime_config_path(&config_path) {
        Ok(path) => path,
        Err(error) => {
            return DaemonResponse::Error {
                message: format!("failed to prepare system runtime config: {error}"),
            };
        }
    };
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
                s.launched_config_revision = None;
            }
        }
    }

    let api_endpoint = match preflight_system_core_start_request(&config_path, &core_binary) {
        Ok(endpoint) => endpoint,
        Err(message) => return DaemonResponse::Error { message },
    };
    let config_revision = match config_content_revision(&config_path) {
        Ok(revision) => revision,
        Err(error) => {
            return DaemonResponse::Error {
                message: format!("failed to read Core config revision: {error}"),
            }
        }
    };
    if endpoint_is_connectable(&api_endpoint) {
        return DaemonResponse::Error {
            message: duplicate_core_endpoint_message(&api_endpoint),
        };
    }

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
    cmd.args(core_command_args(
        &config_path,
        &runtime_data_dir_for_config(&config_path),
    ))
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
                        "failed to inspect started core process: {e}\n  Logs: {}",
                        log_file.display()
                    ),
                },
                Ok(None) => {
                    s.core_running = true;
                    s.config_path = Some(config_path.clone());
                    s.launched_config_revision = Some(config_revision);
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
        s.config_path = None;
        s.launched_config_revision = None;
        s.core_binary = None;
        return DaemonResponse::Error {
            message: "core is not running".to_string(),
        };
    };

    let kill_result = child.kill().await;
    let _ = child.wait().await;
    s.core_running = false;
    s.core_pid = None;
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
#[allow(dead_code)]
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
        DaemonResponse::Success { .. } => DaemonResponse::Success {
            message: format!("TUN {}", if enable { "enabled" } else { "disabled" }),
        },
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
    /// Path to the active config.
    config_path: Option<PathBuf>,
    /// Revision of the config content loaded when Core was launched.
    launched_config_revision: Option<String>,
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
            config_path: None,
            launched_config_revision: None,
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

/// The daemon's authoritative runtime directory.
fn daemon_config_dir() -> std::path::PathBuf {
    system_runtime_data_dir()
}

/// Durable record of the per-user config dir holding selection-state.yaml, so
/// the daemon can replay pinned selections after a daemon restart or boot-time
/// autostart (the pid file lives on tmpfs and does not survive boot).
#[cfg(unix)]
fn selection_intent_dir_state_path() -> PathBuf {
    daemon_config_dir().join("selection-intent-dir")
}

#[cfg(unix)]
fn persist_selection_intent_dir(intent_dir: &std::path::Path) {
    let path = selection_intent_dir_state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, format!("{}\n", intent_dir.display()));
}

#[cfg(unix)]
fn read_persisted_selection_intent_dir() -> Option<PathBuf> {
    let raw = std::fs::read_to_string(selection_intent_dir_state_path()).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}

/// Best-effort replay of persisted selection intent against the running Core
/// (SPEC-select-persistence §3.2/§3.3). Returns user-facing report lines;
/// failures degrade to warning lines instead of failing the caller. Holds no
/// state lock while awaiting (replay has its own ≤5s budget).
#[cfg(unix)]
struct DaemonSelectionApiClient {
    state: Arc<Mutex<DaemonState>>,
    inner: mihomo_api::EndpointMihomoApiClient,
}

#[cfg(unix)]
impl mihomo_api::MihomoApiClient for DaemonSelectionApiClient {
    async fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        self.inner.get(path).await
    }

    async fn put(&self, path: &str, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let prefix = "/proxies/";
        if let Some(encoded_group) = path.strip_prefix(prefix) {
            let group = percent_decode_proxy_segment(encoded_group)?;
            let node = body["name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("proxy selection payload has no node name"))?;
            return match select_system_proxy(Arc::clone(&self.state), group, node.to_string()).await
            {
                DaemonResponse::Success { .. } => Ok(serde_json::Value::Null),
                DaemonResponse::Error { message } => anyhow::bail!(message),
                response => anyhow::bail!("unexpected daemon selection response: {response:?}"),
            };
        }
        self.inner.put(path, body).await
    }

    async fn patch(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.inner.patch(path, body).await
    }

    async fn delete(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        self.inner.delete(path).await
    }
}

#[cfg(unix)]
fn percent_decode_proxy_segment(value: &str) -> anyhow::Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                anyhow::bail!("invalid encoded proxy group path");
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])?;
            decoded.push(u8::from_str_radix(hex, 16)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(String::from_utf8(decoded)?)
}

#[cfg(unix)]
async fn replay_selection_intent(
    state: &Arc<Mutex<DaemonState>>,
    intent_dir: &std::path::Path,
    subscription_id: &str,
) -> Vec<String> {
    let api_endpoint = {
        let mut s = state.lock().await;
        reap_exited_core(&mut s);
        if !s.core_running {
            return Vec::new();
        }
        s.api_endpoint.clone()
    };
    let Some(api_endpoint) = api_endpoint else {
        return Vec::new();
    };
    let ep = ApiEndpoint::UnixSocket(
        endpoint_unix_path(&api_endpoint)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(api_endpoint)),
    );
    let client = mihomo_api::EndpointMihomoApiClient::new(ep);
    let deadline = std::time::Instant::now() + crate::selection::REPLAY_TOTAL_BUDGET;
    loop {
        if client.get("/configs").await.is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return vec![format!(
                "⚠ Selections not replayed: Core API not ready within {}s",
                crate::selection::REPLAY_TOTAL_BUDGET.as_secs()
            )];
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let paths = crate::utils::AppPaths::new(intent_dir.to_path_buf());
    let scope = crate::selection::SelectionScope {
        subscription_id: subscription_id.to_string(),
        path: paths.selection_state_path_for_subscription(subscription_id),
    };
    let replay_client = DaemonSelectionApiClient {
        state: Arc::clone(state),
        inner: client,
    };
    match crate::selection::replay_scope_until(&scope, &replay_client, deadline).await {
        Ok(report) => report.format_lines(),
        Err(err) => vec![format!("⚠ Selections not replayed: {err:#}")],
    }
}

fn daemon_transaction_context() -> Option<crate::instance::InstanceContext> {
    let mut ctx = crate::instance::planned_current_context(crate::instance::InstanceMode::System)?;
    let config_dir = daemon_config_dir();
    ctx.paths.config_dir = config_dir.clone();
    ctx.paths.config_file = config_dir.join("config.yaml");
    ctx.paths.intent_config_file = config_dir.join("config.yaml");
    Some(ctx)
}

#[cfg(any(unix, windows))]
fn active_journal_status() -> (Option<crate::tun_transaction::JournalPhase>, Option<String>) {
    let Some(ctx) = daemon_transaction_context() else {
        return (None, None);
    };
    journal_status_from_result(crate::tun_transaction::read_active_journal(&ctx))
}

#[cfg(any(unix, windows))]
fn journal_status_from_result(
    result: anyhow::Result<Option<crate::tun_transaction::TunJournal>>,
) -> (Option<crate::tun_transaction::JournalPhase>, Option<String>) {
    match result {
        Ok(Some(journal)) => (Some(journal.phase), None),
        Ok(None) => (None, None),
        Err(error) => (
            Some(crate::tun_transaction::JournalPhase::RecoveryRequired),
            Some(error.to_string()),
        ),
    }
}

#[cfg(unix)]
fn allowed_with_unreadable_journal(cmd: &DaemonCommand) -> bool {
    matches!(
        cmd,
        DaemonCommand::GetStatus { .. } | DaemonCommand::GetTransactionStatus { .. }
    )
}

fn managed_system_tun_snapshot_path() -> Result<PathBuf, String> {
    crate::instance::planned_current_context(crate::instance::InstanceMode::System)
        .map(|ctx| ctx.paths.tun_config_file)
        .ok_or_else(|| "system instance paths are unavailable".to_string())
}

#[cfg(unix)]
/// Run the daemon main loop.
///
/// This function blocks until the daemon is shut down.
/// It should be called as the main entry point when the binary
/// is invoked as a system service daemon.
pub async fn run_daemon(socket_path: PathBuf, cancel: CancellationToken) -> anyhow::Result<()> {
    let _ = daemon_executable_revision();
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
            initial_state.launched_config_revision = metadata.config_revision.clone();
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
        let config_path = system_runtime_data_dir().join("active-config.yaml");
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

    // If a core ended up running (pid-file recovery or autostart), replay the
    // persisted selection intent once — idempotent and best-effort
    // (SPEC-select-persistence §3.2 daemon lifecycle hook).
    let core_running_at_startup = state.lock().await.core_running;
    if core_running_at_startup {
        if let Some(intent_dir) = read_persisted_selection_intent_dir() {
            let paths = crate::utils::AppPaths::new(intent_dir.clone());
            if let Ok(Some(subscription_id)) = crate::config::get_active_id_at(&paths) {
                for line in replay_selection_intent(&state, &intent_dir, &subscription_id).await {
                    eprintln!("[mihomo-daemon] {line}");
                }
            }
        }
    }

    eprintln!("[mihomo-daemon] listening on {}", socket_path.display());

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    loop {
        tokio::select! {
            _ = sigterm.recv() => {
                eprintln!("[mihomo-daemon] SIGTERM received");
                match stop_core(Arc::clone(&state)).await {
                    DaemonResponse::Success { message } => eprintln!("[mihomo-daemon] {message}"),
                    DaemonResponse::Error { message } if message != "core is not running" => {
                        eprintln!("[mihomo-daemon] core shutdown cleanup failed: {message}");
                    }
                    _ => {}
                }
                let _ = std::fs::remove_file(&socket_path);
                return Ok(());
            }
            _ = sigint.recv() => {
                eprintln!("[mihomo-daemon] SIGINT received");
                match stop_core(Arc::clone(&state)).await {
                    DaemonResponse::Success { message } => eprintln!("[mihomo-daemon] {message}"),
                    DaemonResponse::Error { message } if message != "core is not running" => {
                        eprintln!("[mihomo-daemon] core shutdown cleanup failed: {message}");
                    }
                    _ => {}
                }
                let _ = std::fs::remove_file(&socket_path);
                return Ok(());
            }
            _ = cancel.cancelled() => {
                eprintln!("[mihomo-daemon] shutdown requested, stopping managed core");
                match stop_core(Arc::clone(&state)).await {
                    DaemonResponse::Success { message } => {
                        eprintln!("[mihomo-daemon] {message}");
                    }
                    DaemonResponse::Error { message } if message != "core is not running" => {
                        eprintln!("[mihomo-daemon] core shutdown cleanup failed: {message}");
                    }
                    _ => {}
                }
                let _ = std::fs::remove_file(&socket_path);
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
    let is_lifecycle = !matches!(
        cmd,
        DaemonCommand::GetStatus { .. } | DaemonCommand::GetTransactionStatus { .. }
    );
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

static DAEMON_EXECUTABLE_REVISION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

fn daemon_executable_revision() -> Option<String> {
    DAEMON_EXECUTABLE_REVISION
        .get_or_init(|| {
            let executable = std::env::current_exe().ok()?;
            let bytes = std::fs::read(executable).ok()?;
            Some(crate::tun_transaction::sha256_revision(&bytes))
        })
        .clone()
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
        | DaemonCommand::ApplySystemTunSnapshot { token, .. }
        | DaemonCommand::PromoteSystemConfig { token, .. }
        | DaemonCommand::SelectSystemProxy { token, .. }
        | DaemonCommand::StopCore { token }
        | DaemonCommand::DisableTun { token }
        | DaemonCommand::GetStatus { token }
        | DaemonCommand::CoreApiRequest { token, .. }
        | DaemonCommand::SetAutostart { token, .. }
        | DaemonCommand::ValidatePreparedRuntime { token, .. }
        | DaemonCommand::ApplyPromotedSnapshot { token, .. }
        | DaemonCommand::QuiesceCandidateRuntime { token, .. }
        | DaemonCommand::RestoreOldRuntime { token, .. }
        | DaemonCommand::AttestCurrentTransaction { token, .. }
        | DaemonCommand::ApplyLegacyRecoveryTarget { token, .. }
        | DaemonCommand::GetTransactionStatus { token } => token.as_deref(),
    };
    if let Err(message) = validate_client_token_for_peer(client_token, peer_uid) {
        return DaemonResponse::Error { message };
    }
    if matches!(
        cmd,
        DaemonCommand::ApplySystemTunSnapshot { .. } | DaemonCommand::DisableTun { .. }
    ) {
        if let Err(message) = validate_tun_peer_is_root(peer_uid) {
            return DaemonResponse::Error { message };
        }
    }

    // SPEC §12.4: Phase command allowlist gate for active transaction
    if let Some(ctx) = daemon_transaction_context() {
        match crate::tun_transaction::read_active_journal(&ctx) {
            Ok(Some(journal)) => {
                if !matches!(
                    journal.phase,
                    crate::tun_transaction::JournalPhase::IntentCommitted
                        | crate::tun_transaction::JournalPhase::RolledBack
                ) {
                    let is_allowed_txn_cmd = match &cmd {
                        DaemonCommand::ValidatePreparedRuntime { fence, .. } => {
                            fence.transaction_id == journal.transaction_id
                                && fence.generation == journal.generation
                                && journal.phase == crate::tun_transaction::JournalPhase::Prepared
                        }
                        DaemonCommand::ApplyPromotedSnapshot { fence, .. } => {
                            fence.transaction_id == journal.transaction_id
                                && fence.generation == journal.generation
                                && journal.phase
                                    == crate::tun_transaction::JournalPhase::SnapshotPromoted
                        }
                        DaemonCommand::QuiesceCandidateRuntime { fence, .. } => {
                            fence.transaction_id == journal.transaction_id
                                && fence.generation == journal.generation
                                && fence.expected_candidate_revision == journal.candidate_revision
                                && journal.phase
                                    == crate::tun_transaction::JournalPhase::RollbackPending
                        }
                        DaemonCommand::RestoreOldRuntime { fence, .. } => {
                            fence.transaction_id == journal.transaction_id
                                && fence.generation == journal.generation
                                && journal.phase
                                    == crate::tun_transaction::JournalPhase::RollbackPending
                        }
                        DaemonCommand::AttestCurrentTransaction { fence, .. } => {
                            fence.transaction_id == journal.transaction_id
                                && fence.generation == journal.generation
                                && fence.expected_candidate_revision == journal.candidate_revision
                                && journal.phase
                                    == crate::tun_transaction::JournalPhase::CoreApplied
                        }
                        DaemonCommand::ApplyLegacyRecoveryTarget { fence, .. } => {
                            fence.transaction_id == journal.transaction_id
                                && fence.generation == journal.generation
                                && journal.phase
                                    == crate::tun_transaction::JournalPhase::RecoveryRequired
                        }
                        DaemonCommand::GetTransactionStatus { .. }
                        | DaemonCommand::GetStatus { .. } => true,
                        _ => false,
                    };

                    if !is_allowed_txn_cmd {
                        return DaemonResponse::Error {
                            message: format!(
                                "A system TUN transaction ({}) is in progress (phase: {:?}).
                             Inspect status or restart the service:
                               mihomo-cli status
                               mihomo-cli restart --system",
                                journal.transaction_id, journal.phase
                            ),
                        };
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                let is_allowed = allowed_with_unreadable_journal(&cmd);
                if !is_allowed {
                    return DaemonResponse::Error {
                        message: format!(
                            "Active system TUN transaction journal is unreadable or corrupted: {}.
                             Inspect diagnostics:
                               mihomo-cli status",
                            e
                        ),
                    };
                }
            }
        }
    }
    match cmd {
        DaemonCommand::GetStatus { .. } => {
            let mut s = state.lock().await;
            reap_exited_core(&mut s);
            let (tun_journal_state, tun_journal_error) = active_journal_status();
            DaemonResponse::Status {
                running: s.core_running,
                core_pid: s.core_pid,
                config_path: s.config_path.clone(),
                tun_snapshot_revision: managed_system_tun_snapshot_path()
                    .ok()
                    .and_then(|path| crate::ipc::managed_snapshot_revision(&path).ok()),
                launched_config_revision: s.launched_config_revision.clone(),
                autostart_enabled: daemon_config_dir().join("autostart").exists(),
                daemon_executable_revision: daemon_executable_revision(),
                tun_journal_state,
                tun_journal_error,
            }
        }
        DaemonCommand::CoreApiRequest {
            method, path, body, ..
        } => process_core_api_request(method, path, body, state, peer_uid).await,
        DaemonCommand::PromoteSystemConfig {
            config_content,
            config_revision,
            selection_intent_dir,
            subscription_id,
            ..
        } => {
            promote_system_config(
                state,
                config_content,
                config_revision,
                selection_intent_dir,
                subscription_id,
            )
            .await
        }
        DaemonCommand::SelectSystemProxy { group, node, .. } => {
            select_system_proxy(state, group, node).await
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
        DaemonCommand::StartCore {
            config_content,
            config_revision,
            selection_intent_dir,
            subscription_id,
            ..
        } => {
            promote_system_config(
                state,
                config_content,
                config_revision,
                selection_intent_dir,
                subscription_id,
            )
            .await
        }
        DaemonCommand::StopCore { .. } => stop_core(state).await,
        DaemonCommand::RestartCore {
            config_content,
            config_revision,
            selection_intent_dir,
            subscription_id,
            ..
        } => {
            promote_system_config(
                state,
                config_content,
                config_revision,
                selection_intent_dir,
                subscription_id,
            )
            .await
        }
        DaemonCommand::ApplySystemTunSnapshot { .. } | DaemonCommand::DisableTun { .. } => {
            let mut s = state.lock().await;
            reap_exited_core(&mut s);
            if !s.core_running {
                return DaemonResponse::Error {
                    message: "system Core is not running. Fix: mihomo-cli restart --system"
                        .to_string(),
                };
            }
            DaemonResponse::Error {
                message: "legacy TUN IPC command is deprecated; use transaction-based tun on/off"
                    .to_string(),
            }
        }
        DaemonCommand::ValidatePreparedRuntime {
            fence,
            expected_old_runtime_revision,
            expected_old_runtime_tun,
            ..
        } => {
            handle_validate_prepared_runtime(
                state,
                fence,
                expected_old_runtime_revision,
                expected_old_runtime_tun,
                peer_uid,
            )
            .await
        }
        DaemonCommand::ApplyPromotedSnapshot {
            fence,
            target_runtime_tun,
            ..
        } => handle_apply_promoted_snapshot(state, fence, target_runtime_tun, peer_uid).await,
        DaemonCommand::QuiesceCandidateRuntime { fence, .. } => {
            handle_quiesce_candidate_runtime(state, fence, peer_uid).await
        }
        DaemonCommand::RestoreOldRuntime {
            fence,
            expected_old_runtime_revision,
            expected_old_runtime_tun,
            ..
        } => {
            handle_restore_old_runtime(
                state,
                fence,
                expected_old_runtime_revision,
                expected_old_runtime_tun,
                peer_uid,
            )
            .await
        }
        DaemonCommand::AttestCurrentTransaction {
            fence,
            expected_runtime_revision,
            expected_runtime_tun,
            ..
        } => {
            handle_attest_current_transaction(
                state,
                fence,
                expected_runtime_revision,
                expected_runtime_tun,
                peer_uid,
            )
            .await
        }
        DaemonCommand::ApplyLegacyRecoveryTarget {
            fence,
            expected_recovery_target_revision,
            ..
        } => {
            handle_apply_legacy_recovery_target(
                state,
                fence,
                expected_recovery_target_revision,
                peer_uid,
            )
            .await
        }
        DaemonCommand::GetTransactionStatus { .. } => handle_get_transaction_status(state).await,
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
            s.launched_config_revision = metadata.config_revision.clone();
        } else {
            s.core_running = false;
            s.core_pid = None;
            s.launched_config_revision = None;
            remove_pid_file(&s.pid_file);
        }
        return;
    };
    match child.try_wait() {
        Ok(Some(_)) | Err(_) => {
            s.core_child = None;
            s.core_running = false;
            s.core_pid = None;
            s.launched_config_revision = None;
            // Preserve the last successful start intent. ADR-23 uses these
            // values to recover a crashed core on the next command.
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct CorePidMetadata {
    pid: u32,
    config_path: PathBuf,
    core_binary: PathBuf,
    api_endpoint: Option<String>,
    #[serde(default)]
    config_revision: Option<String>,
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
            config_revision: None,
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
        return Err(format!(
            "core binary not found: {}. Fix: mihomo-cli install --system",
            core_binary.display()
        ));
    }
    if !config_path.exists() {
        return Err(format!(
            "config not found: {}. Fix: mihomo-cli config",
            config_path.display()
        ));
    }
    read_required_system_core_api_endpoint(config_path)
}

/// Read the last N lines of a log file. Returns empty string if file doesn't exist.
fn read_log_tail(log_file: &std::path::Path, n: usize) -> String {
    let content = match std::fs::read_to_string(log_file) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    content
        .lines()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Classification of core startup failures (ADR-24).
#[derive(Debug, Clone, PartialEq, Eq)]
enum CoreFailure {
    /// GeoIP MMDB file missing — auto-fixable by downloading.
    GeoipMissing,
    /// GeoSite.dat file missing — auto-fixable by downloading.
    GeositeMissing,
    /// Config syntax error — report with details, don't auto-fix.
    ConfigError { detail: String },
    /// Port already in use — report with details.
    PortConflict { detail: String },
    /// Permission denied — report with guidance.
    PermissionDenied { detail: String },
    /// Unknown failure — report last N lines of log.
    Unknown { log_tail: String },
}

/// Classify a core startup failure from its log output (ADR-24).
fn classify_core_startup_failure(log_tail: &str) -> CoreFailure {
    let lower = log_tail.to_lowercase();

    if lower.contains("can't find mmdb")
        || lower.contains("can't download mmdb")
        || lower.contains("geoip") && lower.contains("no such file")
    {
        return CoreFailure::GeoipMissing;
    }

    if lower.contains("can't find geosite")
        || lower.contains("geosite") && lower.contains("no such file")
    {
        return CoreFailure::GeositeMissing;
    }

    if lower.contains("permission denied")
        && lower.contains("/var/lib/mihomo-cli")
        && (lower.contains("geoip.metadb") || lower.contains("geosite.dat"))
    {
        let detail = log_tail
            .lines()
            .find(|line| line.to_lowercase().contains("permission denied"))
            .unwrap_or("permission denied")
            .to_string();
        return CoreFailure::PermissionDenied { detail };
    }

    if let Some(pos) = lower.rfind("parse config error") {
        let detail = log_tail[pos..]
            .lines()
            .next()
            .unwrap_or("unknown config error");
        return CoreFailure::ConfigError {
            detail: detail.to_string(),
        };
    }

    if lower.contains("address already in use") {
        let detail = log_tail
            .lines()
            .find(|l| l.to_lowercase().contains("address already in use"))
            .unwrap_or("address already in use")
            .to_string();
        return CoreFailure::PortConflict { detail };
    }

    if lower.contains("permission denied") {
        let detail = log_tail
            .lines()
            .find(|l| l.to_lowercase().contains("permission denied"))
            .unwrap_or("permission denied")
            .to_string();
        return CoreFailure::PermissionDenied { detail };
    }

    CoreFailure::Unknown {
        log_tail: log_tail.to_string(),
    }
}

/// Try to auto-download missing geo data and retry core start (ADR-24).
/// Returns the retry result, or the original error if recovery doesn't apply.
#[cfg(unix)]
async fn try_geo_recovery_and_retry(
    state: &Arc<Mutex<DaemonState>>,
    failure: &CoreFailure,
    config_path: PathBuf,
    core_binary: PathBuf,
    original_error: String,
) -> DaemonResponse {
    let config_dir = runtime_data_dir_for_config(&config_path);

    match failure {
        CoreFailure::GeoipMissing => {
            let dest = config_dir.join("geoip.metadb");
            match crate::installer::download_geo_file(crate::installer::GEOIP_URL, &dest).await {
                Ok(()) => {
                    crate::log!("Auto-downloaded geoip.metadb to {}", dest.display());
                    Box::pin(start_core(Arc::clone(state), config_path, core_binary)).await
                }
                Err(e) => DaemonResponse::Error {
                    message: format!(
                        "GeoIP MMDB data missing and auto-download failed: {e}\n  \
                         Fix: mihomo-cli install --system --force"
                    ),
                },
            }
        }
        CoreFailure::GeositeMissing => {
            let dest = config_dir.join("GeoSite.dat");
            match crate::installer::download_geo_file(crate::installer::GEOSITE_URL, &dest).await {
                Ok(()) => {
                    crate::log!("Auto-downloaded GeoSite.dat to {}", dest.display());
                    Box::pin(start_core(Arc::clone(state), config_path, core_binary)).await
                }
                Err(e) => DaemonResponse::Error {
                    message: format!(
                        "GeoSite data missing and auto-download failed: {e}\n  \
                         Fix: mihomo-cli install --system --force"
                    ),
                },
            }
        }
        _ => DaemonResponse::Error {
            message: original_error,
        },
    }
}

/// Format a CoreFailure into a user-facing error message (ADR-24).
fn format_core_failure_message(failure: &CoreFailure, log_file: &std::path::Path) -> String {
    match failure {
        CoreFailure::GeoipMissing => {
            "GeoIP MMDB data missing — auto-download should have been attempted. \
             Fix: mihomo-cli install --system --force (re-downloads geo data)"
                .to_string()
        }
        CoreFailure::GeositeMissing => {
            "GeoSite data missing. Auto-downloading is not yet implemented in daemon. \
             Fix: mihomo-cli install --system --force (re-downloads geo data)"
                .to_string()
        }
        CoreFailure::ConfigError { detail } => {
            format!("core config error: {detail}\n  Fix: mihomo-cli config --validate")
        }
        CoreFailure::PortConflict { detail } => {
            format!("{detail}\n  Fix: stop the conflicting process or change the port in config")
        }
        CoreFailure::PermissionDenied { detail }
            if detail.contains("/var/lib/mihomo-cli")
                && (detail.contains("geoip.metadb") || detail.contains("GeoSite.dat")) =>
        {
            format!(
                "{detail}\n  Fix: system Mihomo data directory permissions are invalid. \
                 Run: mihomo-cli install --system --force"
            )
        }
        CoreFailure::PermissionDenied { detail } => {
            format!("{detail}\n  Fix: check file permissions for mihomo binary and config")
        }
        CoreFailure::Unknown { log_tail } => {
            let last_lines: String = log_tail
                .lines()
                .rev()
                .take(10)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "core failed to start. Last log entries:\n{last_lines}\n  Logs: {}",
                log_file.display()
            )
        }
    }
}

#[cfg(windows)]
fn early_exit_message(
    _status: std::process::ExitStatus,
    _core_binary: &std::path::Path,
    log_file: &std::path::Path,
) -> String {
    let log_tail = read_log_tail(log_file, 40);
    format_core_failure_message(&classify_core_startup_failure(&log_tail), log_file)
}

#[cfg(any(unix, windows))]
fn system_runtime_data_dir() -> PathBuf {
    crate::instance::planned_current_context(crate::instance::InstanceMode::System)
        .and_then(|ctx| ctx.paths.tun_config_file.parent().map(PathBuf::from))
        .unwrap_or_else(|| {
            if cfg!(target_os = "windows") {
                std::env::var_os("ProgramData")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
                    .join("mihomo-cli")
            } else if cfg!(target_os = "macos") {
                PathBuf::from("/Library/Application Support/mihomo-cli")
            } else {
                PathBuf::from("/var/lib/mihomo-cli")
            }
        })
}

#[cfg(any(unix, windows))]
fn system_runtime_config_path(config_path: &std::path::Path) -> anyhow::Result<PathBuf> {
    let runtime_dir = system_runtime_data_dir();
    if config_path == runtime_dir.join("tun-config.yaml") {
        return Ok(config_path.to_path_buf());
    }
    let active_path = runtime_dir.join("active-config.yaml");
    if config_path != active_path {
        let bytes = crate::utils::read_file_no_follow_limited(
            config_path,
            crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
        )?;
        crate::utils::atomic_write_bytes_no_follow(&active_path, &bytes, 0o640)?;
    }
    Ok(active_path)
}

#[cfg(any(unix, windows))]
fn runtime_data_dir_for_config(config_path: &std::path::Path) -> PathBuf {
    let system_dir = system_runtime_data_dir();
    if config_path.starts_with(&system_dir) {
        system_dir
    } else {
        config_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }
}

#[cfg(any(unix, windows))]
fn core_command_args(
    config_path: &std::path::Path,
    runtime_data_dir: &std::path::Path,
) -> Vec<std::ffi::OsString> {
    vec![
        "-d".into(),
        runtime_data_dir.into(),
        "-f".into(),
        config_path.into(),
    ]
}

#[cfg(any(unix, windows))]
fn config_content_revision(path: &std::path::Path) -> anyhow::Result<String> {
    let bytes = crate::utils::read_file_no_follow_limited(
        path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    )?;
    Ok(crate::tun_transaction::content_revision(&bytes))
}

#[cfg(unix)]
fn remove_stale_unix_endpoint(endpoint: &str) {
    if let Some(path) = endpoint_unix_path(endpoint) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
async fn promote_system_config(
    state: Arc<Mutex<DaemonState>>,
    config_content: String,
    config_revision: String,
    selection_intent_dir: Option<String>,
    subscription_id: Option<String>,
) -> DaemonResponse {
    if config_content.is_empty() || config_content.len() > 16 * 1024 * 1024 {
        return DaemonResponse::Error {
            message: "system configuration payload is empty or too large".to_string(),
        };
    }
    if crate::tun_transaction::sha256_revision(config_content.as_bytes()) != config_revision {
        return DaemonResponse::Error {
            message: "system configuration revision does not match its payload".to_string(),
        };
    }
    let Some(transaction_ctx) = daemon_transaction_context() else {
        return DaemonResponse::Error {
            message: "system instance paths are unavailable".to_string(),
        };
    };

    // Check if active transaction exists
    match crate::tun_transaction::read_active_journal(&transaction_ctx) {
        Ok(Some(journal)) => {
            if !matches!(
                journal.phase,
                crate::tun_transaction::JournalPhase::IntentCommitted
                    | crate::tun_transaction::JournalPhase::RolledBack
            ) {
                return DaemonResponse::Error {
                    message: format!(
                        "A system TUN transaction ({}) is in progress (phase: {:?}).
                     Inspect status or restart the service:
                       mihomo-cli status
                       mihomo-cli restart --system",
                        journal.transaction_id, journal.phase
                    ),
                };
            }
        }
        Ok(None) => {}
        Err(e) => {
            return DaemonResponse::Error {
                message: format!(
                    "Active system TUN transaction journal is unreadable: {}.
                     Inspect status:
                       mihomo-cli status",
                    e
                ),
            };
        }
    }
    let is_tun_snapshot = {
        let mut s = state.lock().await;
        reap_exited_core(&mut s);
        s.config_path.as_ref() == Some(&transaction_ctx.paths.tun_config_file)
    };

    let selection_intent_dir =
        match validate_optional_selection_intent_dir(selection_intent_dir.as_deref()) {
            Ok(path) => path,
            Err(message) => return DaemonResponse::Error { message },
        };
    let selection_scope = match (selection_intent_dir.as_deref(), subscription_id.as_deref()) {
        (Some(dir), Some(id)) => {
            let paths = crate::utils::AppPaths::new(dir.to_path_buf());
            match crate::config::get_active_id_at(&paths) {
                Ok(Some(active)) if active == id => Some((dir.to_path_buf(), id.to_string())),
                Ok(Some(active)) => {
                    return DaemonResponse::Error {
                        message: format!(
                        "selection subscription identity mismatch: active={active}, request={id}"
                    ),
                    }
                }
                Ok(None) => {
                    return DaemonResponse::Error {
                        message: "selection replay requires an active subscription".to_string(),
                    }
                }
                Err(error) => {
                    return DaemonResponse::Error {
                        message: format!(
                            "cannot validate active subscription for selection replay: {error}"
                        ),
                    }
                }
            }
        }
        (Some(_), None) => {
            return DaemonResponse::Error {
                message: "selection replay requires subscription_id".to_string(),
            }
        }
        _ => None,
    };

    let target_path = if is_tun_snapshot {
        transaction_ctx.paths.tun_config_file.clone()
    } else {
        crate::tun_transaction::active_config_path(&transaction_ctx)
    };

    let endpoint = match read_api_endpoint_from_content(&config_content) {
        Some(endpoint) => endpoint,
        None => {
            return DaemonResponse::Error {
                message: "system configuration is missing the managed Core API endpoint"
                    .to_string(),
            }
        }
    };
    if let Err(message) = validate_system_core_api_endpoint(&endpoint) {
        return DaemonResponse::Error { message };
    }
    if let Err(error) = serde_yaml::from_str::<serde_yaml::Value>(&config_content) {
        return DaemonResponse::Error {
            message: format!("system configuration YAML is invalid: {error}"),
        };
    }

    if let Err(e) = std::fs::write(&target_path, config_content.as_bytes()) {
        return DaemonResponse::Error {
            message: format!("failed to persist promoted system configuration: {e}"),
        };
    }

    let core_binary = expected_system_core_binary_path();
    if let Err(message) = preflight_system_core_start_request(&target_path, &core_binary) {
        return DaemonResponse::Error { message };
    }

    let _ = stop_core(Arc::clone(&state)).await;
    match start_core(Arc::clone(&state), target_path.clone(), core_binary).await {
        DaemonResponse::Success { .. } => {
            let mut state_guard = state.lock().await;
            reap_exited_core(&mut state_guard);
            let runtime_revision = state_guard
                .launched_config_revision
                .clone()
                .unwrap_or_default();
            drop(state_guard);
            if runtime_revision != config_revision {
                return DaemonResponse::Error {
                    message: "system configuration was loaded but its runtime revision could not be attested"
                        .to_string(),
                };
            }
            let mut message = "system configuration promoted and runtime applied".to_string();
            // D6: replay persisted selection intent synchronously (≤5s budget)
            // so start/restart report restored selections in the same response.
            if let Some((intent_dir, subscription_id)) = selection_scope {
                persist_selection_intent_dir(&intent_dir);
                for line in replay_selection_intent(&state, &intent_dir, &subscription_id).await {
                    message.push('\n');
                    message.push_str(&line);
                }
            }
            DaemonResponse::Success { message }
        }
        DaemonResponse::Error { message } => DaemonResponse::Error { message },
        response => response,
    }
}

// ---------------- Transaction IPC Handlers ----------------

#[cfg(unix)]
async fn get_runtime_observation(
    state: &Arc<Mutex<DaemonState>>,
) -> crate::tun_transaction::RuntimeObservation {
    let mut s = state.lock().await;
    reap_exited_core(&mut s);
    let core_running = s.core_running;
    let core_pid = s.core_pid;
    let launched_revision = s.launched_config_revision.clone();
    let core_binary = s.core_binary.clone();
    let api_endpoint_str = s.api_endpoint.clone();
    drop(s);

    let (api_ready, runtime_tun) =
        if let (true, Some(api_endpoint)) = (core_running, api_endpoint_str.as_deref()) {
            let ep = ApiEndpoint::UnixSocket(
                endpoint_unix_path(api_endpoint)
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(api_endpoint)),
            );
            let client = mihomo_api::EndpointMihomoApiClient::new(ep);
            match client.get("/configs").await {
                Ok(val) => {
                    let tun_enabled = val
                        .get("tun")
                        .and_then(|v| v.get("enable"))
                        .and_then(|v| v.as_bool());
                    (true, tun_enabled)
                }
                Err(_) => (false, None),
            }
        } else {
            (false, None)
        };

    let core_identity = core_binary
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "mihomo".to_string());

    crate::tun_transaction::RuntimeObservation {
        core_running,
        core_identity: Some(core_identity),
        core_pid,
        launched_revision,
        runtime_tun,
        api_ready,
    }
}

#[cfg(unix)]
async fn handle_validate_prepared_runtime(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    expected_old_runtime_revision: String,
    expected_old_runtime_tun: bool,
    peer_uid: Option<u32>,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer_uid) {
        return DaemonResponse::Error { message };
    }
    let Some(ctx) = daemon_transaction_context() else {
        return DaemonResponse::Error {
            message: "system instance paths are unavailable".to_string(),
        };
    };
    let journal = match crate::tun_transaction::read_active_journal(&ctx) {
        Ok(Some(j)) => j,
        Ok(None) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Stale {
                    observed_transaction_id: None,
                    observed_generation: None,
                    observed_phase: None,
                },
            };
        }
        Err(e) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Unavailable {
                    observation: None,
                    error: crate::tun_transaction::StructuredError {
                        code:
                            crate::tun_transaction::TransactionErrorCode::UnsupportedJournalSchema,
                        stage: "read_journal".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != crate::tun_transaction::JournalPhase::Prepared
    {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Stale {
                observed_transaction_id: Some(journal.transaction_id),
                observed_generation: Some(journal.generation),
                observed_phase: Some(journal.phase),
            },
        };
    }

    let obs = get_runtime_observation(&state).await;
    if !obs.core_running || !obs.api_ready {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Unavailable {
                observation: Some(obs),
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::ApiUnavailable,
                    stage: "validate_prepared_runtime".to_string(),
                    retryable: true,
                    message: "core is not running or API is not ready".to_string(),
                },
            },
        };
    }

    if obs.launched_revision.as_deref() != Some(&expected_old_runtime_revision)
        || obs.runtime_tun != Some(expected_old_runtime_tun)
    {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs.clone(),
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                    stage: "validate_prepared_runtime".to_string(),
                    retryable: false,
                    message: format!(
                        "current runtime revision/tun ({:?}/{:?}) does not match expected ({:?}/{:?})",
                        obs.launched_revision, obs.runtime_tun, expected_old_runtime_revision, expected_old_runtime_tun
                    ),
                },
            },
        };
    }

    DaemonResponse::Transaction {
        response: crate::tun_transaction::TransactionResponse::Completed(
            crate::tun_transaction::RuntimeProof {
                transaction_id: fence.transaction_id,
                generation: fence.generation,
                observed_phase: crate::tun_transaction::JournalPhase::Prepared,
                proof_kind: crate::tun_transaction::RuntimeProofKind::OldRuntimeValidated,
                core_identity: obs.core_identity.unwrap_or_default(),
                core_pid: obs.core_pid.unwrap_or_default(),
                launched_revision: obs.launched_revision.unwrap_or_default(),
                runtime_tun: obs.runtime_tun.unwrap_or(false),
                api_ready: true,
            },
        ),
    }
}

#[cfg(unix)]
async fn handle_apply_promoted_snapshot(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    target_runtime_tun: bool,
    peer_uid: Option<u32>,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer_uid) {
        return DaemonResponse::Error { message };
    }
    let Some(ctx) = daemon_transaction_context() else {
        return DaemonResponse::Error {
            message: "system instance paths are unavailable".to_string(),
        };
    };
    let journal = match crate::tun_transaction::read_active_journal(&ctx) {
        Ok(Some(j)) => j,
        Ok(None) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Stale {
                    observed_transaction_id: None,
                    observed_generation: None,
                    observed_phase: None,
                },
            };
        }
        Err(e) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Unavailable {
                    observation: None,
                    error: crate::tun_transaction::StructuredError {
                        code:
                            crate::tun_transaction::TransactionErrorCode::UnsupportedJournalSchema,
                        stage: "read_journal".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != crate::tun_transaction::JournalPhase::SnapshotPromoted
    {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Stale {
                observed_transaction_id: Some(journal.transaction_id),
                observed_generation: Some(journal.generation),
                observed_phase: Some(journal.phase),
            },
        };
    }

    // Read and validate candidate file
    let candidate_path = crate::tun_transaction::active_candidate_path(&ctx);
    let candidate_bytes = match crate::utils::read_file_no_follow_limited(
        &candidate_path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    ) {
        Ok(b) => b,
        Err(e) => {
            let obs = get_runtime_observation(&state).await;
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                        stage: "read_candidate".to_string(),
                        retryable: false,
                        message: format!("cannot read candidate artifact: {e}"),
                    },
                },
            };
        }
    };
    let candidate_rev = crate::tun_transaction::sha256_revision(&candidate_bytes);
    if candidate_rev != fence.expected_candidate_revision {
        let obs = get_runtime_observation(&state).await;
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::ArtifactRevisionMismatch,
                    stage: "validate_candidate".to_string(),
                    retryable: false,
                    message: "candidate file hash does not match fence expected revision"
                        .to_string(),
                },
            },
        };
    }

    // Validate candidate tun.enable
    let candidate_yaml: serde_yaml::Value = match serde_yaml::from_slice(&candidate_bytes) {
        Ok(y) => y,
        Err(e) => {
            let obs = get_runtime_observation(&state).await;
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                        stage: "parse_candidate".to_string(),
                        retryable: false,
                        message: format!("candidate is invalid yaml: {e}"),
                    },
                },
            };
        }
    };
    let candidate_tun = candidate_yaml
        .get("tun")
        .and_then(|t| t.get("enable"))
        .and_then(|e| e.as_bool())
        .unwrap_or(false);
    if candidate_tun != target_runtime_tun {
        let obs = get_runtime_observation(&state).await;
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::RuntimeTunMismatch,
                    stage: "validate_candidate_tun".to_string(),
                    retryable: false,
                    message: format!(
                        "candidate tun.enable ({}) != target_runtime_tun ({})",
                        candidate_tun, target_runtime_tun
                    ),
                },
            },
        };
    }

    // Validate snapshot matches candidate revision
    let snapshot_path = &ctx.paths.tun_config_file;
    let snapshot_bytes = match crate::utils::read_file_no_follow_limited(
        snapshot_path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    ) {
        Ok(b) => b,
        Err(e) => {
            let obs = get_runtime_observation(&state).await;
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::SnapshotConflict,
                        stage: "read_snapshot".to_string(),
                        retryable: false,
                        message: format!("snapshot file not readable: {e}"),
                    },
                },
            };
        }
    };
    if crate::tun_transaction::sha256_revision(&snapshot_bytes) != candidate_rev {
        let obs = get_runtime_observation(&state).await;
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::SnapshotConflict,
                    stage: "validate_snapshot".to_string(),
                    retryable: false,
                    message: "snapshot file has not been promoted to candidate revision"
                        .to_string(),
                },
            },
        };
    }

    // Read and verify immutable old-runtime evidence before applying candidate
    let old_runtime = match crate::tun_transaction::read_and_validate_old_runtime(&ctx, &journal) {
        Ok(ev) => ev,
        Err(e) => {
            let obs = get_runtime_observation(&state).await;
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                        stage: "validate_old_runtime_for_apply".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    // Check if already satisfied (Idempotency, SPEC §12.5)
    let obs = get_runtime_observation(&state).await;
    if obs.core_running
        && obs.api_ready
        && obs.launched_revision.as_deref() == Some(&candidate_rev)
        && obs.runtime_tun == Some(target_runtime_tun)
    {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::AlreadySatisfied(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id,
                    generation: fence.generation,
                    observed_phase: crate::tun_transaction::JournalPhase::SnapshotPromoted,
                    proof_kind: crate::tun_transaction::RuntimeProofKind::CandidateApplied,
                    core_identity: obs.core_identity.unwrap_or_default(),
                    core_pid: obs.core_pid.unwrap_or_default(),
                    launched_revision: candidate_rev,
                    runtime_tun: target_runtime_tun,
                    api_ready: true,
                },
            ),
        };
    }

    // If core is running, only stop if it matches the attested old runtime
    if obs.core_running {
        if crate::tun_transaction::runtime_matches_old_evidence(&old_runtime, &obs) {
            let stop_res = stop_core(Arc::clone(&state)).await;
            if !matches!(stop_res, DaemonResponse::Success { .. }) {
                let message = match stop_res {
                    DaemonResponse::Error { message } => message,
                    other => format!("unexpected stop response: {other:?}"),
                };
                return DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                        observation: get_runtime_observation(&state).await,
                        error: crate::tun_transaction::StructuredError {
                            code: crate::tun_transaction::TransactionErrorCode::CoreStopFailed,
                            stage: "stop_old_core".to_string(),
                            retryable: false,
                            message,
                        },
                    },
                };
            }
            let obs_stopped = get_runtime_observation(&state).await;
            if obs_stopped.core_running {
                return DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                        observation: obs_stopped,
                        error: crate::tun_transaction::StructuredError {
                            code: crate::tun_transaction::TransactionErrorCode::CoreStopFailed,
                            stage: "verify_old_core_stopped".to_string(),
                            retryable: false,
                            message: "old core is still running after stop request".to_string(),
                        },
                    },
                };
            }
        } else {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                        stage: "verify_running_core_before_apply".to_string(),
                        retryable: false,
                        message: "running core does not match attested old runtime; refusing to stop unknown runtime".to_string(),
                    },
                },
            };
        }
    }

    // Start core from snapshot path
    let core_binary = expected_system_core_binary_path();
    let start_res = start_core(Arc::clone(&state), snapshot_path.clone(), core_binary).await;
    let obs_after = get_runtime_observation(&state).await;

    match start_res {
        DaemonResponse::Success { .. } => {
            if obs_after.core_running
                && obs_after.api_ready
                && obs_after.launched_revision.as_deref() == Some(&candidate_rev)
                && obs_after.runtime_tun == Some(target_runtime_tun)
            {
                DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::Completed(
                        crate::tun_transaction::RuntimeProof {
                            transaction_id: fence.transaction_id,
                            generation: fence.generation,
                            observed_phase: crate::tun_transaction::JournalPhase::SnapshotPromoted,
                            proof_kind: crate::tun_transaction::RuntimeProofKind::CandidateApplied,
                            core_identity: obs_after.core_identity.unwrap_or_default(),
                            core_pid: obs_after.core_pid.unwrap_or_default(),
                            launched_revision: candidate_rev,
                            runtime_tun: target_runtime_tun,
                            api_ready: true,
                        },
                    ),
                }
            } else {
                DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                        observation: obs_after,
                        error: crate::tun_transaction::StructuredError {
                            code: crate::tun_transaction::TransactionErrorCode::RuntimeTunMismatch,
                            stage: "attest_candidate_after_start".to_string(),
                            retryable: false,
                            message:
                                "core started but runtime revision or tun did not match target"
                                    .to_string(),
                        },
                    },
                }
            }
        }
        DaemonResponse::Error { message } => DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                observation: obs_after,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::CoreStartFailed,
                    stage: "start_core".to_string(),
                    retryable: false,
                    message,
                },
            },
        },
        _ => DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                observation: obs_after,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::CoreStartFailed,
                    stage: "start_core".to_string(),
                    retryable: false,
                    message: "unexpected daemon response starting core".to_string(),
                },
            },
        },
    }
}

#[cfg(unix)]
async fn handle_quiesce_candidate_runtime(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    peer_uid: Option<u32>,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer_uid) {
        return DaemonResponse::Error { message };
    }
    let Some(ctx) = daemon_transaction_context() else {
        return DaemonResponse::Error {
            message: "system instance paths are unavailable".to_string(),
        };
    };
    let journal = match crate::tun_transaction::read_active_journal(&ctx) {
        Ok(Some(j)) => j,
        Ok(None) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Stale {
                    observed_transaction_id: None,
                    observed_generation: None,
                    observed_phase: None,
                },
            };
        }
        Err(e) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Unavailable {
                    observation: None,
                    error: crate::tun_transaction::StructuredError {
                        code:
                            crate::tun_transaction::TransactionErrorCode::UnsupportedJournalSchema,
                        stage: "read_journal".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != crate::tun_transaction::JournalPhase::RollbackPending
        || journal.candidate_revision != fence.expected_candidate_revision
    {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Stale {
                observed_transaction_id: Some(journal.transaction_id),
                observed_generation: Some(journal.generation),
                observed_phase: Some(journal.phase),
            },
        };
    }

    let obs = get_runtime_observation(&state).await;
    if !obs.core_running {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::AlreadySatisfied(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id,
                    generation: fence.generation,
                    observed_phase: crate::tun_transaction::JournalPhase::RollbackPending,
                    proof_kind: crate::tun_transaction::RuntimeProofKind::CandidateQuiesced,
                    core_identity: obs.core_identity.unwrap_or_default(),
                    core_pid: 0,
                    launched_revision: String::new(),
                    runtime_tun: false,
                    api_ready: false,
                },
            ),
        };
    }

    let old_evidence = match crate::tun_transaction::read_and_validate_old_runtime(&ctx, &journal) {
        Ok(evidence) => evidence,
        Err(e) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                        stage: "validate_old_runtime_for_quiesce".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    // If core is running candidate, stop it
    if crate::tun_transaction::runtime_matches_candidate(&journal, &obs) {
        let stop_response = stop_core(Arc::clone(&state)).await;
        let obs_after = get_runtime_observation(&state).await;
        if !matches!(stop_response, DaemonResponse::Success { .. }) || obs_after.core_running {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                    observation: obs_after,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::CoreStopFailed,
                        stage: "quiesce_candidate".to_string(),
                        retryable: false,
                        message: "candidate core did not stop".to_string(),
                    },
                },
            };
        }
        DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Completed(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id,
                    generation: fence.generation,
                    observed_phase: crate::tun_transaction::JournalPhase::RollbackPending,
                    proof_kind: crate::tun_transaction::RuntimeProofKind::CandidateQuiesced,
                    core_identity: obs_after.core_identity.unwrap_or_default(),
                    core_pid: 0,
                    launched_revision: String::new(),
                    runtime_tun: false,
                    api_ready: false,
                },
            ),
        }
    } else if crate::tun_transaction::runtime_matches_old_evidence(&old_evidence, &obs) {
        // The old runtime is fully attested, so the candidate is already
        // quiesced without stopping the old Core.
        DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::AlreadySatisfied(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id,
                    generation: fence.generation,
                    observed_phase: crate::tun_transaction::JournalPhase::RollbackPending,
                    proof_kind: crate::tun_transaction::RuntimeProofKind::CandidateQuiesced,
                    core_identity: old_evidence.core_identity,
                    core_pid: old_evidence.core_pid,
                    launched_revision: old_evidence.launched_revision,
                    runtime_tun: old_evidence.runtime_tun,
                    api_ready: obs.api_ready,
                },
            ),
        }
    } else {
        DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                    stage: "quiesce_candidate".to_string(),
                    retryable: false,
                    message: "running runtime is neither candidate nor fully attested old runtime"
                        .to_string(),
                },
            },
        }
    }
}

#[cfg(unix)]
async fn handle_restore_old_runtime(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    expected_old_runtime_revision: String,
    expected_old_runtime_tun: bool,
    peer_uid: Option<u32>,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer_uid) {
        return DaemonResponse::Error { message };
    }
    let Some(ctx) = daemon_transaction_context() else {
        return DaemonResponse::Error {
            message: "system instance paths are unavailable".to_string(),
        };
    };
    let journal = match crate::tun_transaction::read_active_journal(&ctx) {
        Ok(Some(j)) => j,
        Ok(None) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Stale {
                    observed_transaction_id: None,
                    observed_generation: None,
                    observed_phase: None,
                },
            };
        }
        Err(e) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Unavailable {
                    observation: None,
                    error: crate::tun_transaction::StructuredError {
                        code:
                            crate::tun_transaction::TransactionErrorCode::UnsupportedJournalSchema,
                        stage: "read_journal".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != crate::tun_transaction::JournalPhase::RollbackPending
    {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Stale {
                observed_transaction_id: Some(journal.transaction_id),
                observed_generation: Some(journal.generation),
                observed_phase: Some(journal.phase),
            },
        };
    }

    // Read and verify immutable old-runtime evidence before using it.
    let old_runtime = match crate::tun_transaction::read_and_validate_old_runtime(&ctx, &journal) {
        Ok(ev) => ev,
        Err(e) => {
            let obs = get_runtime_observation(&state).await;
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                        stage: "validate_old_runtime".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    // Check if already restored (must match full old runtime evidence)
    let obs = get_runtime_observation(&state).await;
    if crate::tun_transaction::runtime_matches_old_evidence(&old_runtime, &obs) {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::AlreadySatisfied(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id,
                    generation: fence.generation,
                    observed_phase: crate::tun_transaction::JournalPhase::RollbackPending,
                    proof_kind: crate::tun_transaction::RuntimeProofKind::OldRuntimeRestored,
                    core_identity: old_runtime.core_identity,
                    core_pid: old_runtime.core_pid,
                    launched_revision: old_runtime.launched_revision,
                    runtime_tun: old_runtime.runtime_tun,
                    api_ready: true,
                },
            ),
        };
    }

    // Determine launch source
    let launch_source_path = match old_runtime.launch_source {
        crate::tun_transaction::LaunchSource::SystemActiveConfig => {
            crate::tun_transaction::active_config_path(&ctx)
        }
        crate::tun_transaction::LaunchSource::SystemTunSnapshot => {
            ctx.paths.tun_config_file.clone()
        }
    };

    if !launch_source_path.exists() {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::SnapshotConflict,
                    stage: "check_launch_source".to_string(),
                    retryable: false,
                    message: format!(
                        "old launch source {} does not exist",
                        launch_source_path.display()
                    ),
                },
            },
        };
    }

    let src_bytes = match crate::utils::read_file_no_follow_limited(
        &launch_source_path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    ) {
        Ok(bytes) => bytes,
        Err(e) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                        stage: "read_launch_source".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };
    if crate::tun_transaction::sha256_revision(&src_bytes) != expected_old_runtime_revision {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                    stage: "check_launch_source_revision".to_string(),
                    retryable: false,
                    message: format!(
                        "launch source {} revision does not match expected old revision {}",
                        launch_source_path.display(),
                        expected_old_runtime_revision
                    ),
                },
            },
        };
    }

    // Stop candidate runtime if running and verified to belong to candidate
    if obs.core_running {
        if crate::tun_transaction::runtime_matches_candidate(&journal, &obs) {
            let stop_res = stop_core(Arc::clone(&state)).await;
            if !matches!(stop_res, DaemonResponse::Success { .. }) {
                let message = match stop_res {
                    DaemonResponse::Error { message } => message,
                    other => format!("unexpected stop response: {other:?}"),
                };
                return DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                        observation: get_runtime_observation(&state).await,
                        error: crate::tun_transaction::StructuredError {
                            code: crate::tun_transaction::TransactionErrorCode::CoreStopFailed,
                            stage: "stop_candidate_core".to_string(),
                            retryable: false,
                            message,
                        },
                    },
                };
            }
            let obs_stopped = get_runtime_observation(&state).await;
            if obs_stopped.core_running {
                return DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                        observation: obs_stopped,
                        error: crate::tun_transaction::StructuredError {
                            code: crate::tun_transaction::TransactionErrorCode::CoreStopFailed,
                            stage: "verify_candidate_stopped".to_string(),
                            retryable: false,
                            message: "candidate core is still running after stop request"
                                .to_string(),
                        },
                    },
                };
            }
        } else {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                        stage: "verify_running_core_before_restore".to_string(),
                        retryable: false,
                        message: "running core does not match candidate runtime; refusing to stop unknown runtime".to_string(),
                    },
                },
            };
        }
    }

    // Start core from launch source
    let core_binary = expected_system_core_binary_path();
    let start_res = start_core(Arc::clone(&state), launch_source_path, core_binary).await;
    let obs_after = get_runtime_observation(&state).await;

    match start_res {
        DaemonResponse::Success { .. } => {
            if crate::tun_transaction::runtime_matches_old_evidence(&old_runtime, &obs_after)
                && obs_after.launched_revision.as_deref() == Some(&expected_old_runtime_revision)
                && obs_after.runtime_tun == Some(expected_old_runtime_tun)
            {
                DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::Completed(
                        crate::tun_transaction::RuntimeProof {
                            transaction_id: fence.transaction_id,
                            generation: fence.generation,
                            observed_phase: crate::tun_transaction::JournalPhase::RollbackPending,
                            proof_kind:
                                crate::tun_transaction::RuntimeProofKind::OldRuntimeRestored,
                            core_identity: obs_after.core_identity.unwrap_or_default(),
                            core_pid: obs_after.core_pid.unwrap_or_default(),
                            launched_revision: expected_old_runtime_revision,
                            runtime_tun: expected_old_runtime_tun,
                            api_ready: true,
                        },
                    ),
                }
            } else {
                DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                        observation: obs_after,
                        error: crate::tun_transaction::StructuredError {
                            code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                            stage: "attest_old_runtime_after_start".to_string(),
                            retryable: false,
                            message: "old core started but runtime revision or tun did not match expected".to_string(),
                        },
                    },
                }
            }
        }
        DaemonResponse::Error { message } => DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                observation: obs_after,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::CoreStartFailed,
                    stage: "start_old_core".to_string(),
                    retryable: false,
                    message,
                },
            },
        },
        _ => DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                observation: obs_after,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::CoreStartFailed,
                    stage: "start_old_core".to_string(),
                    retryable: false,
                    message: "unexpected daemon response restoring old core".to_string(),
                },
            },
        },
    }
}

#[cfg(unix)]
async fn handle_attest_current_transaction(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    expected_runtime_revision: String,
    expected_runtime_tun: bool,
    peer_uid: Option<u32>,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer_uid) {
        return DaemonResponse::Error { message };
    }
    let Some(ctx) = daemon_transaction_context() else {
        return DaemonResponse::Error {
            message: "system instance paths are unavailable".to_string(),
        };
    };
    let journal = match crate::tun_transaction::read_active_journal(&ctx) {
        Ok(Some(j)) => j,
        Ok(None) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Stale {
                    observed_transaction_id: None,
                    observed_generation: None,
                    observed_phase: None,
                },
            };
        }
        Err(e) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Unavailable {
                    observation: None,
                    error: crate::tun_transaction::StructuredError {
                        code:
                            crate::tun_transaction::TransactionErrorCode::UnsupportedJournalSchema,
                        stage: "read_journal".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    if journal.transaction_id != fence.transaction_id || journal.generation != fence.generation {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Stale {
                observed_transaction_id: Some(journal.transaction_id),
                observed_generation: Some(journal.generation),
                observed_phase: Some(journal.phase),
            },
        };
    }

    let obs = get_runtime_observation(&state).await;
    if obs.core_running
        && obs.api_ready
        && obs.launched_revision.as_deref() == Some(&expected_runtime_revision)
        && obs.runtime_tun == Some(expected_runtime_tun)
    {
        DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Completed(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id,
                    generation: fence.generation,
                    observed_phase: journal.phase,
                    proof_kind: crate::tun_transaction::RuntimeProofKind::CandidateAttested,
                    core_identity: obs.core_identity.unwrap_or_default(),
                    core_pid: obs.core_pid.unwrap_or_default(),
                    launched_revision: expected_runtime_revision,
                    runtime_tun: expected_runtime_tun,
                    api_ready: true,
                },
            ),
        }
    } else {
        DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                    stage: "attest_current_transaction".to_string(),
                    retryable: false,
                    message: "runtime attestation did not match expected revision or tun state"
                        .to_string(),
                },
            },
        }
    }
}

#[cfg(unix)]
async fn handle_apply_legacy_recovery_target(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    expected_recovery_target_revision: String,
    peer_uid: Option<u32>,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer_uid) {
        return DaemonResponse::Error { message };
    }
    let Some(ctx) = daemon_transaction_context() else {
        return DaemonResponse::Error {
            message: "system instance paths are unavailable".to_string(),
        };
    };
    let journal = match crate::tun_transaction::read_active_journal(&ctx) {
        Ok(Some(j)) => j,
        Ok(None) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Stale {
                    observed_transaction_id: None,
                    observed_generation: None,
                    observed_phase: None,
                },
            };
        }
        Err(e) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Unavailable {
                    observation: None,
                    error: crate::tun_transaction::StructuredError {
                        code:
                            crate::tun_transaction::TransactionErrorCode::UnsupportedJournalSchema,
                        stage: "read_journal".to_string(),
                        retryable: false,
                        message: e.to_string(),
                    },
                },
            };
        }
    };

    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != crate::tun_transaction::JournalPhase::RecoveryRequired
        || !journal.legacy_source
        || journal.rollback_evidence_complete
        || journal.recovery_target_revision.as_deref() != Some(&expected_recovery_target_revision)
    {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Stale {
                observed_transaction_id: Some(journal.transaction_id),
                observed_generation: Some(journal.generation),
                observed_phase: Some(journal.phase),
            },
        };
    }

    let recovery_target_path = crate::tun_transaction::active_recovery_target_path(&ctx);
    let target_bytes = match crate::utils::read_file_no_follow_limited(
        &recovery_target_path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    ) {
        Ok(b) => b,
        Err(e) => {
            let obs = get_runtime_observation(&state).await;
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                        stage: "read_recovery_target".to_string(),
                        retryable: false,
                        message: format!("cannot read recovery-target.yaml: {e}"),
                    },
                },
            };
        }
    };
    if crate::tun_transaction::sha256_revision(&target_bytes) != expected_recovery_target_revision {
        let obs = get_runtime_observation(&state).await;
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::ArtifactRevisionMismatch,
                    stage: "validate_recovery_target".to_string(),
                    retryable: false,
                    message: "recovery-target file hash does not match expected".to_string(),
                },
            },
        };
    }

    let obs = get_runtime_observation(&state).await;
    let expected_target_tun =
        match crate::tun_transaction::parse_tun_enabled_from_bytes(&target_bytes) {
            Ok(value) => value,
            Err(error) => {
                return DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                        observation: obs,
                        error: crate::tun_transaction::StructuredError {
                            code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                            stage: "parse_recovery_target".to_string(),
                            retryable: false,
                            message: format!("recovery target is invalid YAML: {error}"),
                        },
                    },
                };
            }
        };

    if crate::tun_transaction::runtime_matches_recovery_target(
        &journal,
        &obs,
        &expected_recovery_target_revision,
        expected_target_tun,
    ) {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::AlreadySatisfied(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id,
                    generation: fence.generation,
                    observed_phase: crate::tun_transaction::JournalPhase::RecoveryRequired,
                    proof_kind:
                        crate::tun_transaction::RuntimeProofKind::LegacyRecoveryTargetApplied,
                    core_identity: obs.core_identity.unwrap_or_default(),
                    core_pid: obs.core_pid.unwrap_or_default(),
                    launched_revision: expected_recovery_target_revision,
                    runtime_tun: expected_target_tun,
                    api_ready: true,
                },
            ),
        };
    }

    if obs.core_running {
        let is_legacy_candidate = crate::tun_transaction::runtime_matches_candidate(&journal, &obs);
        if !is_legacy_candidate {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                        stage: "verify_running_core_before_legacy_apply".to_string(),
                        retryable: false,
                        message: "running core does not match legacy candidate runtime; refusing to stop unknown runtime".to_string(),
                    },
                },
            };
        }

        let stop_res = stop_core(Arc::clone(&state)).await;
        if !matches!(stop_res, DaemonResponse::Success { .. }) {
            let message = match stop_res {
                DaemonResponse::Error { message } => message,
                other => format!("unexpected stop response: {other:?}"),
            };
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                    observation: get_runtime_observation(&state).await,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::CoreStopFailed,
                        stage: "stop_legacy_candidate_core".to_string(),
                        retryable: false,
                        message,
                    },
                },
            };
        }
        let obs_stopped = get_runtime_observation(&state).await;
        if obs_stopped.core_running {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                    observation: obs_stopped,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::CoreStopFailed,
                        stage: "verify_legacy_candidate_stopped".to_string(),
                        retryable: false,
                        message: "legacy candidate core is still running after stop request"
                            .to_string(),
                    },
                },
            };
        }
    }

    let core_binary = expected_system_core_binary_path();
    let start_res = start_core(Arc::clone(&state), recovery_target_path, core_binary).await;
    let obs_after = get_runtime_observation(&state).await;

    match start_res {
        DaemonResponse::Success { .. } => {
            if obs_after.core_running
                && obs_after.api_ready
                && obs_after
                    .core_identity
                    .as_deref()
                    .is_some_and(|v| !v.is_empty())
                && obs_after.core_pid.is_some_and(|pid| pid > 0)
                && obs_after.launched_revision.as_deref()
                    == Some(&expected_recovery_target_revision)
                && obs_after.runtime_tun == Some(expected_target_tun)
            {
                DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::Completed(
                        crate::tun_transaction::RuntimeProof {
                            transaction_id: fence.transaction_id,
                            generation: fence.generation,
                            observed_phase: crate::tun_transaction::JournalPhase::RecoveryRequired,
                            proof_kind: crate::tun_transaction::RuntimeProofKind::LegacyRecoveryTargetApplied,
                            core_identity: obs_after.core_identity.unwrap_or_default(),
                            core_pid: obs_after.core_pid.unwrap_or_default(),
                            launched_revision: expected_recovery_target_revision,
                            runtime_tun: obs_after.runtime_tun.unwrap_or(false),
                            api_ready: true,
                        },
                    ),
                }
            } else {
                DaemonResponse::Transaction {
                    response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                        observation: obs_after,
                        error: crate::tun_transaction::StructuredError {
                            code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                            stage: "attest_legacy_target_after_start".to_string(),
                            retryable: false,
                            message: "core started but runtime revision did not match recovery target".to_string(),
                        },
                    },
                }
            }
        }
        DaemonResponse::Error { message } => DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                observation: obs_after,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::CoreStartFailed,
                    stage: "start_recovery_core".to_string(),
                    retryable: false,
                    message,
                },
            },
        },
        _ => DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                observation: obs_after,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::CoreStartFailed,
                    stage: "start_recovery_core".to_string(),
                    retryable: false,
                    message: "unexpected daemon response applying recovery target".to_string(),
                },
            },
        },
    }
}

#[cfg(unix)]
fn transaction_status_response(
    obs: crate::tun_transaction::RuntimeObservation,
    journal_result: anyhow::Result<Option<crate::tun_transaction::TunJournal>>,
) -> DaemonResponse {
    let journal = match journal_result {
        Ok(Some(journal)) => journal,
        Ok(None) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Unavailable {
                    observation: Some(obs),
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::ObservationUnavailable,
                        stage: "get_transaction_status".to_string(),
                        retryable: true,
                        message: "no active TUN transaction journal".to_string(),
                    },
                },
            };
        }
        Err(error) => {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::UnsafeArtifact,
                        stage: "read_active_journal".to_string(),
                        retryable: false,
                        message: error.to_string(),
                    },
                },
            };
        }
    };

    if obs.core_running
        && obs.api_ready
        && obs
            .core_identity
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        && obs.core_pid.is_some_and(|pid| pid > 0)
        && obs
            .launched_revision
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        && obs.runtime_tun.is_some()
    {
        DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Completed(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: journal.transaction_id,
                    generation: journal.generation,
                    observed_phase: journal.phase,
                    proof_kind: crate::tun_transaction::RuntimeProofKind::CandidateAttested,
                    core_identity: obs.core_identity.clone().unwrap_or_default(),
                    core_pid: obs.core_pid.unwrap_or_default(),
                    launched_revision: obs.launched_revision.clone().unwrap_or_default(),
                    runtime_tun: obs.runtime_tun.unwrap_or(false),
                    api_ready: true,
                },
            ),
        }
    } else {
        DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Unavailable {
                observation: Some(obs),
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::ObservationUnavailable,
                    stage: "get_transaction_status".to_string(),
                    retryable: true,
                    message: "system Core is not running or its API is not ready".to_string(),
                },
            },
        }
    }
}

#[cfg(unix)]
async fn handle_get_transaction_status(state: Arc<Mutex<DaemonState>>) -> DaemonResponse {
    let obs = get_runtime_observation(&state).await;
    let journal_result = if let Some(ctx) = daemon_transaction_context() {
        crate::tun_transaction::read_active_journal(&ctx)
    } else {
        Err(anyhow::anyhow!("system instance paths are unavailable"))
    };
    transaction_status_response(obs, journal_result)
}

#[cfg(unix)]
async fn select_system_proxy(
    state: Arc<Mutex<DaemonState>>,
    group: String,
    node: String,
) -> DaemonResponse {
    if group.is_empty() || node.is_empty() || group.len() > 512 || node.len() > 512 {
        return DaemonResponse::Error {
            message: "proxy group and node must be non-empty and reasonably sized".to_string(),
        };
    }
    let endpoint = {
        let mut state_guard = state.lock().await;
        reap_exited_core(&mut state_guard);
        if !state_guard.core_running {
            return DaemonResponse::Error {
                message: "system Core is not running. Fix: mihomo-cli restart --system".to_string(),
            };
        }
        state_guard.api_endpoint.clone()
    };
    let Some(socket) = endpoint
        .as_deref()
        .and_then(endpoint_unix_path)
        .map(PathBuf::from)
    else {
        return DaemonResponse::Error {
            message: "system Core has no usable Unix API endpoint".to_string(),
        };
    };
    let client = mihomo_api::EndpointMihomoApiClient::new(ApiEndpoint::UnixSocket(socket));
    if let Err(error) = mihomo_api::select_proxy_with_client(&client, &group, &node).await {
        return DaemonResponse::Error {
            message: format!("failed to select proxy {group} → {node}: {error}"),
        };
    }
    match client
        .get(&format!("/proxies/{}", encode_proxy_path_segment(&group)))
        .await
    {
        Ok(config) if config["now"].as_str() == Some(node.as_str()) => DaemonResponse::Success {
            message: format!("proxy selection applied: {group} → {node}"),
        },
        Ok(_) => DaemonResponse::Error {
            message: "proxy selection was accepted but runtime attestation did not match"
                .to_string(),
        },
        Err(error) => DaemonResponse::Error {
            message: format!("proxy selection runtime attestation failed: {error}"),
        },
    }
}

#[cfg(unix)]
fn encode_proxy_path_segment(value: &str) -> String {
    value.bytes().fold(String::new(), |mut encoded, byte| {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
        encoded
    })
}

#[cfg(unix)]
async fn start_core(
    state: Arc<Mutex<DaemonState>>,
    config_path: PathBuf,
    core_binary: PathBuf,
) -> DaemonResponse {
    let config_path = match system_runtime_config_path(&config_path) {
        Ok(path) => path,
        Err(error) => {
            return DaemonResponse::Error {
                message: format!("failed to prepare system runtime config: {error}"),
            };
        }
    };
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
    let config_revision = match config_content_revision(&config_path) {
        Ok(revision) => revision,
        Err(error) => {
            return DaemonResponse::Error {
                message: format!("failed to read Core config revision: {error}"),
            }
        }
    };
    remove_stale_unix_endpoint(&api_endpoint);

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
    cmd.args(core_command_args(
        &config_path,
        &runtime_data_dir_for_config(&config_path),
    ))
    .stdin(Stdio::null())
    .stdout(Stdio::from(stdout_log))
    .stderr(Stdio::from(stderr_log));

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            match child.try_wait() {
                Ok(Some(_status)) => {
                    s.core_running = false;
                    s.core_pid = None;
                    s.api_endpoint = None;
                    remove_pid_file(&s.pid_file);
                    // ADR-24: classify the failure and attempt geo auto-recovery.
                    let log_tail = read_log_tail(&s.core_log_file, 50);
                    let failure = classify_core_startup_failure(&log_tail);
                    let error_msg = format_core_failure_message(&failure, &s.core_log_file);
                    drop(s);
                    return try_geo_recovery_and_retry(
                        &state,
                        &failure,
                        config_path,
                        core_binary,
                        error_msg,
                    )
                    .await;
                }
                Err(e) => {
                    s.core_running = false;
                    s.core_pid = None;
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
                        if let Some(mut child) = s.core_child.take() {
                            let _ = child.start_kill();
                            let _ = child.try_wait();
                        }
                        s.core_running = false;
                        s.core_pid = None;
                        s.api_endpoint = None;
                        remove_pid_file(&s.pid_file);
                        // ADR-24: classify the failure and attempt geo auto-recovery.
                        let log_tail = read_log_tail(&s.core_log_file, 50);
                        let failure = classify_core_startup_failure(&log_tail);
                        let error_msg = format_core_failure_message(&failure, &s.core_log_file);
                        drop(s);
                        return try_geo_recovery_and_retry(
                            &state,
                            &failure,
                            config_path,
                            core_binary,
                            error_msg,
                        )
                        .await;
                    }
                    s.launched_config_revision = Some(config_revision.clone());
                    if let Some(pid) = pid {
                        write_pid_file(
                            &s.pid_file,
                            &CorePidMetadata {
                                pid,
                                config_path: config_path.clone(),
                                core_binary: core_binary.clone(),
                                api_endpoint: s.api_endpoint.clone(),
                                config_revision: Some(config_revision.clone()),
                            },
                        );
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
            s.launched_config_revision = None;
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
        if let Some(endpoint) = orphan_endpoint {
            return DaemonResponse::Error {
                message: format!(
                    "core API endpoint is reachable at {endpoint}, but this daemon does not own the process and has no live pid file. Restart the system service or stop the orphan core manually."
                ),
            };
        }
        s.core_running = false;
        s.core_pid = None;
        s.launched_config_revision = None;
        s.api_endpoint = None;
        remove_pid_file(&s.pid_file);
        return DaemonResponse::Success {
            message: "core already stopped".to_string(),
        };
    };

    let kill_result = child.kill().await;
    let _ = child.wait().await;
    s.core_running = false;
    s.core_pid = None;
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
fn validate_core_api_request(
    method: CoreApiMethod,
    path: &str,
    body: Option<&serde_json::Value>,
    peer_uid: Option<u32>,
) -> Result<(), String> {
    if path.len() > 2048
        || !path.starts_with('/')
        || path.contains(['\r', '\n', '\\'])
        || path.contains("://")
        || path.split(['/', '?', '&']).any(|part| part == "..")
    {
        return Err("refusing malformed Core API path".to_string());
    }

    let allowed = match method {
        CoreApiMethod::Get => {
            matches!(path, "/configs" | "/proxies" | "/connections" | "/version")
                || path.starts_with("/proxies/")
                || path.starts_with("/group/")
        }
        CoreApiMethod::Put => path == "/configs" || path.starts_with("/proxies/"),
        CoreApiMethod::Patch => path == "/configs",
        CoreApiMethod::Delete => path == "/connections",
    };
    if !allowed {
        return Err(format!(
            "Core API request is not allowlisted: {method:?} {path}"
        ));
    }

    if method == CoreApiMethod::Patch
        && body
            .and_then(serde_json::Value::as_object)
            .is_some_and(|object| object.contains_key("tun"))
    {
        return Err("TUN changes must use the root-authorized TUN command".to_string());
    }

    if method == CoreApiMethod::Put && path == "/configs" {
        let config_path = body
            .and_then(|value| value.get("path"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Core config reload requires an absolute config path".to_string())?;
        validate_daemon_config_path_for_peer(std::path::Path::new(config_path), peer_uid)?;
    }
    Ok(())
}

#[cfg(unix)]
fn core_api_mutation_requires_promotion(
    method: CoreApiMethod,
    path: &str,
    runtime_tun: Option<bool>,
) -> anyhow::Result<()> {
    let is_effective_mutation = match method {
        CoreApiMethod::Put if path == "/configs" => true,
        CoreApiMethod::Patch if path == "/configs" => true,
        CoreApiMethod::Put if path.starts_with("/proxies/") => true,
        _ => false,
    };
    if !is_effective_mutation {
        return Ok(());
    }
    match runtime_tun {
        Some(false) => Ok(()),
        Some(true) => anyhow::bail!(
            "system TUN is active; this mutation must use the active-config promotion dispatcher"
        ),
        None => anyhow::bail!(
            "system TUN runtime is unknown; refusing an untracked Core API mutation. Run `mihomo-cli restart --system` and retry"
        ),
    }
}

#[cfg(all(test, unix))]
mod core_api_mutation_tests {
    use super::*;

    #[test]
    fn direct_core_api_mutations_are_blocked_when_tun_is_active_or_unknown() {
        for runtime_tun in [Some(true), None] {
            assert!(core_api_mutation_requires_promotion(
                CoreApiMethod::Put,
                "/configs",
                runtime_tun,
            )
            .is_err());
            assert!(core_api_mutation_requires_promotion(
                CoreApiMethod::Put,
                "/proxies/Group",
                runtime_tun,
            )
            .is_err());
        }
    }

    #[test]
    fn config_reload_is_allowed_only_when_tun_is_observed_disabled() {
        assert!(
            core_api_mutation_requires_promotion(CoreApiMethod::Put, "/configs", Some(false),)
                .is_ok()
        );
        assert!(
            core_api_mutation_requires_promotion(CoreApiMethod::Get, "/configs", None,).is_ok()
        );
    }
}

#[cfg(unix)]
async fn process_core_api_request(
    method: CoreApiMethod,
    path: String,
    body: Option<serde_json::Value>,
    state: Arc<Mutex<DaemonState>>,
    peer_uid: Option<u32>,
) -> DaemonResponse {
    if let Err(message) = validate_core_api_request(method, &path, body.as_ref(), peer_uid) {
        return DaemonResponse::Error { message };
    }
    let endpoint = {
        let mut state = state.lock().await;
        reap_exited_core(&mut state);
        if !state.core_running {
            return DaemonResponse::Error {
                message: "system Core is not running. Fix: mihomo-cli restart --system".to_string(),
            };
        }
        state.api_endpoint.clone()
    };
    let Some(socket) = endpoint
        .as_deref()
        .and_then(endpoint_unix_path)
        .map(PathBuf::from)
    else {
        return DaemonResponse::Error {
            message:
                "system Core has no usable Unix API endpoint. Fix: mihomo-cli restart --system"
                    .to_string(),
        };
    };
    let client = mihomo_api::EndpointMihomoApiClient::new(ApiEndpoint::UnixSocket(socket));
    let runtime_tun = if matches!(method, CoreApiMethod::Put | CoreApiMethod::Patch)
        && (path == "/configs" || path.starts_with("/proxies/"))
    {
        match client.get("/configs").await {
            Ok(config) => config["tun"]["enable"].as_bool(),
            Err(error) => {
                return DaemonResponse::Error {
                    message: format!(
                        "cannot observe system Core TUN runtime before mutation: {error}. Run `mihomo-cli restart --system` and retry"
                    ),
                }
            }
        }
    } else {
        None
    };
    if let Err(error) = core_api_mutation_requires_promotion(method, &path, runtime_tun) {
        return DaemonResponse::Error {
            message: error.to_string(),
        };
    }
    let result = match method {
        CoreApiMethod::Get => client.get(&path).await,
        CoreApiMethod::Put => {
            client
                .put(&path, body.unwrap_or(serde_json::Value::Null))
                .await
        }
        CoreApiMethod::Patch => {
            client
                .patch(&path, body.unwrap_or(serde_json::Value::Null))
                .await
        }
        CoreApiMethod::Delete => client.delete(&path).await,
    };
    match result {
        Ok(data) => DaemonResponse::CoreApi { data },
        Err(error) => DaemonResponse::Error {
            message: format!("Core API request failed: {error}"),
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
    read_api_endpoint_from_content(&content)
}

#[cfg(any(unix, windows))]
fn read_api_endpoint_from_content(content: &str) -> Option<String> {
    let config: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
    fn corrupt_journal_only_allows_read_only_status_commands() {
        assert!(allowed_with_unreadable_journal(&DaemonCommand::GetStatus {
            token: None
        }));
        assert!(allowed_with_unreadable_journal(
            &DaemonCommand::GetTransactionStatus { token: None }
        ));
        assert!(!allowed_with_unreadable_journal(
            &DaemonCommand::CoreApiRequest {
                method: CoreApiMethod::Put,
                path: "/configs".to_string(),
                body: None,
                token: None,
            }
        ));
        assert!(!allowed_with_unreadable_journal(
            &DaemonCommand::PromoteSystemConfig {
                config_content: "tun:\n  enable: false\n".to_string(),
                config_revision: "revision".to_string(),
                selection_intent_dir: None,
                subscription_id: None,
                token: None,
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn get_status_preserves_corrupt_journal_diagnostic() {
        let result: anyhow::Result<Option<crate::tun_transaction::TunJournal>> =
            Err(anyhow::anyhow!("unsupported active TUN journal schema 99"));
        let (phase, error) = journal_status_from_result(result);
        assert_eq!(
            phase,
            Some(crate::tun_transaction::JournalPhase::RecoveryRequired)
        );
        assert_eq!(
            error.as_deref(),
            Some("unsupported active TUN journal schema 99")
        );
    }
    use crate::ipc::DaemonResponse;

    #[test]
    fn core_api_proxy_allowlist_rejects_tun_and_unrecognized_paths() {
        assert!(
            validate_core_api_request(CoreApiMethod::Get, "/configs", None, Some(1000)).is_ok()
        );
        assert!(validate_core_api_request(
            CoreApiMethod::Get,
            "/group/selector/delay?url=http%3A%2F%2Fexample.com&timeout=5000",
            None,
            Some(1000)
        )
        .is_ok());

        let tun = serde_json::json!({"tun": {"enable": true}});
        let error =
            validate_core_api_request(CoreApiMethod::Patch, "/configs", Some(&tun), Some(1000))
                .unwrap_err();
        assert!(error.contains("root-authorized TUN"));
        assert!(validate_core_api_request(
            CoreApiMethod::Get,
            "/providers/proxies",
            None,
            Some(1000)
        )
        .is_err());
        assert!(validate_core_api_request(
            CoreApiMethod::Get,
            "/configs\r\nInjected: true",
            None,
            Some(1000)
        )
        .is_err());
    }

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
    fn authorized_clients_table_roundtrip_sets_daemon_readable_mode() {
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
        #[cfg(target_os = "linux")]
        let expected_mode = 0o640;
        #[cfg(not(target_os = "linux"))]
        let expected_mode = 0o600;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            expected_mode
        );
    }

    #[test]
    fn revoke_authorized_client_matches_uid_and_token_only() {
        let mut table = AuthorizedClients {
            clients: vec![
                AuthorizedClient {
                    user: "alice".into(),
                    uid: 1000,
                    token: "alice-token".into(),
                },
                AuthorizedClient {
                    user: "bob".into(),
                    uid: 1001,
                    token: "bob-token".into(),
                },
            ],
        };
        assert!(revoke_authorized_client(&mut table, 1000, "alice-token").unwrap());
        assert_eq!(table.clients.len(), 1);
        assert_eq!(table.clients[0].user, "bob");
        assert!(!revoke_authorized_client(&mut table, 1000, "wrong-token").unwrap());
        assert_eq!(table.clients.len(), 1);
    }

    #[test]
    fn revoke_authorized_client_rejects_duplicate_uid_token_entries() {
        let mut table = AuthorizedClients {
            clients: vec![
                AuthorizedClient {
                    user: "alice".into(),
                    uid: 1000,
                    token: "same-token".into(),
                },
                AuthorizedClient {
                    user: "alias".into(),
                    uid: 1000,
                    token: "same-token".into(),
                },
            ],
        };
        assert!(revoke_authorized_client(&mut table, 1000, "same-token").is_err());
        assert_eq!(table.clients.len(), 2);
    }

    #[test]
    fn revoke_authorized_client_rejects_empty_token() {
        let mut table = AuthorizedClients::default();
        assert!(revoke_authorized_client(&mut table, 1000, "").is_err());
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
        let _guard = crate::utils::env_test_lock().lock().unwrap();
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
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let response =
            process_command(DaemonCommand::GetStatus { token: None }, state, Some(1000)).await;
        assert!(matches!(response, DaemonResponse::Error { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_command_rejects_apply_tun_from_non_root_peer() {
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let response = process_command(
            DaemonCommand::ApplySystemTunSnapshot {
                expected_revision: "test-revision".to_string(),
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
        let injected = pipe_sddl_for_installer("S-1-5-21-1000)(A;;GA;;;WD");
        assert_eq!(sddl, injected);
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
        assert!(err.contains("config_content"));
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
            config_revision: Some("rev-1".to_string()),
        }
    }

    #[test]
    fn pid_file_content_roundtrips_and_rejects_invalid_values() {
        let metadata = sample_metadata(1234);
        let content = format_pid_metadata(&metadata);
        assert_eq!(parse_pid_file_content(&content), Some(metadata));
        assert_eq!(parse_pid_file_content("1234\n").map(|m| m.pid), Some(1234));
        assert_eq!(
            parse_pid_file_content("1234\n").and_then(|m| m.config_revision),
            None
        );
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
    fn core_command_args_keeps_runtime_data_dir_independent_from_config_file() {
        let config = PathBuf::from("/var/lib/mihomo-cli/transactions/active/recovery-target.yaml");
        let runtime = PathBuf::from("/var/lib/mihomo-cli");
        let args: Vec<String> = core_command_args(&config, &runtime)
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec![
                "-d",
                "/var/lib/mihomo-cli",
                "-f",
                "/var/lib/mihomo-cli/transactions/active/recovery-target.yaml",
            ]
        );
    }

    #[test]
    fn core_command_args_explicitly_selects_nondefault_config_file() {
        let config = PathBuf::from("/var/lib/mihomo-cli/tun-config.yaml");
        let runtime = PathBuf::from("/var/lib/mihomo-cli");
        let args: Vec<String> = core_command_args(&config, &runtime)
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec![
                "-d",
                "/var/lib/mihomo-cli",
                "-f",
                "/var/lib/mihomo-cli/tun-config.yaml",
            ]
        );
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
    async fn stop_core_clears_runtime_state_but_preserves_last_start_intent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let api_endpoint = tmp.path().join("missing-core.sock");
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: true,
            core_child: None,
            core_pid: Some(4242),
            config_path: Some(PathBuf::from("/home/alice/.config/mihomo/config.yaml")),
            launched_config_revision: None,
            core_binary: Some(expected_system_core_binary_path()),
            api_endpoint: Some(api_endpoint.display().to_string()),
            pid_file: tmp.path().join("missing-core.pid"),
            core_log_file: tmp.path().join("mihomo.log"),
        }));

        let response = stop_core(Arc::clone(&state)).await;

        match response {
            DaemonResponse::Success { message } => {
                assert_eq!(message, "core already stopped")
            }
            other => panic!("expected idempotent stop success, got {other:?}"),
        }
        let s = state.lock().await;
        assert!(!s.core_running);
        assert_eq!(s.core_pid, None);
        assert_eq!(
            s.config_path,
            Some(PathBuf::from("/home/alice/.config/mihomo/config.yaml"))
        );
        assert_eq!(s.core_binary, Some(expected_system_core_binary_path()));
        assert_eq!(s.api_endpoint, None);
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
            config_path: Some(config_path.clone()),
            launched_config_revision: None,
            core_binary: Some(expected_system_core_binary_path()),
            api_endpoint: Some("/var/run/mihomo/mihomo.sock".to_string()),
            pid_file: tmp.path().join("core.pid"),
            core_log_file: tmp.path().join("mihomo.log"),
        }));
        let response = process_command(
            DaemonCommand::RestartCore {
                config_content: "invalid: [yaml".to_string(),
                config_revision: "invalid-revision".to_string(),
                selection_intent_dir: None,
                subscription_id: None,
                token: None,
            },
            Arc::clone(&state),
            Some(0),
        )
        .await;

        match response {
            DaemonResponse::Error { message } => assert!(
                message.contains("revision")
                    || message.contains("YAML")
                    || message.contains("endpoint")
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

        // TUN config path (system-level) must also be accepted.
        #[cfg(target_os = "macos")]
        let valid_tun = "/Library/Application Support/mihomo-cli/tun-config.yaml";
        #[cfg(target_os = "linux")]
        let valid_tun = "/var/lib/mihomo-cli/tun-config.yaml";
        #[cfg(target_os = "windows")]
        let valid_tun = "C:/ProgramData/mihomo-cli/tun-config.yaml";
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        assert!(validate_daemon_config_path_shape(std::path::Path::new(valid_tun)).is_ok());

        // The other platform's path shape must be rejected.
        let platform_other_home = if cfg!(target_os = "macos") {
            "/home/alice/.config/mihomo/config.yaml"
        } else {
            "/Users/alice/.config/mihomo/config.yaml"
        };
        let err = validate_daemon_config_path_shape(std::path::Path::new(platform_other_home))
            .unwrap_err();
        assert!(err.contains("system daemon only accepts"));

        let err =
            validate_daemon_config_path_shape(std::path::Path::new("/etc/shadow")).unwrap_err();
        assert!(err.contains("expected config.yaml or tun-config.yaml"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/alice/.config/other/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("system daemon only accepts"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/alice/extra/.config/mihomo/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("system daemon only accepts"));
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

        // Invalid tun-config.yaml path should be rejected.
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/alice/.config/mihomo/tun-config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("system daemon only accepts"));
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
            config_revision: None,
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
        let is_lifecycle = !matches!(cmd, DaemonCommand::GetStatus { .. });
        assert!(!is_lifecycle, "GetStatus must not be a lifecycle command");

        let cmd = DaemonCommand::GetStatus {
            token: Some("authorized-token".to_string()),
        };
        let is_lifecycle = !matches!(cmd, DaemonCommand::GetStatus { .. });
        assert!(
            !is_lifecycle,
            "authenticated GetStatus must not take the lifecycle lock"
        );

        drop(guard);

        // 生命周期命令（StartCore）应标记为需要锁
        let cmd = DaemonCommand::StartCore {
            config_content: "mode: rule\n".to_string(),
            config_revision: "revision".to_string(),
            selection_intent_dir: None,
            subscription_id: None,
            token: None,
        };
        let is_lifecycle = !matches!(cmd, DaemonCommand::GetStatus { .. });
        assert!(is_lifecycle, "StartCore must be a lifecycle command");
    }

    #[test]
    fn reaping_crashed_core_preserves_recovery_intent() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = PathBuf::from("/home/alice/.config/mihomo/config.yaml");
        let core_binary = expected_system_core_binary_path();
        let mut state = DaemonState {
            core_running: true,
            core_pid: Some(u32::MAX),
            config_path: Some(config_path.clone()),
            core_binary: Some(core_binary.clone()),
            api_endpoint: Some("/var/run/mihomo/mihomo.sock".to_string()),
            pid_file: tmp.path().join("missing.pid"),
            core_log_file: tmp.path().join("mihomo.log"),
            ..DaemonState::default()
        };

        reap_exited_core(&mut state);

        assert!(!state.core_running);
        assert_eq!(state.config_path, Some(config_path));
        assert_eq!(state.core_binary, Some(core_binary));
        assert_eq!(
            state.api_endpoint.as_deref(),
            Some("/var/run/mihomo/mihomo.sock")
        );
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

    #[tokio::test]
    async fn apply_tun_requires_explicit_core_restart_when_stopped() {
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            core_binary: None,
            config_path: None,
            ..DaemonState::default()
        }));

        let response = process_command(
            DaemonCommand::ApplySystemTunSnapshot {
                expected_revision: "test-revision".to_string(),
                stack: None,
                dns_hijack: None,
                token: None,
            },
            state,
            Some(0),
        )
        .await;

        match response {
            DaemonResponse::Error { ref message } => {
                assert!(
                    message.contains("restart --system"),
                    "error must require explicit restart: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn core_api_request_does_not_start_stopped_core() {
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            ..DaemonState::default()
        }));

        let response = process_core_api_request(
            CoreApiMethod::Get,
            "/configs".to_string(),
            None,
            state,
            Some(1000),
        )
        .await;

        match response {
            DaemonResponse::Error { message } => {
                assert!(
                    message.contains("restart --system"),
                    "unexpected error: {message}"
                );
            }
            other => panic!("expected stopped-core error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn disable_tun_requires_explicit_core_restart_when_stopped() {
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            core_binary: None,
            config_path: None,
            ..DaemonState::default()
        }));

        let response =
            process_command(DaemonCommand::DisableTun { token: None }, state, Some(0)).await;

        match response {
            DaemonResponse::Error { ref message } => {
                assert!(
                    message.contains("restart --system"),
                    "error must require explicit restart: {message}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn enable_tun_with_core_running_does_not_error_on_binary_check() {
        let config_path = if cfg!(target_os = "macos") {
            PathBuf::from("/Users/alice/.config/mihomo/config.yaml")
        } else {
            PathBuf::from("/home/alice/.config/mihomo/config.yaml")
        };
        let core_binary = if cfg!(target_os = "macos") {
            PathBuf::from("/Library/Application Support/mihomo/bin/mihomo")
        } else {
            PathBuf::from("/usr/local/lib/mihomo/mihomo")
        };

        // Write a pid file so reap_exited_core doesn't clear core_running.
        let tmp = tempfile::TempDir::new().unwrap();
        let pid_file = tmp.path().join("core.pid");
        let metadata = CorePidMetadata {
            pid: std::process::id(),
            config_path: config_path.clone(),
            core_binary: core_binary.clone(),
            api_endpoint: Some("unix:///var/run/mihomo/mihomo.sock".to_string()),
            config_revision: Some("test-revision".to_string()),
        };
        std::fs::write(
            &pid_file,
            serde_json::to_string(&metadata).unwrap().as_bytes(),
        )
        .unwrap();

        let state = Arc::new(Mutex::new(DaemonState {
            core_running: true,
            core_pid: Some(std::process::id()),
            core_binary: Some(core_binary),
            config_path: Some(config_path.clone()),
            api_endpoint: Some("unix:///var/run/mihomo/mihomo.sock".to_string()),
            pid_file,
            ..DaemonState::default()
        }));

        let response = process_command(
            DaemonCommand::ApplySystemTunSnapshot {
                expected_revision: "test-revision".to_string(),
                stack: None,
                dns_hijack: None,
                token: None,
            },
            state,
            Some(0),
        )
        .await;

        // Core is running, so the error should NOT be about missing binary.
        // It may fail at the API call (no real core), but that's expected.
        if let DaemonResponse::Error { ref message } = response {
            assert!(
                !message.contains("core binary path unknown"),
                "should not fail on binary check when core is running: {message}"
            );
            assert!(
                !message.contains("core is not running"),
                "should not report core not running when it is: {message}"
            );
        }
    }

    // ADR-24: classify_core_startup_failure tests.
    #[test]
    fn classify_geoip_mmdb_missing() {
        let log = r#"time="2026-08-12T14:50:39" level=info msg="Can't find MMDB, start download"
time="2026-08-12T14:52:09" level=error msg="can't download MMDB: context deadline exceeded"
time="2026-08-12T14:52:09" level=fatal msg="Parse config error: rules[3848] [GEOIP,telegram,Telegram,no-resolve] error: can't download MMDB""#;

        let failure = classify_core_startup_failure(log);
        assert_eq!(failure, CoreFailure::GeoipMissing);
    }

    #[test]
    fn classify_geosite_missing() {
        let log = r#"time="2026-08-12T14:50:39" level=info msg="Can't find GeoSite.dat""#;
        let failure = classify_core_startup_failure(log);
        assert_eq!(failure, CoreFailure::GeositeMissing);
    }

    #[test]
    fn classify_config_parse_error() {
        let log = r#"time="2026-08-12T14:50:39" level=fatal msg="Parse config error: invalid key 'foo' at line 42""#;
        let failure = classify_core_startup_failure(log);
        match failure {
            CoreFailure::ConfigError { ref detail } => {
                assert!(detail.contains("Parse config error"));
            }
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn classify_system_geo_permission_before_config_error() {
        let log = r#"time="2026-08-13T14:50:39" level=fatal msg="Parse config error: rules[3848] error: can't remove invalid MMDB: remove /var/lib/mihomo-cli/geoip.metadb: permission denied""#;
        let failure = classify_core_startup_failure(log);
        assert!(matches!(failure, CoreFailure::PermissionDenied { .. }));
        let message = format_core_failure_message(
            &failure,
            std::path::Path::new("/var/log/mihomo/mihomo.log"),
        );
        assert!(message.contains("system Mihomo data directory permissions"));
        assert!(message.contains("mihomo-cli install --system --force"));
        assert!(!message.contains("config --validate"));
    }

    #[test]
    fn classify_port_conflict() {
        let log = r#"time="2026-08-12T14:50:39" level=error msg="listen tcp 0.0.0.0:9090: address already in use""#;
        let failure = classify_core_startup_failure(log);
        match failure {
            CoreFailure::PortConflict { ref detail } => {
                assert!(detail.contains("address already in use"));
            }
            other => panic!("expected PortConflict, got {other:?}"),
        }
    }

    #[test]
    fn classify_permission_denied() {
        let log = r#"time="2026-08-12T14:50:39" level=fatal msg="open /etc/mihomo/config.yaml: permission denied""#;
        let failure = classify_core_startup_failure(log);
        match failure {
            CoreFailure::PermissionDenied { ref detail } => {
                assert!(detail.contains("permission denied"));
            }
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
    }

    #[test]
    fn classify_unknown_fallback() {
        let log = r#"time="2026-08-12T14:50:39" level=fatal msg="something completely unexpected happened""#;
        let failure = classify_core_startup_failure(log);
        assert!(matches!(failure, CoreFailure::Unknown { .. }));
    }

    #[test]
    fn classify_empty_log_returns_unknown() {
        let failure = classify_core_startup_failure("");
        assert!(matches!(failure, CoreFailure::Unknown { .. }));
    }

    // ADR-24: format_core_failure_message tests.
    #[test]
    fn format_geoip_message_contains_fix() {
        let msg = format_core_failure_message(
            &CoreFailure::GeoipMissing,
            std::path::Path::new("/var/log/mihomo/mihomo.log"),
        );
        assert!(msg.contains("GeoIP"));
        assert!(msg.contains("Fix:"));
    }

    #[test]
    fn format_config_error_shows_detail_and_fix() {
        let msg = format_core_failure_message(
            &CoreFailure::ConfigError {
                detail: "Parse config error: invalid key at line 42".to_string(),
            },
            std::path::Path::new("/var/log/mihomo/mihomo.log"),
        );
        assert!(msg.contains("Parse config error"));
        assert!(msg.contains("config --validate"));
    }

    #[test]
    fn format_unknown_shows_log_tail() {
        let msg = format_core_failure_message(
            &CoreFailure::Unknown {
                log_tail: "line1\nline2\nline3".to_string(),
            },
            std::path::Path::new("/var/log/mihomo/mihomo.log"),
        );
        assert!(msg.contains("line1"));
        assert!(msg.contains("Logs:"));
    }

    // ADR-24: read_log_tail tests.
    #[test]
    fn read_log_tail_returns_last_n_lines() {
        let tmp = tempfile::TempDir::new().unwrap();
        let log_file = tmp.path().join("test.log");
        std::fs::write(&log_file, "line1\nline2\nline3\nline4\nline5\n").unwrap();

        let result = read_log_tail(&log_file, 3);
        assert_eq!(result, "line3\nline4\nline5");
    }

    #[test]
    fn read_log_tail_missing_file_returns_empty() {
        let result = read_log_tail(std::path::Path::new("/nonexistent/file.log"), 10);
        assert_eq!(result, "");
    }
}

#[cfg(any(unix, test))]
#[allow(dead_code)]
fn active_config_matches_requested(active: Option<&PathBuf>, requested: &std::path::Path) -> bool {
    active.map(|path| path == requested).unwrap_or(false)
}

#[cfg(test)]
mod windows_auth_model_tests {
    use super::validate_windows_client_token_value;
    use crate::ipc::DaemonResponse;

    #[test]
    fn windows_daemon_accepts_only_the_canonical_service_token_value() {
        assert!(
            validate_windows_client_token_value(Some("same-token"), Some("same-token")).is_ok()
        );
        assert!(
            validate_windows_client_token_value(Some("wrong-token"), Some("same-token")).is_err()
        );
        assert!(validate_windows_client_token_value(None, Some("same-token")).is_err());
        assert!(validate_windows_client_token_value(Some("same-token"), None).is_err());
        assert!(validate_windows_client_token_value(Some(""), Some("")).is_err());
    }

    #[test]
    fn windows_reinstall_invalidates_the_old_client_copy() {
        let old_token = crate::service::generate_auth_token();
        let new_token = crate::service::generate_auth_token();
        assert_ne!(old_token, new_token);

        assert!(validate_windows_client_token_value(Some(&new_token), Some(&new_token)).is_ok());
        assert!(validate_windows_client_token_value(Some(&old_token), Some(&new_token)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn transaction_status_never_returns_completed_without_a_valid_journal() {
        let observation = crate::tun_transaction::RuntimeObservation {
            core_running: true,
            core_identity: Some("core".to_string()),
            core_pid: Some(42),
            launched_revision: Some("candidate".to_string()),
            runtime_tun: Some(true),
            api_ready: true,
        };

        let no_journal = super::transaction_status_response(observation.clone(), Ok(None));
        assert!(matches!(
            no_journal,
            DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::Unavailable { .. }
            }
        ));

        let corrupt = super::transaction_status_response(
            observation,
            Err(anyhow::anyhow!("unsupported active journal schema 99")),
        );
        assert!(matches!(
            corrupt,
            DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch { .. }
            }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn quiesce_candidate_rejects_and_does_not_stop_unknown_core_with_same_revision() {
        use super::*;
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(DaemonState {
            core_running: true,
            core_pid: Some(9999),
            core_binary: Some(std::path::PathBuf::from("/usr/bin/unknown-core-identity")),
            launched_config_revision: Some("cand-rev".to_string()),
            ..DaemonState::default()
        }));

        let fence = crate::tun_transaction::TransactionFence {
            transaction_id: "tx-quiesce-test".to_string(),
            generation: 1,
            expected_phase: crate::tun_transaction::JournalPhase::RollbackPending,
            expected_candidate_revision: "cand-rev".to_string(),
        };

        // If journal is not found, it returns Stale/Unavailable
        let _response =
            handle_quiesce_candidate_runtime(std::sync::Arc::clone(&state), fence.clone(), Some(0))
                .await;

        // Core must still be running after rejection
        let state_guard = state.lock().await;
        assert!(state_guard.core_running, "unknown core must not be stopped");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_promoted_snapshot_rejects_and_does_not_stop_unknown_core() {
        use super::*;
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(DaemonState {
            core_running: true,
            core_pid: Some(8888),
            core_binary: Some(std::path::PathBuf::from("/usr/bin/unknown-core-identity")),
            launched_config_revision: Some("unknown-rev".to_string()),
            ..DaemonState::default()
        }));

        let fence = crate::tun_transaction::TransactionFence {
            transaction_id: "tx-apply-test".to_string(),
            generation: 1,
            expected_phase: crate::tun_transaction::JournalPhase::SnapshotPromoted,
            expected_candidate_revision: "candidate-rev".to_string(),
        };

        // When journal is absent or evidence does not match, it must return EvidenceMismatch / Stale
        // and under no circumstance stop the running unknown core.
        let _response = handle_apply_promoted_snapshot(
            std::sync::Arc::clone(&state),
            fence.clone(),
            true,
            Some(0),
        )
        .await;

        let state_guard = state.lock().await;
        assert!(
            state_guard.core_running,
            "running unknown core must not be stopped by apply_promoted_snapshot"
        );
    }
}
