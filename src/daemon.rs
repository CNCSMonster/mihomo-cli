//! System service daemon for mihomo-cli.
//!
//! Runs as the dedicated non-root `mihomo` service user where supported, listens on a Unix socket,
//! and manages the core with service-manager-granted capabilities.
//!
//! The daemon is started by `mihomo-cli install --system` and managed by
//! the system's service manager (systemd/launchd/Windows Service).

#[cfg(any(unix, windows))]
use crate::instance::ApiEndpoint;
use crate::instance::{SystemPaths, TargetOs};
#[cfg(any(unix, windows))]
use crate::ipc::CoreApiMethod;
#[cfg(any(unix, windows))]
use crate::ipc::{DaemonCommand, DaemonResponse, TUN_GUARD_ERROR_MARKER};
#[cfg(any(unix, windows))]
use crate::mihomo_api;
#[cfg(any(unix, windows))]
use crate::mihomo_api::MihomoApiClient;
#[cfg(any(unix, windows))]
use std::path::Path;
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

/// Fixed system runtime directory text per platform, normalized to forward
/// slashes. Must stay in sync with `instance::planned_tun_config_file`
/// (Linux `/var/lib/mihomo-cli`, macOS `/Library/Application Support/mihomo-cli`,
/// Windows `%ProgramData%/mihomo-cli`); a drift here silently rejects the
/// daemon's own runtime configs (Bug #12).
#[cfg(any(unix, windows))]
fn managed_runtime_dir_text() -> &'static str {
    if cfg!(target_os = "macos") {
        "/Library/Application Support/mihomo-cli"
    } else if cfg!(windows) {
        "/ProgramData/mihomo-cli"
    } else {
        "/var/lib/mihomo-cli"
    }
}

/// Shape check for managed runtime files, evaluated against a normalized
/// (backslash→slash) path text. Testable for every platform's runtime dir on
/// any host so the macOS/Windows branches cannot silently diverge from Linux.
/// The comparison is anchored: only a single Windows drive prefix may be
/// stripped, so no unrelated path can `ends_with` its way into acceptance.
#[cfg(any(unix, windows))]
fn runtime_config_text_allowed(text: &str, file_name: Option<&str>, runtime_dir: &str) -> bool {
    let suffix = match file_name {
        Some("tun-config.yaml") => "/tun-config.yaml",
        Some("active-config.yaml") => "/active-config.yaml",
        Some("recovery-target.yaml") => "/transactions/active/recovery-target.yaml",
        _ => return false,
    };
    let candidate = format!("{runtime_dir}{suffix}");
    let anchored = match text.split_once(':') {
        Some((drive, rest)) if drive.chars().count() == 1 && rest.starts_with('/') => {
            rest.to_string()
        }
        _ => text.to_string(),
    };
    anchored == candidate
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

    let runtime_allowed = runtime_config_text_allowed(&text, file_name, managed_runtime_dir_text());
    let user_config_ok = if cfg!(target_os = "windows") {
        // User config: %APPDATA%/mihomo/config.yaml
        (text.ends_with("/AppData/Roaming/mihomo/config.yaml")
            || text.ends_with("/AppData/Roaming/Mihomo/config.yaml"))
            && (text.contains(":/") || text.starts_with("//"))
    } else if cfg!(target_os = "macos") {
        // User config: /Users/<user>/.config/mihomo/config.yaml
        let parts: Vec<&str> = text.split('/').collect();
        matches!(
            parts.as_slice(),
            ["", "Users", user, ".config", "mihomo", "config.yaml"] if !user.is_empty()
        ) || matches!(
            parts.as_slice(),
            ["", "var", "root", ".config", "mihomo", "config.yaml"]
        )
    } else {
        // Linux user config: /home/<user>/.config/mihomo/config.yaml
        let parts: Vec<&str> = text.split('/').collect();
        matches!(
            parts.as_slice(),
            ["", "home", user, ".config", "mihomo", "config.yaml"] if !user.is_empty()
        )
    };

    if runtime_allowed || user_config_ok {
        Ok(())
    } else {
        Err(format!(
            "refusing to use config path {}; system daemon only accepts per-user config.yaml or managed runtime configs",
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
fn validate_daemon_config_path_for_peer(
    config_path: &std::path::Path,
    _peer_uid: Option<u32>,
) -> Result<(), String> {
    // Mirror the Unix branch's pre-check: reject internal recovery config
    // paths before shape validation so clients cannot submit these paths via
    // Core API forwarding.
    if matches!(
        config_path.file_name().and_then(|name| name.to_str()),
        Some("active-config.yaml") | Some("recovery-target.yaml")
    ) {
        return Err(format!(
            "refusing to use internal recovery config path {} through daemon IPC",
            config_path.display()
        ));
    }
    validate_daemon_config_path_shape(config_path)
}

#[cfg(unix)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct OwnerRecord {
    pub uid: u32,
    pub token_sha256: String,
}

#[cfg(unix)]
pub(crate) fn owner_record_path() -> PathBuf {
    // 提权上下文（root）下忽略可伪造的环境变量覆盖，避免 root 借它写到任意路径
    if unsafe { libc::geteuid() } == 0 {
        return PathBuf::from("/var/lib/mihomo-cli/owner");
    }
    std::env::var_os("MIHOMO_CLI_OWNER_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/mihomo-cli/owner"))
}

#[cfg(unix)]
pub(crate) fn legacy_authorized_clients_path() -> PathBuf {
    if unsafe { libc::geteuid() } == 0 {
        return PathBuf::from("/var/lib/mihomo-cli/authorized-clients.json");
    }
    std::env::var_os("MIHOMO_CLI_LEGACY_AUTHORIZED_CLIENTS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/mihomo-cli/authorized-clients.json"))
}

#[cfg(unix)]
pub(crate) fn legacy_migration_warning(path: &std::path::Path) -> String {
    format!(
        "提示：检测到旧版授权配置文件 {}。系统服务现已采用单所有者模型，此文件中的非所有者条目将不再生效。",
        path.display()
    )
}

#[cfg(unix)]
pub(crate) fn check_legacy_authorized_clients_migration_notice() -> Option<String> {
    let legacy_path = legacy_authorized_clients_path();
    if legacy_path.exists() {
        Some(legacy_migration_warning(&legacy_path))
    } else {
        None
    }
}

#[cfg(unix)]
pub(crate) fn check_and_warn_legacy_authorized_clients() {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        if let Some(notice) = check_legacy_authorized_clients_migration_notice() {
            eprintln!("{notice}");
        }
    }
}

#[cfg(unix)]
#[allow(dead_code)]
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
pub(crate) fn read_owner_record_from(path: &std::path::Path) -> anyhow::Result<OwnerRecord> {
    if !path.exists() {
        anyhow::bail!("owner record file does not exist: {}", path.display());
    }
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(unix)]
fn write_root_owner_record_file(
    path: &std::path::Path,
    bytes: &[u8],
    mode: u16,
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    if unsafe { libc::geteuid() } != 0 || path != owner_record_path() {
        anyhow::bail!(
            "refusing privileged owner record write outside {}",
            owner_record_path().display()
        );
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("owner record path has no parent"))?;
    if !parent.is_dir() {
        anyhow::bail!(
            "owner record state directory is missing: {}. Reinstall the system service",
            parent.display()
        );
    }
    let dir = crate::utils::open_directory_no_follow(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("owner record path has no file name"))?;
    let name = std::ffi::CString::new(name.as_bytes())?;
    let temp_name = std::ffi::CString::new(format!(".owner.{}.tmp", std::process::id()))?;
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
pub(crate) fn write_owner_record_to(
    path: &std::path::Path,
    record: &OwnerRecord,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    let mode = 0o640;
    #[cfg(not(target_os = "linux"))]
    let mode = 0o600;
    let bytes = serde_json::to_vec_pretty(record)?;
    let privileged_system_path = unsafe { libc::geteuid() } == 0 && path == owner_record_path();
    if privileged_system_path {
        write_root_owner_record_file(path, &bytes, mode)?;
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
    let token = token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "invalid or missing auth token".to_string())?;
    let uid = peer_uid.ok_or_else(|| "cannot determine IPC peer uid".to_string())?;
    let record = read_owner_record_from(&owner_record_path())
        .map_err(|e| format!("cannot read owner record: {e}"))?;
    if uid != record.uid {
        return Err("auth token does not belong to IPC peer uid".to_string());
    }
    let token_sha256 = crate::tun_transaction::sha256_revision(token.as_bytes());
    if constant_time_token_eq(&token_sha256, &record.token_sha256) {
        Ok(())
    } else {
        Err("invalid or missing auth token".to_string())
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
    let state = Arc::new(Mutex::new(DaemonState::default()));

    // ADR-19: if core autostart is enabled, start the core automatically on
    // daemon startup (e.g. boot). The marker is daemon-owned at the
    // authoritative config dir — NOT the CLI's possibly-elevated home.
    let autostart_marker = daemon_config_dir().join("autostart");
    if autostart_marker.exists() {
        let runtime_dir = system_runtime_data_dir();
        if let Some(reason) = autostart_transaction_blocker() {
            eprintln!("[mihomo-daemon] autostart skipped: {reason}");
        } else if let Some(config_path) = resolve_autostart_target_config_path(&runtime_dir) {
            eprintln!(
                "[mihomo-daemon] autostart marker present; starting core with {}",
                config_path.display()
            );
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
                "[mihomo-daemon] autostart marker present but no valid config snapshot; skipping core start"
            );
        }
    }

    // SPEC §3.8.1 mirror contract: if a core ended up running at startup,
    // replay the selection mirror once — idempotent and best-effort.
    let core_running_at_startup = state.lock().await.core_running;
    if core_running_at_startup {
        // SPEC §3.8.3: normal boot recovers selections when the mirror can be
        // proven to belong to the loaded configuration; otherwise every entry
        // skips replay with an explicit annotation (never a silent claim).
        for line in startup_replay_lines(&state).await {
            eprintln!("[mihomo-daemon] {}", render_replay_line_for_log(&line));
        }
    }

    eprintln!("[mihomo-daemon] listening on {pipe_name}");

    // SPEC A2/A3: read-only cache.db diagnosis at startup, then every tick.
    diagnose_cache_db_hygiene();

    tokio::spawn(run_selection_drift_sampler(
        Arc::clone(&state),
        cancel.child_token(),
    ));

    // first_pipe_instance only for the first create (P1-2).
    let mut first_instance = true;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                eprintln!("[mihomo-daemon] shutdown requested, exiting accept loop");
                // Stop the managed core child before exiting — dropping the
                // Child alone does NOT kill the process (P1-1).
                let _ = stop_core(Arc::clone(&state)).await;
                return Ok(());
            }
            accepted = accept_one_pipe_connection(&pipe_name, first_instance) => {
                first_instance = false;
                match accepted {
                    Ok(Some(server)) => {
                        let st = Arc::clone(&state);
                        tokio::spawn(async move {
                            if let Err(e) = handle_windows_pipe(server, st).await {
                                log_connection_error(&e);
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
/// Returns `Ok(true)` if handled by the SCM dispatcher.
/// Returns `Ok(false)` when not launched by SCM (e.g. manual `mihomo-cli daemon`
/// from a console/shell) so the caller can fall back to the console loop in its
/// existing async runtime without creating a nested tokio runtime.
pub fn run_windows_service() -> anyhow::Result<bool> {
    match windows_service_entry::run_dispatcher() {
        Ok(()) => Ok(true),
        Err(e) => {
            // Not running under SCM (manual `mihomo-cli daemon` from a shell):
            // dispatcher fails immediately; report and let caller fall back.
            eprintln!("[mihomo-daemon] dispatcher unavailable ({e}), running raw console loop");
            Ok(false)
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
async fn handle_windows_pipe(
    mut pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    state: Arc<Mutex<DaemonState>>,
) -> anyhow::Result<()> {
    let cmd_buf = crate::ipc::read_json_payload(&mut pipe, "daemon command").await?;
    let cmd = match parse_daemon_command(&cmd_buf) {
        Ok(cmd) => cmd,
        Err(message) => {
            crate::ipc::write_json_message(&mut pipe, &DaemonResponse::Error { message }).await?;
            return Ok(());
        }
    };
    let peer = PeerAuth {
        uid: None,
        elevated: windows_pipe_client_is_elevated(&pipe),
    };
    // Mirrors the Unix connection handler: lifecycle commands serialize
    // end-to-end under OWNER_LIFECYCLE_LOCK while non-lifecycle commands stay
    // lock-free.
    let response = if is_lifecycle_command(&cmd) {
        let lock = OWNER_LIFECYCLE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
        let _guard = lock.lock().await;
        process_command(cmd, state, peer).await
    } else {
        process_command(cmd, state, peer).await
    };
    crate::ipc::write_json_message(&mut pipe, &response).await?;
    Ok(())
}

#[cfg(windows)]
/// Whether the named-pipe client runs with an elevated (Administrator) token.
/// Windows counterpart of the Unix root-only TUN gate. Fail closed: any lookup
/// failure denies.
fn windows_pipe_client_is_elevated(
    pipe: &tokio::net::windows::named_pipe::NamedPipeServer,
) -> bool {
    use std::os::windows::io::AsRawHandle;

    // SAFETY: handles are null-checked and closed on every path; no pointer
    // escapes this function.
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::Security::{
            GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
        };
        use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        let pipe_handle = pipe.as_raw_handle() as HANDLE;
        let mut client_pid: u32 = 0;
        if GetNamedPipeClientProcessId(pipe_handle, &mut client_pid) == 0 {
            return false;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, client_pid);
        if process.is_null() {
            return false;
        }
        let mut token: HANDLE = std::ptr::null_mut();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token);
        CloseHandle(process);
        if opened == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut returned: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
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

/// Global lifecycle lock — serializes all lifecycle commands (start/stop/restart/
/// TUN toggle) end-to-end, aligned with clash-verge-service OWNER_LIFECYCLE_LOCK.
static OWNER_LIFECYCLE_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> =
    std::sync::OnceLock::new();

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

/// test-only override for the privileged runtime directory.
/// On machines where the system service is installed, `/var/lib/mihomo-cli`
/// is root-owned mode 0770 and unit tests reading it fail with EACCES.
/// Tests that exercise `process_command` (journal gate) call
/// [`isolate_daemon_runtime_for_tests`] to point runtime resolution at an
/// isolated directory. Production code paths are unaffected (`#[cfg(test)]`).
#[cfg(test)]
pub(crate) static TEST_RUNTIME_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn isolate_daemon_runtime_for_tests() {
    TEST_RUNTIME_DIR.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("mihomo-cli-test-runtime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create isolated test runtime dir");
        dir
    });
}

// ---------------- Selection intent mirror (fixed runtime) ----------------
// The daemon never reads the user home tree. CLI pushes selection intent
// copies via RecordSelectionIntent / RecordActiveSubscription; the daemon is
// the only writer of this mirror and replays from it after restart/autostart
// (SPEC §3.8.1 mirror contract).

fn selection_mirror_dir_in(base: &std::path::Path) -> PathBuf {
    base.join("selections")
}

fn selection_mirror_active_path_in(base: &std::path::Path) -> PathBuf {
    selection_mirror_dir_in(base).join("active")
}

fn selection_mirror_file_path_in(base: &std::path::Path, subscription_id: &str) -> PathBuf {
    selection_mirror_dir_in(base).join(format!("{subscription_id}.yaml"))
}

fn valid_mirror_subscription_id(subscription_id: &str) -> bool {
    subscription_id.starts_with("sub-")
        && subscription_id.len() == 12
        && subscription_id.as_bytes()[4..]
            .iter()
            .all(u8::is_ascii_hexdigit)
}

fn write_selection_mirror_file(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("selection mirror path has no parent"))?;
    if !dir.is_dir() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o750));
        }
    }
    crate::utils::atomic_write_bytes_no_follow(path, bytes, 0o640)
}

fn record_selection_mirror_in(
    base: &std::path::Path,
    subscription_id: &str,
    content_yaml: &str,
) -> anyhow::Result<()> {
    if !valid_mirror_subscription_id(subscription_id) {
        anyhow::bail!("invalid subscription id for selection mirror: {subscription_id}");
    }
    // Validate schema before persisting anything.
    crate::selection::parse_selection_map(content_yaml.as_bytes())?;
    write_selection_mirror_file(
        &selection_mirror_file_path_in(base, subscription_id),
        content_yaml.as_bytes(),
    )
}

fn record_selection_mirror(subscription_id: &str, content_yaml: &str) -> anyhow::Result<()> {
    record_selection_mirror_in(&daemon_config_dir(), subscription_id, content_yaml)
}

fn record_active_subscription_mirror_in(
    base: &std::path::Path,
    subscription_id: &str,
) -> anyhow::Result<()> {
    if !valid_mirror_subscription_id(subscription_id) {
        anyhow::bail!("invalid subscription id for active mirror: {subscription_id}");
    }
    write_selection_mirror_file(
        &selection_mirror_active_path_in(base),
        subscription_id.as_bytes(),
    )
}

fn record_active_subscription_mirror(subscription_id: &str) -> anyhow::Result<()> {
    record_active_subscription_mirror_in(&daemon_config_dir(), subscription_id)
}

/// SPEC §3.8.3: the mirror is bound to the `(config revision, subscription id)`
/// the CLI asked to run. The daemon is the only writer and publishes the sidecar
/// atomically next to the mirror files (same directory, tmp + rename).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct MirrorBinding {
    pub subscription_id: String,
    /// Missing on bindings recorded before this binding existed: such a record is
    /// unauthenticated (not trusted, and not a permanent disable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_revision: Option<String>,
}

fn selection_mirror_binding_path_in(base: &std::path::Path) -> PathBuf {
    selection_mirror_dir_in(base).join("binding.json")
}

fn record_mirror_binding_in(
    base: &std::path::Path,
    subscription_id: &str,
    config_revision: Option<&str>,
) -> anyhow::Result<()> {
    if !valid_mirror_subscription_id(subscription_id) {
        anyhow::bail!("invalid subscription id for mirror binding: {subscription_id}");
    }
    let binding = MirrorBinding {
        subscription_id: subscription_id.to_string(),
        config_revision: config_revision.map(str::to_string),
    };
    let json = serde_json::to_vec(&binding)?;
    write_selection_mirror_file(&selection_mirror_binding_path_in(base), &json)
}

fn record_mirror_binding(
    subscription_id: &str,
    config_revision: Option<&str>,
) -> anyhow::Result<()> {
    record_mirror_binding_in(&daemon_config_dir(), subscription_id, config_revision)
}

fn read_mirror_binding_in(base: &std::path::Path) -> Option<MirrorBinding> {
    let raw = std::fs::read(selection_mirror_binding_path_in(base)).ok()?;
    let binding: MirrorBinding = serde_json::from_slice(&raw).ok()?;
    valid_mirror_subscription_id(&binding.subscription_id).then_some(binding)
}

fn read_mirror_active_id_in(base: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(selection_mirror_active_path_in(base)).ok()?;
    let id = raw.trim();
    valid_mirror_subscription_id(id).then(|| id.to_string())
}

fn read_mirror_active_id() -> Option<String> {
    read_mirror_active_id_in(&daemon_config_dir())
}

fn read_mirror_selection_map_in(
    base: &std::path::Path,
    subscription_id: &str,
) -> Option<std::collections::BTreeMap<String, String>> {
    let raw = std::fs::read(selection_mirror_file_path_in(base, subscription_id)).ok()?;
    crate::selection::parse_selection_map(&raw).ok()
}

fn read_mirror_selection_map(
    subscription_id: &str,
) -> Option<std::collections::BTreeMap<String, String>> {
    read_mirror_selection_map_in(&daemon_config_dir(), subscription_id)
}

/// Sampler-side mirror read outcome: absence of intent is "nothing to compare"
/// (not drift), while an existing-but-unreadable/unparsable mirror is a fault
/// that must never be recorded as a healthy sample.
enum MirrorSample {
    Absent,
    Map(std::collections::BTreeMap<String, String>),
    Fault(anyhow::Error),
}

fn read_mirror_selection_map_for_sample(subscription_id: &str) -> MirrorSample {
    let path = selection_mirror_file_path_in(&daemon_config_dir(), subscription_id);
    match std::fs::read(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => MirrorSample::Absent,
        Err(err) => MirrorSample::Fault(anyhow::anyhow!("read {}: {err}", path.display())),
        Ok(raw) => match crate::selection::parse_selection_map(&raw) {
            Ok(map) => MirrorSample::Map(map),
            Err(err) => MirrorSample::Fault(anyhow::anyhow!("parse {}: {err:#}", path.display())),
        },
    }
}

/// 结构化信号：selection mirror 缺失。
///
/// review MAJOR-5：daemon 按 SPEC（§3.8.1）**从不读用户 home**，因此无法判断用户是否
/// 真有持久化选点——把它当 mirror 自己去判断会恒等于 mirror 是否存在（判断退化）。
/// daemon 只回传这个信号，最终措辞（“无事可回放” vs “需要重建 mirror”）由 CLI 用
/// **用户自己的 selections 树**决定，避免把真实 mirror 丢失掩盖成 `✓`（§4.8）。
pub(crate) const SELECTION_MIRROR_MISSING_SIGNAL: &[u8] = b"[selection-mirror-missing]";
pub(crate) const SELECTION_IDENTITY_MISMATCH_SIGNAL: &[u8] = b"[selection-identity-mismatch]";

/// Content revisions of selection mirror files for reconciliation (R3.2).
/// `None` per id means the mirror file is absent. Ids that would not be
/// accepted for recording are rejected wholesale (fail-closed, no path
/// traversal into the mirror directory).
fn selection_mirror_revisions_in(
    base: &std::path::Path,
    subscription_ids: &[String],
) -> anyhow::Result<std::collections::BTreeMap<String, Option<String>>> {
    if let Some(invalid) = subscription_ids
        .iter()
        .find(|id| !valid_mirror_subscription_id(id))
    {
        anyhow::bail!("invalid subscription id for selection mirror query: {invalid}");
    }
    Ok(subscription_ids
        .iter()
        .map(|id| {
            let revision = std::fs::read(selection_mirror_file_path_in(base, id))
                .ok()
                .map(|bytes| crate::tun_transaction::content_revision(&bytes));
            (id.clone(), revision)
        })
        .collect())
}

// ---------------- Selection drift tracker (B1) ----------------

const DRIFT_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
const DRIFT_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const DRIFT_RESPONSE_BYTE_LIMIT: usize = 1024 * 1024;
const DRIFT_RING_CAPACITY: usize = 256;
const DRIFT_WARN_INTERVAL_PER_GROUP: std::time::Duration = std::time::Duration::from_secs(3600);
const DRIFT_REPLAY_SUPPRESSION: std::time::Duration = std::time::Duration::from_secs(30);

struct DriftTrackerState {
    /// Monotonic operation sequence: every managed mutation (selection,
    /// promote, replay, mirror scope change) increments it on start AND end.
    seq: u64,
    /// Managed mutations currently in flight.
    in_flight: u64,
    /// A select segment is open (runtime PUT → intent persistence → mirror
    /// push). Sampling is deferred until the segment closes explicitly.
    /// No TTL self-heal: operations that cannot prove completion stay unknown.
    select_segment_open: bool,
    /// Subscription the open segment belongs to (mirror active id captured at
    /// open). A `RecordSelectionIntent` only closes the segment when it is
    /// addressed to this subscription (or the owner was unknown).
    select_segment_owner: Option<String>,
    /// While set, replay suppression is active (do not sample).
    suppress_until: Option<std::time::Instant>,
    /// Single-flight guard for the sampler.
    sampling: bool,
    observations: std::collections::VecDeque<crate::ipc::SelectionDriftObservation>,
    unknown_samples: u64,
    last_sample_status: String,
    last_warn_at: std::collections::HashMap<String, std::time::Instant>,
}

impl Default for DriftTrackerState {
    fn default() -> Self {
        DriftTrackerState {
            seq: 0,
            in_flight: 0,
            select_segment_open: false,
            select_segment_owner: None,
            suppress_until: None,
            sampling: false,
            observations: std::collections::VecDeque::new(),
            unknown_samples: 0,
            // Never an empty string: `status` consumers must see a valid value
            // from the very first observation.
            last_sample_status: "unknown".to_string(),
            last_warn_at: std::collections::HashMap::new(),
        }
    }
}

fn drift_tracker() -> &'static std::sync::Mutex<DriftTrackerState> {
    static TRACKER: std::sync::OnceLock<std::sync::Mutex<DriftTrackerState>> =
        std::sync::OnceLock::new();
    TRACKER.get_or_init(|| std::sync::Mutex::new(DriftTrackerState::default()))
}

/// RAII bracket for a managed mutation: bumps the monotonic sequence and the
/// in-flight counter on both edges so a sampler that straddles the operation
/// observes a sequence change and discards its sample (ABA-safe).
struct DriftOpGuard;

impl DriftOpGuard {
    fn open() -> Self {
        if let Ok(mut tracker) = drift_tracker().lock() {
            tracker.seq = tracker.seq.wrapping_add(1);
            tracker.in_flight += 1;
        }
        DriftOpGuard
    }
}

impl Drop for DriftOpGuard {
    fn drop(&mut self) {
        if let Ok(mut tracker) = drift_tracker().lock() {
            tracker.in_flight = tracker.in_flight.saturating_sub(1);
            tracker.seq = tracker.seq.wrapping_add(1);
        }
    }
}

/// Replay bracket: same as [`DriftOpGuard`] but arms the post-replay
/// suppression window on every exit path (success/failure/cancel/budget).
struct ReplayDriftGuard {
    _op: DriftOpGuard,
}

impl ReplayDriftGuard {
    fn open() -> Self {
        ReplayDriftGuard {
            _op: DriftOpGuard::open(),
        }
    }
}

impl Drop for ReplayDriftGuard {
    fn drop(&mut self) {
        if let Ok(mut tracker) = drift_tracker().lock() {
            tracker.suppress_until = Some(std::time::Instant::now() + DRIFT_REPLAY_SUPPRESSION);
        }
    }
}

/// Open a select segment: managed selection writes bracket the runtime PUT
/// and the companion mirror record. Sampling is deferred until the segment
/// closes explicitly (no TTL self-heal: unproven completion stays unknown).
/// The segment is owned by the mirror active subscription at open time so
/// that records for other subscriptions cannot clear it.
fn drift_open_select_segment() {
    if let Ok(mut tracker) = drift_tracker().lock() {
        tracker.select_segment_open = true;
        tracker.select_segment_owner = read_mirror_active_id();
    }
}

/// The companion `RecordSelectionIntent` closes the segment only when it
/// belongs to the segment's subscription; records for other subscriptions
/// (and active-subscription records) must never clear an open segment.
fn drift_close_select_segment_for(subscription_id: &str) {
    if let Ok(mut tracker) = drift_tracker().lock() {
        if tracker
            .select_segment_owner
            .as_deref()
            .is_none_or(|owner| owner == subscription_id)
        {
            tracker.select_segment_open = false;
            tracker.select_segment_owner = None;
        }
    }
}

/// Unconditional close for select error exits: no companion record is coming,
/// and closing lets the sampler observe the true post-attempt state instead
/// of wedging on an unprovable segment.
fn drift_close_select_segment() {
    if let Ok(mut tracker) = drift_tracker().lock() {
        tracker.select_segment_open = false;
        tracker.select_segment_owner = None;
    }
}

/// Arm the replay suppression window (covers a managed API-path selection
/// write and the gap after it; also armed by [`ReplayDriftGuard`]).
fn drift_arm_suppression() {
    if let Ok(mut tracker) = drift_tracker().lock() {
        tracker.suppress_until = Some(std::time::Instant::now() + DRIFT_REPLAY_SUPPRESSION);
    }
}

fn drift_snapshot() -> (Vec<crate::ipc::SelectionDriftObservation>, u64, String) {
    match drift_tracker().lock() {
        Ok(tracker) => (
            tracker.observations.iter().cloned().collect(),
            tracker.unknown_samples,
            tracker.last_sample_status.clone(),
        ),
        Err(_) => (Vec::new(), 0, "unknown".to_string()),
    }
}

/// Most recent observation by `at_unix` (ties broken by tail position).
/// In-place updates keep an observation at its original insertion index, so
/// "latest" must not be taken as `.last()`.
pub(crate) fn latest_drift_observation(
    observations: &[crate::ipc::SelectionDriftObservation],
) -> Option<&crate::ipc::SelectionDriftObservation> {
    observations
        .iter()
        .enumerate()
        .max_by(|(i, a), (j, b)| a.at_unix.cmp(&b.at_unix).then_with(|| i.cmp(j)))
        .map(|(_, o)| o)
}

/// Compare the daemon-owned mirror intent map against a Core `/proxies`
/// payload. Comparison set = mirror map keys, excluding `GLOBAL`/`COMPATIBLE`
/// (synthetic entries); groups absent from the Core response produce no
/// drift (no comparable runtime). Shared by the daemon sampler and the CLI
/// doctor check (B3).
pub(crate) fn compare_selection_drift(
    mirror: &std::collections::BTreeMap<String, String>,
    proxies: &serde_json::Value,
    at_unix: u64,
    subscription_id: Option<&str>,
    mirror_revision: Option<&str>,
) -> Vec<crate::ipc::SelectionDriftObservation> {
    let Some(map) = proxies.get("proxies").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut drifts = Vec::new();
    for (group, intent) in mirror {
        if group == "GLOBAL" || group == "COMPATIBLE" {
            continue;
        }
        let Some(runtime) = map
            .get(group)
            .and_then(|entry| entry.get("now"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if runtime != intent {
            drifts.push(crate::ipc::SelectionDriftObservation {
                group: group.clone(),
                intent: intent.clone(),
                runtime: runtime.to_string(),
                at_unix,
                subscription_id: subscription_id.map(str::to_string),
                mirror_revision: mirror_revision.map(str::to_string),
            });
        }
    }
    drifts
}

/// Apply sampled drift observations to the tracker: per-group in-place update
/// (ring never grows on repeat drift), FIFO eviction beyond the cap, and
/// per-group warning throttling (1/group/hour).
fn apply_drift_observations(
    tracker: &mut DriftTrackerState,
    drifts: Vec<crate::ipc::SelectionDriftObservation>,
    now: std::time::Instant,
) {
    for drift in drifts {
        let should_warn = tracker
            .last_warn_at
            .get(&drift.group)
            .map(|at| now.duration_since(*at) >= DRIFT_WARN_INTERVAL_PER_GROUP)
            .unwrap_or(true);
        if should_warn {
            tracker.last_warn_at.insert(drift.group.clone(), now);
            eprintln!(
                "⚠ selection drift: {} runtime={} intent={}",
                drift.group, drift.runtime, drift.intent
            );
        }
        if let Some(existing) = tracker
            .observations
            .iter_mut()
            .find(|obs| obs.group == drift.group)
        {
            *existing = drift;
        } else {
            if tracker.observations.len() >= DRIFT_RING_CAPACITY {
                tracker.observations.pop_front();
            }
            tracker.observations.push_back(drift);
        }
    }
}

/// One sampler tick: read-only comparison of the mirror intent vs the live
/// Core. Discards the sample (records `unknown`) whenever the daemon was
/// mutating around the fetch, and skips sampling inside suppression/select
/// windows. Never writes config, never fixes, never touches the user tree.
async fn drift_sample_once(state: &Arc<Mutex<DaemonState>>) {
    let seq_before;
    {
        let Ok(mut tracker) = drift_tracker().lock() else {
            return;
        };
        if tracker.sampling {
            return;
        }
        let now = std::time::Instant::now();
        let replay_suppressed = tracker.suppress_until.is_some_and(|until| now < until);
        if replay_suppressed || tracker.select_segment_open || tracker.in_flight > 0 {
            // N4: discarded ticks (suppressed/segment/in_flight) do NOT count
            // as unknown samples — they are pure skips, not failed attempts.
            // Only real failures (Core down, fetch timeout, mirror fault,
            // consistency check failure) increment unknown_samples.
            tracker.last_sample_status = if replay_suppressed && !tracker.select_segment_open {
                "suppressed".to_string()
            } else {
                "unknown".to_string()
            };
            return;
        }
        tracker.sampling = true;
        seq_before = tracker.seq;
    }

    // Final consistency check + publish happen under one tracker lock: the
    // sample is verified only after ALL sampling reads (Core /proxies and the
    // mirror) completed, and observations are published atomically with the
    // verification — no window for a mutation between check and publish.
    // N4: `status="unknown"` means a real failure (Core down, fetch timeout,
    // mirror fault) — these increment unknown_samples. Consistency check
    // failures also increment unknown_samples.
    let finish = |status: &str, drifts: Vec<crate::ipc::SelectionDriftObservation>| {
        if let Ok(mut tracker) = drift_tracker().lock() {
            tracker.sampling = false;
            if status == "unknown" || tracker.seq != seq_before || tracker.in_flight > 0 {
                tracker.unknown_samples = tracker.unknown_samples.wrapping_add(1);
                tracker.last_sample_status = "unknown".to_string();
                return;
            }
            tracker.last_sample_status = status.to_string();
            apply_drift_observations(&mut tracker, drifts, std::time::Instant::now());
        }
    };

    let endpoint = {
        let mut s = state.lock().await;
        reap_exited_core(&mut s);
        if !s.core_running {
            finish("unknown", Vec::new());
            return;
        }
        s.api_endpoint.clone()
    };
    let Some(core_endpoint) = endpoint.as_deref().and_then(api_endpoint_transport) else {
        finish("unknown", Vec::new());
        return;
    };
    let client = mihomo_api::EndpointMihomoApiClient::new(core_endpoint);
    let fetched = tokio::time::timeout(
        DRIFT_FETCH_TIMEOUT,
        client.get_bounded("/proxies", DRIFT_RESPONSE_BYTE_LIMIT),
    )
    .await;
    let proxies = match fetched {
        Ok(Ok(value)) => value,
        _ => {
            finish("unknown", Vec::new());
            return;
        }
    };

    let at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let Some(active_id) = read_mirror_active_id() else {
        // No active mirror intent → nothing to compare (missing intent is not
        // drift); the consistency verdict is still made inside `finish`.
        finish("ok", Vec::new());
        return;
    };
    let mirror = match read_mirror_selection_map_for_sample(&active_id) {
        MirrorSample::Map(map) => map,
        // The mirror exists but cannot be read/parsed: the comparison basis is
        // unavailable — record `unknown`, never `ok`.
        MirrorSample::Fault(error) => {
            crate::log!("selection mirror sample read failed (recorded as unknown): {error:#}");
            finish("unknown", Vec::new());
            return;
        }
        MirrorSample::Absent => {
            finish("ok", Vec::new());
            return;
        }
    };

    let mirror_path = selection_mirror_file_path_in(&daemon_config_dir(), &active_id);
    let mirror_revision = std::fs::read(&mirror_path)
        .ok()
        .map(|bytes| crate::tun_transaction::content_revision(&bytes));
    let drifts = compare_selection_drift(
        &mirror,
        &proxies,
        at_unix,
        Some(&active_id),
        mirror_revision.as_deref(),
    );
    finish("ok", drifts);
}

/// Sampler loop: fixed 60s cadence; daemon restart starts fresh (no inherited
/// `ok`, counters reset with the process).
async fn run_selection_drift_sampler(state: Arc<Mutex<DaemonState>>, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(DRIFT_SAMPLE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // First tick fires immediately; skip it so the sampler starts from a
    // quiet baseline rather than racing daemon startup replay.
    interval.tick().await;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = interval.tick() => {
                drift_sample_once(&state).await;
                // SPEC B2 / A2: merged diagnostic tick (60s cycle, same as drift sampler)
                diagnose_transaction_staleness().await;
                diagnose_cache_db_hygiene();
                diagnose_quarantine_strays();
            },
        }
    }
}

/// SPEC B2: diagnostic tick for transaction staleness detection.
/// Reads journal without holding locks, alerts on stale or age=unknown
/// non-terminal phases. Rate limited to 1 alert per hour.
async fn diagnose_transaction_staleness() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_ALERT_SECS: AtomicU64 = AtomicU64::new(0);

    let Some(ctx) = daemon_transaction_context() else {
        return;
    };

    let journal = match crate::tun_transaction::read_active_journal(&ctx) {
        Ok(Some(j)) => j,
        Ok(None) => return,
        Err(_) => return,
    };

    let age = match crate::tun_transaction::journal_age_classification(&journal) {
        Some(age) => age,
        // Old journals without `updated_at`: diagnose only, never write. Not
        // silent — B2 requires an alert for age=unknown as well.
        None => crate::tun_transaction::JournalAge::Unknown,
    };

    let age_info = match age {
        crate::tun_transaction::JournalAge::Stale { age_secs } => format!("stale ({}s)", age_secs),
        crate::tun_transaction::JournalAge::Unknown => "unknown age".to_string(),
        crate::tun_transaction::JournalAge::Terminal
        | crate::tun_transaction::JournalAge::Fresh { .. } => return,
    };

    // Rate limit: 1 alert per hour
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let last_alert = LAST_ALERT_SECS.load(Ordering::Relaxed);
    if now_secs.saturating_sub(last_alert) < 3600 {
        return;
    }
    LAST_ALERT_SECS.store(now_secs, Ordering::Relaxed);

    eprintln!(
        "⚠ TUN transaction {} (gen {}, phase {:?}) is {} — fix: mihomo-cli restart --system",
        journal.transaction_id, journal.generation, journal.phase, age_info
    );
}

/// SPEC A2/A3: read-only cache.db diagnosis on the merged 60s tick.
/// Zero write actions; at most one alert per hour per anomaly kind.
#[cfg(any(unix, windows))]
fn diagnose_cache_db_hygiene() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_ALERT_SECS: AtomicU64 = AtomicU64::new(0);

    let path = system_runtime_data_dir().join(crate::utils::CACHE_DB_FILE_NAME);
    let classification = crate::utils::classify_cache_db_detailed(&path, cache_db_service_user());
    let state = classification.state;
    let needs_attention = matches!(
        state,
        crate::utils::CacheDbState::PermissionDrift
            | crate::utils::CacheDbState::ContentInvalid
            | crate::utils::CacheDbState::Unrecognized
    ) || classification.needs_isolation();
    if !needs_attention {
        return;
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let last_alert = LAST_ALERT_SECS.load(Ordering::Relaxed);
    if now_secs.saturating_sub(last_alert) < 3600 {
        return;
    }
    LAST_ALERT_SECS.store(now_secs, Ordering::Relaxed);

    eprintln!(
        "⚠ cache.db is {} at {} — fix: mihomo-cli restart --system",
        state.as_str(),
        path.display()
    );
}

#[cfg(not(any(unix, windows)))]
fn diagnose_cache_db_hygiene() {}

/// SPEC A2: patrol the runtime data dir for `cache.db.corrupt-*` files
/// that do not match the strict quarantine artifact name (hand-crafted or
/// foreign). These滞留 silently; the diagnostic tick surfaces them as a
/// rate-limited warning (1/hour) without deleting.
#[cfg(any(unix, windows))]
fn diagnose_quarantine_strays() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_ALERT_SECS: AtomicU64 = AtomicU64::new(0);

    let dir = system_runtime_data_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let prefix = crate::utils::CACHE_DB_QUARANTINE_PREFIX;
    let strays: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name_str = name.to_str()?;
            if !name_str.starts_with(prefix) {
                return None;
            }
            if crate::utils::is_quarantine_artifact_name(name_str) {
                return None;
            }
            Some(name_str.to_string())
        })
        .collect();
    if strays.is_empty() {
        return;
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let last_alert = LAST_ALERT_SECS.load(Ordering::Relaxed);
    if now_secs.saturating_sub(last_alert) < 3600 {
        return;
    }
    LAST_ALERT_SECS.store(now_secs, Ordering::Relaxed);

    let sample = strays.first().cloned().unwrap_or_default();
    eprintln!(
        "⚠ {} unrecognized quarantine-like file(s) in {} (e.g. {}) — inspect manually, not auto-removed",
        strays.len(),
        dir.display(),
        sample
    );
}

