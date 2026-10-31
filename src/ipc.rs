//! IPC protocol for CLI ↔ System Service communication.
//!
//! Transport: Unix domain socket (Linux/macOS) or Named pipe (Windows).
//! Format: Length-prefixed JSON messages.
//!
//! The system service daemon runs as root and handles privileged operations
//! on behalf of the unprivileged CLI.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// review MAJOR-1：TUN 护栏 / attestation Error 的共享标记前缀。
/// daemon 侧所有宣告"运行态 TUN 已被拆除或无法观测"的 Error 文案必须以它起始，
/// CLI 侧 `is_tun_guard_error` 只认这个常量：判据锚定常量而非自由文案，
/// 文案措辞改动不会让护栏 Error 静默折叠回 Pending。
pub(crate) const TUN_GUARD_ERROR_MARKER: &str = "tun-guard: ";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CoreApiMethod {
    Get,
    Put,
    Patch,
    Delete,
}

/// Commands sent from CLI → Daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum DaemonCommand {
    /// Start the system mihomo core with CLI-validated configuration content.
    StartCore {
        config_content: String,
        config_revision: String,
        /// Active subscription identity for selection replay.
        #[serde(default)]
        subscription_id: Option<String>,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Stop the mihomo core.
    StopCore {
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Restart the system mihomo core with CLI-validated configuration content.
    RestartCore {
        config_content: String,
        config_revision: String,
        /// Active subscription identity for selection replay.
        #[serde(default)]
        subscription_id: Option<String>,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Apply the daemon-owned system TUN snapshot for the expected revision.
    ApplySystemTunSnapshot {
        expected_revision: String,
        stack: Option<String>,
        dns_hijack: Option<String>,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Promote a fully validated system effective configuration and attest it.
    PromoteSystemConfig {
        config_content: String,
        config_revision: String,
        /// Active subscription identity for selection replay.
        #[serde(default)]
        subscription_id: Option<String>,
        /// Optional explicit change kind (R2); when None, daemon computes it from content diff.
        #[serde(default)]
        change_kind: Option<crate::config::ChangeKind>,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Record a selection-intent mirror copy in the fixed system runtime so the
    /// daemon can replay selections after restart without reading the user tree
    /// (SPEC §3.8.1 mirror contract; daemon is the only runtime writer).
    RecordSelectionIntent {
        subscription_id: String,
        content_yaml: String,
        /// SPEC §3.8.3: the request-side config revision this mirror is bound to.
        /// The daemon records it in the mirror binding sidecar so a later startup
        /// can prove the mirror belongs to the config it actually loaded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_revision: Option<String>,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Record the active subscription identity mirror in the fixed system runtime.
    RecordActiveSubscription {
        subscription_id: String,
        /// SPEC §3.8.3: request-side config revision (see `RecordSelectionIntent`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_revision: Option<String>,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Select a proxy group member through the daemon-owned runtime API.
    SelectSystemProxy {
        group: String,
        node: String,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Disable TUN mode.
    DisableTun {
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Query daemon status.
    GetStatus {
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Forward an authenticated, allowlisted request to the system Core API.
    CoreApiRequest {
        method: CoreApiMethod,
        path: String,
        #[serde(default)]
        body: Option<serde_json::Value>,
        #[serde(default)]
        token: Option<String>,
    },
    /// Enable/disable core autostart (ADR-19: daemon owns the marker so the
    /// root/sudo identity never skews the per-user config dir).
    SetAutostart {
        enabled: bool,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },

    // Transaction IPC commands (SPEC §12.2)
    ValidatePreparedRuntime {
        fence: crate::tun_transaction::TransactionFence,
        expected_old_runtime_revision: String,
        expected_old_runtime_tun: bool,
        #[serde(default)]
        token: Option<String>,
    },
    ApplyPromotedSnapshot {
        fence: crate::tun_transaction::TransactionFence,
        target_runtime_tun: bool,
        #[serde(default)]
        token: Option<String>,
    },
    QuiesceCandidateRuntime {
        fence: crate::tun_transaction::TransactionFence,
        /// set only by the authorized controlled-reset stop
        /// (`restart --system` after the runtime-reset confirmation). It unlocks
        /// the migrated-legacy-placeholder admission and nothing else; omitted on
        /// the wire when false so non-reset callers keep the original request.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_legacy_reset: bool,
        /// explicit `restart --system --yes` authorization to stop a
        /// managed Core that still runs the old revision/TUN with a new PID. Kept
        /// separate from the legacy-placeholder admission; omitted when false so
        /// older daemons see the original request.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        allow_restored_old_quiesce: bool,
        #[serde(default)]
        token: Option<String>,
    },
    RestoreOldRuntime {
        fence: crate::tun_transaction::TransactionFence,
        expected_old_runtime_revision: String,
        expected_old_runtime_tun: bool,
        #[serde(default)]
        token: Option<String>,
    },
    AttestCurrentTransaction {
        fence: crate::tun_transaction::TransactionFence,
        expected_runtime_revision: String,
        expected_runtime_tun: bool,
        #[serde(default)]
        token: Option<String>,
    },
    ApplyLegacyRecoveryTarget {
        fence: crate::tun_transaction::TransactionFence,
        expected_recovery_target_revision: String,
        #[serde(default)]
        token: Option<String>,
    },
    GetTransactionStatus {
        #[serde(default)]
        token: Option<String>,
    },
    /// Query content revisions of the daemon-owned selection-intent mirrors
    /// for external reconciliation (R3.2 mirror contract). The daemon
    /// rejects any subscription id it would not accept for recording;
    /// a `None` revision in the response means the mirror file is absent.
    GetSelectionMirrorRevisions {
        subscription_ids: Vec<String>,
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
    /// Read-only query of the daemon-cached selection drift observations
    /// (B2). Never triggers a core request, a fix, or a path
    /// access; authorization is fully inherited from the existing read-only
    /// command contract (peer UID + owner record + token).
    GetSelectionDrift {
        /// Windows-only auth token (None on unix — peer uid validation).
        #[serde(default)]
        token: Option<String>,
    },
}

/// Responses sent from Daemon → CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonResponse {
    /// Operation completed successfully.
    Success { message: String },
    /// Operation failed.
    Error { message: String },
    /// Status information.
    Status {
        running: bool,
        core_pid: Option<u32>,
        config_path: Option<PathBuf>,
        /// Revision of the daemon-managed system TUN snapshot, if readable.
        #[serde(default)]
        tun_snapshot_revision: Option<String>,
        /// Revision of the configuration loaded when Core was launched.
        #[serde(default)]
        launched_config_revision: Option<String>,
        /// Whether core autostart is enabled (ADR-19, daemon-owned marker).
        #[serde(default)]
        autostart_enabled: bool,
        /// SHA-256 of the executable backing the responding daemon process.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        daemon_executable_revision: Option<String>,
        /// Durable TUN transaction state observed by the daemon.
        #[serde(default)]
        tun_journal_state: Option<crate::tun_transaction::JournalPhase>,
        /// Diagnostic for an unreadable/unsupported active journal.
        /// Optional so older daemon responses remain decodable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tun_journal_error: Option<String>,
    },
    /// Successful response from an allowlisted Core API request.
    CoreApi { data: serde_json::Value },
    /// Transaction structured response.
    Transaction {
        response: crate::tun_transaction::TransactionResponse,
    },
    /// Selection mirror revisions keyed by subscription id;
    /// `None` value means the mirror file is absent.
    SelectionMirrorRevisions {
        revisions: BTreeMap<String, Option<String>>,
    },
    /// Cached selection drift observation snapshot (B2).
    SelectionDrift {
        observations: Vec<SelectionDriftObservation>,
        /// Samples discarded because the daemon was mutating/suppressed/
        /// inconsistent. No finite visibility bound is promised for these.
        unknown_samples: u64,
        /// Health of the sampler: `ok` (last sample healthy),
        /// `unknown`, or `suppressed` (replay suppression window).
        sample_status: String,
        /// Set when the active TUN journal is in-flight/corrupt: the cache is
        /// still returned, but the CLI must annotate the fault state.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        journal: Option<String>,
    },
}

/// One drift observation: runtime `now` differs from the daemon-owned
/// mirror intent for the group (B1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionDriftObservation {
    pub group: String,
    /// Intent node recorded in the fixed-runtime selection mirror.
    pub intent: String,
    /// Runtime `now` reported by the Core at sample time.
    pub runtime: String,
    /// Unix timestamp (seconds) of the observation.
    pub at_unix: u64,
    /// Subscription the mirror intent belongs to (Wave 1 §3.8.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
    /// SHA-256 revision of the mirror file at sample time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_revision: Option<String>,
}

pub(crate) fn managed_snapshot_revision(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(crate::tun_transaction::sha256_revision(&bytes))
}

fn read_client_token(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientTokenLocation {
    pub(crate) token_path: PathBuf,
}

pub(crate) fn client_token_location_from_inputs(
    os: crate::instance::TargetOs,
    inputs: &crate::instance::PathInputs,
) -> ClientTokenLocation {
    ClientTokenLocation {
        token_path: crate::instance::planned_daemon_credential_paths(os, inputs).token,
    }
}

pub(crate) fn client_token_location() -> ClientTokenLocation {
    let inputs = crate::instance::PathInputs::from_current_env();
    let os = crate::instance::TargetOs::current().unwrap_or(crate::instance::TargetOs::Linux);
    client_token_location_from_inputs(os, &inputs)
}

pub(crate) fn current_client_token() -> Option<String> {
    read_client_token(&client_token_location().token_path)
}

/// Test-only override so an isolated child process can drive the real pipeline
/// against a fake daemon socket instead of the machine-wide one. Compiled only
/// under `cfg(test)`; the production binary never reads any env for this path.
#[cfg(test)]
pub(crate) const TEST_DAEMON_SOCKET_ENV: &str = "MIHOMO_TEST_DAEMON_SOCKET";

/// IPC socket path for the system service.
pub fn system_service_socket_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = std::env::var_os(TEST_DAEMON_SOCKET_ENV) {
        return PathBuf::from(path);
    }
    if cfg!(unix) {
        PathBuf::from("/var/run/mihomo/service.sock")
    } else {
        // Windows uses named pipes, not Unix sockets
        PathBuf::from(r"\\.\pipe\mihomo-service")
    }
}

const MAX_IPC_MESSAGE_BYTES: usize = 10 * 1024 * 1024;

pub(crate) async fn write_json_message<W, T>(writer: &mut W, value: &T) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
    T: Serialize,
{
    use tokio::io::AsyncWriteExt;

    let json = serde_json::to_string(value)?;
    let len = json.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

pub(crate) async fn read_json_payload<R>(
    reader: &mut R,
    too_large_label: &str,
) -> anyhow::Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;

    if len > MAX_IPC_MESSAGE_BYTES {
        anyhow::bail!("{too_large_label} too large: {len} bytes");
    }

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

pub(crate) async fn read_json_message<R, T>(
    reader: &mut R,
    too_large_label: &str,
) -> anyhow::Result<T>
where
    R: tokio::io::AsyncRead + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let buf = read_json_payload(reader, too_large_label).await?;
    Ok(serde_json::from_slice(&buf)?)
}

/// Send a command to the daemon and wait for a response.
#[cfg(unix)]
pub async fn send_command(cmd: &DaemonCommand) -> anyhow::Result<DaemonResponse> {
    use tokio::io::BufReader;
    use tokio::net::UnixStream;

    let token = current_client_token();
    let cmd = with_ipc_token(cmd.clone(), token);

    let sock_path = system_service_socket_path();
    let mut stream = UnixStream::connect(&sock_path).await.map_err(|e| {
        anyhow::anyhow!(
            "failed to connect to system service at {}\n  \
             Is the system service installed and running?\n  \
             Install: mihomo-cli install --system\n  \
             Error: {e}",
            sock_path.display()
        )
    })?;

    write_json_message(&mut stream, &cmd).await?;

    let mut reader = BufReader::new(stream);
    let read = async { read_json_message(&mut reader, "daemon response").await };
    tokio::time::timeout(std::time::Duration::from_secs(20), read)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out waiting for the system service to respond after 20s.\n  \
                 The daemon may be busy with another lifecycle operation or stuck.\n  \
                 Inspect the system daemon service status or logs for this platform."
            )
        })?
}

/// Determine retry budget for Windows named pipe connect failures.
/// Transient ERROR_FILE_NOT_FOUND (2) is budgeted at 1500ms to allow brief startup / restart windows
/// while still failing fast for non-installed services.
/// ERROR_PIPE_BUSY (231) and ERROR_BROKEN_PIPE (109) get up to 3000ms.
/// Fatal errors (like ERROR_ACCESS_DENIED 5 or unknown errors) fail immediately (None).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn windows_ipc_retry_budget(raw_code: Option<i32>) -> Option<std::time::Duration> {
    const ERROR_FILE_NOT_FOUND: i32 = 2;
    const ERROR_BROKEN_PIPE: i32 = 109;
    const ERROR_PIPE_BUSY: i32 = 231;

    match raw_code {
        Some(ERROR_FILE_NOT_FOUND) => Some(std::time::Duration::from_millis(1500)),
        Some(ERROR_PIPE_BUSY) | Some(ERROR_BROKEN_PIPE) => {
            Some(std::time::Duration::from_millis(3000))
        }
        _ => None,
    }
}

/// Send a command to the daemon and wait for a response.
#[cfg(windows)]
pub async fn send_command(cmd: &DaemonCommand) -> anyhow::Result<DaemonResponse> {
    use tokio::io::BufReader;
    use tokio::net::windows::named_pipe::ClientOptions;

    let token = current_client_token();
    let cmd = with_ipc_token(cmd.clone(), token);

    let pipe_path = system_service_socket_path();
    let start_time = std::time::Instant::now();
    let mut pipe = loop {
        match ClientOptions::new().open(&pipe_path) {
            Ok(p) => break p,
            Err(e) => {
                let raw = e.raw_os_error();
                let elapsed = start_time.elapsed();
                if let Some(budget) = windows_ipc_retry_budget(raw) {
                    if elapsed < budget {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        continue;
                    }
                }
                if raw == Some(231) || raw == Some(109) {
                    anyhow::bail!(
                        "failed to connect to system service at {} (service is busy or resetting after multiple retries)\n  \
                         The service may be handling another request or restarting.\n  \
                         Check status: mihomo-cli status\n  \
                         Error: {e}",
                        pipe_path.display()
                    );
                }
                anyhow::bail!(
                    "failed to connect to system service at {}\n  \
                     Is the system service installed and running?\n  \
                     Install: mihomo-cli install --system\n  \
                     Error: {e}",
                    pipe_path.display()
                );
            }
        }
    };

    write_json_message(&mut pipe, &cmd).await?;
    let mut reader = BufReader::new(pipe);
    let read = async { read_json_message(&mut reader, "daemon response").await };
    tokio::time::timeout(std::time::Duration::from_secs(20), read)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "timed out waiting for the system service to respond after 20s.\n  \
                 The daemon may be busy with another lifecycle operation or stuck.\n  \
                 Check the service status."
            )
        })?
}

/// Inject the auth token into a cloned command (all variants carry `token`).
#[cfg(any(unix, windows))]
fn with_ipc_token(cmd: DaemonCommand, token: Option<String>) -> DaemonCommand {
    use DaemonCommand::*;
    match cmd {
        StartCore {
            config_content,
            config_revision,
            subscription_id,
            ..
        } => StartCore {
            config_content,
            config_revision,
            subscription_id,
            token,
        },
        StopCore { .. } => StopCore { token },
        RestartCore {
            config_content,
            config_revision,
            subscription_id,
            ..
        } => RestartCore {
            config_content,
            config_revision,
            subscription_id,
            token,
        },
        ApplySystemTunSnapshot {
            expected_revision,
            stack,
            dns_hijack,
            ..
        } => ApplySystemTunSnapshot {
            expected_revision,
            stack,
            dns_hijack,
            token,
        },
        PromoteSystemConfig {
            config_content,
            config_revision,
            subscription_id,
            change_kind,
            ..
        } => PromoteSystemConfig {
            config_content,
            config_revision,
            subscription_id,
            change_kind,
            token,
        },
        RecordSelectionIntent {
            subscription_id,
            content_yaml,
            config_revision,
            ..
        } => RecordSelectionIntent {
            subscription_id,
            content_yaml,
            config_revision,
            token,
        },
        RecordActiveSubscription {
            subscription_id,
            config_revision,
            ..
        } => RecordActiveSubscription {
            subscription_id,
            config_revision,
            token,
        },
        SelectSystemProxy { group, node, .. } => SelectSystemProxy { group, node, token },
        DisableTun { .. } => DisableTun { token },
        GetStatus { .. } => GetStatus { token },
        CoreApiRequest {
            method, path, body, ..
        } => CoreApiRequest {
            method,
            path,
            body,
            token,
        },
        SetAutostart { enabled, .. } => SetAutostart { enabled, token },
        ValidatePreparedRuntime {
            fence,
            expected_old_runtime_revision,
            expected_old_runtime_tun,
            ..
        } => ValidatePreparedRuntime {
            fence,
            expected_old_runtime_revision,
            expected_old_runtime_tun,
            token,
        },
        ApplyPromotedSnapshot {
            fence,
            target_runtime_tun,
            ..
        } => ApplyPromotedSnapshot {
            fence,
            target_runtime_tun,
            token,
        },
        QuiesceCandidateRuntime {
            fence,
            allow_legacy_reset,
            allow_restored_old_quiesce,
            ..
        } => QuiesceCandidateRuntime {
            fence,
            allow_legacy_reset,
            allow_restored_old_quiesce,
            token,
        },
        RestoreOldRuntime {
            fence,
            expected_old_runtime_revision,
            expected_old_runtime_tun,
            ..
        } => RestoreOldRuntime {
            fence,
            expected_old_runtime_revision,
            expected_old_runtime_tun,
            token,
        },
        AttestCurrentTransaction {
            fence,
            expected_runtime_revision,
            expected_runtime_tun,
            ..
        } => AttestCurrentTransaction {
            fence,
            expected_runtime_revision,
            expected_runtime_tun,
            token,
        },
        ApplyLegacyRecoveryTarget {
            fence,
            expected_recovery_target_revision,
            ..
        } => ApplyLegacyRecoveryTarget {
            fence,
            expected_recovery_target_revision,
            token,
        },
        GetTransactionStatus { .. } => GetTransactionStatus { token },
        GetSelectionMirrorRevisions {
            subscription_ids, ..
        } => GetSelectionMirrorRevisions {
            subscription_ids,
            token,
        },
        GetSelectionDrift { .. } => GetSelectionDrift { token },
    }
}

/// Send a command to the daemon and wait for a response.
#[cfg(not(any(unix, windows)))]
pub async fn send_command(_cmd: &DaemonCommand) -> anyhow::Result<DaemonResponse> {
    anyhow::bail!("system service IPC is not implemented on this platform")
}

/// Check if the daemon socket/pipe is available (daemon is running).
pub fn is_daemon_running_blocking() -> bool {
    is_daemon_running_blocking_at(system_service_socket_path())
}

#[cfg(unix)]
fn is_daemon_running_blocking_at(path: PathBuf) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(windows)]
fn is_daemon_running_blocking_at(path: PathBuf) -> bool {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(path)
        .is_ok()
}

