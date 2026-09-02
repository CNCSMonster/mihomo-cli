//! IPC protocol for CLI ↔ System Service communication.
//!
//! Transport: Unix domain socket (Linux/macOS) or Named pipe (Windows).
//! Format: Length-prefixed JSON messages.
//!
//! The system service daemon runs as root and handles privileged operations
//! on behalf of the unprivileged CLI.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
        /// Per-user config dir holding selection-state.yaml (SPEC-select-persistence).
        /// The daemon replays pinned selections against the freshly started Core.
        #[serde(default)]
        selection_intent_dir: Option<String>,
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
        /// Per-user config dir holding selection-state.yaml (SPEC-select-persistence).
        #[serde(default)]
        selection_intent_dir: Option<String>,
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
        /// Per-user config dir holding selection-state.yaml (SPEC-select-persistence).
        #[serde(default)]
        selection_intent_dir: Option<String>,
        /// Active subscription identity for selection replay.
        #[serde(default)]
        subscription_id: Option<String>,
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

/// IPC socket path for the system service.
pub fn system_service_socket_path() -> PathBuf {
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

/// Send a command to the daemon and wait for a response.
#[cfg(windows)]
pub async fn send_command(cmd: &DaemonCommand) -> anyhow::Result<DaemonResponse> {
    use tokio::io::BufReader;
    use tokio::net::windows::named_pipe::ClientOptions;

    let token = current_client_token();
    let cmd = with_ipc_token(cmd.clone(), token);

    let pipe_path = system_service_socket_path();
    let mut pipe = ClientOptions::new().open(&pipe_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to connect to system service at {}\n  \
             Is the system service installed and running?\n  \
             Install: mihomo-cli install --system\n  \
             Error: {e}",
            pipe_path.display()
        )
    })?;

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
            selection_intent_dir,
            subscription_id,
            ..
        } => StartCore {
            config_content,
            config_revision,
            selection_intent_dir,
            subscription_id,
            token,
        },
        StopCore { .. } => StopCore { token },
        RestartCore {
            config_content,
            config_revision,
            selection_intent_dir,
            subscription_id,
            ..
        } => RestartCore {
            config_content,
            config_revision,
            selection_intent_dir,
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
            selection_intent_dir,
            subscription_id,
            ..
        } => PromoteSystemConfig {
            config_content,
            config_revision,
            selection_intent_dir,
            subscription_id,
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
        QuiesceCandidateRuntime { fence, .. } => QuiesceCandidateRuntime { fence, token },
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
            selection_intent_dir: None,
            subscription_id: None,
            token: None,
        };
        let promote_json = serde_json::to_string(&promote).unwrap();
        assert!(promote_json.contains("PromoteSystemConfig"));
        assert!(!promote_json.contains("config_path"));
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
    fn lifecycle_commands_carry_selection_intent_dir() {
        // SPEC-select-persistence: lifecycle commands tell the daemon where
        // the per-user selection-state.yaml lives so it can replay after
        // Core (re)starts. Older daemons tolerate the field being absent.
        for cmd in [
            DaemonCommand::StartCore {
                config_content: "mode: rule\n".to_string(),
                config_revision: "rev".to_string(),
                selection_intent_dir: Some("/home/alice/.config/mihomo".to_string()),
                subscription_id: Some("sub-abcdef12".to_string()),
                token: None,
            },
            DaemonCommand::RestartCore {
                config_content: "mode: rule\n".to_string(),
                config_revision: "rev".to_string(),
                selection_intent_dir: Some("/home/alice/.config/mihomo".to_string()),
                subscription_id: Some("sub-abcdef12".to_string()),
                token: None,
            },
            DaemonCommand::PromoteSystemConfig {
                config_content: "mode: rule\n".to_string(),
                config_revision: "rev".to_string(),
                selection_intent_dir: Some("/home/alice/.config/mihomo".to_string()),
                subscription_id: Some("sub-abcdef12".to_string()),
                token: None,
            },
        ] {
            let json = serde_json::to_string(&cmd).unwrap();
            let parsed = serde_json::from_str::<DaemonCommand>(&json).unwrap();
            let intent_dir = match &parsed {
                DaemonCommand::StartCore {
                    selection_intent_dir,
                    ..
                }
                | DaemonCommand::RestartCore {
                    selection_intent_dir,
                    ..
                }
                | DaemonCommand::PromoteSystemConfig {
                    selection_intent_dir,
                    ..
                } => selection_intent_dir.as_deref(),
                other => panic!("unexpected command variant: {other:?}"),
            };
            assert_eq!(intent_dir, Some("/home/alice/.config/mihomo"));
        }

        // Missing field (legacy CLI) must still deserialize via serde default.
        let legacy =
            r#"{"type":"StartCore","config_content":"mode: rule\n","config_revision":"rev"}"#;
        assert!(matches!(
            serde_json::from_str::<DaemonCommand>(legacy).unwrap(),
            DaemonCommand::StartCore {
                selection_intent_dir: None,
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
}