#[cfg(not(any(unix, windows)))]
fn diagnose_quarantine_strays() {}

/// Best-effort replay of persisted selection intent against the running Core
/// (SPEC-select-persistence §3.2/§3.3). Returns user-facing report lines;
/// failures degrade to warning lines instead of failing the caller. Holds no
/// state lock while awaiting (replay has its own ≤5s budget).
struct DaemonSelectionApiClient {
    state: Arc<Mutex<DaemonState>>,
    inner: mihomo_api::EndpointMihomoApiClient,
}

impl mihomo_api::MihomoApiClient for DaemonSelectionApiClient {
    async fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
        self.inner.get(path).await
    }

    async fn get_bounded(
        &self,
        path: &str,
        byte_limit: usize,
    ) -> anyhow::Result<serde_json::Value> {
        self.inner.get_bounded(path, byte_limit).await
    }

    async fn put(&self, path: &str, body: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let prefix = "/proxies/";
        if let Some(encoded_group) = path.strip_prefix(prefix) {
            let group = percent_decode_proxy_segment(encoded_group)?;
            let node = body["name"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("proxy selection payload has no node name"))?;
            return match select_system_proxy(
                Arc::clone(&self.state),
                group,
                node.to_string(),
                SelectSegmentMode::ApiPath,
            )
            .await
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

/// SPEC §3.8.3 startup replay admission inputs. The decision is a pure function so
/// the suppression table cannot drift between the two startup paths.
pub(crate) struct StartupReplayInputs<'a> {
    pub mirror_active: Option<&'a str>,
    /// `"ready"` | `"missing"` | `"corrupt"`.
    pub map_state: &'a str,
    pub binding: Option<&'a MirrorBinding>,
    pub launched_revision: Option<&'a str>,
    pub transaction_unsettled: bool,
}

/// Why startup replay is suppressed (rendered as a fact line, never as success).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartupReplaySkip {
    pub reason: String,
    /// True when the cause is an unprovable/mismatched subscription identity —
    /// rendered with the structured identity signal so the CLI can translate it.
    pub identity: bool,
}

/// SPEC §3.8.3: only replay when the mirror can be proven to belong to the
/// configuration the Core actually loaded. Absence of a CLI request is NOT a
/// suppression reason (normal boot must recover selections after a reboot).
pub(crate) fn startup_replay_admission(
    inputs: &StartupReplayInputs<'_>,
) -> Result<(), StartupReplaySkip> {
    let skip = |reason: String, identity: bool| Err(StartupReplaySkip { reason, identity });
    if inputs.transaction_unsettled {
        return skip(
            "promotion/recovery transaction is not settled".to_string(),
            false,
        );
    }
    let Some(active) = inputs.mirror_active else {
        return skip(
            "selection mirror has no active subscription".to_string(),
            false,
        );
    };
    match inputs.map_state {
        "missing" => return skip("selection mirror unavailable".to_string(), false),
        "corrupt" => return skip("selection mirror unreadable".to_string(), false),
        _ => {}
    }
    let Some(binding) = inputs.binding else {
        return skip(
            "launched configuration subscription identity unprovable (no mirror binding)"
                .to_string(),
            true,
        );
    };
    if binding.subscription_id != active {
        return skip(
            format!(
                "mirror identity mismatch: mirror_active={active}, binding={}",
                binding.subscription_id
            ),
            true,
        );
    }
    let Some(launched_revision) = inputs.launched_revision else {
        return skip(
            "launched configuration revision unknown (ownership unprovable)".to_string(),
            true,
        );
    };
    match binding.config_revision.as_deref() {
        Some(bound) if bound == launched_revision => Ok(()),
        Some(bound) => skip(
            format!(
                "mirror bound to configuration {} but the loaded configuration is {}",
                crate::short_revision(bound),
                crate::short_revision(launched_revision)
            ),
            true,
        ),
        None => skip(
            "mirror binding carries no configuration revision (unauthenticated record)".to_string(),
            true,
        ),
    }
}

fn startup_skip_line(skip: &StartupReplaySkip) -> String {
    if skip.identity {
        format!(
            "{} selection subscription identity unprovable at startup: {} (0 replays); run `mihomo-cli restart --system` to replay",
            String::from_utf8_lossy(SELECTION_IDENTITY_MISMATCH_SIGNAL),
            skip.reason
        )
    } else {
        format!(
            "⚠ Selections not replayed at startup: {} (0 replays)",
            skip.reason
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MirrorMapState {
    Missing,
    Corrupt,
    Ready(std::collections::BTreeMap<String, String>),
}

fn read_mirror_map_state_in(base: &std::path::Path, subscription_id: &str) -> MirrorMapState {
    match std::fs::read(selection_mirror_file_path_in(base, subscription_id)) {
        Ok(raw) => match crate::selection::parse_selection_map(&raw) {
            Ok(map) => MirrorMapState::Ready(map),
            Err(_) => MirrorMapState::Corrupt,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => MirrorMapState::Missing,
        Err(_) => MirrorMapState::Corrupt,
    }
}

async fn wait_core_api_ready<C: crate::mihomo_api::MihomoApiClient>(
    client: &C,
    deadline: std::time::Instant,
) -> Result<(), String> {
    loop {
        if client.get("/configs").await.is_ok() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "⚠ Selections not replayed: Core API not ready within {}s (0 replays)",
                crate::selection::REPLAY_TOTAL_BUDGET.as_secs()
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// SPEC §3.8.3: read the mirror + binding from `base`, apply the admission table
/// and replay through the request-carrying budget/degradation path.
pub(crate) async fn replay_mirror_at_startup<C: crate::mihomo_api::MihomoApiClient>(
    base: &std::path::Path,
    launched_revision: Option<&str>,
    transaction_unsettled: bool,
    client: &C,
) -> Vec<String> {
    let mirror_active = read_mirror_active_id_in(base);
    let map = match mirror_active.as_deref() {
        Some(id) => read_mirror_map_state_in(base, id),
        None => MirrorMapState::Missing,
    };
    let map_state = match &map {
        MirrorMapState::Missing => "missing",
        MirrorMapState::Corrupt => "corrupt",
        MirrorMapState::Ready(_) => "ready",
    };
    let binding = read_mirror_binding_in(base);
    let inputs = StartupReplayInputs {
        mirror_active: mirror_active.as_deref(),
        map_state,
        binding: binding.as_ref(),
        launched_revision,
        transaction_unsettled,
    };
    if let Err(skip) = startup_replay_admission(&inputs) {
        return vec![startup_skip_line(&skip)];
    }
    let MirrorMapState::Ready(selections) = &map else {
        return Vec::new();
    };
    let deadline = std::time::Instant::now() + crate::selection::REPLAY_TOTAL_BUDGET;
    if let Err(line) = wait_core_api_ready(client, deadline).await {
        return vec![line];
    }
    match crate::selection::replay_map_until(selections, client, deadline).await {
        Ok(report) => selection_replay_report_lines(&report),
        Err(err) => vec![format!("⚠ Selections not replayed: {err:#}")],
    }
}

/// SPEC §3.8.3: an in-flight promotion/recovery transaction means the runtime is
/// mid-commit — replay is suppressed until it settles.
fn startup_transaction_unsettled() -> bool {
    let Some(ctx) = crate::instance::planned_current_context(crate::instance::InstanceMode::System)
    else {
        return false;
    };
    match crate::tun_transaction::read_active_journal(&ctx) {
        Ok(Some(journal)) => !matches!(
            journal.phase,
            crate::tun_transaction::JournalPhase::IntentCommitted
                | crate::tun_transaction::JournalPhase::RolledBack
        ),
        Ok(None) => false,
        // An unreadable journal is not proof of a settled runtime.
        Err(_) => true,
    }
}

async fn startup_replay_lines(state: &Arc<Mutex<DaemonState>>) -> Vec<String> {
    let (endpoint, launched) = {
        let guard = state.lock().await;
        (
            guard.api_endpoint.clone(),
            guard.launched_config_revision.clone(),
        )
    };
    let Some(endpoint) = endpoint.as_deref().and_then(api_endpoint_transport) else {
        return vec![startup_skip_line(&StartupReplaySkip {
            reason: "system Core has no usable API endpoint".to_string(),
            identity: false,
        })];
    };
    let _replay_guard = ReplayDriftGuard::open();
    let client = mihomo_api::EndpointMihomoApiClient::new(endpoint);
    replay_mirror_at_startup(
        &daemon_config_dir(),
        launched.as_deref(),
        startup_transaction_unsettled(),
        &client,
    )
    .await
}

/// daemon 日志侧渲染：信号只写事实，不下“有没有东西可回放”的结论。
fn render_replay_line_for_log(line: &str) -> String {
    if line.starts_with("[selection-mirror-missing]") {
        "⚠ selection mirror unavailable; replay skipped (daemon does not consult the user tree)"
            .to_string()
    } else {
        line.to_string()
    }
}

async fn replay_mirror_selection_intent(
    state: &Arc<Mutex<DaemonState>>,
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
    let Some(ep) = api_endpoint_transport(&api_endpoint) else {
        return vec!["⚠ Selections not replayed: unsupported Core API endpoint".to_string()];
    };
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
    let Some(selections) = read_mirror_selection_map(subscription_id) else {
        // C + review MAJOR-5：mirror 缺失只回传结构化信号。
        // 两种情形（从未持久化 vs mirror 真丢）由 CLI 侧用用户树判定，见
        // `main.rs::translate_selection_replay_signal`。
        return vec![format!(
            "{} selection mirror unavailable",
            String::from_utf8_lossy(SELECTION_MIRROR_MISSING_SIGNAL)
        )];
    };
    let replay_client = DaemonSelectionApiClient {
        state: Arc::clone(state),
        inner: client,
    };
    match crate::selection::replay_map_until(&selections, &replay_client, deadline).await {
        Ok(report) => {
            // Defect 8: every replay exit (start/restart/promote) carries the
            // remedy matrix, built from the replay's own fetched facts.
            selection_replay_report_lines(&report)
        }
        Err(err) => vec![format!("⚠ Selections not replayed: {err:#}")],
    }
}

/// Daemon-side rendering shared by every replay exit (startup, restart,
/// promotion): the replay report plus the outcome remedy matrix. Regular
/// outcomes are rendered here; mirror-missing / identity-mismatch stay
/// structured signals for the CLI, which owns the user-tree decision.
fn selection_replay_report_lines(report: &crate::selection::ReplayReport) -> Vec<String> {
    let mut lines = report.format_lines();
    lines.extend(crate::selection::selection_replay_remedy_lines(report));
    lines
}

fn daemon_transaction_context() -> Option<crate::instance::InstanceContext> {
    let mut ctx = crate::instance::planned_current_context(crate::instance::InstanceMode::System)?;
    let config_dir = daemon_config_dir();
    #[cfg(test)]
    if let Some(dir) = TEST_RUNTIME_DIR.get() {
        ctx.paths.tun_config_file = dir.join("tun-config.yaml");
    }
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

fn allowed_with_unreadable_journal(cmd: &DaemonCommand) -> bool {
    matches!(
        cmd,
        DaemonCommand::GetStatus { .. }
            | DaemonCommand::GetTransactionStatus { .. }
            | DaemonCommand::GetSelectionMirrorRevisions { .. }
            | DaemonCommand::GetSelectionDrift { .. }
    )
}

/// SPEC §12.4: terminal phases release the gate for every command.
fn transaction_gate_phase_allows_changes(phase: crate::tun_transaction::JournalPhase) -> bool {
    matches!(
        phase,
        crate::tun_transaction::JournalPhase::IntentCommitted
            | crate::tun_transaction::JournalPhase::RolledBack
    )
}

/// The per-command matrix of SPEC §12.4: read-only commands always pass, fenced
/// transaction commands pass only for their own transaction/generation/phase, and
/// every other change command is refused.
fn transaction_command_admitted(
    cmd: &DaemonCommand,
    journal: &crate::tun_transaction::TunJournal,
) -> bool {
    match cmd {
        DaemonCommand::ValidatePreparedRuntime { fence, .. } => {
            fence.transaction_id == journal.transaction_id
                && fence.generation == journal.generation
                && journal.phase == crate::tun_transaction::JournalPhase::Prepared
        }
        DaemonCommand::ApplyPromotedSnapshot { fence, .. } => {
            fence.transaction_id == journal.transaction_id
                && fence.generation == journal.generation
                && journal.phase == crate::tun_transaction::JournalPhase::SnapshotPromoted
        }
        DaemonCommand::QuiesceCandidateRuntime { fence, .. } => {
            fence.transaction_id == journal.transaction_id
                && fence.generation == journal.generation
                && fence.expected_candidate_revision == journal.candidate_revision
                && matches!(
                    journal.phase,
                    crate::tun_transaction::JournalPhase::RollbackPending
                        | crate::tun_transaction::JournalPhase::RecoveryRequired
                )
        }
        DaemonCommand::RestoreOldRuntime { fence, .. } => {
            fence.transaction_id == journal.transaction_id
                && fence.generation == journal.generation
                && journal.phase == crate::tun_transaction::JournalPhase::RollbackPending
        }
        DaemonCommand::AttestCurrentTransaction { fence, .. } => {
            fence.transaction_id == journal.transaction_id
                && fence.generation == journal.generation
                && fence.expected_candidate_revision == journal.candidate_revision
                && journal.phase == crate::tun_transaction::JournalPhase::CoreApplied
        }
        DaemonCommand::ApplyLegacyRecoveryTarget { fence, .. } => {
            fence.transaction_id == journal.transaction_id
                && fence.generation == journal.generation
                && journal.phase == crate::tun_transaction::JournalPhase::RecoveryRequired
        }
        DaemonCommand::GetTransactionStatus { .. }
        | DaemonCommand::GetStatus { .. }
        | DaemonCommand::GetSelectionMirrorRevisions { .. }
        | DaemonCommand::GetSelectionDrift { .. } => true,
        // SPEC B6: Read-only Core API requests are allowed during non-terminal phases
        DaemonCommand::CoreApiRequest { method, .. } => *method == crate::ipc::CoreApiMethod::Get,
        _ => false,
    }
}

/// the SPEC §12.4 gate as a pure decision, so the whole non-terminal
/// matrix is unit-testable and `process_command` cannot drift away from it.
pub(crate) fn gate_admits(
    cmd: &DaemonCommand,
    journal: Option<&crate::tun_transaction::TunJournal>,
) -> bool {
    match journal {
        None => true,
        Some(journal) => {
            transaction_gate_phase_allows_changes(journal.phase)
                || transaction_command_admitted(cmd, journal)
        }
    }
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

    check_and_warn_legacy_authorized_clients();

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
        let runtime_dir = system_runtime_data_dir();
        if let Some(reason) = autostart_transaction_blocker() {
            eprintln!("[mihomo-daemon] autostart skipped: {reason}");
        } else if let Some(config_path) = resolve_autostart_target_config_path(&runtime_dir) {
            eprintln!(
                "[mihomo-daemon] autostart marker present; starting core with {}",
                config_path.display()
            );
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
                "[mihomo-daemon] autostart marker present but no valid config snapshot; skipping core start"
            );
        }
    }

    // If a core ended up running (pid-file recovery or autostart), replay the
    // selection mirror once — idempotent and best-effort (SPEC §3.8.1 mirror
    // contract). The daemon never reads the user home tree; a missing mirror
    // degrades to a warning instead of a fallback read.
    let core_running_at_startup = state.lock().await.core_running;
    if core_running_at_startup {
        // SPEC §3.8.3: normal boot recovers selections when the mirror can be
        // proven to belong to the loaded configuration; otherwise every entry
        // skips replay with an explicit annotation (never a silent claim).
        for line in startup_replay_lines(&state).await {
            eprintln!("[mihomo-daemon] {}", render_replay_line_for_log(&line));
        }
    }

    eprintln!("[mihomo-daemon] listening on {}", socket_path.display());

    // SPEC A2/A3: read-only cache.db diagnosis at startup, then every tick.
    diagnose_cache_db_hygiene();

    tokio::spawn(run_selection_drift_sampler(
        Arc::clone(&state),
        cancel.child_token(),
    ));

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
                        log_connection_error(&e);
                    }
                });
            }
        }
    }
}

/// Determines if a command performs mutating lifecycle or transaction operations
/// that require exclusive serialization under OWNER_LIFECYCLE_LOCK.
/// Non-lifecycle commands (GetStatus, CoreApiRequest, SelectSystemProxy, RecordSelectionIntent, etc.)
/// do not hold the lifecycle lock, avoiding head-of-line blocking.
pub(crate) fn is_lifecycle_command(cmd: &DaemonCommand) -> bool {
    matches!(
        cmd,
        DaemonCommand::StartCore { .. }
            | DaemonCommand::StopCore { .. }
            | DaemonCommand::RestartCore { .. }
            | DaemonCommand::ApplySystemTunSnapshot { .. }
            | DaemonCommand::DisableTun { .. }
            | DaemonCommand::PromoteSystemConfig { .. }
            | DaemonCommand::ValidatePreparedRuntime { .. }
            | DaemonCommand::ApplyPromotedSnapshot { .. }
            | DaemonCommand::QuiesceCandidateRuntime { .. }
            | DaemonCommand::RestoreOldRuntime { .. }
            | DaemonCommand::AttestCurrentTransaction { .. }
            | DaemonCommand::ApplyLegacyRecoveryTarget { .. }
    )
}

#[cfg(unix)]
/// Handle a single IPC connection.
async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: Arc<Mutex<DaemonState>>,
) -> anyhow::Result<()> {
    let peer_uid = stream.peer_cred().ok().map(|cred| cred.uid());
    let peer = PeerAuth {
        uid: peer_uid,
        elevated: false,
    };
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
    // lifecycle commands (start/stop/restart/TUN toggle/transactions) are serialized by a
    // single global mutex held for the entire operation — including core spawn and
    // readiness wait — so concurrent clients cannot interleave lifecycle transitions.
    // Non-lifecycle commands (GetStatus, CoreApiRequest, SelectSystemProxy, etc.)
    // do not take the lifecycle lock, avoiding head-of-line blocking.
    let is_lifecycle = is_lifecycle_command(&cmd);
    let response = if is_lifecycle {
        let lock = OWNER_LIFECYCLE_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
        let _guard = lock.lock().await;
        process_command(cmd, state, peer).await
    } else {
        process_command(cmd, state, peer).await
    };

    // Send response
    send_response(&mut writer, &response).await?;

    Ok(())
}

/// D3: a CLI probe connects, sends its command, and may drop the socket before the
/// response is written. Those transport errors are expected; they are rate limited
/// per kind so they cannot bury the real diagnostic lines.
const PROBE_NOISE_INTERVAL_SECS: u64 = 60;

fn probe_noise_kind(error: &anyhow::Error) -> Option<&'static str> {
    error.chain().find_map(|cause| {
        let io = cause.downcast_ref::<std::io::Error>()?;
        match io.kind() {
            std::io::ErrorKind::BrokenPipe => Some("broken-pipe"),
            std::io::ErrorKind::ConnectionReset => Some("connection-reset"),
            _ => None,
        }
    })
}

/// One emission per kind per window; `admit` returns the number of identical
/// events it swallowed since the previous emission.
#[derive(Default)]
struct ProbeNoiseLimiter {
    last_emitted_secs: std::collections::HashMap<&'static str, u64>,
    suppressed: std::collections::HashMap<&'static str, u64>,
}

impl ProbeNoiseLimiter {
    fn admit(&mut self, kind: &'static str, now_secs: u64) -> Option<u64> {
        match self.last_emitted_secs.get(kind) {
            Some(last) if now_secs.saturating_sub(*last) < PROBE_NOISE_INTERVAL_SECS => {
                *self.suppressed.entry(kind).or_insert(0) += 1;
                None
            }
            _ => {
                self.last_emitted_secs.insert(kind, now_secs);
                Some(self.suppressed.insert(kind, 0).unwrap_or_default())
            }
        }
    }
}