#[cfg(not(any(unix, windows)))]
fn is_daemon_running_blocking_at(_path: PathBuf) -> bool {
    false
}

/// Check if the daemon socket/pipe is available (daemon is running).
pub async fn is_daemon_running() -> bool {
    is_daemon_running_blocking()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_command_serialization() {
        let cmd = DaemonCommand::GetStatus { token: None };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"GetStatus\""));

        let deserialized: DaemonCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            DaemonCommand::GetStatus { token: None }
        ));
    }

    #[test]
    fn core_api_command_roundtrip_keeps_request_shape() {
        let command = DaemonCommand::CoreApiRequest {
            method: CoreApiMethod::Get,
            path: "/configs".to_string(),
            body: None,
            token: None,
        };
        let json = serde_json::to_string(&command).unwrap();
        let decoded: DaemonCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            DaemonCommand::CoreApiRequest {
                method: CoreApiMethod::Get,
                ref path,
                body: None,
                token: None,
            } if path == "/configs"
        ));
    }

    #[test]
    fn promotion_commands_roundtrip_without_client_paths() {
        let promote = DaemonCommand::PromoteSystemConfig {
            config_content: "mode: rule\n".to_string(),
            config_revision: "0123456789abcdef".to_string(),
            subscription_id: None,
            change_kind: None,
            token: None,
        };
        let promote_json = serde_json::to_string(&promote).unwrap();
        assert!(promote_json.contains("PromoteSystemConfig"));
        assert!(!promote_json.contains("config_path"));
        assert!(!promote_json.contains("selection_intent_dir"));
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(&promote_json).unwrap(),
            DaemonCommand::PromoteSystemConfig { config_revision, .. }
                if config_revision == "0123456789abcdef"
        ));

        let select = DaemonCommand::SelectSystemProxy {
            group: "Proxy".to_string(),
            node: "NodeA".to_string(),
            token: None,
        };
        let select_json = serde_json::to_string(&select).unwrap();
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(&select_json).unwrap(),
            DaemonCommand::SelectSystemProxy { group, node, .. }
                if group == "Proxy" && node == "NodeA"
        ));
    }

    #[test]
    fn selection_mirror_commands_roundtrip_with_subscription_identity() {
        // SPEC §3.8.1 mirror contract: the CLI pushes selection content and
        // the active identity; the daemon resolves replay scopes from the
        // fixed-runtime mirror and never receives user-tree paths.
        for cmd in [
            DaemonCommand::RecordSelectionIntent {
                subscription_id: "sub-abcdef12".to_string(),
                content_yaml: "selections:\n  G: N\n".to_string(),
                config_revision: Some("rev-1".to_string()),
                token: None,
            },
            DaemonCommand::RecordActiveSubscription {
                subscription_id: "sub-abcdef12".to_string(),
                config_revision: Some("rev-1".to_string()),
                token: None,
            },
        ] {
            let json = serde_json::to_string(&cmd).unwrap();
            assert!(!json.contains("selection_intent_dir"));
            let parsed = serde_json::from_str::<DaemonCommand>(&json).unwrap();
            let id = match &parsed {
                DaemonCommand::RecordSelectionIntent {
                    subscription_id, ..
                }
                | DaemonCommand::RecordActiveSubscription {
                    subscription_id, ..
                } => subscription_id.as_str(),
                other => panic!("unexpected command variant: {other:?}"),
            };
            assert_eq!(id, "sub-abcdef12");
        }
    }

    #[test]
    fn selection_mirror_revision_query_roundtrips() {
        let cmd = DaemonCommand::GetSelectionMirrorRevisions {
            subscription_ids: vec!["sub-abcdef12".to_string(), "sub-0000ff00".to_string()],
            token: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"GetSelectionMirrorRevisions\""));
        // Client must never transmit paths, only subscription identities.
        assert!(!json.contains("selection_intent_dir"));
        assert!(!json.contains("/var/run"));
        let parsed = serde_json::from_str::<DaemonCommand>(&json).unwrap();
        match parsed {
            DaemonCommand::GetSelectionMirrorRevisions {
                subscription_ids,
                token: None,
            } => assert_eq!(
                subscription_ids,
                vec!["sub-abcdef12".to_string(), "sub-0000ff00".to_string()]
            ),
            other => panic!("unexpected command variant: {other:?}"),
        }

        let mut revisions = BTreeMap::new();
        revisions.insert("sub-abcdef12".to_string(), Some("aa".repeat(32)));
        revisions.insert("sub-0000ff00".to_string(), None);
        let resp = DaemonResponse::SelectionMirrorRevisions {
            revisions: revisions.clone(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        match serde_json::from_str::<DaemonResponse>(&json).unwrap() {
            DaemonResponse::SelectionMirrorRevisions { revisions: got } => {
                assert_eq!(got, revisions)
            }
            other => panic!("unexpected response variant: {other:?}"),
        }
    }

    #[test]
    fn selection_drift_query_roundtrips_and_injects_token() {
        // B2: read-only cache query; no path or fix payload.
        let cmd = DaemonCommand::GetSelectionDrift { token: None };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"GetSelectionDrift\""));
        assert!(!json.contains("selection_intent_dir"));
        assert!(!json.contains("/var/lib"));
        let parsed = serde_json::from_str::<DaemonCommand>(&json).unwrap();
        assert!(matches!(
            parsed,
            DaemonCommand::GetSelectionDrift { token: None }
        ));

        // Token injection keeps authorization explicit (Windows path).
        let with_token = with_ipc_token(
            DaemonCommand::GetSelectionDrift { token: None },
            Some("t0".into()),
        );
        match with_token {
            DaemonCommand::GetSelectionDrift { token } => assert_eq!(token.as_deref(), Some("t0")),
            other => panic!("unexpected command variant: {other:?}"),
        }

        let resp = DaemonResponse::SelectionDrift {
            observations: vec![SelectionDriftObservation {
                group: "自动选择".to_string(),
                intent: "HK-01".to_string(),
                runtime: "SG-02".to_string(),
                at_unix: 1_700_000_000,
                subscription_id: Some("sub-abcdef12".to_string()),
                mirror_revision: Some("abc123".to_string()),
            }],
            unknown_samples: 3,
            sample_status: "ok".to_string(),
            journal: Some("in-flight (Prepared)".to_string()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        match serde_json::from_str::<DaemonResponse>(&json).unwrap() {
            DaemonResponse::SelectionDrift {
                observations,
                unknown_samples,
                sample_status,
                journal,
            } => {
                assert_eq!(observations.len(), 1);
                assert_eq!(observations[0].group, "自动选择");
                assert_eq!(observations[0].runtime, "SG-02");
                assert_eq!(unknown_samples, 3);
                assert_eq!(sample_status, "ok");
                assert_eq!(journal.as_deref(), Some("in-flight (Prepared)"));
            }
            other => panic!("unexpected response variant: {other:?}"),
        }

        // Healthy daemon: journal annotation is omitted entirely (back-compat).
        let resp = DaemonResponse::SelectionDrift {
            observations: Vec::new(),
            unknown_samples: 0,
            sample_status: "suppressed".to_string(),
            journal: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("journal"));
        match serde_json::from_str::<DaemonResponse>(&json).unwrap() {
            DaemonResponse::SelectionDrift { journal: None, .. } => {}
            other => panic!("unexpected response variant: {other:?}"),
        }
    }

    #[test]
    fn lifecycle_commands_carry_subscription_identity() {
        // SPEC §3.8.1: lifecycle commands carry the active subscription
        // identity so the daemon can replay the fixed-runtime mirror after
        // Core (re)starts. User-tree paths are never transmitted.
        for cmd in [
            DaemonCommand::StartCore {
                config_content: "mode: rule\n".to_string(),
                config_revision: "rev".to_string(),
                subscription_id: Some("sub-abcdef12".to_string()),
                token: None,
            },
            DaemonCommand::RestartCore {
                config_content: "mode: rule\n".to_string(),
                config_revision: "rev".to_string(),
                subscription_id: Some("sub-abcdef12".to_string()),
                token: None,
            },
            DaemonCommand::PromoteSystemConfig {
                config_content: "mode: rule\n".to_string(),
                config_revision: "rev".to_string(),
                subscription_id: Some("sub-abcdef12".to_string()),
                change_kind: None,
                token: None,
            },
        ] {
            let json = serde_json::to_string(&cmd).unwrap();
            let parsed = serde_json::from_str::<DaemonCommand>(&json).unwrap();
            let subscription_id = match &parsed {
                DaemonCommand::StartCore {
                    subscription_id, ..
                }
                | DaemonCommand::RestartCore {
                    subscription_id, ..
                }
                | DaemonCommand::PromoteSystemConfig {
                    subscription_id, ..
                } => subscription_id.as_deref(),
                other => panic!("unexpected command variant: {other:?}"),
            };
            assert_eq!(subscription_id, Some("sub-abcdef12"));
        }

        // Missing optional identity (legacy CLI) must still deserialize via
        // serde default.
        let legacy =
            r#"{"type":"StartCore","config_content":"mode: rule\n","config_revision":"rev"}"#;
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(legacy).unwrap(),
            DaemonCommand::StartCore {
                subscription_id: None,
                ..
            }
        ));
    }

    #[test]
    fn system_tun_apply_command_has_no_client_path() {
        let command = DaemonCommand::ApplySystemTunSnapshot {
            expected_revision: "0123456789abcdef".to_string(),
            stack: Some("system".to_string()),
            dns_hijack: Some("any:53".to_string()),
            token: None,
        };
        let json = serde_json::to_string(&command).unwrap();
        assert!(json.contains("ApplySystemTunSnapshot"));
        assert!(json.contains("expected_revision"));
        assert!(!json.contains("config_path"));

        let decoded: DaemonCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            decoded,
            DaemonCommand::ApplySystemTunSnapshot {
                expected_revision,
                stack: Some(stack),
                dns_hijack: Some(dns_hijack),
                token: None,
            } if expected_revision == "0123456789abcdef"
                && stack == "system"
                && dns_hijack == "any:53"
        ));
    }

    #[test]
    fn daemon_response_serialization() {
        let resp = DaemonResponse::Success {
            message: "ok".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"type\":\"Success\""));

        let resp = DaemonResponse::Status {
            running: true,
            core_pid: Some(1234),
            config_path: Some(PathBuf::from("/tmp/config.yaml")),
            tun_snapshot_revision: Some("snapshot-revision".to_string()),
            launched_config_revision: Some("launched-revision".to_string()),
            autostart_enabled: false,
            daemon_executable_revision: Some("daemon-revision".to_string()),
            tun_journal_state: Some(crate::tun_transaction::JournalPhase::IntentCommitted),
            tun_journal_error: Some("unsupported active TUN journal schema 99".to_string()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: DaemonResponse = serde_json::from_str(&json).unwrap();
        if let DaemonResponse::Status {
            running,
            core_pid,
            tun_snapshot_revision,
            launched_config_revision,
            tun_journal_error,
            ..
        } = deserialized
        {
            assert!(running);
            assert_eq!(core_pid, Some(1234));
            assert_eq!(tun_snapshot_revision.as_deref(), Some("snapshot-revision"));
            assert_eq!(
                launched_config_revision.as_deref(),
                Some("launched-revision")
            );
            assert_eq!(
                tun_journal_error.as_deref(),
                Some("unsupported active TUN journal schema 99")
            );
        } else {
            panic!("expected Status response");
        }
    }

    #[test]
    fn older_status_response_without_journal_error_remains_compatible() {
        let json = r#"{
            "type": "Status",
            "running": false,
            "core_pid": null,
            "config_path": null,
            "tun_snapshot_revision": null,
            "launched_config_revision": null,
            "autostart_enabled": false,
            "tun_journal_state": "RecoveryRequired"
        }"#;
        let response: DaemonResponse = serde_json::from_str(json).unwrap();
        match response {
            DaemonResponse::Status {
                tun_journal_state,
                tun_journal_error,
                daemon_executable_revision,
                ..
            } => {
                assert_eq!(
                    tun_journal_state,
                    Some(crate::tun_transaction::JournalPhase::RecoveryRequired)
                );
                assert_eq!(tun_journal_error, None);
                assert_eq!(daemon_executable_revision, None);
            }
            other => panic!("expected Status response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn length_prefixed_json_helpers_roundtrip() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let cmd = DaemonCommand::GetStatus { token: None };
        let writer = tokio::spawn(async move { write_json_message(&mut client, &cmd).await });
        let decoded: DaemonCommand = read_json_message(&mut server, "daemon command")
            .await
            .unwrap();
        writer.await.unwrap().unwrap();
        assert!(matches!(decoded, DaemonCommand::GetStatus { token: None }));
    }

    #[tokio::test]
    async fn length_prefixed_json_helpers_reject_oversized_message() {
        use tokio::io::AsyncWriteExt;

        let (mut client, mut server) = tokio::io::duplex(16);
        let writer = tokio::spawn(async move {
            let too_large = (MAX_IPC_MESSAGE_BYTES as u32) + 1;
            client.write_all(&too_large.to_le_bytes()).await.unwrap();
        });
        let result: anyhow::Result<DaemonCommand> =
            read_json_message(&mut server, "daemon command").await;
        writer.await.unwrap();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("daemon command too large"));
    }

    #[test]
    fn test_windows_ipc_retry_budget_policy() {
        use super::windows_ipc_retry_budget;
        assert_eq!(
            windows_ipc_retry_budget(Some(2)),
            Some(std::time::Duration::from_millis(1500)),
            "ERROR_FILE_NOT_FOUND should have 1500ms budget"
        );
        assert_eq!(
            windows_ipc_retry_budget(Some(231)),
            Some(std::time::Duration::from_millis(3000)),
            "ERROR_PIPE_BUSY should have 3000ms budget"
        );
        assert_eq!(
            windows_ipc_retry_budget(Some(109)),
            Some(std::time::Duration::from_millis(3000)),
            "ERROR_BROKEN_PIPE should have 3000ms budget"
        );
        assert_eq!(
            windows_ipc_retry_budget(Some(5)),
            None,
            "ERROR_ACCESS_DENIED must fail immediately"
        );
        assert_eq!(
            windows_ipc_retry_budget(Some(999)),
            None,
            "Unknown OS error must fail immediately"
        );
        assert_eq!(
            windows_ipc_retry_budget(None),
            None,
            "Missing raw error must fail immediately"
        );
    }
}
