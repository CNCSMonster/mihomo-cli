//! IPC protocol for CLI ↔ System Service communication.
//!
//! Transport: Unix domain socket (Linux/macOS) or Named pipe (Windows).
//! Format: Length-prefixed JSON messages.
//!
//! The system service daemon runs as root and handles privileged operations
//! on behalf of the unprivileged CLI.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Commands sent from CLI → Daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum DaemonCommand {
    /// Start the mihomo core with the given config.
    StartCore { config_path: PathBuf },
    /// Stop the mihomo core.
    StopCore,
    /// Restart the mihomo core.
    RestartCore { config_path: PathBuf },
    /// Enable TUN mode.
    EnableTun {
        config_path: PathBuf,
        stack: Option<String>,
        dns_hijack: Option<String>,
    },
    /// Disable TUN mode.
    DisableTun,
    /// Query daemon status.
    GetStatus,
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
        tun_enabled: bool,
        core_pid: Option<u32>,
        config_path: Option<PathBuf>,
    },
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

    write_json_message(&mut stream, cmd).await?;

    let mut reader = BufReader::new(stream);
    read_json_message(&mut reader, "daemon response").await
}

/// Send a command to the daemon and wait for a response.
#[cfg(windows)]
pub async fn send_command(cmd: &DaemonCommand) -> anyhow::Result<DaemonResponse> {
    use tokio::io::BufReader;
    use tokio::net::windows::named_pipe::ClientOptions;

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

    write_json_message(&mut pipe, cmd).await?;
    let mut reader = BufReader::new(pipe);
    read_json_message(&mut reader, "daemon response").await
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
        let cmd = DaemonCommand::GetStatus;
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"GetStatus\""));

        let deserialized: DaemonCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, DaemonCommand::GetStatus));
    }

    #[test]
    fn daemon_start_restart_commands_do_not_accept_client_supplied_core_binary() {
        let start = DaemonCommand::StartCore {
            config_path: PathBuf::from("/home/alice/.config/mihomo/config.yaml"),
        };
        let json = serde_json::to_string(&start).unwrap();
        assert!(json.contains("\"type\":\"StartCore\""));
        assert!(json.contains("config_path"));
        assert!(!json.contains("core_binary"));

        let restart = DaemonCommand::RestartCore {
            config_path: PathBuf::from("/home/alice/.config/mihomo/config.yaml"),
        };
        let json = serde_json::to_string(&restart).unwrap();
        assert!(json.contains("\"type\":\"RestartCore\""));
        assert!(json.contains("config_path"));
        assert!(!json.contains("core_binary"));
    }

    #[test]
    fn daemon_rejects_legacy_start_command_with_client_supplied_core_binary() {
        let legacy = r#"{
            "type": "StartCore",
            "config_path": "/home/alice/.config/mihomo/config.yaml",
            "core_binary": "/tmp/untrusted-mihomo"
        }"#;

        assert!(serde_json::from_str::<DaemonCommand>(legacy).is_err());
    }

    #[test]
    fn daemon_rejects_shutdown_command_not_in_v3_ipc_contract() {
        let shutdown = r#"{ "type": "Shutdown" }"#;
        assert!(serde_json::from_str::<DaemonCommand>(shutdown).is_err());
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
            tun_enabled: false,
            core_pid: Some(1234),
            config_path: Some(PathBuf::from("/tmp/config.yaml")),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: DaemonResponse = serde_json::from_str(&json).unwrap();
        if let DaemonResponse::Status {
            running,
            tun_enabled,
            core_pid,
            ..
        } = deserialized
        {
            assert!(running);
            assert!(!tun_enabled);
            assert_eq!(core_pid, Some(1234));
        } else {
            panic!("expected Status response");
        }
    }

    #[tokio::test]
    async fn length_prefixed_json_helpers_roundtrip() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let cmd = DaemonCommand::GetStatus;
        let writer = tokio::spawn(async move { write_json_message(&mut client, &cmd).await });
        let decoded: DaemonCommand = read_json_message(&mut server, "daemon command")
            .await
            .unwrap();
        writer.await.unwrap().unwrap();
        assert!(matches!(decoded, DaemonCommand::GetStatus));
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
    fn start_core_command_roundtrip() {
        let cmd = DaemonCommand::StartCore {
            config_path: PathBuf::from("/home/user/.config/mihomo/config.yaml"),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deserialized: DaemonCommand = serde_json::from_str(&json).unwrap();
        if let DaemonCommand::StartCore { config_path } = deserialized {
            assert_eq!(
                config_path,
                PathBuf::from("/home/user/.config/mihomo/config.yaml")
            );
        } else {
            panic!("expected StartCore command");
        }
    }
}