fn daemon_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn log_connection_error(error: &anyhow::Error) {
    let Some(kind) = probe_noise_kind(error) else {
        // Real connection failures stay visible on every occurrence.
        eprintln!("[mihomo-daemon] connection error: {error}");
        return;
    };
    static LIMITER: std::sync::OnceLock<std::sync::Mutex<ProbeNoiseLimiter>> =
        std::sync::OnceLock::new();
    let limiter = LIMITER.get_or_init(std::sync::Mutex::default);
    let now_secs = daemon_now_secs();
    let admitted = match limiter.lock() {
        Ok(mut limiter) => limiter.admit(kind, now_secs),
        Err(poisoned) => poisoned.into_inner().admit(kind, now_secs),
    };
    match admitted {
        None => {}
        Some(0) => eprintln!("[mihomo-daemon] connection error (probe noise: {kind}): {error}"),
        Some(suppressed) => eprintln!(
            "[mihomo-daemon] connection error (probe noise: {kind}, {suppressed} similar suppressed): {error}"
        ),
    }
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

/// Authenticated IPC peer identity. Unix supplies the peer uid; named pipes
/// expose no uid, so Windows carries the client's token elevation instead.
#[derive(Debug, Clone, Copy)]
struct PeerAuth {
    uid: Option<u32>,
    elevated: bool,
}

/// Pure TUN privilege decision, split from the per-OS peer probing so every
/// branch is testable on any host.
fn tun_peer_privilege_allowed(
    is_windows: bool,
    peer_uid: Option<u32>,
    peer_elevated: bool,
) -> Result<(), String> {
    if is_windows {
        return if peer_elevated {
            Ok(())
        } else {
            Err("TUN on/off requires an elevated (Administrator) client".to_string())
        };
    }
    match peer_uid {
        Some(0) => Ok(()),
        Some(_) => Err("TUN on/off requires root privileges".to_string()),
        None => {
            Err("TUN on/off requires root privileges; cannot determine IPC peer uid".to_string())
        }
    }
}

/// Only root (uid 0) may toggle TUN on/off; on Windows only an elevated
/// (Administrator) client may.
fn validate_tun_peer_is_root(peer: PeerAuth) -> Result<(), String> {
    tun_peer_privilege_allowed(cfg!(windows), peer.uid, peer.elevated)
}

async fn process_command(
    cmd: DaemonCommand,
    state: Arc<Mutex<DaemonState>>,
    peer: PeerAuth,
) -> DaemonResponse {
    let client_token = match &cmd {
        DaemonCommand::StartCore { token, .. }
        | DaemonCommand::RestartCore { token, .. }
        | DaemonCommand::ApplySystemTunSnapshot { token, .. }
        | DaemonCommand::PromoteSystemConfig { token, .. }
        | DaemonCommand::RecordSelectionIntent { token, .. }
        | DaemonCommand::RecordActiveSubscription { token, .. }
        | DaemonCommand::GetSelectionMirrorRevisions { token, .. }
        | DaemonCommand::GetSelectionDrift { token }
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
    if let Err(message) = validate_client_token_for_peer(client_token, peer.uid) {
        return DaemonResponse::Error { message };
    }
    if matches!(
        cmd,
        DaemonCommand::ApplySystemTunSnapshot { .. } | DaemonCommand::DisableTun { .. }
    ) {
        if let Err(message) = validate_tun_peer_is_root(peer) {
            return DaemonResponse::Error { message };
        }
    }

    // SPEC §12.4: Phase command allowlist gate for active transaction
    if let Some(ctx) = daemon_transaction_context() {
        match crate::tun_transaction::read_active_journal(&ctx) {
            Ok(Some(journal)) => {
                if !gate_admits(&cmd, Some(&journal)) {
                    return DaemonResponse::Error {
                        message: format!(
                            "Change rejected: a system TUN transaction ({}) is in progress (phase: {:?}).
                             Read-only commands remain available; inspect status or restart the service:
                               mihomo-cli status
                               mihomo-cli restart --system",
                            journal.transaction_id, journal.phase
                        ),
                    };
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
        } => process_core_api_request(method, path, body, state, peer.uid).await,
        DaemonCommand::PromoteSystemConfig {
            config_content,
            config_revision,
            subscription_id,
            change_kind,
            ..
        } => {
            promote_system_config(
                state,
                config_content,
                config_revision,
                subscription_id,
                change_kind,
            )
            .await
        }
        DaemonCommand::RecordSelectionIntent {
            subscription_id,
            content_yaml,
            config_revision,
            ..
        } => {
            let _drift_op = DriftOpGuard::open();
            // Companion edge of the select bracket: closes the segment only
            // when this record belongs to the segment's subscription.
            drift_close_select_segment_for(&subscription_id);
            // SPEC §3.8.3: the binding follows whichever record carries the
            // freshest request identity.
            let _ = record_mirror_binding(&subscription_id, config_revision.as_deref());
            match record_selection_mirror(&subscription_id, &content_yaml) {
                Ok(()) => DaemonResponse::Success {
                    message: "selection intent mirror recorded".to_string(),
                },
                Err(err) => DaemonResponse::Error {
                    message: format!("failed to record selection intent mirror: {err:#}"),
                },
            }
        }
        DaemonCommand::RecordActiveSubscription {
            subscription_id,
            config_revision,
            ..
        } => {
            let _drift_op = DriftOpGuard::open();
            // SPEC §3.8.3: bind the mirror to the request-side config revision so
            // a later startup can prove ownership against the loaded config.
            let _ = record_mirror_binding(&subscription_id, config_revision.as_deref());
            // Recording the active subscription is not the companion record
            // of any select segment: it must never clear an open segment.
            match record_active_subscription_mirror(&subscription_id) {
                Ok(()) => DaemonResponse::Success {
                    message: "active subscription mirror recorded".to_string(),
                },
                Err(err) => {
                    // A2': mirror absence is a fact, not an error
                    // verdict — log it without inferring user-tree intent.
                    crate::log!(
                        "active subscription mirror record failed (fact only, no intent inferred): {err:#}"
                    );
                    DaemonResponse::Error {
                        message: format!("failed to record active subscription mirror: {err:#}"),
                    }
                }
            }
        }
        DaemonCommand::GetSelectionDrift { .. } => {
            let (observations, unknown_samples, sample_status) = drift_snapshot();
            // Fault annotation only: the cache is returned regardless.
            // Journal status comes from bounded read-only `read_active_journal`;
            // it annotates faults but never blocks the cached response.
            let journal = match active_journal_status() {
                (Some(phase), None)
                    if !matches!(
                        phase,
                        crate::tun_transaction::JournalPhase::IntentCommitted
                            | crate::tun_transaction::JournalPhase::RolledBack
                    ) =>
                {
                    Some(format!("in-flight ({phase:?})"))
                }
                (_, Some(error)) => Some(format!("unreadable: {error}")),
                _ => None,
            };
            DaemonResponse::SelectionDrift {
                observations,
                unknown_samples,
                sample_status,
                journal,
            }
        }
        DaemonCommand::GetSelectionMirrorRevisions {
            subscription_ids, ..
        } => match selection_mirror_revisions_in(&daemon_config_dir(), &subscription_ids) {
            Ok(revisions) => DaemonResponse::SelectionMirrorRevisions { revisions },
            Err(err) => DaemonResponse::Error {
                message: format!("failed to read selection mirror revisions: {err:#}"),
            },
        },
        DaemonCommand::SelectSystemProxy { group, node, .. } => {
            select_system_proxy(state, group, node, SelectSegmentMode::Explicit).await
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
            subscription_id,
            ..
        } => {
            promote_system_config(
                state,
                config_content,
                config_revision,
                subscription_id,
                Some(crate::config::ChangeKind::Promote),
            )
            .await
        }
        DaemonCommand::StopCore { .. } => stop_core(state).await,
        DaemonCommand::RestartCore {
            config_content,
            config_revision,
            subscription_id,
            ..
        } => {
            promote_system_config(
                state,
                config_content,
                config_revision,
                subscription_id,
                Some(crate::config::ChangeKind::Promote),
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
                peer,
            )
            .await
        }
        DaemonCommand::ApplyPromotedSnapshot {
            fence,
            target_runtime_tun,
            ..
        } => handle_apply_promoted_snapshot(state, fence, target_runtime_tun, peer).await,
        DaemonCommand::QuiesceCandidateRuntime {
            fence,
            allow_legacy_reset,
            allow_restored_old_quiesce,
            ..
        } => {
            handle_quiesce_candidate_runtime(
                state,
                fence,
                allow_legacy_reset,
                allow_restored_old_quiesce,
                peer,
            )
            .await
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
                peer,
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
                peer,
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
                peer,
            )
            .await
        }
        DaemonCommand::GetTransactionStatus { .. } => handle_get_transaction_status(state).await,
    }
}

fn reap_exited_core(s: &mut DaemonState) {
    let Some(child) = s.core_child.as_mut() else {
        // Orphan recovery from the pid file (with process identity probing) is
        // Unix-only for now; Windows stays child-tracked.
        #[cfg(unix)]
        {
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
        }
        #[cfg(windows)]
        {
            s.core_running = false;
            s.core_pid = None;
            s.launched_config_revision = None;
            return;
        }
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct CorePidMetadata {
    pid: u32,
    config_path: PathBuf,
    core_binary: PathBuf,
    api_endpoint: Option<String>,
    #[serde(default)]
    config_revision: Option<String>,
}

/// SPEC B3: process-level Core liveness, used by the CLI reset path when the
/// daemon is unreachable. Read-only: it never signals or writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CoreLiveness {
    /// No managed Core process is alive.
    #[cfg_attr(not(unix), allow(dead_code))]
    Stopped,
    #[cfg_attr(not(unix), allow(dead_code))]
    Running(u32),
    /// The platform cannot prove it (no cmdline identity probe). Only produced by
    /// the non-Unix implementation.
    #[cfg_attr(unix, allow(dead_code))]
    Unprovable(String),
}

#[cfg(unix)]
pub(crate) fn managed_core_liveness_at(pid_file: &std::path::Path) -> CoreLiveness {
    let Some(metadata) = read_pid_file(pid_file).filter(pid_metadata_is_trusted_system_core) else {
        return CoreLiveness::Stopped;
    };
    if !process_alive(metadata.pid) {
        return CoreLiveness::Stopped;
    }
    if process_matches_core_metadata(&metadata) {
        CoreLiveness::Running(metadata.pid)
    } else {
        // The recorded pid now belongs to an unrelated process: no managed Core.
        CoreLiveness::Stopped
    }
}

#[cfg(unix)]
pub(crate) fn live_managed_core_liveness() -> CoreLiveness {
    managed_core_liveness_at(&core_pid_file_path())
}

#[cfg(not(unix))]
pub(crate) fn live_managed_core_liveness() -> CoreLiveness {
    CoreLiveness::Unprovable("process cmdline identity probing is Unix-only".to_string())
}

fn core_pid_file_path() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/var/run/mihomo/core.pid")
    }
    #[cfg(windows)]
    {
        windows_core_log_file_path().with_file_name("core.pid")
    }
}

fn core_log_file_path() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/var/log/mihomo/mihomo.log")
    }
    #[cfg(windows)]
    {
        windows_core_log_file_path()
    }
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

fn write_pid_file(path: &std::path::Path, metadata: &CorePidMetadata) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format_pid_metadata(metadata));
}

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

/// Map a configured Core API endpoint string to the cross-platform transport.
/// Unix socket paths (plain or `unix://`) and named pipes (`pipe://` or
/// `\\.\pipe\...`) are accepted; anything else (e.g. plain TCP) is refused.
fn api_endpoint_transport(endpoint: &str) -> Option<ApiEndpoint> {
    if let Some(path) = endpoint_unix_path(endpoint) {
        return Some(ApiEndpoint::UnixSocket(PathBuf::from(path)));
    }
    if let Some(pipe) = endpoint_windows_pipe_path(endpoint) {
        return Some(ApiEndpoint::WindowsNamedPipe(pipe.to_string()));
    }
    None
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
/// SPEC 裁决 D-4: "the API answers" only proves *someone* answers. A start
/// may not borrow that response as this attempt's readiness: the peer must be
/// bound to a trusted managed process, the artifact being started, the config
/// revision and this start's causality. A proven managed orphan has identity but
/// no causality; an unprovable peer has neither — both are refused, keeping the
/// evidence and starting nothing.
fn unreusable_endpoint_message(
    endpoint: &str,
    core_binary: &std::path::Path,
    config_path: &std::path::Path,
) -> String {
    #[cfg(unix)]
    {
        let state_dir = runtime_data_dir_for_config(config_path);
        let service_uid = Some(unsafe { libc::geteuid() });
        match crate::core_identity::prove_core_api_peer(endpoint, core_binary, &state_dir, service_uid) {
            crate::core_identity::CoreIdentityProof::Proven(identity) => format!(
                "core API endpoint {endpoint} is served by the managed Core pid {} (exe {}), which this daemon did not start in this attempt: its start causality is unprovable, so its response is not this start's readiness. No Core was started. Run an explicit managed stop/restart (mihomo-cli restart --system) to take the orphan over with an exit proof.",
                identity.pid,
                identity.exe.display()
            ),
            crate::core_identity::CoreIdentityProof::Unprovable(reason) => format!(
                "core API endpoint {endpoint} is reachable but the responding process's managed identity is unprovable ({reason}); refusing to start a second Core. No Core was started; runtime evidence is retained."
            ),
            crate::core_identity::CoreIdentityProof::Absent => duplicate_core_endpoint_message(endpoint),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (core_binary, config_path);
        duplicate_core_endpoint_message(endpoint)
    }
}

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
    /// cache.db could not be handed to the Core (SPEC A3).
    CacheDbInvalid,
    /// Unknown failure — report last N lines of log.
    Unknown { log_tail: String },
}

/// Classify a core startup failure from its log output (ADR-24).
fn classify_core_startup_failure(
    log_tail: &str,
    cache_db: crate::utils::CacheDbState,
) -> CoreFailure {
    let lower = log_tail.to_lowercase();

    let cache_db_failure = lower.contains("invalid database")
        || lower.contains("checksum error")
        || lower.contains("unexpected magic")
        || lower.contains("version mismatch");
    if cache_db_failure
        && (cache_db.requires_quarantine() || lower.contains(crate::utils::CACHE_DB_FILE_NAME))
    {
        return CoreFailure::CacheDbInvalid;
    }

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
        CoreFailure::CacheDbInvalid => {
            "cache.db cannot be opened by the Core and could not be quarantined safely.\n  \
             Fix: mihomo-cli restart --system"
                .to_string()
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

#[cfg(any(unix, windows))]
fn system_runtime_data_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(dir) = TEST_RUNTIME_DIR.get() {
        return dir.clone();
    }
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

/// Inspect a YAML configuration snapshot and return true if `tun.enable == true`.
#[cfg(any(unix, windows))]
pub(crate) fn snapshot_tun_enabled(path: &std::path::Path) -> bool {
    if !path.exists() {
        return false;
    }
    crate::utils::read_file_no_follow_limited(
        path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    )
    .ok()
    .and_then(|bytes| serde_yaml::from_slice::<serde_yaml::Value>(&bytes).ok())
    .and_then(|val| {
        val.get("tun")
            .and_then(|t| t.get("enable"))
            .and_then(|e| e.as_bool())
    })
    .unwrap_or(false)
}

/// Resolve the target configuration file to start on daemon autostart.
///
/// If `tun-config.yaml` exists and contains a valid YAML document with
/// `tun.enable == true`, TUN mode is preferred so that TUN survives daemon/system restarts.
/// Otherwise, fallback to `active-config.yaml` if it exists.
/// Returns `None` if neither valid candidate exists.
#[cfg(any(unix, windows))]
pub(crate) fn resolve_autostart_target_config_path(
    runtime_dir: &std::path::Path,
) -> Option<PathBuf> {
    let tun_path = runtime_dir.join("tun-config.yaml");
    if snapshot_tun_enabled(&tun_path) {
        return Some(tun_path);
    }

    let active_path = runtime_dir.join("active-config.yaml");
    if active_path.exists() {
        return Some(active_path);
    }

    None
}

/// 准入④: a Core start must stay impossible from the stopped proof
/// until destructive cleanup ends. The recovery marker only excludes other
/// recovery agents, so the lifecycle layer enforces the same rule: autostart
/// never starts a Core behind a non-terminal transaction (or an un-migrated
/// legacy journal), under the daemon's startup path.
fn autostart_transaction_blocker() -> Option<String> {
    daemon_transaction_context().and_then(|ctx| autostart_transaction_blocker_for(&ctx))
}

fn autostart_transaction_blocker_for(ctx: &crate::instance::InstanceContext) -> Option<String> {
    if crate::tun_transaction::legacy_journal_path(ctx).exists() {
        return Some("a legacy TUN transaction journal is pending migration".to_string());
    }
    match crate::tun_transaction::read_active_journal(ctx) {
        Ok(Some(journal)) if !transaction_gate_phase_allows_changes(journal.phase) => {
            Some(format!(
                "an active TUN transaction ({}) is in phase {:?}",
                journal.transaction_id, journal.phase
            ))
        }
        Ok(_) => None,
        Err(error) => Some(format!("the active TUN journal is unreadable: {error}")),
    }
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

/// Outcome of resolving the replay scope against the fixed-runtime mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MirrorScopeResolution {
    Match(String),
    /// Mirror active points at a different subscription than the request
    /// (A3' identity-mismatch signal).
    IdentityMismatch {
        request: String,
        mirror_active: String,
    },
    /// No replayable scope (missing/invalid id, no mirror active).
    Unavailable,
}

fn resolve_mirror_selection_scope_in(
    base: &std::path::Path,
    subscription_id: Option<&str>,
) -> MirrorScopeResolution {
    let subscription_id = match subscription_id {
        Some(id) if valid_mirror_subscription_id(id) => id,
        Some(id) => {
            eprintln!("[mihomo-daemon] selection replay skipped: invalid subscription id {id}");
            return MirrorScopeResolution::Unavailable;
        }
        None => {
            eprintln!("[mihomo-daemon] selection replay skipped: selection replay requires subscription_id");
            return MirrorScopeResolution::Unavailable;
        }
    };
    match read_mirror_active_id_in(base) {
        Some(active) if active == subscription_id => {
            MirrorScopeResolution::Match(subscription_id.to_string())
        }
        Some(active) => {
            eprintln!(
                "[mihomo-daemon] selection replay skipped: selection subscription identity mismatch: mirror_active={active}, request={subscription_id}"
            );
            MirrorScopeResolution::IdentityMismatch {
                request: subscription_id.to_string(),
                mirror_active: active,
            }
        }
        None => {
            eprintln!(
                "[mihomo-daemon] selection replay skipped: selection mirror has no active subscription"
            );
            MirrorScopeResolution::Unavailable
        }
    }
}

fn resolve_mirror_selection_scope(subscription_id: Option<&str>) -> MirrorScopeResolution {
    resolve_mirror_selection_scope_in(&daemon_config_dir(), subscription_id)
}

fn prepare_promoted_system_config_content(
    config_content: &str,
    config_revision: &str,
    is_tun_snapshot: bool,
    old_content_backup: Option<&[u8]>,
) -> anyhow::Result<(String, String)> {
    if is_tun_snapshot {
        let existing_tun_block = old_content_backup.and_then(|bytes| {
            let val: serde_yaml::Value = serde_yaml::from_slice(bytes).ok()?;
            val.get("tun").cloned()
        });

        if let Some(existing_tun) = existing_tun_block {
            return crate::tun_transaction::merge_snapshot_tun_block_into_config_yaml(
                config_content,
                &existing_tun,
            );
        } else {
            anyhow::bail!(
                "active system TUN snapshot is missing or lacks a valid 'tun' block. Fix: mihomo-cli restart --system or mihomo-cli tun on --yes"
            );
        }
    }
    Ok((config_content.to_string(), config_revision.to_string()))
}

/// SPEC B3 / R3: rollback must observe the stop result. A Core that cannot
/// be stopped is never restarted on top of, and the failure is reported.
async fn restore_last_known_good(
    state: &Arc<Mutex<DaemonState>>,
    target_path: &Path,
    core_binary: &Path,
    old_content_backup: Option<&[u8]>,
) -> Result<(), String> {
    let Some(old_bytes) = old_content_backup else {
        let _ = std::fs::remove_file(target_path);
        return Err(
            "no previous configuration backup exists; the Core was not restarted during rollback. Fix: mihomo-cli restart --system"
                .to_string(),
        );
    };
    let _ = std::fs::write(target_path, old_bytes);
    match stop_core(Arc::clone(state)).await {
        DaemonResponse::Success { .. } => {}
        DaemonResponse::Error { message } => {
            return Err(format!(
                "previous Core could not be stopped before rollback: {message}.                  Fix: mihomo-cli restart --system"
            ));
        }
        other => {
            return Err(format!(
                "unexpected stop response before rollback: {other:?}"
            ));
        }
    }
    match start_core(
        Arc::clone(state),
        target_path.to_path_buf(),
        core_binary.to_path_buf(),
    )
    .await
    {
        DaemonResponse::Success { .. } => Ok(()),
        DaemonResponse::Error { message } => Err(format!(
            "last-known-good Core could not be restarted: {message}.              Fix: mihomo-cli restart --system"
        )),
        other => Err(format!("unexpected restart response: {other:?}")),
    }
}

/// C / ：判定 promotion 目标是否为受管 TUN 快照。
/// 持久快照（`tun_config_file` 的 `tun.enable`）是唯一判定依据；运行态
/// `config_path` 不参与判定——失配/缺失态同样采信持久证据。
#[cfg(any(unix, windows))]
fn resolve_is_tun_snapshot(tun_config_file: &Path) -> bool {
    snapshot_tun_enabled(tun_config_file)
}

/// apply 前后运行态 TUN 不变量的护栏判据。
/// 不变量：若某次 apply（热重载或全量重启）之前运行态 `tun.enable == true`，
/// 则 apply 之后它必须仍为 `true`——后态为 `Some(false)`（被禁用）或 `None`
/// （不可观测）都视为破坏不变量并触发回滚；apply 前为 `None`（不可观测，
/// 护栏降级为告警）或 `Some(false)`（用户经 `tun off` 显式关闭）都不触发。
fn tun_apply_requires_rollback(tun_before: Option<bool>, tun_after: Option<bool>) -> bool {
    tun_before == Some(true) && tun_after != Some(true)
}

async fn promote_system_config(
    state: Arc<Mutex<DaemonState>>,
    config_content: String,
    config_revision: String,
    subscription_id: Option<String>,
    explicit_change_kind: Option<crate::config::ChangeKind>,
) -> DaemonResponse {
    // B1: promotion is a managed mutation (mirror scope can
    // change); bracket the whole operation so sampling straddling it is
    // discarded.
    let _drift_op = DriftOpGuard::open();
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
            if !transaction_gate_phase_allows_changes(journal.phase) {
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
        // C：`is_tun_snapshot` 不能只依赖易失的运行态证据。daemon 刚重启、
        // Core 未运行或 pid 元数据不可恢复时 `config_path` 为空，若此时判定为非 TUN 快照，
        // restart 会 promote 到 `active-config.yaml`，把受管 TUN 快照永久晾在旧内容上，
        // 三方 revision（launched / active snapshot / intent）从此对不上 →
        // `TUN: unknown`、`Configuration: unknown`、`Health: degraded` 永不闭环。
        // 失配态（`config_path` 漂到 `active-config.yaml`）与缺失态同权，
        // 一律采信持久证据——持久快照是 TUN 意图的权威来源，`config_path` 只是易失运行态，
        // 失配时硬判 false 会把 promotion 降级为热重载，在跑的 TUN 被静默拆除。
        // `tun off` 成功后快照必然写入 `enable: false`，故无条件采信持久快照
        // 不会复活经 `tun off` 关闭的 TUN；经 Core API 直接 `PATCH /configs`
        // 关闭的 TUN 不改写快照，下次 promotion 会按快照拉回 `enable: true`
        // （out-of-band 关闭不被此判定尊重）。
        resolve_is_tun_snapshot(&transaction_ctx.paths.tun_config_file)
    };

    let selection_scope = resolve_mirror_selection_scope(subscription_id.as_deref());

    let target_path = if is_tun_snapshot {
        transaction_ctx.paths.tun_config_file.clone()
    } else {
        crate::tun_transaction::active_config_path(&transaction_ctx)
    };

    let old_content_backup = if target_path.exists() {
        std::fs::read(&target_path).ok()
    } else {
        None
    };

    if let Err(error) = serde_yaml::from_str::<serde_yaml::Value>(&config_content) {
        return DaemonResponse::Error {
            message: format!("system configuration YAML is invalid: {error}"),
        };
    }

    let (final_content, expected_revision) = match prepare_promoted_system_config_content(
        &config_content,
        &config_revision,
        is_tun_snapshot,
        old_content_backup.as_deref(),
    ) {
        Ok(res) => res,
        Err(error) => {
            return DaemonResponse::Error {
                message: error.to_string(),
            };
        }
    };

    let endpoint = match read_api_endpoint_from_content(&final_content) {
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

    if let Some(parent) = target_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&target_path, final_content.as_bytes()) {
        if let Some(old_bytes) = old_content_backup.as_ref() {
            let _ = std::fs::write(&target_path, old_bytes);
        }
        return DaemonResponse::Error {
            message: format!("failed to persist promoted system configuration: {e}"),
        };
    }

    let core_binary = expected_system_core_binary_path();
    if let Err(message) = preflight_system_core_start_request(&target_path, &core_binary) {
        // 终审 MAJOR：此臂位于停止 Core 之前——运行中的 Core 仍在跑旧配置、
        // TUN 未被本次操作动过。这里若调用带 Core 停启的 `restore_last_known_good`，
        // 会把一次无副作用的拒绝升级成"杀掉健康 Core"；而 preflight 失败的现实触发条件
        // （core 二进制缺失）下重启必然失败，数据面全断。故只做纯文件级回滚，
        // 与上方写失败臂对齐。
        if let Some(old_bytes) = old_content_backup.as_ref() {
            let _ = std::fs::write(&target_path, old_bytes);
        } else {
            let _ = std::fs::remove_file(&target_path);
        }
        return DaemonResponse::Error {
            message: format!(
                "{message}. The promoted file was rolled back on disk; no Core stop/start was performed, so a running Core (if any) stays untouched."
            ),
        };
    }

    let (core_running, current_endpoint) = {
        let mut s = state.lock().await;
        reap_exited_core(&mut s);
        (s.core_child.is_some(), s.api_endpoint.clone())
    };

    let change_kind = explicit_change_kind.unwrap_or_else(|| {
        if !is_tun_snapshot {
            if let Some(old_bytes) = old_content_backup.as_deref() {
                if let Ok(old_str) = std::str::from_utf8(old_bytes) {
                    crate::config::ChangeKind::classify_diff(old_str, &final_content)
                } else {
                    crate::config::ChangeKind::Promote
                }
            } else {
                crate::config::ChangeKind::Promote
            }
        } else {
            crate::config::ChangeKind::Promote
        }
    });

    // 把运行态 TUN 观测前移为函数级前置证据——热重载与全量重启
    // 共用同一判据（`tun_apply_requires_rollback`）：apply 前 `tun.enable == true`
    // 时，apply 之后它必须仍为 `true`，否则回滚并显式报错；全量重启分支另并列
    // 要求 payload 声明的 TUN（`is_tun_snapshot`）被 attested 为 true。
    // Core 未运行、无可用 API endpoint、读取失败或超时记为 None，
    // 护栏降级为告警/不触发。
    let tun_before = if core_running {
        match current_endpoint.as_deref().and_then(api_endpoint_transport) {
            Some(ep) => {
                let client = mihomo_api::EndpointMihomoApiClient::new(ep);
                // review MINOR-1：这是护栏前置观测，只给 2s 预算，
                // 避免无读超时的 GET 吃掉 CLI 的 20s 响应预算；超时同样记 None。
                match tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    client.get("/configs"),
                )
                .await
                {
                    Ok(Ok(config)) => config["tun"]["enable"].as_bool(),
                    Ok(Err(_)) | Err(_) => None,
                }
            }
            None => None,
        }
    } else {
        None
    };

    if core_running && change_kind == crate::config::ChangeKind::HotReload {
        if let Some(ep) = current_endpoint.as_deref().and_then(api_endpoint_transport) {
            let client = mihomo_api::EndpointMihomoApiClient::new(ep);
            let reload_result =
                mihomo_api::reload_configs_with_client(&client, &target_path.display().to_string())
                    .await;

            match reload_result {
                Ok(()) => {
                    let tun_after = match client.get("/configs").await {
                        Ok(config) => config["tun"]["enable"].as_bool(),
                        Err(_) => None,
                    };
                    // apply 前 TUN 在跑（Some(true)）而 apply 后为
                    // Some(false)（被禁用）或 None（GET /configs 读不到，不可观测）
                    // 都意味着 TUN 存活无法证明 → 回滚并显式报错，两种情形文案区分；
                    // apply 前为 None/Some(false) 不触发（降级为告警 / 用户显式关闭）。
                    if tun_apply_requires_rollback(tun_before, tun_after) {
                        // review MAJOR-1：护栏文案以共享标记起始，
                        // CLI 侧 `is_tun_guard_error` 只认该常量。
                        let detail = match tun_after {
                            Some(false) => {
                                format!("{TUN_GUARD_ERROR_MARKER}hot reload disabled the running TUN (runtime tun.enable changed from true to false)")
                            }
                            // None：apply 前已证明 TUN 在跑，apply 后却读不到——
                            // 只声明不可观测，不谎称被改。
                            _ => {
                                format!("{TUN_GUARD_ERROR_MARKER}hot reload left the runtime TUN state unobservable (GET /configs no longer reports tun.enable after the reload)")
                            }
                        };
                        let rollback = restore_last_known_good(
                            &state,
                            &target_path,
                            &core_binary,
                            old_content_backup.as_deref(),
                        )
                        .await;
                        let mut message = match &rollback {
                            Ok(()) => format!("{detail} and has been rolled back"),
                            Err(_) => format!("{detail}; rollback did not complete"),
                        };
                        message.push_str(
                            ". Fix: mihomo-cli tun on --yes to restore TUN, then mihomo-cli tun status to verify",
                        );
                        return DaemonResponse::Error {
                            message: with_rollback_outcome(message, rollback),
                        };
                    }
                    let mut state_guard = state.lock().await;
                    reap_exited_core(&mut state_guard);
                    state_guard.launched_config_revision = Some(expected_revision.clone());
                    state_guard.config_path = Some(target_path.clone());
                    if let Some(pid) = state_guard.core_child.as_ref().and_then(|c| c.id()) {
                        write_pid_file(
                            &state_guard.pid_file,
                            &CorePidMetadata {
                                pid,
                                config_path: target_path.clone(),
                                core_binary: core_binary.clone(),
                                api_endpoint: state_guard.api_endpoint.clone(),
                                config_revision: Some(expected_revision.clone()),
                            },
                        );
                    }
                    drop(state_guard);

                    let mut message =
                        "system configuration promoted and runtime applied".to_string();
                    match &selection_scope {
                        MirrorScopeResolution::Match(subscription_id) => {
                            let _replay_guard = ReplayDriftGuard::open();
                            for line in
                                replay_mirror_selection_intent(&state, subscription_id).await
                            {
                                message.push('\n');
                                message.push_str(&line);
                            }
                        }
                        MirrorScopeResolution::IdentityMismatch {
                            request,
                            mirror_active,
                        } => {
                            message.push('\n');
                            message.push_str(&format!(
                                "{} selection subscription identity mismatch: mirror_active={mirror_active}, request={request}",
                                String::from_utf8_lossy(SELECTION_IDENTITY_MISMATCH_SIGNAL)
                            ));
                        }
                        MirrorScopeResolution::Unavailable => {}
                    }
                    // reload 前未能读到 TUN 运行态，护栏未生效，不得静默报成功。
                    if tun_before.is_none() {
                        message.push('\n');
                        message.push_str(
                            "warning: TUN runtime state was not attested before hot reload; TUN survival after promotion is unverified",
                        );
                    }
                    return DaemonResponse::Success { message };
                }
                Err(reload_err) => {
                    crate::log!(
                        "system config hot-reload failed ({reload_err:#}); failing open to promotion"
                    );
                }
            }
        }
    }

    // R3 / SPEC B3: the promotion stop must be observed; a Core that is
    // still running is never replaced underneath itself.
    match stop_core(Arc::clone(&state)).await {
        DaemonResponse::Success { .. } => {}
        response => {
            let detail = match response {
                DaemonResponse::Error { message } => message,
                other => format!("unexpected stop response: {other:?}"),
            };
            return DaemonResponse::Error {
                message: format!(
                    "failed to stop the running Core before applying the system configuration: {detail}\n                       Fix: mihomo-cli restart --system"
                ),
            };
        }
    }
    match start_core(Arc::clone(&state), target_path.clone(), core_binary.clone()).await {
        DaemonResponse::Success { .. } => {
            let mut state_guard = state.lock().await;
            reap_exited_core(&mut state_guard);
            let runtime_revision = state_guard
                .launched_config_revision
                .clone()
                .unwrap_or_default();
            let api_endpoint = state_guard.api_endpoint.clone();
            drop(state_guard);
            if runtime_revision != expected_revision {
                let rollback = restore_last_known_good(
                    &state,
                    &target_path,
                    &core_binary,
                    old_content_backup.as_deref(),
                )
                .await;
                let message = with_rollback_outcome(
                    "system configuration was loaded but its runtime revision could not be attested"
                        .to_string(),
                    rollback,
                );
                // 收口：与 `start_core` 失败臂（下方）完全对称——此臂
                // 同样发生在旧 Core 已被停掉、promoted 配置已接管之后，apply 前
                // TUN 在跑时它不会自己回来；不带共享标记会被 CLI 折叠为 Pending，
                // 用户看不到 TUN 已丢失。标记只作前缀，原文与 rollback 结果全保留。
                let message = match tun_before {
                    Some(true) => format!("{TUN_GUARD_ERROR_MARKER}{message}"),
                    _ => message,
                };
                return DaemonResponse::Error { message };
            }
            // 读取全量重启后的运行态 `tun.enable`，读不到记 None。
            // 运行态不变量与热重载分支共用同一纯函数 `tun_apply_requires_rollback`：
            // apply 前 TUN 在跑（Some(true)）而后态不为 true 就回滚——只凭
            // is_tun_snapshot 会漏掉热重载失败 fail-open 到全量重启的降级分支：
            // `!is_tun_snapshot` 但 TUN 实际在跑时，无 tun 块的 active-config
            // 全量重启会静默拆除 TUN。payload 声明了 TUN（is_tun_snapshot）则
            // 并列要求 attested 为 true。`tun off` 场景 tun_before 为 Some(false)，
            // 不会进入本护栏。
            let tun_after =
                if let Some(ep) = api_endpoint.as_deref().and_then(api_endpoint_transport) {
                    let client = mihomo_api::EndpointMihomoApiClient::new(ep);
                    match client.get("/configs").await {
                        Ok(config) => config["tun"]["enable"].as_bool(),
                        Err(_) => None,
                    }
                } else {
                    None
                };
            if (is_tun_snapshot || tun_apply_requires_rollback(tun_before, tun_after))
                && tun_after != Some(true)
            {
                let rollback = restore_last_known_good(
                    &state,
                    &target_path,
                    &core_binary,
                    old_content_backup.as_deref(),
                )
                .await;
                // review MAJOR-1 + MINOR-2：共享标记让 CLI 侧识别为
                // 护栏 Error；Fix 指引与热重载分支对称。
                let mut message = format!(
                    "{TUN_GUARD_ERROR_MARKER}promoted configuration was loaded but TUN runtime attestation failed: tun.enable is not true"
                );
                message.push_str(
                    ". Fix: mihomo-cli tun on --yes to restore TUN, then mihomo-cli tun status to verify",
                );
                return DaemonResponse::Error {
                    message: with_rollback_outcome(message, rollback),
                };
            }
            let mut message = "system configuration promoted and runtime applied".to_string();
            // Replay the fixed-runtime selection mirror synchronously (≤5s
            // budget) so start/restart report restored selections in the same
            // response. The daemon never falls back to the user tree.
            match &selection_scope {
                MirrorScopeResolution::Match(subscription_id) => {
                    let _replay_guard = ReplayDriftGuard::open();
                    for line in replay_mirror_selection_intent(&state, subscription_id).await {
                        message.push('\n');
                        message.push_str(&line);
                    }
                }
                MirrorScopeResolution::IdentityMismatch {
                    request,
                    mirror_active,
                } => {
                    message.push('\n');
                    message.push_str(&format!(
                        "{} selection subscription identity mismatch: mirror_active={mirror_active}, request={request}",
                        String::from_utf8_lossy(SELECTION_IDENTITY_MISMATCH_SIGNAL)
                    ));
                }
                MirrorScopeResolution::Unavailable => {}
            }
            // review BLOCKER：payload 未声明 TUN、Core 一直在跑，
            // 却没读到 apply 前的 TUN 运行态（前置 GET 失败/超时）——护栏未生效，
            // 全量重启可能已拆掉 TUN，不得静默报成功。措辞与热重载分支对称。
            if !is_tun_snapshot && core_running && tun_before.is_none() {
                message.push('\n');
                message.push_str(
                    "warning: TUN runtime state was not attested before the full restart; TUN survival after promotion is unverified",
                );
            }
            DaemonResponse::Success { message }
        }
        DaemonResponse::Error { message } => {
            let rollback = restore_last_known_good(
                &state,
                &target_path,
                &core_binary,
                old_content_backup.as_deref(),
            )
            .await;
            // review MAJOR-2：TUN 在跑时旧 Core 已被停掉（TUN 设备
            // 随之消失），新配置启动失败意味着 TUN 不会自己回来——该 Error 必须
            // 带护栏标记走 Err 呈现，否则 CLI 折叠为 Pending，用户只看到
            // "Run: mihomo-cli restart"，而 restart 仍从无 tun 块的配置启动。
            // 标记只是前缀，`failed to start core` 的原始细节全部保留。
            let message = with_rollback_outcome(message, rollback);
            let message = match tun_before {
                Some(true) => format!("{TUN_GUARD_ERROR_MARKER}{message}"),
                _ => message,
            };
            DaemonResponse::Error { message }
        }
        response => response,
    }
}

/// R3: surface a rollback failure instead of swallowing it.
fn with_rollback_outcome(message: String, rollback: Result<(), String>) -> String {
    match rollback {
        Ok(()) => message,
        Err(rollback_error) => format!("{message}\n  {rollback_error}"),
    }
}

// ---------------- Transaction IPC Handlers ----------------

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
            match api_endpoint_transport(api_endpoint) {
                Some(ep) => {
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
                }
                None => (false, None),
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

async fn handle_validate_prepared_runtime(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    expected_old_runtime_revision: String,
    expected_old_runtime_tun: bool,
    peer: PeerAuth,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer) {
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

async fn handle_apply_promoted_snapshot(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    target_runtime_tun: bool,
    peer: PeerAuth,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer) {
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

/// 准入③: independent proof that the running runtime belongs to the
/// *current* managed instance. The journal's own claims (`core_running`, the pid
/// file, a binary path string, a matching revision, `legacy_source`) are
/// explicitly not enough: the proof is either a live child handle created by the
/// managed spawn path and still consistent with the instance launch record, or
/// the existing four-part managed-Core attribution (which the stop then follows
/// with a kernel-handle exit observation).
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentInstanceRuntime {
    /// Spawned by this daemon; the pid still matches the launch record.
    LiveChild(u32),
    /// Not a child (daemon restart / adoption); proven through the Core API peer.
    AttestedPeer(u32),
}

#[cfg_attr(not(unix), allow(dead_code))]
impl CurrentInstanceRuntime {
    fn pid(self) -> u32 {
        match self {
            Self::LiveChild(pid) | Self::AttestedPeer(pid) => pid,
        }
    }
}

/// Strict launch-record agreement: the pid file is only the record, the live
/// process is proven separately (child handle or API peer).
#[cfg_attr(not(unix), allow(dead_code))]
fn launch_record_matches_state(metadata: &CorePidMetadata, state: &DaemonState) -> bool {
    state.core_pid == Some(metadata.pid)
        && state
            .core_binary
            .as_ref()
            .is_some_and(|binary| binary == &metadata.core_binary)
        && state
            .api_endpoint
            .as_ref()
            .is_some_and(|endpoint| metadata.api_endpoint.as_ref() == Some(endpoint))
}

#[cfg(unix)]
async fn prove_current_instance_ownership(
    state: &Arc<Mutex<DaemonState>>,
) -> Result<CurrentInstanceRuntime, String> {
    let (runtime, core_binary, api_endpoint) = {
        let mut guard = state.lock().await;
        let metadata = read_pid_file(&guard.pid_file)
            .ok_or_else(|| format!("no instance launch record at {}", guard.pid_file.display()))?;
        if !pid_metadata_is_trusted_system_core(&metadata) {
            return Err(
                "the instance launch record is not a trusted system Core record".to_string(),
            );
        }
        if !launch_record_matches_state(&metadata, &guard)
            || !pid_metadata_matches_state(&metadata, &guard)
        {
            return Err(
                "the instance launch record does not match the daemon's recorded instance"
                    .to_string(),
            );
        }
        let runtime = match guard.core_child.as_mut() {
            Some(child) => {
                match child.try_wait() {
                    Ok(None) => {}
                    Ok(Some(_)) => return Err("the managed Core child already exited".to_string()),
                    Err(error) => {
                        return Err(format!(
                            "cannot observe the managed Core child state: {error}"
                        ))
                    }
                }
                let child_pid = child
                    .id()
                    .ok_or_else(|| "the managed Core child carries no pid".to_string())?;
                if guard.core_pid != Some(child_pid) {
                    return Err(format!(
                        "live Core child pid {child_pid} does not match the recorded pid {:?}",
                        guard.core_pid
                    ));
                }
                if metadata.pid != child_pid {
                    return Err(format!(
                        "instance launch record pid {} does not match the live Core child pid {child_pid}",
                        metadata.pid
                    ));
                }
                CurrentInstanceRuntime::LiveChild(child_pid)
            }
            None => {
                if guard.core_pid.is_none() {
                    return Err("the daemon records no current instance pid".to_string());
                }
                CurrentInstanceRuntime::AttestedPeer(metadata.pid)
            }
        };
        (
            runtime,
            guard.core_binary.clone(),
            guard.api_endpoint.clone(),
        )
    };

    if let CurrentInstanceRuntime::AttestedPeer(recorded_pid) = runtime {
        let (Some(core_binary), Some(api_endpoint)) = (core_binary, api_endpoint) else {
            return Err(
                "the daemon cannot name the managed Core binary/endpoint for the peer proof"
                    .to_string(),
            );
        };
        let state_dir = system_runtime_data_dir();
        let endpoint = api_endpoint;
        let service_uid = Some(unsafe { libc::geteuid() });
        match crate::core_identity::prove_core_api_peer(
            &endpoint,
            &core_binary,
            &state_dir,
            service_uid,
        ) {
            crate::core_identity::CoreIdentityProof::Proven(identity) => {
                if identity.pid != recorded_pid {
                    return Err(format!(
                        "the attested managed Core pid {} does not match the recorded instance pid {recorded_pid}",
                        identity.pid
                    ));
                }
            }
            crate::core_identity::CoreIdentityProof::Absent => return Err(
                "no process answers the managed Core API endpoint; the recorded runtime is stale"
                    .to_string(),
            ),
            crate::core_identity::CoreIdentityProof::Unprovable(reason) => {
                return Err(format!(
                    "cannot prove the managed Core belongs to this instance: {reason}"
                ))
            }
        }
    }
    Ok(runtime)
}

#[cfg(not(unix))]
async fn prove_current_instance_ownership(
    _state: &Arc<Mutex<DaemonState>>,
) -> Result<CurrentInstanceRuntime, String> {
    Err("managed Core ownership is not provable on this platform".to_string())
}

#[cfg(any(unix, windows))]
fn core_api_endpoint_label(endpoint: &crate::instance::ApiEndpoint) -> String {
    match endpoint {
        crate::instance::ApiEndpoint::UnixSocket(path) => format!("unix://{}", path.display()),
        crate::instance::ApiEndpoint::WindowsNamedPipe(pipe) => pipe.clone(),
    }
}

/// 准入⑤: `core_running == false` is daemon bookkeeping, not a stop
/// proof. Before a stopped-now proof is issued, the managed endpoint must be
/// provably absent — a managed Core that still answers (or an unidentifiable
/// listener) refuses and keeps every artifact.
#[cfg(unix)]
async fn assert_managed_runtime_absent(
    ctx: &crate::instance::InstanceContext,
) -> Result<(), String> {
    let state_dir = ctx
        .paths
        .tun_config_file
        .parent()
        .ok_or_else(|| "the managed state directory is undefined".to_string())?;
    let endpoint = core_api_endpoint_label(&ctx.paths.api_endpoint);
    match crate::core_identity::prove_core_api_peer(
        &endpoint,
        &ctx.paths.core_binary,
        state_dir,
        Some(unsafe { libc::geteuid() }),
    ) {
        crate::core_identity::CoreIdentityProof::Absent => Ok(()),
        crate::core_identity::CoreIdentityProof::Proven(identity) => Err(format!(
            "managed Core process {} still answers the API endpoint; refusing a stopped-now proof",
            identity.pid
        )),
        crate::core_identity::CoreIdentityProof::Unprovable(reason) => Err(format!(
            "cannot prove the managed Core is stopped: {reason}"
        )),
    }
}

#[cfg(not(unix))]
async fn assert_managed_runtime_absent(
    ctx: &crate::instance::InstanceContext,
) -> Result<(), String> {
    let endpoint = core_api_endpoint_label(&ctx.paths.api_endpoint);
    if endpoint_is_connectable(&endpoint) {
        return Err(
            "the Core API endpoint still accepts connections and its peer identity cannot be proven on this platform"
                .to_string(),
        );
    }
    Ok(())
}

/// admission (裁决 d = 严格化 b). Every condition below must hold;
/// any failure keeps every artifact and leaves destructive cleanup at zero.
struct LegacyResetRefusal {
    stage: &'static str,
    message: String,
}

async fn legacy_reset_admission(
    ctx: &crate::instance::InstanceContext,
    journal: &crate::tun_transaction::TunJournal,
    observation: &crate::tun_transaction::RuntimeObservation,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<(), LegacyResetRefusal> {
    // ② transaction form + placeholder evidence: present, safely readable,
    // digest-matching, and exactly the migration shape.
    crate::tun_transaction::legacy_reset_evidence_admission(ctx, journal).map_err(|message| {
        LegacyResetRefusal {
            stage: "legacy_reset_evidence",
            message,
        }
    })?;
    // ③ current-instance ownership, independent of the journal.
    let runtime = prove_current_instance_ownership(state)
        .await
        .map_err(|message| LegacyResetRefusal {
            stage: "legacy_reset_ownership",
            message,
        })?;
    if observation.core_pid != Some(runtime.pid()) {
        return Err(LegacyResetRefusal {
            stage: "legacy_reset_ownership",
            message: format!(
                "the proven instance pid {} does not match the observed runtime pid {:?}",
                runtime.pid(),
                observation.core_pid
            ),
        });
    }
    Ok(())
}

async fn handle_quiesce_candidate_runtime(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    allow_legacy_reset: bool,
    allow_restored_old_quiesce: bool,
    peer: PeerAuth,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer) {
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

    // SPEC B3: fence.expected_phase must match journal.phase (unified check)
    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != fence.expected_phase
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

    // SPEC B3: Only RollbackPending and RecoveryRequired phases are allowed
    if !matches!(
        journal.phase,
        crate::tun_transaction::JournalPhase::RollbackPending
            | crate::tun_transaction::JournalPhase::RecoveryRequired
    ) {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Stale {
                observed_transaction_id: Some(journal.transaction_id),
                observed_generation: Some(journal.generation),
                observed_phase: Some(journal.phase),
            },
        };
    }

    let obs = get_runtime_observation(&state).await;

    // SPEC B3 / 去脂裁决②: one stopped proof, no "attested but still running"
    // third state. Nothing running is already a true stopped state; a running
    // candidate or attested old runtime is stopped for both phases.
    if !obs.core_running {
        // `core_running` is bookkeeping. A managed Core that still
        // answers at the endpoint is not stopped, even when the pid record is
        // unreadable; issuing a stopped-now proof here would authorize cleanup
        // under a live process.
        if let Err(reason) = assert_managed_runtime_absent(&ctx).await {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                        stage: "quiesce_stopped_verification".to_string(),
                        retryable: false,
                        message: reason,
                    },
                },
            };
        }
        let proof = CoreStoppedProof::stopped_now(0);
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Completed(quiesced_proof(
                &fence,
                journal.phase,
                &obs,
                &proof,
            )),
        };
    }

    // Identity is a disjunction: a candidate that matches the journal fingerprint is
    // self-proving and must not depend on the old-runtime artifact. Only an
    // unproven candidate needs `old-runtime.json`; its failure stays a rejection.
    let matches_candidate = crate::tun_transaction::runtime_matches_candidate(&journal, &obs);
    let old_evidence = if matches_candidate {
        None
    } else {
        match crate::tun_transaction::read_and_validate_old_runtime(&ctx, &journal) {
            Ok(evidence) => Some(evidence),
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
        }
    };
    let matches_old = old_evidence.as_ref().is_some_and(|evidence| {
        crate::tun_transaction::runtime_matches_old_evidence(evidence, &obs)
    });
    let matches_restored_old = old_evidence.as_ref().is_some_and(|evidence| {
        crate::tun_transaction::runtime_matches_restored_old_state(evidence, &obs)
    });

    let mut stage = if matches_candidate {
        "quiesce_candidate"
    } else {
        "quiesce_old_runtime"
    };
    if !matches_candidate && !matches_old {
        // a managed Core may still run the old revision/TUN with a new
        // PID after a restart. Only an explicit runtime reset may stop it, and only
        // after the daemon proves the live process belongs to this instance.
        if allow_restored_old_quiesce && matches_restored_old {
            let observed_pid = obs.core_pid;
            match prove_current_instance_ownership(&state).await {
                Ok(runtime) if Some(runtime.pid()) == observed_pid => {
                    stage = "quiesce_restored_old_runtime";
                }
                Ok(runtime) => {
                    return DaemonResponse::Transaction {
                        response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                            observation: obs,
                            error: crate::tun_transaction::StructuredError {
                                code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                                stage: "quiesce_restored_old_ownership".to_string(),
                                retryable: false,
                                message: format!(
                                    "managed ownership proof returned pid {} but the observed Core pid is {:?}",
                                    runtime.pid(),
                                    observed_pid
                                ),
                            },
                        },
                    };
                }
                Err(reason) => {
                    return DaemonResponse::Transaction {
                        response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                            observation: obs,
                            error: crate::tun_transaction::StructuredError {
                                code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                                stage: "quiesce_restored_old_ownership".to_string(),
                                retryable: false,
                                message: format!(
                                    "cannot prove the current managed Core owns this instance: {reason}"
                                ),
                            },
                        },
                    };
                }
            }
        } else if !allow_legacy_reset {
            return DaemonResponse::Transaction {
                response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                    observation: obs,
                    error: crate::tun_transaction::StructuredError {
                        code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                        stage: stage.to_string(),
                        retryable: false,
                        message:
                            "running runtime is neither candidate nor fully attested old runtime"
                                .to_string(),
                    },
                },
            };
        } else {
            match legacy_reset_admission(&ctx, &journal, &obs, &state).await {
                Ok(()) => stage = "quiesce_legacy_reset",
                Err(refusal) => {
                    return DaemonResponse::Transaction {
                        response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                            observation: obs,
                            error: crate::tun_transaction::StructuredError {
                                code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                                stage: refusal.stage.to_string(),
                                retryable: false,
                                message: refusal.message,
                            },
                        },
                    };
                }
            }
        }
    }

    match quiesce_stop_runtime(Arc::clone(&state), stage).await {
        Ok((proof, obs_after)) => DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::Completed(quiesced_proof(
                &fence,
                journal.phase,
                &obs_after,
                &proof,
            )),
        },
        Err((obs_after, detail)) => DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::NotSatisfied {
                observation: obs_after,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::CoreStopFailed,
                    stage: stage.to_string(),
                    retryable: false,
                    message: detail,
                },
            },
        },
    }
}

/// SPEC B3 / 去脂裁决②: stop the running managed runtime and return the
/// single stopped proof. It does not care whether the running runtime is the
/// candidate or the attested old Core — both phases must reach a true stop.
async fn quiesce_stop_runtime(
    state: Arc<Mutex<DaemonState>>,
    stage: &str,
) -> Result<
    (CoreStoppedProof, crate::tun_transaction::RuntimeObservation),
    (crate::tun_transaction::RuntimeObservation, String),
> {
    let (stop_response, stop_proof) = stop_core_with_proof(Arc::clone(&state)).await;
    let obs_after = get_runtime_observation(&state).await;
    match (stop_response, stop_proof) {
        (DaemonResponse::Success { .. }, Some(proof)) if !obs_after.core_running => {
            Ok((proof, obs_after))
        }
        (response, _) => {
            let detail = match response {
                DaemonResponse::Error { message } => message,
                other => format!("unexpected stop response: {other:?}"),
            };
            crate::log!("quiesce stop failed at {stage}: {detail}");
            Err((obs_after, detail))
        }
    }
}

/// SPEC B3: build the quiesce runtime proof from the single stopped proof.
fn quiesced_proof(
    fence: &crate::tun_transaction::TransactionFence,
    phase: crate::tun_transaction::JournalPhase,
    obs_after: &crate::tun_transaction::RuntimeObservation,
    stopped: &CoreStoppedProof,
) -> crate::tun_transaction::RuntimeProof {
    crate::log!(
        "quiesce stopped proof: transaction={} gen={} stopped_pid={} stopped_at={}",
        fence.transaction_id,
        fence.generation,
        stopped.pid,
        stopped.stopped_at
    );
    crate::tun_transaction::RuntimeProof {
        transaction_id: fence.transaction_id.clone(),
        generation: fence.generation,
        observed_phase: phase,
        proof_kind: crate::tun_transaction::RuntimeProofKind::CandidateQuiesced,
        core_identity: obs_after.core_identity.clone().unwrap_or_default(),
        core_pid: 0,
        launched_revision: String::new(),
        runtime_tun: false,
        api_ready: false,
    }
}

async fn handle_restore_old_runtime(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    expected_old_runtime_revision: String,
    expected_old_runtime_tun: bool,
    peer: PeerAuth,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer) {
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
        || journal.phase != fence.expected_phase
        || journal.candidate_revision != fence.expected_candidate_revision
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

    // the request may only ask for the target recorded in the durable
    // evidence, not redefine it.
    if expected_old_runtime_revision != old_runtime.launched_revision
        || expected_old_runtime_tun != old_runtime.runtime_tun
    {
        let obs = get_runtime_observation(&state).await;
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: obs,
                error: crate::tun_transaction::StructuredError {
                    code: crate::tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                    stage: "validate_restore_target".to_string(),
                    retryable: false,
                    message: "restore target does not match the durable old-runtime evidence"
                        .to_string(),
                },
            },
        };
    }

    // Determine launch source
    let obs = get_runtime_observation(&state).await;
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
    // already-restored uses the restored-state predicate (current PID
    // allowed) and reports the current process, not the historical PID.
    if crate::tun_transaction::runtime_matches_restored_old_state(&old_runtime, &obs)
        && matches!(prove_current_instance_ownership(&state).await, Ok(runtime) if Some(runtime.pid()) == obs.core_pid)
    {
        return DaemonResponse::Transaction {
            response: crate::tun_transaction::TransactionResponse::AlreadySatisfied(
                crate::tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id,
                    generation: fence.generation,
                    observed_phase: crate::tun_transaction::JournalPhase::RollbackPending,
                    proof_kind: crate::tun_transaction::RuntimeProofKind::OldRuntimeRestored,
                    core_identity: obs.core_identity.unwrap_or_default(),
                    core_pid: obs.core_pid.unwrap_or_default(),
                    launched_revision: old_runtime.launched_revision,
                    runtime_tun: old_runtime.runtime_tun,
                    api_ready: obs.api_ready,
                },
            ),
        };
    }

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
            if crate::tun_transaction::runtime_matches_restored_old_state(&old_runtime, &obs_after)
                && obs_after.launched_revision.as_deref() == Some(&expected_old_runtime_revision)
                && obs_after.runtime_tun == Some(expected_old_runtime_tun)
                && matches!(prove_current_instance_ownership(&state).await, Ok(runtime) if Some(runtime.pid()) == obs_after.core_pid)
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

async fn handle_attest_current_transaction(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    expected_runtime_revision: String,
    expected_runtime_tun: bool,
    peer: PeerAuth,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer) {
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

async fn handle_apply_legacy_recovery_target(
    state: Arc<Mutex<DaemonState>>,
    fence: crate::tun_transaction::TransactionFence,
    expected_recovery_target_revision: String,
    peer: PeerAuth,
) -> DaemonResponse {
    if let Err(message) = validate_tun_peer_is_root(peer) {
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

async fn handle_get_transaction_status(state: Arc<Mutex<DaemonState>>) -> DaemonResponse {
    let obs = get_runtime_observation(&state).await;
    let journal_result = if let Some(ctx) = daemon_transaction_context() {
        crate::tun_transaction::read_active_journal(&ctx)
    } else {
        Err(anyhow::anyhow!("system instance paths are unavailable"))
    };
    transaction_status_response(obs, journal_result)
}

/// How a selection write is guarded for the drift sampler.
#[derive(Clone, Copy, PartialEq)]
enum SelectSegmentMode {
    /// `SelectSystemProxy` IPC: bracket runtime PUT → intent persistence →
    /// companion mirror record via an owner-matched select segment.
    Explicit,
    /// Core API passthrough (`CoreApiRequest` PUT /proxies/): replay, exit-IP
    /// probes and raw API writes go through here with no companion record in
    /// sight, so they guard via the replay suppression window instead of a
    /// segment (a segment would wedge sampling until the next explicit select).
    ApiPath,
}

async fn select_system_proxy(
    state: Arc<Mutex<DaemonState>>,
    group: String,
    node: String,
    mode: SelectSegmentMode,
) -> DaemonResponse {
    if group.is_empty() || node.is_empty() || group.len() > 512 || node.len() > 512 {
        return DaemonResponse::Error {
            message: "proxy group and node must be non-empty and reasonably sized".to_string(),
        };
    }
    // B1: bracket the managed selection write so the drift sampler
    // never compares a half-applied runtime. Explicit selects open the segment
    // that the companion mirror record closes; API-path writes arm the replay
    // suppression window instead (covers the write plus the trailing gap).
    let _drift_op = DriftOpGuard::open();
    let segment = match mode {
        SelectSegmentMode::Explicit => {
            drift_open_select_segment();
            true
        }
        SelectSegmentMode::ApiPath => {
            drift_arm_suppression();
            false
        }
    };
    let endpoint = {
        let mut state_guard = state.lock().await;
        reap_exited_core(&mut state_guard);
        if !state_guard.core_running {
            if segment {
                drift_close_select_segment();
            }
            return DaemonResponse::Error {
                message: "system Core is not running. Fix: mihomo-cli restart --system".to_string(),
            };
        }
        state_guard.api_endpoint.clone()
    };
    let Some(core_endpoint) = endpoint.as_deref().and_then(api_endpoint_transport) else {
        if segment {
            drift_close_select_segment();
        }
        return DaemonResponse::Error {
            message: "system Core has no usable API endpoint".to_string(),
        };
    };
    let client = mihomo_api::EndpointMihomoApiClient::new(core_endpoint);
    if let Err(error) = mihomo_api::select_proxy_with_client(&client, &group, &node).await {
        if segment {
            drift_close_select_segment();
        }
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
        Ok(_) => {
            if segment {
                drift_close_select_segment();
            }
            DaemonResponse::Error {
                message: "proxy selection was accepted but runtime attestation did not match"
                    .to_string(),
            }
        }
        Err(error) => {
            if segment {
                drift_close_select_segment();
            }
            DaemonResponse::Error {
                message: format!("proxy selection runtime attestation failed: {error}"),
            }
        }
    }
}

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

#[cfg(any(unix, windows))]
fn cache_db_service_user() -> Option<&'static str> {
    // Tests run without the installed service account; the daemon/Core identity
    // there is the current user, which `None` resolves to.
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        Some("mihomo")
    }
}

#[cfg(not(any(unix, windows)))]
fn cache_db_service_user() -> Option<&'static str> {
    None
}

/// SPEC A2 / D2: the single cache.db evidence precondition shared by every
/// Core spawn path (IPC start/restart/promote, daemon autostart, startup and
/// failure retry). `start_core` is their only spawn primitive, and it runs under
/// the lifecycle lock with the Core already proven stopped.
///
/// The daemon **only classifies**: permission drift is reported as
/// `repair_required` and handed to the CLI's confirmation contract (single
/// repair track). Content that cannot be handed over safely is quarantined here,
/// because this is the only holder of the stop-write window.
fn cache_db_gate_for_start(
    state: &mut DaemonState,
    data_dir: &std::path::Path,
) -> Result<crate::utils::CacheDbState, String> {
    if let Some(child) = state.core_child.as_mut() {
        // A live Core still holds the old inode; never rename underneath it.
        if matches!(child.try_wait(), Ok(None)) {
            return Err(
                "refusing to touch cache.db while the Core process is still running".to_string(),
            );
        }
        state.core_child = None;
        state.core_pid = None;
        state.core_running = false;
    }

    let path = data_dir.join(crate::utils::CACHE_DB_FILE_NAME);
    let classification = crate::utils::classify_cache_db_detailed(&path, cache_db_service_user());

    if classification.state == crate::utils::CacheDbState::PermissionDrift {
        // D2: repair is the CLI's single track (SPEC §12.3.1 confirmation contract).
        return Err(format!(
            "repair_required: {} ownership/permissions drifted from the mihomo service user;              the daemon does not self-heal permissions.\n               Fix: mihomo-cli restart --system",
            path.display()
        ));
    }

    if classification.needs_isolation() {
        let reason = if classification.core_openable {
            classification.state.as_str()
        } else {
            "not openable by the Core"
        };
        let quarantined = crate::utils::quarantine_cache_db(&path).map_err(|error| {
            format!(
                "cache.db is {} ({reason}) and could not be quarantined ({error}); refusing to start the Core.\n                   Fix: mihomo-cli restart --system",
                classification.state.as_str()
            )
        })?;
        crate::log!(
            "quarantined cache.db {} as {}",
            path.display(),
            quarantined.display()
        );
        eprintln!(
            "⚠ quarantine {} moved to {} ({reason}; the Core will rebuild it)",
            path.display(),
            quarantined.display()
        );
    }

    Ok(classification.state)
}

async fn start_core(
    state: Arc<Mutex<DaemonState>>,
    config_path: PathBuf,
    core_binary: PathBuf,
) -> DaemonResponse {
    // SPEC B3: shared budget - lifecycle operations ≤15s
    let lifecycle_deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);

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
            message: unreusable_endpoint_message(&api_endpoint, &core_binary, &config_path),
        };
    }

    // SPEC A2: cache.db evidence precondition — only reached with a proven
    // stopped Core (no live child, no reachable endpoint), inside the lifecycle lock.
    let cache_db_data_dir = runtime_data_dir_for_config(&config_path);
    let cache_db_state = match cache_db_gate_for_start(&mut s, &cache_db_data_dir) {
        Ok(state) => state,
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
    #[cfg(unix)]
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
                    let failure = classify_core_startup_failure(&log_tail, cache_db_state);
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
                    // SPEC B3: use shared lifecycle deadline, no independent timing.
                    let endpoint = api_endpoint_transport(&api_endpoint);
                    let ready = match &endpoint {
                        Some(ep) => {
                            mihomo_api::wait_for_api_ready_with_deadline(ep, lifecycle_deadline)
                                .await
                        }
                        None => false,
                    };
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
                        let failure = classify_core_startup_failure(&log_tail, cache_db_state);
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

/// SPEC B3: the single "Core is stopped" proof. Produced only by
/// `stop_core_with_proof` and consumed by the quiesce proof and the reset path.
/// `pid == 0` means "no managed Core process was running"; a non-zero pid means
/// the process was killed and reaped (`wait`) successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CoreStoppedProof {
    pub pid: u32,
    pub stopped_at: String,
}

impl CoreStoppedProof {
    fn stopped_now(pid: u32) -> Self {
        Self {
            pid,
            stopped_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// the privileged CLI compatibility executor produces the same
    /// proof type after a kernel-handle exit observation, so consumers cannot
    /// tell a "different" stopped state.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn observed(pid: u32) -> Self {
        Self::stopped_now(pid)
    }
}

/// Bound for reaping a killed Core; a process that ignores SIGKILL past this is
/// treated as still running so no evidence may be touched.
const STOP_CORE_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn stop_core(state: Arc<Mutex<DaemonState>>) -> DaemonResponse {
    stop_core_with_proof(state).await.0
}

async fn stop_core_with_proof(
    state: Arc<Mutex<DaemonState>>,
) -> (DaemonResponse, Option<CoreStoppedProof>) {
    stop_core_with_proof_timeout(state, STOP_CORE_WAIT_TIMEOUT).await
}

/// SPEC B3: stop the managed Core and return the stopped proof on success.
/// A failed kill or an unreaped child keeps `core_child` in state so liveness
/// checks (including the cache.db gate) still see a running process.
async fn stop_core_with_proof_timeout(
    state: Arc<Mutex<DaemonState>>,
    wait_timeout: std::time::Duration,
) -> (DaemonResponse, Option<CoreStoppedProof>) {
    let mut s = state.lock().await;
    let Some(mut child) = s.core_child.take() else {
        // Orphan termination via pid-file identity probing is Unix-only for
        // now; Windows has no process_cmdline equivalent yet.
        #[cfg(unix)]
        {
            if let Some(metadata) = read_pid_file(&s.pid_file)
                .filter(pid_metadata_is_trusted_system_core)
                .filter(|metadata| process_alive(metadata.pid))
            {
                if !pid_metadata_matches_state(&metadata, &s)
                    || !process_matches_core_metadata(&metadata)
                {
                    return (
                        DaemonResponse::Error {
                            message: format!(
                                "refusing to terminate orphan core process {} because pid metadata/process identity does not match daemon state",
                                metadata.pid
                            ),
                        },
                        None,
                    );
                }
                let pid = metadata.pid;
                // SPEC 裁决 D-2: a successful signal is NOT a stopped proof.
                // Only a stable kernel-handle observation of the exit counts.
                if let Err(error) = terminate_process(pid) {
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return (
                            DaemonResponse::Error {
                                message: format!(
                                    "failed to signal orphan core process {pid}: {error}"
                                ),
                            },
                            None,
                        );
                    }
                }
                match crate::core_identity::observe_process_exit(pid, STOP_CORE_WAIT_TIMEOUT) {
                    crate::core_identity::ProcessExitProof::Exited => {
                        remove_pid_file(&s.pid_file);
                        s.core_running = false;
                        s.core_pid = None;
                        s.launched_config_revision = None;
                        s.api_endpoint = None;
                        let proof = CoreStoppedProof::stopped_now(pid);
                        crate::log!(
                            "core stopped proof: pid={} stopped_at={} (exit observed)",
                            proof.pid,
                            proof.stopped_at
                        );
                        (
                            DaemonResponse::Success {
                                message: format!("orphan core process {pid} exited"),
                            },
                            Some(proof),
                        )
                    }
                    crate::core_identity::ProcessExitProof::StillRunning => (
                        DaemonResponse::Error {
                            message: format!(
                                "orphan core process {pid} did not exit within {}s; no stopped proof (evidence retained, no further action taken)",
                                STOP_CORE_WAIT_TIMEOUT.as_secs()
                            ),
                        },
                        None,
                    ),
                    crate::core_identity::ProcessExitProof::Unprovable(reason) => (
                        DaemonResponse::Error {
                            message: format!(
                                "cannot observe the exit of orphan core process {pid}: {reason}; no stopped proof (evidence retained)"
                            ),
                        },
                        None,
                    ),
                };
            }
        }
        let orphan_endpoint = s
            .api_endpoint
            .as_ref()
            .filter(|endpoint| endpoint_is_connectable(endpoint))
            .cloned();
        if let Some(endpoint) = orphan_endpoint {
            return (
                DaemonResponse::Error {
                    message: format!(
                        "core API endpoint is reachable at {endpoint}, but this daemon cannot prove ownership of the process and has no live pid file. No termination was attempted; runtime evidence is retained."
                    ),
                },
                None,
            );
        }
        s.core_running = false;
        s.core_pid = None;
        s.launched_config_revision = None;
        s.api_endpoint = None;
        remove_pid_file(&s.pid_file);
        let proof = CoreStoppedProof::stopped_now(0);
        crate::log!(
            "core stopped proof: pid=0 stopped_at={} (no managed core was running)",
            proof.stopped_at
        );
        return (
            DaemonResponse::Success {
                message: "core already stopped".to_string(),
            },
            Some(proof),
        );
    };

    let pid = child.id().unwrap_or(0);
    let kill_result = child.kill().await;
    let waited = tokio::time::timeout(wait_timeout, child.wait()).await;
    settle_stop(&mut s, child, pid, kill_result, waited, wait_timeout)
}

/// SPEC B3: turn a kill/wait outcome into a response plus the single stopped
/// proof. A failed or unreaped stop keeps the child in state so liveness proofs
/// (including the cache.db gate) still see a running process.
fn settle_stop(
    s: &mut DaemonState,
    child: tokio::process::Child,
    pid: u32,
    kill_result: std::io::Result<()>,
    waited: Result<std::io::Result<std::process::ExitStatus>, tokio::time::error::Elapsed>,
    wait_timeout: std::time::Duration,
) -> (DaemonResponse, Option<CoreStoppedProof>) {
    if !matches!(waited, Ok(Ok(_))) {
        // Never clear the child on failure: liveness proofs must keep seeing it.
        s.core_child = Some(child);
        s.core_running = true;
        let detail = match waited {
            Ok(Err(error)) => error.to_string(),
            Err(_) => format!(
                "process {} did not exit within {}s",
                pid,
                wait_timeout.as_secs()
            ),
            Ok(Ok(_)) => unreachable!("wait success already handled"),
        };
        return (
            DaemonResponse::Error {
                message: format!("failed to stop core: {detail}"),
            },
            None,
        );
    }
    if let Err(error) = kill_result {
        // The process exited on its own between take and kill; the reaped child
        // is still a valid stopped proof.
        crate::log!("core kill reported {error} but the process was reaped");
    }
    s.core_child = None;
    s.core_running = false;
    s.core_pid = None;
    s.launched_config_revision = None;
    s.api_endpoint = None;
    remove_pid_file(&s.pid_file);
    let proof = CoreStoppedProof::stopped_now(pid);
    crate::log!(
        "core stopped proof: pid={} stopped_at={}",
        proof.pid,
        proof.stopped_at
    );
    (
        DaemonResponse::Success {
            message: "core stopped".to_string(),
        },
        Some(proof),
    )
}

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
    let Some(core_endpoint) = endpoint.as_deref().and_then(api_endpoint_transport) else {
        return DaemonResponse::Error {
            message: "system Core has no usable API endpoint. Fix: mihomo-cli restart --system"
                .to_string(),
        };
    };

    if method == CoreApiMethod::Put && path.starts_with("/proxies/") {
        let group_segment = path
            .strip_prefix("/proxies/")
            .unwrap_or_default()
            .trim_end_matches('/');
        let decoded_group = percent_decode_proxy_segment(group_segment)
            .ok()
            .or_else(|| {
                if !group_segment.is_empty() {
                    Some(group_segment.to_string())
                } else {
                    None
                }
            });
        let node_name = body
            .as_ref()
            .and_then(|b| b.get("name").and_then(|n| n.as_str().map(str::to_string)));
        if let (Some(group), Some(node)) = (decoded_group, node_name) {
            if !group.is_empty() && !node.is_empty() {
                return match select_system_proxy(
                    Arc::clone(&state),
                    group,
                    node,
                    SelectSegmentMode::ApiPath,
                )
                .await
                {
                    DaemonResponse::Success { .. } => DaemonResponse::CoreApi {
                        data: serde_json::Value::Null,
                    },
                    DaemonResponse::Error { message } => DaemonResponse::Error { message },
                    other => other,
                };
            }
        }
    }

    let client = mihomo_api::EndpointMihomoApiClient::new(core_endpoint);
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
        // `--dns-hijack` 支持逗号分隔的多个目标（如 any:53,tcp://any:53）。
        let entries = crate::config::dns_hijack_entries(dns_hijack);
        if !entries.is_empty() {
            tun.insert(
                serde_yaml::Value::String("dns-hijack".to_string()),
                serde_yaml::Value::Sequence(
                    entries.into_iter().map(serde_yaml::Value::String).collect(),
                ),
            );
        }
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
        // `--dns-hijack` 支持逗号分隔的多个目标（如 any:53,tcp://any:53）。
        let entries: Vec<serde_json::Value> = crate::config::dns_hijack_entries(dns_hijack)
            .into_iter()
            .map(serde_json::Value::String)
            .collect();
        if !entries.is_empty() {
            tun.insert("dns-hijack".to_string(), serde_json::Value::Array(entries));
        }
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

/// Tests that write an active journal into the shared isolated runtime dir must
/// hold this lock for the whole window: the §12.4 gate in `process_command`
/// reads that journal and would otherwise see a foreign in-flight transaction.
#[cfg(all(test, unix))]
fn runtime_journal_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(all(test, unix))]
fn peer_with_uid(uid: Option<u32>) -> PeerAuth {
    PeerAuth {
        uid,
        elevated: false,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn runtime_shape_allows_managed_configs_on_every_platform() {
        // Bug #12 regression: the daemon starts Core from
        // <runtime>/active-config.yaml (autostart/promotion) and
        // <runtime>/transactions/active/recovery-target.yaml (recovery) on all
        // platforms; previously only Linux accepted them.
        const LINUX: &str = "/var/lib/mihomo-cli";
        const MACOS: &str = "/Library/Application Support/mihomo-cli";
        const WINDOWS: &str = "/ProgramData/mihomo-cli";

        for runtime in [LINUX, MACOS, WINDOWS] {
            assert!(
                runtime_config_text_allowed(
                    &format!("{runtime}/tun-config.yaml"),
                    Some("tun-config.yaml"),
                    runtime
                ),
                "tun-config must be accepted under {runtime}"
            );
            assert!(
                runtime_config_text_allowed(
                    &format!("{runtime}/active-config.yaml"),
                    Some("active-config.yaml"),
                    runtime
                ),
                "active-config must be accepted under {runtime}"
            );
            assert!(
                runtime_config_text_allowed(
                    &format!("{runtime}/transactions/active/recovery-target.yaml"),
                    Some("recovery-target.yaml"),
                    runtime
                ),
                "recovery-target must be accepted under {runtime}"
            );
        }

        // Windows paths arrive backslash-normalized with a drive prefix.
        assert!(runtime_config_text_allowed(
            "C:/ProgramData/mihomo-cli/active-config.yaml",
            Some("active-config.yaml"),
            WINDOWS
        ));
    }

    #[test]
    fn runtime_shape_rejects_lookalike_and_unknown_paths() {
        const LINUX: &str = "/var/lib/mihomo-cli";
        // Prefix smuggling must not pass the anchored comparison.
        assert!(!runtime_config_text_allowed(
            "/tmp/fake/var/lib/mihomo-cli/active-config.yaml",
            Some("active-config.yaml"),
            LINUX
        ));
        assert!(!runtime_config_text_allowed(
            "/var/lib/mihomo-cli/transactions/active/evil.yaml",
            Some("evil.yaml"),
            LINUX
        ));
        // recovery-target is only valid under transactions/active, not at root.
        assert!(!runtime_config_text_allowed(
            "/var/lib/mihomo-cli/recovery-target.yaml",
            Some("recovery-target.yaml"),
            LINUX
        ));
        // A macOS runtime file is not accepted when checking the Linux runtime.
        assert!(!runtime_config_text_allowed(
            "/Library/Application Support/mihomo-cli/active-config.yaml",
            Some("active-config.yaml"),
            LINUX
        ));
    }

    #[test]
    fn test_promote_preserves_tun_block_when_tun_snapshot() {
        let old_yaml = r#"
mixed-port: 7890
external-controller: 127.0.0.1:9090
tun:
  enable: true
  stack: mixed
  auto-route: true
  dns-hijack:
    - any:53
"#;
        let new_yaml = r#"
mixed-port: 7891
external-controller: 127.0.0.1:9090
rules:
  - DOMAIN-SUFFIX,example.com,DIRECT
"#;
        let new_revision = crate::tun_transaction::sha256_revision(new_yaml.as_bytes());
        let (promoted_content, promoted_revision) = prepare_promoted_system_config_content(
            new_yaml,
            &new_revision,
            true,
            Some(old_yaml.as_bytes()),
        )
        .expect("promotion should succeed");

        let doc: serde_yaml::Value = serde_yaml::from_str(&promoted_content).unwrap();
        assert_eq!(doc["mixed-port"].as_i64(), Some(7891));
        assert_eq!(doc["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(doc["tun"]["stack"].as_str(), Some("mixed"));
        assert_eq!(doc["tun"]["auto-route"].as_bool(), Some(true));
        assert_eq!(
            doc["rules"][0].as_str(),
            Some("DOMAIN-SUFFIX,example.com,DIRECT")
        );
        assert_ne!(promoted_revision, new_revision);
        assert_eq!(
            promoted_revision,
            crate::tun_transaction::sha256_revision(promoted_content.as_bytes())
        );

        // Also test that if new_yaml had tun: enable: false, it is replaced by active tun block
        let new_yaml_with_disabled_tun = r#"
mixed-port: 7891
external-controller: 127.0.0.1:9090
tun:
  enable: false
"#;
        let new_rev2 =
            crate::tun_transaction::sha256_revision(new_yaml_with_disabled_tun.as_bytes());
        let (promoted2, _) = prepare_promoted_system_config_content(
            new_yaml_with_disabled_tun,
            &new_rev2,
            true,
            Some(old_yaml.as_bytes()),
        )
        .expect("promotion should replace inactive tun with snapshot tun");
        let doc2: serde_yaml::Value = serde_yaml::from_str(&promoted2).unwrap();
        assert_eq!(doc2["tun"]["enable"].as_bool(), Some(true));
    }

    #[test]
    fn test_promote_applies_payload_noncontrolled_keys_over_snapshot() {
        let old_yaml = r#"
mixed-port: 7890
tun:
  enable: true
  stack: mixed
  route-exclude-address:
    - 203.0.113.0/24
"#;
        let new_yaml = r#"
mixed-port: 7891
tun:
  route-exclude-address:
    - 192.0.2.0/24
    - 198.51.100.0/24
"#;
        let new_revision = crate::tun_transaction::sha256_revision(new_yaml.as_bytes());
        let (promoted, revision) = prepare_promoted_system_config_content(
            new_yaml,
            &new_revision,
            true,
            Some(old_yaml.as_bytes()),
        )
        .expect("promotion should preserve new non-controlled keys");

        let doc: serde_yaml::Value = serde_yaml::from_str(&promoted).unwrap();
        assert_eq!(doc["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(doc["tun"]["stack"].as_str(), Some("mixed"));
        assert_eq!(
            doc["tun"]["route-exclude-address"],
            serde_yaml::from_str::<serde_yaml::Value>("- 192.0.2.0/24\n- 198.51.100.0/24\n")
                .unwrap()
        );
        assert_eq!(
            revision,
            crate::tun_transaction::sha256_revision(promoted.as_bytes())
        );
    }

    #[test]
    fn test_promote_matches_cli_freeze_merge_bytes_and_revision() {
        let old_yaml = r#"
mixed-port: 7890
tun:
  enable: true
  stack: mixed
  mtu: 1400
"#;
        let new_yaml = r#"
mixed-port: 7891
tun:
  route-exclude-address:
    - 192.0.2.0/24
"#;
        let new_revision = crate::tun_transaction::sha256_revision(new_yaml.as_bytes());
        let old_tun: serde_yaml::Value = serde_yaml::from_str(old_yaml).unwrap();
        let (expected, expected_revision) =
            crate::tun_transaction::merge_snapshot_tun_block_into_config_yaml(
                new_yaml,
                &old_tun["tun"],
            )
            .unwrap();
        let (promoted, revision) = prepare_promoted_system_config_content(
            new_yaml,
            &new_revision,
            true,
            Some(old_yaml.as_bytes()),
        )
        .expect("promotion should succeed");

        assert_eq!(promoted, expected);
        assert_eq!(revision, expected_revision);
        // payload-strict: snapshot-only mtu is not re-injected into the new intent.
        let doc: serde_yaml::Value = serde_yaml::from_str(&promoted).unwrap();
        assert!(doc["tun"].get("mtu").is_none(), "{doc:?}");
    }

    #[test]
    fn test_cli_freeze_and_daemon_promotion_match_bytes_and_revision() {
        let temp = tempfile::tempdir().unwrap();
        let inputs = crate::instance::PathInputs::for_tests();
        let mut ctx = crate::instance::InstanceContext::planned(
            crate::instance::TargetOs::Linux,
            crate::instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.tun_config_file = temp.path().join("tun-config.yaml");
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        let old_yaml = r#"
mixed-port: 7890
tun:
  enable: true
  stack: mixed
  route-exclude-address:
    - 203.0.113.0/24
"#;
        std::fs::write(&ctx.paths.tun_config_file, old_yaml).unwrap();
        let new_yaml = r#"
mixed-port: 7891
tun:
  route-exclude-address:
    - 192.0.2.0/24
"#;
        let new_revision = crate::tun_transaction::sha256_revision(new_yaml.as_bytes());

        let (cli_content, cli_revision) =
            crate::tun_transaction::merge_system_tun_snapshot_into_payload(
                &ctx,
                new_yaml,
                &new_revision,
            )
            .unwrap();
        let (daemon_content, daemon_revision) = prepare_promoted_system_config_content(
            new_yaml,
            &new_revision,
            true,
            Some(old_yaml.as_bytes()),
        )
        .unwrap();

        assert_eq!(cli_content, daemon_content);
        assert_eq!(cli_revision, daemon_revision);
        assert_eq!(
            cli_revision,
            crate::tun_transaction::sha256_revision(cli_content.as_bytes())
        );
    }

    #[test]
    fn test_promote_does_not_inject_tun_when_inactive() {
        let old_yaml = r#"
mixed-port: 7890
external-controller: 127.0.0.1:9090
tun:
  enable: true
"#;
        let new_yaml = r#"
mixed-port: 7891
external-controller: 127.0.0.1:9090
"#;
        let new_revision = crate::tun_transaction::sha256_revision(new_yaml.as_bytes());
        let (promoted_content, promoted_revision) = prepare_promoted_system_config_content(
            new_yaml,
            &new_revision,
            false,
            Some(old_yaml.as_bytes()),
        )
        .expect("promotion should succeed");

        let doc: serde_yaml::Value = serde_yaml::from_str(&promoted_content).unwrap();
        assert_eq!(doc["mixed-port"].as_i64(), Some(7891));
        assert!(doc.get("tun").is_none());
        assert_eq!(promoted_revision, new_revision);
        assert_eq!(promoted_content, new_yaml);
    }

    #[test]
    fn test_promote_fails_fast_when_tun_snapshot_missing_tun_block() {
        let new_yaml = r#"
mixed-port: 7891
external-controller: 127.0.0.1:9090
"#;
        let new_revision = crate::tun_transaction::sha256_revision(new_yaml.as_bytes());

        // Case 1: no backup content at all
        let err1 = prepare_promoted_system_config_content(new_yaml, &new_revision, true, None)
            .unwrap_err();
        assert!(err1.to_string().contains("active system TUN snapshot"));

        // Case 2: backup content exists but lacks 'tun' key
        let backup_without_tun = b"mixed-port: 7890\n";
        let err2 = prepare_promoted_system_config_content(
            new_yaml,
            &new_revision,
            true,
            Some(backup_without_tun),
        )
        .unwrap_err();
        assert!(err2.to_string().contains("active system TUN snapshot"));
    }

    #[test]
    fn test_resolve_is_tun_snapshot_trusts_snapshot_on_runtime_path_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let tun_path = dir.path().join("tun-config.yaml");

        std::fs::write(&tun_path, "mixed-port: 7890\ntun:\n  enable: true\n").unwrap();

        // 回归：持久快照 `tun.enable: true` 时必须判定为 TUN 快照。
        // 运行态 config_path 已从判定中移除（失配/缺失/匹配一律采信持久证据，
        // 由函数签名本身保证）；旧实现失配态硬判 false，promotion 被降级为热
        // 重载，在跑的 TUN 被静默拆除。
        assert!(resolve_is_tun_snapshot(&tun_path));
    }

    #[test]
    fn test_resolve_is_tun_snapshot_never_revives_disabled_tun() {
        let dir = tempfile::tempdir().unwrap();
        let tun_path = dir.path().join("tun-config.yaml");

        // `tun off` 成功后快照写入 `enable: false`，无条件采信持久快照
        // 不得复活经 `tun off` 关闭的 TUN。
        std::fs::write(&tun_path, "mixed-port: 7890\ntun:\n  enable: false\n").unwrap();
        assert!(!resolve_is_tun_snapshot(&tun_path));

        // 快照缺失（文件不存在）同样判定为非 TUN 快照。
        assert!(!resolve_is_tun_snapshot(&dir.path().join("missing.yaml")));
    }

    #[test]
    fn test_tun_apply_requires_rollback_when_tun_was_enabled_then_disabled() {
        assert!(tun_apply_requires_rollback(Some(true), Some(false)));
    }

    #[test]
    fn test_tun_apply_requires_rollback_when_tun_was_enabled_then_unobservable() {
        assert!(tun_apply_requires_rollback(Some(true), None));
    }

    #[test]
    fn test_tun_apply_requires_rollback_never_triggers_when_before_unobservable() {
        assert!(!tun_apply_requires_rollback(None, Some(true)));
        assert!(!tun_apply_requires_rollback(None, Some(false)));
        assert!(!tun_apply_requires_rollback(None, None));
    }

    #[test]
    fn test_tun_apply_requires_rollback_never_triggers_when_tun_was_disabled() {
        assert!(!tun_apply_requires_rollback(Some(false), Some(true)));
        assert!(!tun_apply_requires_rollback(Some(false), Some(false)));
        assert!(!tun_apply_requires_rollback(Some(false), None));
    }

    #[test]
    fn tun_guard_errors_carry_the_shared_marker() {
        // review MAJOR-1：CLI 侧 `is_tun_guard_error` 只认共享标记常量，
        // daemon 侧护栏/attestation 文案必须以该常量起始。文案一旦改得丢掉标记，
        // 这里先失败，而不是让护栏 Error 静默折叠回 Pending。
        let source = include_str!("daemon.rs");
        let guard_details = [
            "hot reload disabled the running TUN (runtime tun.enable changed from true to false)",
            "hot reload left the runtime TUN state unobservable (GET /configs no longer reports tun.enable after the reload)",
            "promoted configuration was loaded but TUN runtime attestation failed: tun.enable is not true",
        ];
        for detail in guard_details {
            let expected = format!("{{TUN_GUARD_ERROR_MARKER}}{detail}");
            assert!(
                source.contains(&expected),
                "guard detail must start with TUN_GUARD_ERROR_MARKER: {detail}"
            );
        }
        // start_core 失败臂与 runtime revision attestation 失败臂在 TUN 在跑时
        // 都把整条原始错误套进标记，细节不被吞掉；两臂必须都在（收口）。
        // （测试侧带转义反斜杠，不会自我匹配。）
        let arm_marker = "format!(\"{TUN_GUARD_ERROR_MARKER}{message}\")";
        assert_eq!(
            source.matches(arm_marker).count(),
            2,
            "both the start_core failure arm and the runtime-revision failure arm must mark their Error when TUN was running"
        );
    }

    #[test]
    fn full_restart_tun_gate_reuses_the_shared_pure_invariant() {
        // review MAJOR-3：热重载与全量重启共用 `tun_apply_requires_rollback`，
        // 且全量重启分支不得内联重复实现判据——否则删掉 `tun_before == Some(true)`
        // 这一触发条件后测试仍全绿，fail-open 降级分支会静默拆除 TUN。
        let source = include_str!("daemon.rs");
        let call = concat!("tun_apply_requires_rollback(tun_", "before, tun_after)");
        assert_eq!(
            source.matches(call).count(),
            2,
            "both the hot-reload and the full-restart branch must judge through the shared pure invariant"
        );
        let inline = concat!("is_tun_snapshot || tun_", "before == Some(true)");
        assert!(
            !source.contains(inline),
            "the full-restart gate must not re-implement the invariant inline"
        );
    }

    #[test]
    fn full_restart_success_warns_when_tun_was_never_attested() {
        // review BLOCKER：`tun_before` 不可观测时，全量重启分支
        // 不得静默报成功，必须追加与热重载分支措辞对称的告警行。
        let source = include_str!("daemon.rs");
        assert!(
            source.contains(concat!(
                "warning: TUN runtime state was not attested before the full ",
                "restart"
            )),
            "the full-restart Success must warn when the pre-restart TUN state was unobservable"
        );
        assert!(
            source.contains(concat!(
                "warning: TUN runtime state was not attested before hot ",
                "reload"
            )),
            "the hot-reload Success must keep its symmetric warning"
        );
    }

    #[test]
    fn promote_preflight_failure_rolls_back_file_without_core_restart() {
        // 终审 MAJOR：preflight 失败臂位于停止 Core 之前，Core 还在跑旧配置。
        // 该臂必须只做文件级回滚（与写失败臂对齐），不得走带 Core 停启的回滚——否则
        // 一次无副作用的拒绝会杀掉健康 Core，而此臂的现实触发条件（core 二进制缺失）
        // 下重启必然失败，数据面全断。断言前先剥离注释行，避免注释里的标识符自我匹配。
        let source = include_str!("daemon.rs");
        let arm_start = source
            .find("preflight_system_core_start_request(&target_path")
            .expect("promote must preflight before applying");
        let arm_end = source[arm_start..]
            .find("let (core_running, current_endpoint)")
            .expect("preflight arm must precede the running-state probe");
        let arm: String = source[arm_start..arm_start + arm_end]
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !arm.contains("restore_last_known_good"),
            "preflight failure arm must not restart the Core through restore_last_known_good"
        );
        assert!(
            !arm.contains("stop_core") && !arm.contains("start_core"),
            "preflight failure arm must neither stop nor start the Core"
        );
        assert!(
            arm.contains("std::fs::write(&target_path, old_bytes)"),
            "preflight failure arm must roll the promoted file back to the backup"
        );
        assert!(
            arm.contains("std::fs::remove_file(&target_path)"),
            "preflight failure arm must remove the promoted file when no backup exists"
        );
        assert!(
            arm.contains("{message}."),
            "preflight failure arm must keep the original message and append to it"
        );
    }

    #[tokio::test]
    async fn test_restore_last_known_good_restores_backup_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target_path = tmp.path().join("tun-config.yaml");
        let core_binary = tmp.path().join("mihomo");
        let old_content = b"mixed-port: 7890\ntun:\n  enable: true\n";

        // 初始写入被污染/失败的新配置
        std::fs::write(&target_path, b"corrupted-new-config").unwrap();

        let state = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            ..DaemonState::default()
        }));

        let _rollback =
            restore_last_known_good(&state, &target_path, &core_binary, Some(old_content)).await;

        let restored = std::fs::read(&target_path).unwrap();
        assert_eq!(
            restored, old_content,
            "target config must be restored to last-known-good backup"
        );
    }

    #[tokio::test]
    async fn test_restore_last_known_good_removes_file_when_no_backup() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target_path = tmp.path().join("config.yaml");
        let core_binary = tmp.path().join("mihomo");

        std::fs::write(&target_path, b"failed-first-time-config").unwrap();

        let state = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            ..DaemonState::default()
        }));

        let rollback = restore_last_known_good(&state, &target_path, &core_binary, None).await;

        assert!(
            !target_path.exists(),
            "target config must be removed if no prior backup existed"
        );
        // review MAJOR-2：无备份时 Core 未被重启，回滚并未完成，
        // 不得返回 Ok 让 with_rollback_outcome 声称已回滚。
        let err = rollback.expect_err("no-backup rollback must not claim success");
        assert!(err.contains("no previous configuration backup"), "{err}");
        assert!(err.contains("Core was not restarted"), "{err}");
    }

    #[tokio::test]
    async fn test_process_core_api_request_put_proxies_requires_running_core() {
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            ..DaemonState::default()
        }));

        let response = process_core_api_request(
            CoreApiMethod::Put,
            "/proxies/GLOBAL".to_string(),
            Some(serde_json::json!({"name": "DIRECT"})),
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

    #[cfg(unix)]
    #[test]
    fn test_mirror_record_read_roundtrip_and_schema_validation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();

        let yaml = "selections:\n  自动选择: AA-02\n";
        record_selection_mirror_in(base, "sub-abcdef12", yaml).unwrap();
        record_active_subscription_mirror_in(base, "sub-abcdef12").unwrap();

        assert_eq!(
            read_mirror_active_id_in(base).as_deref(),
            Some("sub-abcdef12")
        );
        let map = read_mirror_selection_map_in(base, "sub-abcdef12").unwrap();
        assert_eq!(map.get("自动选择").map(String::as_str), Some("AA-02"));

        // Round-trip through the canonical payload helpers.
        let reparsed = crate::selection::parse_selection_map(
            crate::selection::format_selection_map(&map).as_bytes(),
        )
        .unwrap();
        assert_eq!(reparsed, map);
    }

    #[cfg(unix)]
    #[test]
    fn test_mirror_rejects_invalid_id_and_schema() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();

        assert!(record_selection_mirror_in(base, "sub-xyz", "selections: {}\n").is_err());
        assert!(record_active_subscription_mirror_in(base, "not-a-sub").is_err());
        assert!(record_selection_mirror_in(base, "sub-abcdef12", "not yaml: [").is_err());
        assert!(
            record_selection_mirror_in(base, "sub-abcdef12", "selections:\n  '': node\n").is_err()
        );
        // Mirror files must not be written for rejected payloads.
        assert!(read_mirror_selection_map_in(base, "sub-abcdef12").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_selection_mirror_revisions_report_missing_and_drift() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();

        let yaml = "selections:\n  自动选择: AA-02\n";
        let expected = crate::tun_transaction::content_revision(yaml.as_bytes());
        record_selection_mirror_in(base, "sub-abcdef12", yaml).unwrap();

        let ids = || vec!["sub-abcdef12".to_string(), "sub-0000ff00".to_string()];
        let revisions = selection_mirror_revisions_in(base, &ids()).unwrap();
        assert_eq!(
            revisions.get("sub-abcdef12").and_then(|r| r.as_deref()),
            Some(expected.as_str()),
            "recorded mirror must report the exact pushed-content revision"
        );
        assert_eq!(
            revisions.get("sub-0000ff00"),
            Some(&None),
            "absent mirror must report None, not an error"
        );

        // Drift after an out-of-band mirror rewrite changes the revision.
        write_selection_mirror_file(
            &selection_mirror_file_path_in(base, "sub-abcdef12"),
            "selections:\n  自动选择: BB-01\n".as_bytes(),
        )
        .unwrap();
        let drifted = selection_mirror_revisions_in(base, &ids()).unwrap();
        assert_ne!(
            drifted.get("sub-abcdef12").and_then(|r| r.as_deref()),
            Some(expected.as_str())
        );

        // Invalid ids are rejected wholesale (fail-closed).
        assert!(selection_mirror_revisions_in(base, &["../escape".to_string()]).is_err());
        assert!(selection_mirror_revisions_in(base, &[]).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn daemon_replay_exit_matrix_renders_regular_outcome_remedies() {
        use crate::selection::{ReplayGroupResult, ReplayOutcome, ReplayReport};

        let result = |group: &str, node: &str, outcome: ReplayOutcome| ReplayGroupResult {
            group: group.to_string(),
            node: node.to_string(),
            outcome,
        };

        // Every replay exit (startup / restart / promotion) renders through
        // this helper, so the matrix is asserted once on the shared renderer.
        let report = ReplayReport {
            results: vec![result(
                "Proxy",
                "old-node",
                ReplayOutcome::SkippedNodeMissing {
                    current: Some("new-node".to_string()),
                    members: vec!["new-node".to_string(), "other".to_string()],
                },
            )],
            intent_error: None,
            selectable_groups: None,
        };
        let lines = selection_replay_report_lines(&report).join("\n");
        assert!(
            lines.contains("node no longer in group (current: new-node)"),
            "{lines}"
        );
        assert!(
            lines.contains("Current members of Proxy: new-node, other"),
            "{lines}"
        );
        assert!(
            lines.contains("mihomo-cli select -g 'Proxy' --node='new-node'"),
            "{lines}"
        );

        let report = ReplayReport {
            results: vec![result("Gone", "n1", ReplayOutcome::SkippedGroupMissing)],
            intent_error: None,
            selectable_groups: Some(vec!["G1".to_string(), "G2".to_string()]),
        };
        let lines = selection_replay_report_lines(&report).join("\n");
        assert!(lines.contains("Current groups: G1, G2"), "{lines}");
        assert!(
            !lines.contains("mihomo-cli select -g 'Gone'"),
            "the vanished group is never a remedy: {lines}"
        );

        // Shared replay budget exhausted: no guessed group list, retry path.
        let report = ReplayReport {
            results: vec![result("Gone", "n1", ReplayOutcome::SkippedGroupMissing)],
            intent_error: None,
            selectable_groups: None,
        };
        let lines = selection_replay_report_lines(&report).join("\n");
        assert!(
            lines.contains("Group list unavailable (budget exhausted or query failed)"),
            "{lines}"
        );
        assert!(!lines.contains("Current groups"), "{lines}");

        let report = ReplayReport {
            results: vec![
                result(
                    "A",
                    "a",
                    ReplayOutcome::Failed {
                        error: "connection refused".to_string(),
                    },
                ),
                result("B", "b", ReplayOutcome::BudgetExceeded),
            ],
            intent_error: None,
            selectable_groups: None,
        };
        let lines = selection_replay_report_lines(&report).join("\n");
        assert!(lines.contains("Intent kept; apply failed"), "{lines}");
        assert!(
            lines.contains("retry") || lines.contains("Retry"),
            "{lines}"
        );
        assert!(
            lines.contains("mihomo-cli select --replay or mihomo-cli restart --system"),
            "{lines}"
        );

        // Applied-only reports carry no remedy noise.
        let report = ReplayReport {
            results: vec![result("A", "a", ReplayOutcome::Applied)],
            intent_error: None,
            selectable_groups: None,
        };
        assert_eq!(
            selection_replay_report_lines(&report),
            vec!["✓ Selections restored: A → a".to_string()]
        );

        // Exit matrix integrity (SPEC P5): the request-carrying exits —
        // restart and the two promote paths — replay via
        // `replay_mirror_selection_intent`; the daemon's own startup/autostart
        // path has no request identity and must NOT replay at all (0 replays).
        let source = include_str!("daemon.rs");
        let replay_call = concat!("replay_mirror_selection_intent(&sta", "te");
        assert_eq!(
            source.matches(replay_call).count(),
            2,
            "request-carrying replay exits must go through the shared replay path"
        );
        // Both startup paths funnel through `startup_replay_lines`, which applies
        // the §3.8.3 admission table before the same budget/degradation replay.
        let startup_entry = concat!("startup_replay_lines(&sta", "te)");
        assert_eq!(
            source.matches(startup_entry).count(),
            2,
            "both daemon startup paths must go through the admitted replay entry"
        );
        assert_eq!(
            source
                .matches(concat!("fn startup_replay_admi", "ssion("))
                .count(),
            1,
            "the suppression table has exactly one definition"
        );
        let report_render = concat!("format_lines", "()");
        assert_eq!(
            source.matches(report_render).count(),
            1,
            "daemon must render reports only through selection_replay_report_lines"
        );
    }

    fn admission_inputs<'a>(
        active: Option<&'a str>,
        map_state: &'a str,
        binding: Option<&'a MirrorBinding>,
        launched: Option<&'a str>,
        unsettled: bool,
    ) -> StartupReplayInputs<'a> {
        StartupReplayInputs {
            mirror_active: active,
            map_state,
            binding,
            launched_revision: launched,
            transaction_unsettled: unsettled,
        }
    }

    /// SPEC §3.8.3 抑制判定表：不是「没有请求」就抑制，而是「归属不可证」才抑制。
    #[test]
    fn startup_replay_admission_table() {
        let id = "sub-abcdef12";
        let bound = MirrorBinding {
            subscription_id: id.to_string(),
            config_revision: Some("rev-a".to_string()),
        };
        // Normal boot: no CLI request, but ownership is provable → replay.
        assert!(startup_replay_admission(&admission_inputs(
            Some(id),
            "ready",
            Some(&bound),
            Some("rev-a"),
            false
        ))
        .is_ok());

        // Legitimate empty map = nothing to do, still admitted.
        assert!(startup_replay_admission(&admission_inputs(
            Some(id),
            "ready",
            Some(&bound),
            Some("rev-a"),
            false
        ))
        .is_ok());

        // Identity unprovable / mismatched / unauthenticated → suppress.
        let other = MirrorBinding {
            subscription_id: "sub-11111111".to_string(),
            config_revision: Some("rev-a".to_string()),
        };
        let legacy = MirrorBinding {
            subscription_id: id.to_string(),
            config_revision: None,
        };
        for (inputs, _identity) in [
            (
                admission_inputs(Some(id), "ready", None, Some("rev-a"), false),
                true,
            ),
            (
                admission_inputs(Some(id), "ready", Some(&other), Some("rev-a"), false),
                true,
            ),
            (
                admission_inputs(Some(id), "ready", Some(&bound), Some("rev-b"), false),
                true,
            ),
            (
                admission_inputs(Some(id), "ready", Some(&bound), None, false),
                true,
            ),
            (
                admission_inputs(Some(id), "ready", Some(&legacy), Some("rev-a"), false),
                true,
            ),
        ] {
            let skip = startup_replay_admission(&inputs).unwrap_err();
            assert!(skip.identity, "{skip:?}");
        }

        // Fact-only suppression (no identity claim).
        for inputs in [
            admission_inputs(Some(id), "ready", Some(&bound), Some("rev-a"), true),
            admission_inputs(None, "missing", Some(&bound), Some("rev-a"), false),
            admission_inputs(Some(id), "missing", Some(&bound), Some("rev-a"), false),
            admission_inputs(Some(id), "corrupt", Some(&bound), Some("rev-a"), false),
        ] {
            let skip = startup_replay_admission(&inputs).unwrap_err();
            assert!(!skip.identity, "{skip:?}");
        }
    }

    /// SPEC §3.8.3 dynamic counter: counts the actual selection PUTs the replay
    /// path issues (no source counting).
    #[cfg(unix)]
    struct CountingSelectionClient {
        puts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[cfg(unix)]
    impl crate::mihomo_api::MihomoApiClient for CountingSelectionClient {
        async fn get(&self, path: &str) -> anyhow::Result<serde_json::Value> {
            if path.starts_with("/proxies/") {
                return Ok(serde_json::json!({"all": ["N"], "now": "DIRECT"}));
            }
            Ok(serde_json::json!({}))
        }

        async fn get_bounded(
            &self,
            path: &str,
            _byte_limit: usize,
        ) -> anyhow::Result<serde_json::Value> {
            self.get(path).await
        }

        async fn put(
            &self,
            _path: &str,
            _body: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            self.puts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(serde_json::Value::Null)
        }

        async fn patch(
            &self,
            _path: &str,
            _body: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }

        async fn delete(&self, _path: &str) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
    }

    #[cfg(unix)]
    fn counting_client() -> (
        CountingSelectionClient,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let puts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (CountingSelectionClient { puts: puts.clone() }, puts)
    }

    /// Seeded mirror with one group/node and a binding to `revision`.
    #[cfg(unix)]
    fn seed_mirror(base: &std::path::Path, subscription_id: &str, revision: Option<&str>) {
        record_active_subscription_mirror_in(base, subscription_id).unwrap();
        record_selection_mirror_in(base, subscription_id, "selections:\n  G: N\n").unwrap();
        record_mirror_binding_in(base, subscription_id, revision).unwrap();
    }

    /// ① Normal single-group boot recovery = exactly 1 PUT.
    #[cfg(unix)]
    #[tokio::test]
    async fn startup_replay_puts_once_when_ownership_is_provable() {
        let tmp = tempfile::TempDir::new().unwrap();
        seed_mirror(tmp.path(), "sub-aaaaaaaa", Some("rev-a"));
        let (client, puts) = counting_client();
        let lines = replay_mirror_at_startup(tmp.path(), Some("rev-a"), false, &client).await;
        assert_eq!(
            puts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Selections restored")),
            "{lines:?}"
        );
    }

    /// ② Every degraded form suppresses all entries: 0 PUTs + explicit fact line.
    #[cfg(unix)]
    #[tokio::test]
    async fn startup_replay_is_suppressed_in_every_degraded_form() {
        use std::sync::atomic::Ordering;
        let id = "sub-aaaaaaaa";

        // (label, seed fn, launched revision, unsettled)
        type DegradedCase<'a> = (
            &'a str,
            Box<dyn Fn(&std::path::Path)>,
            Option<&'a str>,
            bool,
        );
        let cases: Vec<DegradedCase> = vec![
            (
                "binding-missing",
                Box::new(|base: &std::path::Path| {
                    record_active_subscription_mirror_in(base, id).unwrap();
                    record_selection_mirror_in(base, id, "selections:\n  G: N\n").unwrap();
                }),
                Some("rev-a"),
                false,
            ),
            (
                "binding-legacy-without-revision",
                Box::new(|base: &std::path::Path| seed_mirror(base, id, None)),
                Some("rev-a"),
                false,
            ),
            (
                "binding-identity-mismatch",
                Box::new(|base: &std::path::Path| {
                    record_active_subscription_mirror_in(base, id).unwrap();
                    record_selection_mirror_in(base, id, "selections:\n  G: N\n").unwrap();
                    record_mirror_binding_in(base, "sub-bbbbbbbb", Some("rev-a")).unwrap();
                }),
                Some("rev-a"),
                false,
            ),
            (
                "launched-revision-unknown",
                Box::new(|base: &std::path::Path| seed_mirror(base, id, Some("rev-a"))),
                None,
                false,
            ),
            (
                "map-source-unreadable",
                Box::new(|base: &std::path::Path| {
                    record_active_subscription_mirror_in(base, id).unwrap();
                    record_mirror_binding_in(base, id, Some("rev-a")).unwrap();
                }),
                Some("rev-a"),
                false,
            ),
            (
                "map-corrupt",
                Box::new(|base: &std::path::Path| {
                    record_active_subscription_mirror_in(base, id).unwrap();
                    record_mirror_binding_in(base, id, Some("rev-a")).unwrap();
                    write_selection_mirror_file(
                        &selection_mirror_file_path_in(base, id),
                        b":::not-yaml",
                    )
                    .unwrap();
                }),
                Some("rev-a"),
                false,
            ),
            (
                "transaction-unsettled",
                Box::new(|base: &std::path::Path| seed_mirror(base, id, Some("rev-a"))),
                Some("rev-a"),
                true,
            ),
        ];

        for (label, seed, launched, unsettled) in cases {
            let tmp = tempfile::TempDir::new().unwrap();
            seed(tmp.path());
            let (client, puts) = counting_client();
            let lines = replay_mirror_at_startup(tmp.path(), launched, unsettled, &client).await;
            assert_eq!(
                puts.load(Ordering::SeqCst),
                0,
                "{label}: degraded state must never PUT ({lines:?})"
            );
            assert!(
                lines.iter().any(|line| line.contains("0 replays")),
                "{label}: suppression must be explicit ({lines:?})"
            );
        }
    }

    /// ③ Commit-gap: mirror pushed for B, config promotion crashed, boot loads A.
    /// B must never be replayed into A (cross-subscription PUT).
    #[cfg(unix)]
    #[tokio::test]
    async fn startup_never_replays_b_after_a_crash_on_a() {
        use std::sync::atomic::Ordering;
        let tmp = tempfile::TempDir::new().unwrap();
        // Mirror/binding describe B, while the loaded configuration is A.
        seed_mirror(tmp.path(), "sub-bbbbbbbb", Some("rev-b"));
        let (client, puts) = counting_client();
        let lines = replay_mirror_at_startup(tmp.path(), Some("rev-a"), false, &client).await;
        assert_eq!(puts.load(Ordering::SeqCst), 0, "{lines:?}");
        let joined = lines.join("\n");
        assert!(
            joined.contains("[selection-identity-mismatch]"),
            "identity mismatch must be signalled: {joined}"
        );
        assert!(joined.contains("0 replays"), "{joined}");
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_mirror_selection_scope_skips_on_mismatch_or_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();

        // subscription_id 为 None / 非法 id 时跳过
        assert_eq!(
            resolve_mirror_selection_scope_in(base, None),
            MirrorScopeResolution::Unavailable
        );
        assert_eq!(
            resolve_mirror_selection_scope_in(base, Some("sub1")),
            MirrorScopeResolution::Unavailable
        );

        // 镜像缺失 active 时跳过
        assert_eq!(
            resolve_mirror_selection_scope_in(base, Some("sub-abcdef12")),
            MirrorScopeResolution::Unavailable
        );

        // active 不匹配时上报 identity-mismatch（A3'）
        record_active_subscription_mirror_in(base, "sub-abcdef99").unwrap();
        assert_eq!(
            resolve_mirror_selection_scope_in(base, Some("sub-abcdef12")),
            MirrorScopeResolution::IdentityMismatch {
                request: "sub-abcdef12".to_string(),
                mirror_active: "sub-abcdef99".to_string(),
            }
        );

        // active 匹配时返回作用域
        record_active_subscription_mirror_in(base, "sub-abcdef12").unwrap();
        assert_eq!(
            resolve_mirror_selection_scope_in(base, Some("sub-abcdef12")),
            MirrorScopeResolution::Match("sub-abcdef12".to_string())
        );
    }

    // ── B1: selection drift tracker ──

    /// The drift tracker is process-global; serialize the tests that read or
    /// mutate it so parallel test threads cannot interleave guards.
    fn drift_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn drift_mirror(group: &str, node: &str) -> std::collections::BTreeMap<String, String> {
        let mut map = std::collections::BTreeMap::new();
        map.insert(group.to_string(), node.to_string());
        map
    }

    fn proxies_json(entries: &[(&str, &str)]) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (group, now) in entries {
            map.insert(
                (*group).to_string(),
                serde_json::json!({ "now": now, "type": "Selector" }),
            );
        }
        serde_json::json!({ "proxies": map })
    }

    #[test]
    fn compare_selection_drift_reports_only_mismatches() {
        let mirror = drift_mirror("自动选择", "HK-01");
        let drifts = compare_selection_drift(
            &mirror,
            &proxies_json(&[("自动选择", "HK-01")]),
            1,
            None,
            None,
        );
        assert!(drifts.is_empty());

        let drifts = compare_selection_drift(
            &mirror,
            &proxies_json(&[("自动选择", "SG-02")]),
            42,
            Some("sub-abcdef12"),
            Some("abc123"),
        );
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].group, "自动选择");
        assert_eq!(drifts[0].intent, "HK-01");
        assert_eq!(drifts[0].runtime, "SG-02");
        assert_eq!(drifts[0].at_unix, 42);
        assert_eq!(drifts[0].subscription_id.as_deref(), Some("sub-abcdef12"));
        assert_eq!(drifts[0].mirror_revision.as_deref(), Some("abc123"));
    }

    #[test]
    fn compare_selection_drift_excludes_synthetic_and_absent_groups() {
        // GLOBAL/COMPATIBLE are synthetic; absent groups have no intent to
        // compare (mirror missing → no drift, B1).
        let mut mirror = drift_mirror("GLOBAL", "REJECT");
        mirror.insert("COMPATIBLE".to_string(), "DIRECT".to_string());
        mirror.insert("幽灵组".to_string(), "HK-01".to_string());
        let drifts = compare_selection_drift(
            &mirror,
            &proxies_json(&[("GLOBAL", "DIRECT"), ("COMPATIBLE", "DIRECT")]),
            1,
            None,
            None,
        );
        assert!(drifts.is_empty(), "{drifts:?}");

        // Malformed payload degrades to empty comparison, never panics.
        assert!(
            compare_selection_drift(&mirror, &serde_json::json!(null), 1, None, None).is_empty()
        );
        assert!(compare_selection_drift(&mirror, &serde_json::json!({}), 1, None, None).is_empty());
    }

    #[test]
    fn drift_op_guard_brackets_both_edges() {
        let _drift_test = drift_test_guard();
        // A completed A→B→A operation must still bump the sequence twice so a
        // sampler that straddled it discards the sample (three-round ABA fix).
        let (seq_before, in_flight_before) = {
            let tracker = drift_tracker().lock().unwrap();
            (tracker.seq, tracker.in_flight)
        };
        {
            let _op = DriftOpGuard::open();
            {
                let tracker = drift_tracker().lock().unwrap();
                assert_eq!(tracker.in_flight, in_flight_before + 1);
            }
        }
        let (seq_after, in_flight_after) = {
            let tracker = drift_tracker().lock().unwrap();
            (tracker.seq, tracker.in_flight)
        };
        assert_eq!(seq_after, seq_before.wrapping_add(2));
        assert_eq!(in_flight_after, in_flight_before);
    }

    #[test]
    fn replay_guard_arms_suppression_on_all_exits() {
        let _drift_test = drift_test_guard();
        {
            let _guard = ReplayDriftGuard::open();
        }
        let suppress_until = drift_tracker().lock().unwrap().suppress_until;
        let until = suppress_until.expect("replay end must arm suppression");
        let remaining = until.saturating_duration_since(std::time::Instant::now());
        assert!(
            remaining > DRIFT_REPLAY_SUPPRESSION / 2 && remaining <= DRIFT_REPLAY_SUPPRESSION,
            "{remaining:?}"
        );
    }

    #[test]
    fn select_segment_brackets_and_closes_on_record() {
        let _drift_test = drift_test_guard();
        drift_open_select_segment();
        {
            let tracker = drift_tracker().lock().unwrap();
            assert!(tracker.select_segment_open);
        }
        drift_close_select_segment();
        assert!(!drift_tracker().lock().unwrap().select_segment_open);
    }

    #[test]
    fn drift_ring_is_capped_and_group_entries_update_in_place() {
        let _drift_test = drift_test_guard();
        {
            let mut tracker = drift_tracker().lock().unwrap();
            tracker.observations.clear();
            tracker.unknown_samples = 0;
            tracker.last_warn_at.clear();
        }
        let mk = |group: &str, runtime: &str| crate::ipc::SelectionDriftObservation {
            group: group.to_string(),
            intent: "intent".to_string(),
            runtime: runtime.to_string(),
            at_unix: 0,
            subscription_id: None,
            mirror_revision: None,
        };
        let now = std::time::Instant::now();
        for index in 0..(DRIFT_RING_CAPACITY + 10) {
            let drift = mk(&format!("g{index:04}"), "a");
            let mut tracker = drift_tracker().lock().unwrap();
            apply_drift_observations(&mut tracker, vec![drift], now);
        }
        {
            let mut tracker = drift_tracker().lock().unwrap();
            assert_eq!(tracker.observations.len(), DRIFT_RING_CAPACITY);
            // In-place update does not grow the ring.
            apply_drift_observations(&mut tracker, vec![mk("g0005", "b")], now);
            assert_eq!(tracker.observations.len(), DRIFT_RING_CAPACITY);
            let updated = tracker
                .observations
                .iter()
                .find(|obs| obs.group == "g0005")
                .expect("existing group stays in the ring");
            assert_eq!(updated.runtime, "b");
            // Warning throttling: a second sample inside the window is silent
            // (last_warn_at untouched → timestamp unchanged).
            let warned_at = tracker.last_warn_at.get("g0005").copied();
            apply_drift_observations(&mut tracker, vec![mk("g0005", "c")], now);
            assert_eq!(tracker.last_warn_at.get("g0005").copied(), warned_at);
            tracker.observations.clear();
            tracker.last_warn_at.clear();
        }
    }

    #[test]
    fn drift_latest_observation_picks_max_at_unix() {
        // N2: `.last()` is wrong because in-place updates keep the observation
        // at its original index. The "Latest" display must pick the observation
        // with the highest at_unix (ties broken by tail position).
        let _drift_test = drift_test_guard();
        {
            let mut tracker = drift_tracker().lock().unwrap();
            tracker.observations.clear();
        }
        let mk = |group: &str, runtime: &str, at: u64| crate::ipc::SelectionDriftObservation {
            group: group.to_string(),
            intent: "intent".to_string(),
            runtime: runtime.to_string(),
            at_unix: at,
            subscription_id: None,
            mirror_revision: None,
        };
        let now = std::time::Instant::now();
        {
            let mut tracker = drift_tracker().lock().unwrap();
            // A@t1, B@t2 inserted → A at index 0, B at index 1
            apply_drift_observations(&mut tracker, vec![mk("A", "r1", 100)], now);
            apply_drift_observations(&mut tracker, vec![mk("B", "r2", 200)], now);
            // t3: only A updated in-place (stays at index 0)
            apply_drift_observations(&mut tracker, vec![mk("A", "r3", 300)], now);
        }
        // "Latest" must be A@t3 (at_unix=300), NOT B@t2 (which .last() would pick)
        let (observations, _, _) = drift_snapshot();
        let latest = latest_drift_observation(&observations).expect("at least one observation");
        assert_eq!(latest.group, "A");
        assert_eq!(latest.runtime, "r3");
        assert_eq!(latest.at_unix, 300);
        {
            let mut tracker = drift_tracker().lock().unwrap();
            tracker.observations.clear();
        }
    }

    #[tokio::test]
    async fn drift_unknown_samples_counts_real_failures_not_discarded_ticks() {
        // N4: unknown_samples must count real failures (Core down, fetch
        // timeout, mirror fault), NOT discarded ticks (suppressed/segment/
        // in_flight).
        {
            let _drift_test = drift_test_guard();
            let mut tracker = drift_tracker().lock().unwrap();
            tracker.observations.clear();
            tracker.unknown_samples = 0;
            tracker.last_sample_status = String::new();
            tracker.suppress_until = None;
            tracker.select_segment_open = false;
            tracker.in_flight = 0;
            tracker.sampling = false;
        }

        // Simulate a discarded tick (suppressed) → unknown_samples must NOT increment
        {
            let mut tracker = drift_tracker().lock().unwrap();
            tracker.suppress_until =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(60));
        }
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: true,
            core_child: None,
            core_pid: None,
            config_path: None,
            launched_config_revision: None,
            core_binary: None,
            api_endpoint: Some("/var/run/mihomo/mihomo.sock".to_string()),
            pid_file: std::path::PathBuf::new(),
            core_log_file: std::path::PathBuf::new(),
        }));
        drift_sample_once(&state).await;
        {
            let tracker = drift_tracker().lock().unwrap();
            assert_eq!(
                tracker.unknown_samples, 0,
                "suppressed tick must not increment unknown_samples"
            );
            assert_eq!(tracker.last_sample_status, "suppressed");
        }

        // Clear suppression, simulate Core not running → unknown_samples MUST increment
        {
            let mut tracker = drift_tracker().lock().unwrap();
            tracker.suppress_until = None;
            tracker.sampling = false;
        }
        let state_no_core = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            core_child: None,
            core_pid: None,
            config_path: None,
            launched_config_revision: None,
            core_binary: None,
            api_endpoint: Some("/var/run/mihomo/mihomo.sock".to_string()),
            pid_file: std::path::PathBuf::new(),
            core_log_file: std::path::PathBuf::new(),
        }));
        drift_sample_once(&state_no_core).await;
        {
            let tracker = drift_tracker().lock().unwrap();
            assert_eq!(
                tracker.unknown_samples, 1,
                "Core down must increment unknown_samples"
            );
            assert_eq!(tracker.last_sample_status, "unknown");
        }

        // Cleanup
        {
            let mut tracker = drift_tracker().lock().unwrap();
            tracker.unknown_samples = 0;
            tracker.last_sample_status = String::new();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_promote_system_config_skips_missing_mirror_replay_without_fatal_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            core_child: None,
            core_pid: None,
            config_path: None,
            launched_config_revision: None,
            core_binary: Some(expected_system_core_binary_path()),
            api_endpoint: Some("/var/run/mihomo/mihomo.sock".to_string()),
            pid_file: tmp.path().join("core.pid"),
            core_log_file: tmp.path().join("mihomo.log"),
        }));

        let yaml = "mixed-port: 7890\nexternal-controller: 127.0.0.1:9090\n";
        let revision = crate::tun_transaction::sha256_revision(yaml.as_bytes());

        // Mirror has no record for this subscription: replay must be skipped
        // without failing the promotion fatally.
        let resp = promote_system_config(
            Arc::clone(&state),
            yaml.to_string(),
            revision,
            Some("sub-ffffff01".to_string()),
            None,
        )
        .await;

        if let DaemonResponse::Error { message } = resp {
            assert!(
                !message.contains("selection subscription identity mismatch")
                    && !message.contains("selection replay requires"),
                "promote_system_config must not fail fatally due to selection replay: {message}"
            );
        }
    }

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
                subscription_id: None,
                change_kind: None,
                token: None,
            }
        ));
    }

    /// the SPEC §12.4 exemption matrix over every non-terminal phase.
    /// Read-only commands pass, change commands are refused, and fenced transaction
    /// commands pass only for their own transaction/generation/phase.
    #[test]
    fn gate_exemption_matrix_covers_every_non_terminal_phase() {
        use crate::tun_transaction::{JournalPhase, TransactionFence, TunJournal};

        fn journal_at(phase: JournalPhase) -> TunJournal {
            TunJournal {
                schema_version: 1,
                transaction_id: "txn-gate".to_string(),
                generation: 5,
                phase,
                original_uid: 1000,
                target_runtime_tun: false,
                base_intent_revision: None,
                candidate_revision: "cand".to_string(),
                candidate_core_identity: None,
                candidate_core_pid: None,
                old_snapshot_revision: None,
                old_runtime_digest: "digest".to_string(),
                recovery_target_revision: None,
                legacy_source: false,
                legacy_transaction_id: None,
                legacy_base_revision: None,
                rollback_evidence_complete: true,
                last_error: None,
                outcome: None,
                recovery_core_identity: None,
                recovery_core_pid: None,
                updated_at: None,
            }
        }

        fn fence(phase: JournalPhase) -> TransactionFence {
            TransactionFence {
                transaction_id: "txn-gate".to_string(),
                generation: 5,
                expected_phase: phase,
                expected_candidate_revision: "cand".to_string(),
            }
        }

        fn foreign_fence(phase: JournalPhase) -> TransactionFence {
            TransactionFence {
                transaction_id: "txn-other".to_string(),
                generation: 6,
                expected_phase: phase,
                expected_candidate_revision: "other".to_string(),
            }
        }

        let read_only = [
            DaemonCommand::GetStatus { token: None },
            DaemonCommand::GetTransactionStatus { token: None },
            DaemonCommand::GetSelectionMirrorRevisions {
                subscription_ids: Vec::new(),
                token: None,
            },
            DaemonCommand::GetSelectionDrift { token: None },
            DaemonCommand::CoreApiRequest {
                method: CoreApiMethod::Get,
                path: "/configs".to_string(),
                body: None,
                token: None,
            },
        ];
        let change_commands = [
            DaemonCommand::StartCore {
                config_content: "tun:\n  enable: false\n".to_string(),
                config_revision: "rev".to_string(),
                subscription_id: None,
                token: None,
            },
            DaemonCommand::StopCore { token: None },
            DaemonCommand::RestartCore {
                config_content: "tun:\n  enable: false\n".to_string(),
                config_revision: "rev".to_string(),
                subscription_id: None,
                token: None,
            },
            DaemonCommand::ApplySystemTunSnapshot {
                expected_revision: "rev".to_string(),
                stack: None,
                dns_hijack: None,
                token: None,
            },
            DaemonCommand::PromoteSystemConfig {
                config_content: "tun:\n  enable: false\n".to_string(),
                config_revision: "rev".to_string(),
                subscription_id: None,
                change_kind: None,
                token: None,
            },
            DaemonCommand::RecordSelectionIntent {
                subscription_id: "sub-0123456789".to_string(),
                content_yaml: "proxies: []\n".to_string(),
                config_revision: Some("rev".to_string()),
                token: None,
            },
            DaemonCommand::RecordActiveSubscription {
                subscription_id: "sub-0123456789".to_string(),
                config_revision: Some("rev".to_string()),
                token: None,
            },
            DaemonCommand::SelectSystemProxy {
                group: "G".to_string(),
                node: "N".to_string(),
                token: None,
            },
            DaemonCommand::DisableTun { token: None },
            DaemonCommand::SetAutostart {
                enabled: true,
                token: None,
            },
            DaemonCommand::CoreApiRequest {
                method: CoreApiMethod::Put,
                path: "/configs".to_string(),
                body: None,
                token: None,
            },
            DaemonCommand::CoreApiRequest {
                method: CoreApiMethod::Patch,
                path: "/configs".to_string(),
                body: None,
                token: None,
            },
            DaemonCommand::CoreApiRequest {
                method: CoreApiMethod::Delete,
                path: "/connections".to_string(),
                body: None,
                token: None,
            },
        ];

        for phase in [
            JournalPhase::Prepared,
            JournalPhase::PromotionPending,
            JournalPhase::SnapshotPromoted,
            JournalPhase::CoreApplied,
            JournalPhase::RollbackPending,
            JournalPhase::RecoveryRequired,
        ] {
            let journal = journal_at(phase);
            // No active transaction at all: everything passes.
            for cmd in read_only.iter().chain(change_commands.iter()) {
                assert!(
                    gate_admits(cmd, None),
                    "{cmd:?} must pass with no active transaction"
                );
            }
            // Read-only stays available during every non-terminal phase.
            for cmd in &read_only {
                assert!(
                    gate_admits(cmd, Some(&journal)),
                    "{cmd:?} must stay available in {phase:?}"
                );
            }
            // Every change command is refused during every non-terminal phase.
            for cmd in &change_commands {
                assert!(
                    !gate_admits(cmd, Some(&journal)),
                    "{cmd:?} must be refused in {phase:?}"
                );
            }
        }

        // Fenced transaction commands: own fence + own phase only.
        let cases = [
            (
                JournalPhase::Prepared,
                DaemonCommand::ValidatePreparedRuntime {
                    fence: fence(JournalPhase::Prepared),
                    expected_old_runtime_revision: "old".to_string(),
                    expected_old_runtime_tun: false,
                    token: None,
                },
            ),
            (
                JournalPhase::SnapshotPromoted,
                DaemonCommand::ApplyPromotedSnapshot {
                    fence: fence(JournalPhase::SnapshotPromoted),
                    target_runtime_tun: false,
                    token: None,
                },
            ),
            (
                JournalPhase::RollbackPending,
                DaemonCommand::QuiesceCandidateRuntime {
                    fence: fence(JournalPhase::RollbackPending),
                    allow_legacy_reset: false,
                    allow_restored_old_quiesce: false,
                    token: None,
                },
            ),
            (
                JournalPhase::RecoveryRequired,
                DaemonCommand::QuiesceCandidateRuntime {
                    fence: fence(JournalPhase::RecoveryRequired),
                    allow_legacy_reset: false,
                    allow_restored_old_quiesce: false,
                    token: None,
                },
            ),
            (
                JournalPhase::RollbackPending,
                DaemonCommand::RestoreOldRuntime {
                    fence: fence(JournalPhase::RollbackPending),
                    expected_old_runtime_revision: "old".to_string(),
                    expected_old_runtime_tun: false,
                    token: None,
                },
            ),
            (
                JournalPhase::CoreApplied,
                DaemonCommand::AttestCurrentTransaction {
                    fence: fence(JournalPhase::CoreApplied),
                    expected_runtime_revision: "rev".to_string(),
                    expected_runtime_tun: false,
                    token: None,
                },
            ),
            (
                JournalPhase::RecoveryRequired,
                DaemonCommand::ApplyLegacyRecoveryTarget {
                    fence: fence(JournalPhase::RecoveryRequired),
                    expected_recovery_target_revision: "rev".to_string(),
                    token: None,
                },
            ),
        ];
        for (phase, cmd) in &cases {
            let journal = journal_at(*phase);
            assert!(
                gate_admits(cmd, Some(&journal)),
                "{cmd:?} must pass in its own phase {phase:?}"
            );
            // Same command in every other non-terminal phase is refused, except the
            // quiesce channel which is deliberately open for both stop phases.
            let quiesce = matches!(cmd, DaemonCommand::QuiesceCandidateRuntime { .. });
            for other in [
                JournalPhase::Prepared,
                JournalPhase::PromotionPending,
                JournalPhase::SnapshotPromoted,
                JournalPhase::CoreApplied,
                JournalPhase::RollbackPending,
                JournalPhase::RecoveryRequired,
            ] {
                if other == *phase {
                    continue;
                }
                let expected = quiesce
                    && matches!(
                        other,
                        JournalPhase::RollbackPending | JournalPhase::RecoveryRequired
                    );
                assert_eq!(
                    gate_admits(cmd, Some(&journal_at(other))),
                    expected,
                    "{cmd:?} admission in {other:?} must be {expected}"
                );
            }
        }
        // Fence mismatch on the transaction id / generation / candidate revision.
        let rollback = journal_at(JournalPhase::RollbackPending);
        assert!(!gate_admits(
            &DaemonCommand::RestoreOldRuntime {
                fence: foreign_fence(JournalPhase::RollbackPending),
                expected_old_runtime_revision: "old".to_string(),
                expected_old_runtime_tun: false,
                token: None,
            },
            Some(&rollback)
        ));
        assert!(!gate_admits(
            &DaemonCommand::QuiesceCandidateRuntime {
                fence: foreign_fence(JournalPhase::RollbackPending),
                allow_legacy_reset: false,
                allow_restored_old_quiesce: false,
                token: None,
            },
            Some(&rollback)
        ));

        // Terminal phases release the gate for every command.
        for terminal in [JournalPhase::IntentCommitted, JournalPhase::RolledBack] {
            let journal = journal_at(terminal);
            for cmd in read_only.iter().chain(change_commands.iter()) {
                assert!(
                    gate_admits(cmd, Some(&journal)),
                    "{cmd:?} must pass in terminal phase {terminal:?}"
                );
            }
        }
    }

    /// the `process_command` path must use the same decision — the
    /// matrix above cannot be bypassed by reaching the daemon through IPC.
    #[cfg(unix)]
    #[tokio::test]
    async fn gate_is_enforced_through_process_command() {
        use crate::tun_transaction::{JournalPhase, TransactionFence, TunJournal};

        isolate_daemon_runtime_for_tests();
        let _serialized = runtime_journal_test_lock().lock().await;
        let ctx = daemon_transaction_context().expect("system transaction context");
        let active_dir = crate::tun_transaction::transactions_dir(&ctx).join("active");
        let _ = std::fs::remove_dir_all(&active_dir);
        std::fs::create_dir_all(&active_dir).unwrap();
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "gate-process-command".to_string(),
            generation: 12,
            phase: JournalPhase::RollbackPending,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "cand".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: Some(crate::tun_transaction::now_rfc3339()),
        };
        crate::tun_transaction::write_active_journal(&ctx, &journal).unwrap();
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let gate_marker = "Change rejected: a system TUN transaction";

        // Read-only query: not blocked by the gate.
        let status = process_command(
            DaemonCommand::GetStatus { token: None },
            Arc::clone(&state),
            peer_with_uid(Some(0)),
        )
        .await;
        assert!(
            matches!(status, DaemonResponse::Status { .. }),
            "GetStatus must stay available: {status:?}"
        );
        let read_config = process_command(
            DaemonCommand::CoreApiRequest {
                method: CoreApiMethod::Get,
                path: "/configs".to_string(),
                body: None,
                token: None,
            },
            Arc::clone(&state),
            peer_with_uid(Some(0)),
        )
        .await;
        match read_config {
            DaemonResponse::Error { message } => assert!(
                !message.contains(gate_marker),
                "a read-only Core API request must not be gated: {message}"
            ),
            other => panic!("unexpected read-only response: {other:?}"),
        }

        // Change commands: refused with the change-specific message.
        for cmd in [
            DaemonCommand::SelectSystemProxy {
                group: "G".to_string(),
                node: "N".to_string(),
                token: None,
            },
            DaemonCommand::CoreApiRequest {
                method: CoreApiMethod::Put,
                path: "/configs".to_string(),
                body: Some(serde_json::json!({"tun": {"enable": true}})),
                token: None,
            },
            DaemonCommand::PromoteSystemConfig {
                config_content: "tun:\n  enable: false\n".to_string(),
                config_revision: crate::tun_transaction::sha256_revision(
                    b"tun:\n  enable: false\n",
                ),
                subscription_id: None,
                change_kind: None,
                token: None,
            },
            // Fenced command carrying a foreign fence: refused by the gate.
            DaemonCommand::RestoreOldRuntime {
                fence: TransactionFence {
                    transaction_id: "some-other-transaction".to_string(),
                    generation: 99,
                    expected_phase: JournalPhase::RollbackPending,
                    expected_candidate_revision: "other".to_string(),
                },
                expected_old_runtime_revision: "old".to_string(),
                expected_old_runtime_tun: false,
                token: None,
            },
        ] {
            let response =
                process_command(cmd.clone(), Arc::clone(&state), peer_with_uid(Some(0))).await;
            match response {
                DaemonResponse::Error { message } => {
                    assert!(
                        message.contains(gate_marker),
                        "{cmd:?} must be refused by the gate: {message}"
                    );
                    assert!(
                        message.contains("Read-only commands remain available")
                            && message.contains("restart --system"),
                        "the refusal must distinguish read-only and offer the Fix: {message}"
                    );
                }
                other => panic!("{cmd:?} must be gated, got {other:?}"),
            }
        }

        // The transaction's own fenced command still passes the gate.
        let quiesce = process_command(
            DaemonCommand::QuiesceCandidateRuntime {
                fence: TransactionFence {
                    transaction_id: journal.transaction_id.clone(),
                    generation: journal.generation,
                    expected_phase: JournalPhase::RollbackPending,
                    expected_candidate_revision: journal.candidate_revision.clone(),
                },
                allow_legacy_reset: false,
                allow_restored_old_quiesce: false,
                token: None,
            },
            Arc::clone(&state),
            peer_with_uid(Some(0)),
        )
        .await;
        match quiesce {
            DaemonResponse::Error { message } => assert!(
                !message.contains(gate_marker),
                "the transaction's own fenced command must not be gated: {message}"
            ),
            DaemonResponse::Transaction { .. } => {}
            other => panic!("unexpected quiesce response: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&active_dir);
    }

    /// D3: repeated probe noise collapses to one line per window per kind, and the
    /// next emission reports how many identical events were swallowed.
    #[test]
    fn probe_noise_limiter_collapses_repeated_transport_errors() {
        let mut limiter = ProbeNoiseLimiter::default();
        // A CLI probe storm: 19 identical broken-pipe errors in one window.
        let mut emitted = 0;
        for offset in 0..19 {
            if limiter.admit("broken-pipe", 1_000 + offset).is_some() {
                emitted += 1;
            }
        }
        assert_eq!(emitted, 1, "19 identical noise lines must collapse to one");

        // Still inside the window: silent.
        assert_eq!(limiter.admit("broken-pipe", 1_059), None);

        // Next window re-emits once and reports the suppressed count (18 + 1).
        assert_eq!(limiter.admit("broken-pipe", 1_060), Some(19));
        assert_eq!(limiter.admit("broken-pipe", 1_061), None);

        // Categories are limited independently, so a different transport error is
        // still visible immediately.
        assert_eq!(limiter.admit("connection-reset", 1_061), Some(0));
    }

    /// D3: only transport-close noise is rate limited; real connection errors keep
    /// their unconditional log line.
    #[test]
    fn probe_noise_classification_keeps_real_failures_visible() {
        let broken_pipe = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "Broken pipe",
        ));
        assert_eq!(probe_noise_kind(&broken_pipe), Some("broken-pipe"));
        let context = broken_pipe.context("send response");
        assert_eq!(probe_noise_kind(&context), Some("broken-pipe"));

        let reset = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "Connection reset by peer",
        ));
        assert_eq!(probe_noise_kind(&reset), Some("connection-reset"));

        for real in [
            anyhow::anyhow!("invalid daemon command: missing field `type`"),
            anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "socket gone",
            )),
            anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "token rejected",
            )),
        ] {
            assert_eq!(
                probe_noise_kind(&real),
                None,
                "a real failure must not be folded into probe noise: {real}"
            );
        }
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
        assert!(validate_tun_peer_is_root(peer_with_uid(Some(0))).is_ok());
        assert!(validate_tun_peer_is_root(peer_with_uid(Some(1000))).is_err());
        assert!(validate_tun_peer_is_root(peer_with_uid(None))
            .unwrap_err()
            .contains("cannot determine IPC peer uid"));
    }

    #[test]
    fn api_endpoint_transport_maps_supported_endpoints() {
        assert!(matches!(
            api_endpoint_transport("/var/run/mihomo/mihomo.sock"),
            Some(ApiEndpoint::UnixSocket(p))
                if p == std::path::Path::new("/var/run/mihomo/mihomo.sock")
        ));
        assert!(matches!(
            api_endpoint_transport("unix:///tmp/mihomo.sock"),
            Some(ApiEndpoint::UnixSocket(p))
                if p == std::path::Path::new("/tmp/mihomo.sock")
        ));
        assert!(matches!(
            api_endpoint_transport(r"\\.\pipe\mihomo-core"),
            Some(ApiEndpoint::WindowsNamedPipe(p))
                if p == r"\\.\pipe\mihomo-core"
        ));
        assert!(matches!(
            api_endpoint_transport(r"pipe://\\.\pipe\mihomo-core"),
            Some(ApiEndpoint::WindowsNamedPipe(p))
                if p == r"\\.\pipe\mihomo-core"
        ));
        assert!(api_endpoint_transport("http://127.0.0.1:9090").is_none());
    }

    #[test]
    fn tun_peer_privilege_allowed_covers_both_platforms() {
        // Windows: elevation is the only gate; uid is unavailable on pipes.
        assert!(tun_peer_privilege_allowed(true, None, true).is_ok());
        assert!(tun_peer_privilege_allowed(true, None, false)
            .unwrap_err()
            .contains("elevated (Administrator)"));
        // Unix: root uid is the gate.
        assert!(tun_peer_privilege_allowed(false, Some(0), false).is_ok());
        assert!(tun_peer_privilege_allowed(false, Some(1000), true)
            .unwrap_err()
            .contains("TUN on/off requires root privileges"));
        assert!(tun_peer_privilege_allowed(false, None, false)
            .unwrap_err()
            .contains("cannot determine IPC peer uid"));
    }

    #[test]
    fn owner_record_roundtrip_sets_daemon_readable_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner");
        let record = OwnerRecord {
            uid: 1000,
            token_sha256: crate::tun_transaction::sha256_revision(b"test-secret-token"),
        };
        write_owner_record_to(&path, &record).unwrap();
        assert_eq!(read_owner_record_from(&path).unwrap(), record);
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
    fn validate_client_token_for_peer_all_branches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner");
        let fixture_value = "my-secret-token";
        let record = OwnerRecord {
            uid: 1000,
            token_sha256: crate::tun_transaction::sha256_revision(fixture_value.as_bytes()),
        };
        let _guard = crate::utils::env_test_lock().lock().unwrap();
        let old = std::env::var_os("MIHOMO_CLI_OWNER_PATH");
        unsafe {
            std::env::set_var("MIHOMO_CLI_OWNER_PATH", &path);
        }

        // Branch 1: root peer UID 0 passes without token
        assert!(validate_client_token_for_peer(None, Some(0)).is_ok());
        assert!(validate_client_token_for_peer(Some("irrelevant"), Some(0)).is_ok());

        // Branch 2: missing or empty token fails
        assert_eq!(
            validate_client_token_for_peer(None, Some(1000)),
            Err("invalid or missing auth token".to_string())
        );
        assert_eq!(
            validate_client_token_for_peer(Some(""), Some(1000)),
            Err("invalid or missing auth token".to_string())
        );

        // Branch 3: cannot determine peer UID
        assert_eq!(
            validate_client_token_for_peer(Some(fixture_value), None),
            Err("cannot determine IPC peer uid".to_string())
        );

        // Branch 4: owner record missing / unreadable
        let err = validate_client_token_for_peer(Some(fixture_value), Some(1000)).unwrap_err();
        assert!(err.contains("cannot read owner record"), "{err}");

        // Now write the owner record
        write_owner_record_to(&path, &record).unwrap();

        // Branch 5: peer UID mismatch
        assert_eq!(
            validate_client_token_for_peer(Some(fixture_value), Some(1001)),
            Err("auth token does not belong to IPC peer uid".to_string())
        );

        // Branch 6: token hash mismatch
        assert_eq!(
            validate_client_token_for_peer(Some("wrong-token"), Some(1000)),
            Err("invalid or missing auth token".to_string())
        );

        // Branch 7: valid token and matching peer UID
        assert!(validate_client_token_for_peer(Some(fixture_value), Some(1000)).is_ok());

        if let Some(v) = old {
            unsafe {
                std::env::set_var("MIHOMO_CLI_OWNER_PATH", v);
            }
        } else {
            unsafe {
                std::env::remove_var("MIHOMO_CLI_OWNER_PATH");
            }
        }
    }

    #[test]
    fn validate_client_token_for_peer_rejects_whitespace_only_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner");
        let fixture_value = "real-token";
        let record = OwnerRecord {
            uid: 1000,
            token_sha256: crate::tun_transaction::sha256_revision(fixture_value.as_bytes()),
        };
        write_owner_record_to(&path, &record).unwrap();
        let _guard = crate::utils::env_test_lock().lock().unwrap();
        let old = std::env::var_os("MIHOMO_CLI_OWNER_PATH");
        unsafe {
            std::env::set_var("MIHOMO_CLI_OWNER_PATH", &path);
        }

        assert_eq!(
            validate_client_token_for_peer(Some("   "), Some(1000)),
            Err("invalid or missing auth token".to_string())
        );
        assert_eq!(
            validate_client_token_for_peer(Some("\t\n"), Some(1000)),
            Err("invalid or missing auth token".to_string())
        );

        if let Some(v) = old {
            unsafe {
                std::env::set_var("MIHOMO_CLI_OWNER_PATH", v);
            }
        } else {
            unsafe {
                std::env::remove_var("MIHOMO_CLI_OWNER_PATH");
            }
        }
    }

    #[test]
    fn legacy_authorized_clients_migration_warning_test() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_file = dir.path().join("authorized-clients.json");
        let _guard = crate::utils::env_test_lock().lock().unwrap();
        let old = std::env::var_os("MIHOMO_CLI_LEGACY_AUTHORIZED_CLIENTS_PATH");
        unsafe {
            std::env::set_var("MIHOMO_CLI_LEGACY_AUTHORIZED_CLIENTS_PATH", &legacy_file);
        }

        // File does not exist -> no notice
        assert_eq!(check_legacy_authorized_clients_migration_notice(), None);

        // File exists -> triggers notice mentioning legacy file path and owner migration
        std::fs::write(&legacy_file, b"{}").unwrap();
        let notice = check_legacy_authorized_clients_migration_notice();
        assert!(notice.is_some());
        let notice_text = notice.unwrap();
        assert!(notice_text.contains("authorized-clients.json"));
        assert!(notice_text.contains("单所有者") || notice_text.contains("不再生效"));

        if let Some(v) = old {
            unsafe {
                std::env::set_var("MIHOMO_CLI_LEGACY_AUTHORIZED_CLIENTS_PATH", v);
            }
        } else {
            unsafe {
                std::env::remove_var("MIHOMO_CLI_LEGACY_AUTHORIZED_CLIENTS_PATH");
            }
        }
    }

    #[test]
    fn constant_time_token_eq_handles_different_lengths() {
        assert!(constant_time_token_eq("abc", "abc"));
        assert!(!constant_time_token_eq("abc", "abcd"));
        assert!(!constant_time_token_eq("abcd", "abc"));
        assert!(!constant_time_token_eq("", "a"));
        assert!(!constant_time_token_eq("a", ""));
        assert!(constant_time_token_eq("", ""));
        // Ensure the function doesn't panic on very different lengths
        assert!(!constant_time_token_eq(
            "x",
            "a]very_long_token_string_1234567890"
        ));
        assert!(!constant_time_token_eq(
            "a_very_long_token_string_1234567890",
            "x"
        ));
    }

    #[test]
    fn read_owner_record_from_rejects_corrupted_file() {
        let dir = tempfile::tempdir().unwrap();
        // Empty file
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert!(read_owner_record_from(&empty).is_err());
        // Invalid JSON
        let garbage = dir.path().join("garbage");
        std::fs::write(&garbage, b"not json at all").unwrap();
        assert!(read_owner_record_from(&garbage).is_err());
        // Valid JSON but wrong shape
        let wrong_shape = dir.path().join("wrong");
        std::fs::write(&wrong_shape, br#"{"foo": "bar"}"#).unwrap();
        assert!(read_owner_record_from(&wrong_shape).is_err());
        // Non-existent file
        assert!(read_owner_record_from(&dir.path().join("nonexistent")).is_err());
    }

    #[tokio::test]
    async fn process_command_rejects_unauthorized_client_token() {
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let response = process_command(
            DaemonCommand::GetStatus { token: None },
            state,
            peer_with_uid(Some(1000)),
        )
        .await;
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
            peer_with_uid(Some(1000)),
        )
        .await;

        assert!(matches!(response, DaemonResponse::Error { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn process_command_rejects_disable_tun_from_non_root_peer() {
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let response = process_command(
            DaemonCommand::DisableTun { token: None },
            state,
            peer_with_uid(Some(1000)),
        )
        .await;

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
            "config_path": "/home/user/.config/mihomo/config.yaml",
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

    #[cfg(unix)]
    #[test]
    fn start_core_never_borrows_an_unprovable_endpoint_peer() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket = temp.path().join("api.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let endpoint = format!("unix://{}", socket.display());
        let message = unreusable_endpoint_message(
            &endpoint,
            std::path::Path::new("/bin/sleep"),
            &temp.path().join("config.yaml"),
        );
        assert!(message.contains("unprovable"), "{message}");
        assert!(
            message.contains("refusing to start a second Core"),
            "{message}"
        );
        assert!(
            !message.contains("kill"),
            "no manual kill guidance: {message}"
        );
        drop(listener);

        let absent_endpoint = format!("unix://{}", temp.path().join("missing.sock").display());
        assert_eq!(
            unreusable_endpoint_message(
                &absent_endpoint,
                std::path::Path::new("/bin/sleep"),
                &temp.path().join("config.yaml"),
            ),
            duplicate_core_endpoint_message(&absent_endpoint)
        );
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
            PathBuf::from("/home/user/.config/mihomo/config.yaml"),
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
            config_path: Some(PathBuf::from("/home/user/.config/mihomo/config.yaml")),
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
            Some(PathBuf::from("/home/user/.config/mihomo/config.yaml"))
        );
        assert_eq!(s.core_binary, Some(expected_system_core_binary_path()));
        assert_eq!(s.api_endpoint, None);
    }

    #[tokio::test]
    async fn restart_core_preflights_new_config_before_stopping_current_core() {
        isolate_daemon_runtime_for_tests();
        // Another test may hold a journal in this shared runtime dir.
        let _serialized = runtime_journal_test_lock().lock().await;
        let tmp = tempfile::TempDir::new().unwrap();
        #[cfg(target_os = "macos")]
        let config_path = PathBuf::from("/Users/alice/.config/mihomo/config.yaml");
        #[cfg(not(target_os = "macos"))]
        let config_path = PathBuf::from("/home/user/.config/mihomo/config.yaml");
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
                subscription_id: None,
                token: None,
            },
            Arc::clone(&state),
            peer_with_uid(Some(0)),
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
        let valid = "/home/user/.config/mihomo/config.yaml";
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
            "/home/user/.config/mihomo/config.yaml"
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
            "/home/user/.config/other/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("system daemon only accepts"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/user/extra/.config/mihomo/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("system daemon only accepts"));
        let err = validate_daemon_config_path_shape(std::path::Path::new("relative/config.yaml"))
            .unwrap_err();
        assert!(err.contains("non-absolute"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/user/../bob/.config/mihomo/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("must not contain . or .. components"));
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/user/./.config/mihomo/config.yaml",
        ))
        .unwrap_err();
        assert!(err.contains("must not contain . or .. components"));

        // Invalid tun-config.yaml path should be rejected.
        let err = validate_daemon_config_path_shape(std::path::Path::new(
            "/home/user/.config/mihomo/tun-config.yaml",
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
        assert!(
            !is_lifecycle_command(&cmd),
            "GetStatus must not be a lifecycle command"
        );

        let cmd = DaemonCommand::GetStatus {
            token: Some("authorized-token".to_string()),
        };
        assert!(
            !is_lifecycle_command(&cmd),
            "authenticated GetStatus must not take the lifecycle lock"
        );

        // CoreApiRequest 与 SelectSystemProxy 也无需获取生命周期排他锁
        let cmd_api = DaemonCommand::CoreApiRequest {
            method: crate::ipc::CoreApiMethod::Get,
            path: "/proxies".to_string(),
            body: None,
            token: None,
        };
        assert!(
            !is_lifecycle_command(&cmd_api),
            "CoreApiRequest must not take lifecycle lock"
        );

        let cmd_select = DaemonCommand::SelectSystemProxy {
            group: "Proxy".to_string(),
            node: "NodeA".to_string(),
            token: None,
        };
        assert!(
            !is_lifecycle_command(&cmd_select),
            "SelectSystemProxy must not take lifecycle lock"
        );

        drop(guard);

        // 生命周期命令（StartCore / RestartCore）应标记为需要排他锁
        let cmd = DaemonCommand::StartCore {
            config_content: "mode: rule\n".to_string(),
            config_revision: "revision".to_string(),
            subscription_id: None,
            token: None,
        };
        assert!(
            is_lifecycle_command(&cmd),
            "StartCore must be a lifecycle command"
        );

        let cmd_restart = DaemonCommand::RestartCore {
            config_content: "mode: rule\n".to_string(),
            config_revision: "revision".to_string(),
            subscription_id: None,
            token: None,
        };
        assert!(
            is_lifecycle_command(&cmd_restart),
            "RestartCore must be a lifecycle command"
        );
    }

    #[test]
    fn reaping_crashed_core_preserves_recovery_intent() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = PathBuf::from("/home/user/.config/mihomo/config.yaml");
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
        isolate_daemon_runtime_for_tests();
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
            peer_with_uid(Some(0)),
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
        isolate_daemon_runtime_for_tests();
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: false,
            core_binary: None,
            config_path: None,
            ..DaemonState::default()
        }));

        let response = process_command(
            DaemonCommand::DisableTun { token: None },
            state,
            peer_with_uid(Some(0)),
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
    async fn enable_tun_with_core_running_does_not_error_on_binary_check() {
        let config_path = if cfg!(target_os = "macos") {
            PathBuf::from("/Users/alice/.config/mihomo/config.yaml")
        } else {
            PathBuf::from("/home/user/.config/mihomo/config.yaml")
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
            peer_with_uid(Some(0)),
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

        let failure = classify_core_startup_failure(log, crate::utils::CacheDbState::Healthy);
        assert_eq!(failure, CoreFailure::GeoipMissing);
    }

    #[test]
    fn classify_geosite_missing() {
        let log = r#"time="2026-08-12T14:50:39" level=info msg="Can't find GeoSite.dat""#;
        let failure = classify_core_startup_failure(log, crate::utils::CacheDbState::Healthy);
        assert_eq!(failure, CoreFailure::GeositeMissing);
    }

    #[test]
    fn classify_config_parse_error() {
        let log = r#"time="2026-08-12T14:50:39" level=fatal msg="Parse config error: invalid key 'foo' at line 42""#;
        let failure = classify_core_startup_failure(log, crate::utils::CacheDbState::Healthy);
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
        let failure = classify_core_startup_failure(log, crate::utils::CacheDbState::Healthy);
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
        let failure = classify_core_startup_failure(log, crate::utils::CacheDbState::Healthy);
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
        let failure = classify_core_startup_failure(log, crate::utils::CacheDbState::Healthy);
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
        let failure = classify_core_startup_failure(log, crate::utils::CacheDbState::Healthy);
        assert!(matches!(failure, CoreFailure::Unknown { .. }));
    }

    #[test]
    fn classify_empty_log_returns_unknown() {
        let failure = classify_core_startup_failure("", crate::utils::CacheDbState::Healthy);
        assert!(matches!(failure, CoreFailure::Unknown { .. }));
    }

    #[test]
    fn classify_cache_db_invalid_when_content_invalid_and_core_failed() {
        // SPEC A3: a bbolt failure plus a ContentInvalid verdict is attributable.
        let log = r#"time="2026-10-20T18:00:00" level=error msg="cache.db: invalid database""#;
        let failure =
            classify_core_startup_failure(log, crate::utils::CacheDbState::ContentInvalid);
        assert_eq!(failure, CoreFailure::CacheDbInvalid);
        let message = format_core_failure_message(
            &failure,
            std::path::Path::new("/var/log/mihomo/mihomo.log"),
        );
        assert!(message.contains("Fix: mihomo-cli restart --system"));

        // Without a cache.db verdict and without naming cache.db in the log, a
        // generic bbolt error must not be attributed to the persistence file.
        let anonymous_log = r#"time="2026-10-20T18:00:00" level=error msg="invalid database""#;
        let failure =
            classify_core_startup_failure(anonymous_log, crate::utils::CacheDbState::Healthy);
        assert!(matches!(failure, CoreFailure::Unknown { .. }));
        let failure = classify_core_startup_failure(
            anonymous_log,
            crate::utils::CacheDbState::ContentInvalid,
        );
        assert_eq!(failure, CoreFailure::CacheDbInvalid);
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
    #[cfg_attr(not(unix), allow(unused_imports))]
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
        let _response = handle_quiesce_candidate_runtime(
            std::sync::Arc::clone(&state),
            fence.clone(),
            false,
            false,
            peer_with_uid(Some(0)),
        )
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
            peer_with_uid(Some(0)),
        )
        .await;

        let state_guard = state.lock().await;
        assert!(
            state_guard.core_running,
            "running unknown core must not be stopped by apply_promoted_snapshot"
        );
    }

    /// T2: the daemon's independent defense. A SnapshotPromoted
    /// journal with a valid candidate and matching snapshot digest must still
    /// be refused when the running Core identity does not match the attested
    /// old runtime, and the running process must survive the rejection.
    #[cfg(unix)]
    #[tokio::test]
    async fn apply_promoted_snapshot_refuses_unattested_running_core() {
        use super::*;
        use crate::tun_transaction::{
            JournalPhase, LaunchSource, OldRuntimeEvidence, TransactionFence, TransactionResponse,
            TunJournal,
        };

        isolate_daemon_runtime_for_tests();
        // The journal we write is also the §12.4 gate input of the shared test
        // runtime dir; serialize with the tests that assert on that gate.
        let _serialized = runtime_journal_test_lock().lock().await;
        let ctx = daemon_transaction_context().expect("system transaction context");
        let active_dir = crate::tun_transaction::transactions_dir(&ctx).join("active");

        let candidate_bytes = b"tun:\n  enable: true\n";
        let candidate_revision = crate::tun_transaction::sha256_revision(candidate_bytes);
        std::fs::create_dir_all(&active_dir).unwrap();
        std::fs::write(
            crate::tun_transaction::active_candidate_path(&ctx),
            candidate_bytes,
        )
        .unwrap();
        std::fs::write(&ctx.paths.tun_config_file, candidate_bytes).unwrap();

        let attested_identity = expected_system_core_binary_path()
            .to_string_lossy()
            .to_string();
        let evidence = OldRuntimeEvidence {
            core_running: true,
            core_identity: attested_identity,
            core_pid: 4242,
            launched_revision: "old-revision".to_string(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: true,
            api_endpoint: "/var/run/mihomo/mihomo.sock".to_string(),
            recorded_at_secs: None,
        };
        let evidence_bytes = serde_json::to_vec_pretty(&evidence).unwrap();
        std::fs::write(
            crate::tun_transaction::active_old_runtime_path(&ctx),
            &evidence_bytes,
        )
        .unwrap();

        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "apply-unattested-test".to_string(),
            generation: 13,
            phase: JournalPhase::SnapshotPromoted,
            original_uid: 1000,
            target_runtime_tun: true,
            base_intent_revision: None,
            candidate_revision: candidate_revision.clone(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: crate::tun_transaction::sha256_revision(&evidence_bytes),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: Some(crate::tun_transaction::now_rfc3339()),
        };
        crate::tun_transaction::write_active_journal(&ctx, &journal).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn unknown runtime stand-in");
        let pid = child.id().expect("runtime pid");
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: true,
            core_child: Some(child),
            core_pid: Some(pid),
            core_binary: Some(std::path::PathBuf::from("/usr/bin/unknown-core-identity")),
            launched_config_revision: Some(candidate_revision.clone()),
            api_endpoint: None,
            pid_file: tmp.path().join("core.pid"),
            core_log_file: tmp.path().join("core.log"),
            ..DaemonState::default()
        }));

        let fence = TransactionFence {
            transaction_id: journal.transaction_id.clone(),
            generation: journal.generation,
            expected_phase: journal.phase,
            expected_candidate_revision: candidate_revision,
        };
        let response =
            handle_apply_promoted_snapshot(Arc::clone(&state), fence, true, peer_with_uid(Some(0)))
                .await;

        match response {
            DaemonResponse::Transaction {
                response: TransactionResponse::EvidenceMismatch { error, .. },
            } => {
                assert_eq!(
                    error.stage, "verify_running_core_before_apply",
                    "the apply must be refused before stopping the unattested runtime"
                );
            }
            other => panic!("an unattested running core must be refused: {other:?}"),
        }

        let mut guard = state.lock().await;
        assert!(
            guard.core_child.is_some(),
            "a rejected apply must not stop the running Core"
        );
        if let Some(child) = guard.core_child.as_mut() {
            let waited = child.try_wait().unwrap();
            assert!(waited.is_none(), "the Core must still be alive");
        }
        if let Some(mut child) = guard.core_child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        drop(guard);
        std::fs::remove_dir_all(&active_dir).ok();
        let _ = std::fs::remove_file(&ctx.paths.tun_config_file);
    }
}

#[cfg(test)]
mod autostart_tests {
    use super::*;

    #[test]
    fn test_snapshot_tun_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.yaml");
        assert!(!super::snapshot_tun_enabled(&path));

        std::fs::write(&path, "tun:\n  enable: false\n").unwrap();
        assert!(!super::snapshot_tun_enabled(&path));

        std::fs::write(&path, "tun:\n  enable: true\n").unwrap();
        assert!(super::snapshot_tun_enabled(&path));

        std::fs::write(&path, "not yaml [").unwrap();
        assert!(!super::snapshot_tun_enabled(&path));
    }

    #[test]
    fn test_resolve_autostart_target_config_path_prefers_tun_when_enabled() {
        let temp = tempfile::tempdir().unwrap();
        let runtime_dir = temp.path();
        let tun_path = runtime_dir.join("tun-config.yaml");
        let active_path = runtime_dir.join("active-config.yaml");

        std::fs::write(&active_path, "mixed-port: 7890\n").unwrap();
        std::fs::write(&tun_path, "mixed-port: 7890\ntun:\n  enable: true\n").unwrap();

        let resolved = resolve_autostart_target_config_path(runtime_dir);
        assert_eq!(resolved, Some(tun_path));
    }

    #[test]
    fn test_resolve_autostart_target_config_path_falls_back_when_tun_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let runtime_dir = temp.path();
        let tun_path = runtime_dir.join("tun-config.yaml");
        let active_path = runtime_dir.join("active-config.yaml");

        std::fs::write(&active_path, "mixed-port: 7890\n").unwrap();
        std::fs::write(&tun_path, "mixed-port: 7890\ntun:\n  enable: false\n").unwrap();

        let resolved = resolve_autostart_target_config_path(runtime_dir);
        assert_eq!(resolved, Some(active_path));
    }

    #[test]
    fn test_resolve_autostart_target_config_path_returns_active_when_tun_missing() {
        let temp = tempfile::tempdir().unwrap();
        let runtime_dir = temp.path();
        let active_path = runtime_dir.join("active-config.yaml");

        std::fs::write(&active_path, "mixed-port: 7890\n").unwrap();

        let resolved = resolve_autostart_target_config_path(runtime_dir);
        assert_eq!(resolved, Some(active_path));
    }

    #[test]
    fn test_resolve_autostart_target_config_path_returns_none_when_both_missing() {
        let temp = tempfile::tempdir().unwrap();
        let runtime_dir = temp.path();

        let resolved = resolve_autostart_target_config_path(runtime_dir);
        assert_eq!(resolved, None);
    }

    #[test]
    fn test_resolve_autostart_target_config_path_handles_invalid_yaml_gracefully() {
        let temp = tempfile::tempdir().unwrap();
        let runtime_dir = temp.path();
        let tun_path = runtime_dir.join("tun-config.yaml");
        let active_path = runtime_dir.join("active-config.yaml");

        std::fs::write(&active_path, "mixed-port: 7890\n").unwrap();
        std::fs::write(&tun_path, "invalid yaml: [[[:::").unwrap();

        let resolved = resolve_autostart_target_config_path(runtime_dir);
        assert_eq!(resolved, Some(active_path));
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_autostart_target_config_path_rejects_symlink_tun_config() {
        let temp = tempfile::tempdir().unwrap();
        let runtime_dir = temp.path();
        let active_path = runtime_dir.join("active-config.yaml");
        let real_tun_file = temp.path().join("real-tun.yaml");
        let tun_symlink = runtime_dir.join("tun-config.yaml");

        std::fs::write(&active_path, "mixed-port: 7890\n").unwrap();
        std::fs::write(&real_tun_file, "mixed-port: 7890\ntun:\n  enable: true\n").unwrap();
        std::os::unix::fs::symlink(&real_tun_file, &tun_symlink).unwrap();

        // 遇到符号链接 tun-config.yaml，snapshot_tun_enabled 安全拒绝，回退到 active-config.yaml
        let resolved = resolve_autostart_target_config_path(runtime_dir);
        assert_eq!(resolved, Some(active_path));
    }

    // ---------------- cache.db spawn precondition (SPEC A2) ----------------

    /// Every Core spawn path (IPC start/restart/promote, autostart, startup and
    /// failure retry) funnels through `start_core`, which owns the only spawn site.
    #[test]
    fn every_core_spawn_path_shares_the_cache_db_precondition() {
        let source = include_str!("daemon.rs");
        let spawn_site = format!("cmd.{}", "spawn()");
        assert_eq!(
            source.matches(&spawn_site).count(),
            1,
            "the Core must have exactly one spawn site so the quarantine precondition cannot be bypassed"
        );
        let gate = source
            .find("cache_db_gate_for_start(&mut s,")
            .expect("start_core must call the cache.db gate");
        let spawn = source.find(&spawn_site).expect("core spawn site");
        assert!(
            gate < spawn,
            "the cache.db quarantine precondition must run before the Core is spawned"
        );
    }

    #[cfg(unix)]
    fn write_daemon_cache_db_fixture(dir: &std::path::Path, bytes: &[u8]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(crate::utils::CACHE_DB_FILE_NAME);
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    #[cfg(unix)]
    fn quarantine_names(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_str().map(str::to_string)?;
                name.starts_with(crate::utils::CACHE_DB_QUARANTINE_PREFIX)
                    .then_some(name)
            })
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn cache_db_gate_quarantines_content_invalid_before_any_spawn() {
        let temp = tempfile::tempdir().unwrap();
        let mut image = crate::utils::bolt_test_db_image(4096);
        crate::utils::bolt_test_corrupt_meta(&mut image, 0);
        crate::utils::bolt_test_corrupt_meta(&mut image, 4096);
        let path = write_daemon_cache_db_fixture(temp.path(), &image);

        let mut state = DaemonState {
            pid_file: temp.path().join("core.pid"),
            core_log_file: temp.path().join("core.log"),
            ..DaemonState::default()
        };
        let observed = cache_db_gate_for_start(&mut state, temp.path()).unwrap();

        assert_eq!(observed, crate::utils::CacheDbState::ContentInvalid);
        assert!(!path.exists(), "the invalid cache.db must be moved away");
        assert_eq!(quarantine_names(temp.path()).len(), 1);
    }

    /// D2 single track: the daemon never repairs cache.db permissions, it only
    /// reports `repair_required` so the CLI's confirmation contract repairs them.
    #[cfg(unix)]
    #[test]
    fn cache_db_gate_reports_repair_required_without_repairing() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let image = crate::utils::bolt_test_db_image(4096);
        let path = write_daemon_cache_db_fixture(temp.path(), &image);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let mut state = DaemonState {
            pid_file: temp.path().join("core.pid"),
            core_log_file: temp.path().join("core.log"),
            ..DaemonState::default()
        };
        let error = cache_db_gate_for_start(&mut state, temp.path()).unwrap_err();

        assert!(error.contains("repair_required"), "{error}");
        assert!(
            error.contains("Fix: mihomo-cli restart --system"),
            "{error}"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().mode() & 0o777,
            0o644,
            "the daemon must not change permissions"
        );
        assert_eq!(std::fs::read(&path).unwrap(), image);
        assert!(
            quarantine_names(temp.path()).is_empty(),
            "a permission-only anomaly must never quarantine"
        );
    }

    /// A file that validates but cannot be opened by bbolt (truncated) must be
    /// quarantined, not handed over and deleted by the Core.
    #[cfg(unix)]
    #[test]
    fn cache_db_gate_isolates_truncated_file_that_cannot_be_opened() {
        let temp = tempfile::tempdir().unwrap();
        let image = crate::utils::bolt_test_db_image(4096);
        let path = write_daemon_cache_db_fixture(temp.path(), &image[..6000]);

        let mut state = DaemonState {
            pid_file: temp.path().join("core.pid"),
            core_log_file: temp.path().join("core.log"),
            ..DaemonState::default()
        };
        let observed = cache_db_gate_for_start(&mut state, temp.path()).unwrap();

        assert_eq!(observed, crate::utils::CacheDbState::Degraded);
        assert!(!path.exists(), "the unopenable file must be moved aside");
        assert_eq!(quarantine_names(temp.path()).len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn cache_db_gate_blocks_start_when_quarantine_impossible() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            // root bypasses directory write bits, so the failure cannot be provoked.
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let path = write_daemon_cache_db_fixture(temp.path(), &vec![0u8; 16 * 1024]);
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

        let mut state = DaemonState {
            pid_file: temp.path().join("core.pid"),
            core_log_file: temp.path().join("core.log"),
            ..DaemonState::default()
        };
        let error = cache_db_gate_for_start(&mut state, temp.path()).unwrap_err();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            error.contains("Fix: mihomo-cli restart --system"),
            "isolation failure must fail closed with a fix command: {error}"
        );
        assert!(
            path.exists(),
            "the original file must be preserved as evidence"
        );
    }

    // ---------------- stop proof / quiesce true stop (SPEC B3) ----------------

    #[cfg(unix)]
    async fn running_sleep_core(dir: &std::path::Path) -> (Arc<Mutex<DaemonState>>, u32) {
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn live core stand-in");
        let pid = child.id().expect("child pid");
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: true,
            core_child: Some(child),
            core_pid: Some(pid),
            pid_file: dir.join("core.pid"),
            core_log_file: dir.join("core.log"),
            ..DaemonState::default()
        }));
        (state, pid)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_core_timeout_keeps_child_visible_to_liveness_proofs() {
        let temp = tempfile::tempdir().unwrap();
        let (state, pid) = running_sleep_core(temp.path()).await;
        let elapsed = tokio::time::timeout(std::time::Duration::ZERO, std::future::pending::<()>())
            .await
            .expect_err("zero budget must elapse");
        let child = {
            let mut guard = state.lock().await;
            guard.core_child.take().expect("live child")
        };

        let mut guard = state.lock().await;
        let (response, proof) = settle_stop(
            &mut guard,
            child,
            pid,
            Ok(()),
            Err(elapsed),
            STOP_CORE_WAIT_TIMEOUT,
        );

        assert!(
            matches!(response, DaemonResponse::Error { .. }),
            "a timed-out stop must be an error: {response:?}"
        );
        assert!(proof.is_none(), "no stopped proof may be produced");
        assert!(
            guard.core_child.is_some(),
            "a failed stop must keep the child so liveness proofs still see pid {pid}"
        );
        assert!(guard.core_running, "state must not claim a stopped core");
        if let Some(mut child) = guard.core_child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_core_success_returns_the_single_stopped_proof() {
        let temp = tempfile::tempdir().unwrap();
        let (state, pid) = running_sleep_core(temp.path()).await;

        let (response, proof) = stop_core_with_proof(Arc::clone(&state)).await;

        assert!(matches!(response, DaemonResponse::Success { .. }));
        let proof = proof.expect("a successful stop must produce a proof");
        assert_eq!(proof.pid, pid);
        assert!(!proof.stopped_at.is_empty());
        let guard = state.lock().await;
        assert!(guard.core_child.is_none());
        assert!(!guard.core_running);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn quiesce_true_stop_has_no_phase_carve_out() {
        // 去脂裁决②: a running runtime (candidate or attested old Core) reaches the
        // same true stop for both phases; there is no "attested but running" proof.
        for phase in [
            crate::tun_transaction::JournalPhase::RollbackPending,
            crate::tun_transaction::JournalPhase::RecoveryRequired,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let (state, pid) = running_sleep_core(temp.path()).await;

            let (proof, obs_after) = quiesce_stop_runtime(Arc::clone(&state), "quiesce_test")
                .await
                .expect("a running runtime must be stopped");
            assert_eq!(proof.pid, pid);
            assert!(
                !obs_after.core_running,
                "runtime must be observably stopped for {phase:?}"
            );

            let fence = crate::tun_transaction::TransactionFence {
                transaction_id: "txn".to_string(),
                generation: 1,
                expected_phase: phase,
                expected_candidate_revision: "candidate".to_string(),
            };
            let runtime_proof = quiesced_proof(&fence, phase, &obs_after, &proof);
            assert_eq!(runtime_proof.core_pid, 0, "0 proves the runtime is stopped");
            assert!(runtime_proof.launched_revision.is_empty());
            assert!(!runtime_proof.api_ready);
            assert!(!runtime_proof.runtime_tun);
            assert_eq!(runtime_proof.observed_phase, phase);
        }
    }

    #[cfg(unix)]
    fn quiesce_test_journal(
        ctx: &crate::instance::InstanceContext,
        candidate_pid: u32,
        candidate_identity: &str,
    ) -> crate::tun_transaction::TunJournal {
        use crate::tun_transaction::{JournalPhase, TunJournal};

        std::fs::create_dir_all(crate::tun_transaction::transactions_dir(ctx).join("active"))
            .unwrap();
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "quiesce-identity-test".to_string(),
            generation: 11,
            phase: JournalPhase::RecoveryRequired,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "rev-candidate".to_string(),
            candidate_core_identity: Some(candidate_identity.to_string()),
            candidate_core_pid: Some(candidate_pid),
            old_snapshot_revision: None,
            old_runtime_digest: "expected-digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: Some(crate::tun_transaction::now_rfc3339()),
        };
        crate::tun_transaction::write_active_journal(ctx, &journal).unwrap();
        journal
    }

    /// P1-4: identity is a disjunction. A candidate that matches the journal
    /// fingerprint is self-proving and must be stopped even when the old-runtime
    /// evidence is missing or drifted; an unproven candidate with broken evidence
    /// must still be rejected without stopping anything.
    #[cfg(unix)]
    #[tokio::test]
    async fn quiesce_candidate_identity_does_not_depend_on_old_runtime_evidence() {
        use crate::tun_transaction::{
            JournalPhase, LaunchSource, OldRuntimeEvidence, TransactionFence, TransactionResponse,
        };

        isolate_daemon_runtime_for_tests();
        // The journal we write is also the §12.4 gate input of the shared test
        // runtime dir; serialize with the tests that assert on that gate.
        let _serialized = runtime_journal_test_lock().lock().await;
        let ctx = daemon_transaction_context().expect("system transaction context");
        let core_identity = expected_system_core_binary_path();
        let identity_text = core_identity.to_string_lossy().to_string();
        let active_dir = crate::tun_transaction::transactions_dir(&ctx).join("active");
        // Test hygiene: the "evidence missing" case must not depend on leftovers
        // from earlier tests in this binary (review suggestion, narrow re-review).
        let _ = std::fs::remove_file(active_dir.join("old-runtime.json"));

        let fence_for = |journal: &crate::tun_transaction::TunJournal| TransactionFence {
            transaction_id: journal.transaction_id.clone(),
            generation: journal.generation,
            expected_phase: journal.phase,
            expected_candidate_revision: journal.candidate_revision.clone(),
        };

        let write_old_evidence = |pid: u32, revision: &str| {
            let evidence = OldRuntimeEvidence {
                core_running: true,
                core_identity: identity_text.clone(),
                core_pid: pid,
                launched_revision: revision.to_string(),
                launch_source: LaunchSource::SystemTunSnapshot,
                runtime_tun: false,
                api_endpoint: "/var/run/mihomo/mihomo.sock".to_string(),
                recorded_at_secs: None,
            };
            std::fs::write(
                active_dir.join("old-runtime.json"),
                serde_json::to_vec(&evidence).unwrap(),
            )
            .unwrap();
        };

        // Case 1 (evidence missing) and case 2 (evidence digest drifted): the
        // matched candidate must be quiesced with a stopped proof in both.
        for drift in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let child = tokio::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn candidate stand-in");
            let pid = child.id().expect("candidate pid");
            let journal = quiesce_test_journal(&ctx, pid, &identity_text);
            if drift {
                // Digest in the journal stays "expected-digest" -> validation fails.
                write_old_evidence(pid, "old-revision");
            }
            let state = Arc::new(Mutex::new(DaemonState {
                core_running: true,
                core_child: Some(child),
                core_pid: Some(pid),
                core_binary: Some(core_identity.clone()),
                launched_config_revision: Some(journal.candidate_revision.clone()),
                api_endpoint: None,
                pid_file: tmp.path().join("core.pid"),
                core_log_file: tmp.path().join("core.log"),
                ..DaemonState::default()
            }));

            let response = handle_quiesce_candidate_runtime(
                Arc::clone(&state),
                fence_for(&journal),
                false,
                false,
                peer_with_uid(Some(0)),
            )
            .await;

            match response {
                DaemonResponse::Transaction {
                    response: TransactionResponse::Completed(proof),
                } => {
                    assert_eq!(proof.core_pid, 0, "stopped proof (drift={drift})");
                    assert!(proof.launched_revision.is_empty());
                    assert_eq!(proof.observed_phase, JournalPhase::RecoveryRequired);
                    assert_eq!(proof.transaction_id, journal.transaction_id);
                }
                other => panic!(
                    "a matched candidate must be quiesced regardless of old evidence (drift={drift}): {other:?}"
                ),
            }
            let guard = state.lock().await;
            assert!(
                guard.core_child.is_none(),
                "the candidate must actually be stopped (drift={drift})"
            );
            drop(guard);
            std::fs::remove_dir_all(&active_dir).ok();
        }

        // Case 3: the running Core is not the candidate and the old evidence is
        // broken -> rejection, and the Core keeps running (fail-closed).
        let tmp = tempfile::tempdir().unwrap();
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn unknown runtime stand-in");
        let pid = child.id().expect("runtime pid");
        let journal = quiesce_test_journal(&ctx, pid + 1, &identity_text);
        write_old_evidence(pid, "old-revision");
        let state = Arc::new(Mutex::new(DaemonState {
            core_running: true,
            core_child: Some(child),
            core_pid: Some(pid),
            core_binary: Some(core_identity.clone()),
            launched_config_revision: Some(journal.candidate_revision.clone()),
            api_endpoint: None,
            pid_file: tmp.path().join("core.pid"),
            core_log_file: tmp.path().join("core.log"),
            ..DaemonState::default()
        }));

        let response = handle_quiesce_candidate_runtime(
            Arc::clone(&state),
            fence_for(&journal),
            false,
            false,
            peer_with_uid(Some(0)),
        )
        .await;

        match response {
            DaemonResponse::Transaction {
                response: TransactionResponse::EvidenceMismatch { error, .. },
            } => {
                assert_eq!(
                    error.stage, "validate_old_runtime_for_quiesce",
                    "an unproven candidate must still fail on the broken evidence"
                );
            }
            other => panic!("unknown identity with broken evidence must be rejected: {other:?}"),
        }
        let mut guard = state.lock().await;
        assert!(
            guard.core_child.is_some(),
            "a rejected quiesce must not stop the running Core"
        );
        if let Some(child) = guard.core_child.as_mut() {
            let waited = child.try_wait().unwrap();
            assert!(waited.is_none(), "the Core must still be alive");
        }
        if let Some(mut child) = guard.core_child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        drop(guard);
        std::fs::remove_dir_all(&active_dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn managed_core_liveness_only_trusts_the_recorded_core_process() {
        use std::os::unix::process::CommandExt;

        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("core.pid");

        // No pid file: proven stopped.
        assert_eq!(managed_core_liveness_at(&pid_file), CoreLiveness::Stopped);

        // A live process whose argv[0] is the recorded system core binary.
        let mut child = std::process::Command::new("/bin/sleep")
            .arg0(expected_system_core_binary_path())
            .arg("30")
            .spawn()
            .expect("spawn fake core");
        let metadata = CorePidMetadata {
            pid: child.id(),
            config_path: std::path::PathBuf::from("/var/lib/mihomo-cli/tun-config.yaml"),
            core_binary: expected_system_core_binary_path(),
            api_endpoint: Some(expected_system_core_api_endpoint().to_string()),
            config_revision: None,
        };
        std::fs::write(&pid_file, serde_json::to_vec(&metadata).unwrap()).unwrap();
        // `spawn` returns before `execve`, so poll briefly for the new cmdline.
        let mut observed = managed_core_liveness_at(&pid_file);
        for _ in 0..100 {
            if observed == CoreLiveness::Running(child.id()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            observed = managed_core_liveness_at(&pid_file);
        }
        assert_eq!(observed, CoreLiveness::Running(child.id()));

        // A recycled pid belonging to a different binary is not a managed core.
        let recycled = CorePidMetadata {
            core_binary: PathBuf::from("/usr/local/lib/mihomo/other-core"),
            ..metadata.clone()
        };
        std::fs::write(&pid_file, serde_json::to_vec(&recycled).unwrap()).unwrap();
        assert_eq!(managed_core_liveness_at(&pid_file), CoreLiveness::Stopped);

        // The recorded pid is dead: proven stopped.
        let _ = child.kill();
        let _ = child.wait();
        std::fs::write(&pid_file, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert_eq!(managed_core_liveness_at(&pid_file), CoreLiveness::Stopped);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cache_db_gate_refuses_to_touch_a_running_core() {
        let temp = tempfile::tempdir().unwrap();
        let path = write_daemon_cache_db_fixture(temp.path(), &vec![0u8; 16 * 1024]);

        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn live core stand-in");
        let mut state = DaemonState {
            core_running: true,
            core_child: Some(child),
            pid_file: temp.path().join("core.pid"),
            core_log_file: temp.path().join("core.log"),
            ..DaemonState::default()
        };

        let error = cache_db_gate_for_start(&mut state, temp.path()).unwrap_err();
        assert!(
            error.contains("still running"),
            "a Core that has not exited must block the stop-write window: {error}"
        );
        assert!(
            path.exists(),
            "no isolation may happen while the Core is alive"
        );
        assert!(quarantine_names(temp.path()).is_empty());

        if let Some(mut child) = state.core_child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

#[cfg(test)]
mod legacy_reset_admission_tests {
    use super::*;

    // ----------------: current-instance ownership ----------------

    fn managed_launch_record(pid: u32) -> CorePidMetadata {
        CorePidMetadata {
            pid,
            config_path: PathBuf::from("/var/lib/mihomo-cli/tun-config.yaml"),
            core_binary: expected_system_core_binary_path(),
            api_endpoint: Some(format!("unix://{}", expected_system_core_api_endpoint())),
            config_revision: Some("launch-revision".to_string()),
        }
    }

    fn state_with_launch_record(
        pid_file: &std::path::Path,
        record: &CorePidMetadata,
    ) -> DaemonState {
        DaemonState {
            core_running: true,
            core_pid: Some(record.pid),
            core_binary: Some(record.core_binary.clone()),
            api_endpoint: record.api_endpoint.clone(),
            config_path: Some(record.config_path.clone()),
            launched_config_revision: record.config_revision.clone(),
            pid_file: pid_file.to_path_buf(),
            core_log_file: pid_file.with_file_name("core.log"),
            ..DaemonState::default()
        }
    }

    #[tokio::test]
    async fn current_instance_ownership_accepts_a_live_child_with_matching_record() {
        let temp = tempfile::tempdir().unwrap();
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn child stand-in");
        let pid = child.id().expect("child pid");
        let pid_file = temp.path().join("core.pid");
        let record = managed_launch_record(pid);
        write_pid_file(&pid_file, &record);
        let mut state = state_with_launch_record(&pid_file, &record);
        state.core_child = Some(child);
        let state = Arc::new(Mutex::new(state));

        assert_eq!(
            prove_current_instance_ownership(&state).await.unwrap(),
            CurrentInstanceRuntime::LiveChild(pid)
        );

        // A child whose pid no longer matches the recorded instance is refused.
        {
            let mut guard = state.lock().await;
            guard.core_pid = Some(pid + 1);
        }
        assert!(prove_current_instance_ownership(&state).await.is_err());

        let mut guard = state.lock().await;
        if let Some(mut child) = guard.core_child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }

    #[tokio::test]
    async fn current_instance_ownership_refuses_a_missing_or_mismatched_record() {
        let temp = tempfile::tempdir().unwrap();
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn child stand-in");
        let pid = child.id().expect("child pid");
        let pid_file = temp.path().join("core.pid");
        let record = managed_launch_record(pid);

        // No launch record at all.
        let mut state = state_with_launch_record(&pid_file, &record);
        state.core_child = Some(child);
        let state = Arc::new(Mutex::new(state));
        assert!(prove_current_instance_ownership(&state).await.is_err());

        // Record pid does not match the live child.
        let foreign = managed_launch_record(pid + 1);
        write_pid_file(&pid_file, &foreign);
        {
            let mut guard = state.lock().await;
            guard.core_pid = Some(pid);
        }
        assert!(prove_current_instance_ownership(&state).await.is_err());

        // Untrusted binary / endpoint in the record is refused.
        let mut untrusted = managed_launch_record(pid);
        untrusted.core_binary = PathBuf::from("/tmp/foreign-core");
        write_pid_file(&pid_file, &untrusted);
        assert!(prove_current_instance_ownership(&state).await.is_err());

        let mut untrusted_endpoint = managed_launch_record(pid);
        untrusted_endpoint.api_endpoint = Some("unix:///tmp/foreign.sock".to_string());
        write_pid_file(&pid_file, &untrusted_endpoint);
        assert!(prove_current_instance_ownership(&state).await.is_err());

        let mut guard = state.lock().await;
        if let Some(mut child) = guard.core_child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }

    #[tokio::test]
    async fn current_instance_ownership_without_a_child_needs_a_proven_peer() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("core.pid");
        let record = managed_launch_record(4242);
        write_pid_file(&pid_file, &record);
        let state = Arc::new(Mutex::new(state_with_launch_record(&pid_file, &record)));

        // Nothing answers the recorded endpoint: the recorded runtime is stale and
        // must not be treated as a proven peer.
        let error = prove_current_instance_ownership(&state).await.unwrap_err();
        assert!(
            error.contains("no process answers") || error.contains("cannot prove"),
            "{error}"
        );

        // Without any recorded pid the daemon cannot name the instance.
        {
            let mut guard = state.lock().await;
            guard.core_pid = None;
        }
        assert!(prove_current_instance_ownership(&state).await.is_err());
    }

    #[test]
    fn legacy_reset_gate_is_independent_of_the_admission_flag() {
        // The new flag never widens the SPEC §12.4 matrix: a quiesce with the
        // legacy admission requested is still gated outside its own phases.
        let gate_journal = |phase| crate::tun_transaction::TunJournal {
            schema_version: 1,
            transaction_id: "txn-gate".to_string(),
            generation: 5,
            phase,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "cand".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        let gate_fence = |phase| crate::tun_transaction::TransactionFence {
            transaction_id: "txn-gate".to_string(),
            generation: 5,
            expected_phase: phase,
            expected_candidate_revision: "cand".to_string(),
        };
        assert!(!gate_admits(
            &DaemonCommand::QuiesceCandidateRuntime {
                fence: gate_fence(crate::tun_transaction::JournalPhase::Prepared),
                allow_legacy_reset: true,
                allow_restored_old_quiesce: false,
                token: None,
            },
            Some(&gate_journal(
                crate::tun_transaction::JournalPhase::Prepared
            ))
        ));
        assert!(gate_admits(
            &DaemonCommand::QuiesceCandidateRuntime {
                fence: gate_fence(crate::tun_transaction::JournalPhase::RecoveryRequired),
                allow_legacy_reset: true,
                allow_restored_old_quiesce: false,
                token: None,
            },
            Some(&gate_journal(
                crate::tun_transaction::JournalPhase::RecoveryRequired
            ))
        ));
    }

    #[test]
    fn autostart_is_blocked_by_any_non_terminal_transaction() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = daemon_transaction_context().expect("system instance context");
        ctx.paths.config_dir = temp.path().join("config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = temp.path().join("tun-config.yaml");
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::create_dir_all(crate::tun_transaction::active_dir(&ctx)).unwrap();

        let journal = crate::tun_transaction::TunJournal {
            schema_version: 1,
            transaction_id: "autostart-gate".to_string(),
            generation: 3,
            phase: crate::tun_transaction::JournalPhase::RecoveryRequired,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "cand".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: true,
            legacy_transaction_id: Some("legacy".to_string()),
            legacy_base_revision: None,
            rollback_evidence_complete: false,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        crate::tun_transaction::write_active_journal(&ctx, &journal).unwrap();
        assert!(autostart_transaction_blocker_for(&ctx).is_some());

        let terminal = crate::tun_transaction::TunJournal {
            phase: crate::tun_transaction::JournalPhase::IntentCommitted,
            ..journal.clone()
        };
        crate::tun_transaction::write_active_journal(&ctx, &terminal).unwrap();
        assert!(autostart_transaction_blocker_for(&ctx).is_none());

        // An un-migrated legacy journal also owns the runtime.
        std::fs::remove_file(crate::tun_transaction::active_journal_path(&ctx)).unwrap();
        std::fs::write(crate::tun_transaction::legacy_journal_path(&ctx), b"{}").unwrap();
        assert!(autostart_transaction_blocker_for(&ctx).is_some());
    }
}
