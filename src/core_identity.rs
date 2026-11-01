//! SPEC 裁决 D：受管 Core 的归属证明与退出观察。
//!
//! 归属证明 = 四证齐全（固定 socket 对端 peer UID、`/proc/<pid>/exe` 真实可执行、
//! 精确受管 `-d`/`-f` 参数、受管 state 目录）——仅此四证可授权终止；进程枚举只
//! 做只读发现。归属证明与「目标 core 已启动」是两件事，本模块不做启动因果判断。
//!
//! 停机证明 = **可靠的进程退出证明**（内核句柄 pidfd 观察）。SIGTERM 发送成功
//! 不是证明；不可观察/权限不足/身份冲突一律 `Unprovable`，永不折算成 `Stopped`。

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

/// 已证明归属的受管 Core 进程身份（供日志/证据使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCoreIdentity {
    pub pid: u32,
    pub exe: PathBuf,
    pub cmdline: Vec<String>,
}

/// Core API 端点对端的归属结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreIdentityProof {
    /// 四证齐全：可授权终止（终止仍需显式入口 + 退出证明）。
    Proven(ManagedCoreIdentity),
    /// 没有任何进程在该端点响应（socket 不存在/已拒绝连接）。
    Absent,
    /// 无法证明（权限、/proc 不可读、身份冲突、无法分类的连接错误）。
    Unprovable(String),
}

/// 非子进程退出观察结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessExitProof {
    /// 内核句柄观察到进程已退出。
    Exited,
    /// 预算内仍存活。
    StillRunning,
    /// 无法观察（无 pidfd 支持、权限不足等）。
    Unprovable(String),
}

#[cfg(target_os = "linux")]
pub struct BoundManagedCore {
    fd: std::os::fd::OwnedFd,
    deadline: Instant,
}

#[cfg(target_os = "linux")]
impl BoundManagedCore {
    pub fn bind(pid: u32, budget: Duration) -> Result<Self, String> {
        use std::os::fd::FromRawFd;
        let deadline = Instant::now() + budget;
        if pid == 0 || pid > i32::MAX as u32 || budget.is_zero() {
            return Err("invalid pid or expired stop budget".to_string());
        }
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
        if fd < 0 {
            return Err(format!(
                "pidfd_open failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let bound = Self {
            fd: unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) },
            deadline,
        };
        if !bound.is_live()? {
            return Err("process exited before identity verification".to_string());
        }
        Ok(bound)
    }

    pub fn is_live(&self) -> Result<bool, String> {
        use std::os::fd::AsRawFd;
        if Instant::now() >= self.deadline {
            return Err("stop budget expired".to_string());
        }
        let mut fd = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut fd, 1, 0) };
        if result < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if result > 0 && fd.revents & libc::POLLIN == 0 {
            return Err("unexpected pidfd poll event".to_string());
        }
        Ok(result == 0)
    }

    pub fn terminate_and_wait(&self) -> ProcessExitProof {
        use std::os::fd::AsRawFd;
        match self.is_live() {
            Ok(true) => {}
            Ok(false) => {
                return ProcessExitProof::Unprovable("process exited before signal".to_string())
            }
            Err(e) => return ProcessExitProof::Unprovable(e),
        }
        if unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.fd.as_raw_fd(),
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        } != 0
        {
            return ProcessExitProof::Unprovable(format!(
                "pidfd_send_signal failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        loop {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return ProcessExitProof::StillRunning;
            }
            let mut fd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ms = remaining.as_millis().min(i32::MAX as u128).max(1) as i32;
            let result = unsafe { libc::poll(&mut fd, 1, ms) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return ProcessExitProof::Unprovable(error.to_string());
            }
            if result > 0 {
                return if fd.revents & libc::POLLIN != 0 {
                    ProcessExitProof::Exited
                } else {
                    ProcessExitProof::Unprovable("unexpected pidfd poll event".to_string())
                };
            }
        }
    }
}

struct PeerCredentials {
    pid: u32,
    uid: u32,
}

#[cfg(target_os = "linux")]
fn api_peer_credentials(socket: &Path) -> Result<PeerCredentials, ConnectionClass> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(ConnectionClass::Unprovable(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };

    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = socket.as_os_str().as_encoded_bytes();
    if bytes.len() >= addr.sun_path.len() {
        return Err(ConnectionClass::Unprovable(
            "socket path too long".to_string(),
        ));
    }
    for (slot, byte) in addr.sun_path.iter_mut().zip(bytes.iter()) {
        *slot = *byte as libc::c_char;
    }
    let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;
    let result = unsafe {
        libc::connect(
            owned.as_raw_fd(),
            &addr as *const libc::sockaddr_un as *const libc::sockaddr,
            len,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::ENOENT) | Some(libc::ECONNREFUSED) => ConnectionClass::Absent,
            _ => ConnectionClass::Unprovable(error.to_string()),
        });
    }

    let mut creds: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            owned.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut creds as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if result != 0 {
        return Err(ConnectionClass::Unprovable(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    if creds.pid <= 0 {
        return Err(ConnectionClass::Unprovable(
            "peer credentials carry no pid".to_string(),
        ));
    }
    Ok(PeerCredentials {
        pid: creds.pid as u32,
        uid: creds.uid,
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn api_peer_credentials(_socket: &Path) -> Result<PeerCredentials, ConnectionClass> {
    Err(ConnectionClass::Unprovable(
        "SO_PEERCRED identity probing is Linux-only".to_string(),
    ))
}

enum ConnectionClass {
    Absent,
    Unprovable(String),
}

/// SPEC 裁决 D-1：Core API 端点的归属证明（四证）。
pub fn prove_core_api_peer(
    endpoint: &str,
    expected_core_exe: &Path,
    managed_state_dir: &Path,
    service_uid: Option<u32>,
) -> CoreIdentityProof {
    let Some(socket) = endpoint_unix_path(endpoint) else {
        return CoreIdentityProof::Unprovable(format!("unsupported Core API endpoint: {endpoint}"));
    };
    // 不存在就是 Absent，必须先于平台 peer 探测：macOS/其他 Unix 没有
    // SO_PEERCRED，若先走平台探测会把「没有 socket」误报成 Unprovable。
    match std::fs::metadata(socket) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CoreIdentityProof::Absent
        }
        Err(error) => {
            return CoreIdentityProof::Unprovable(format!(
                "cannot stat Core API socket {socket} ({error})"
            ))
        }
    }
    let credentials = match api_peer_credentials(Path::new(socket)) {
        Ok(credentials) => credentials,
        Err(ConnectionClass::Absent) => return CoreIdentityProof::Absent,
        Err(ConnectionClass::Unprovable(reason)) => return CoreIdentityProof::Unprovable(reason),
    };
    // 1) peer UID 必须是受管服务身份（或 root）。
    if let Some(expected_uid) = service_uid {
        if credentials.uid != expected_uid && credentials.uid != 0 {
            return CoreIdentityProof::Unprovable(format!(
                "peer uid {} is neither the service uid {expected_uid} nor root",
                credentials.uid
            ));
        }
    }
    let pid = credentials.pid;

    // 2) `/proc/<pid>/exe` 必须是目标 Core 可执行文件。
    let exe = match std::fs::read_link(format!("/proc/{pid}/exe")) {
        Ok(exe) => exe,
        Err(error) => {
            return CoreIdentityProof::Unprovable(format!(
                "cannot read /proc/{pid}/exe ({error}); identity unprovable"
            ))
        }
    };
    let expected = match std::fs::canonicalize(expected_core_exe) {
        Ok(path) => path,
        Err(error) => {
            return CoreIdentityProof::Unprovable(format!(
                "cannot canonicalize {} ({error})",
                expected_core_exe.display()
            ))
        }
    };
    let exe = match managed_exe_path(&exe, &expected) {
        Some(exe) => exe,
        None => {
            return CoreIdentityProof::Unprovable(format!(
                "peer process {pid} runs {} instead of the managed core {}",
                exe.display(),
                expected.display()
            ))
        }
    };

    // 3) 精确受管 `-d`/`-f` 参数。
    let raw = match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(raw) => raw,
        Err(error) => {
            return CoreIdentityProof::Unprovable(format!(
                "cannot read /proc/{pid}/cmdline ({error}); identity unprovable"
            ))
        }
    };
    let cmdline: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).to_string())
        .collect();
    let managed = managed_state_dir.to_string_lossy().to_string();
    if argument_value(&cmdline, "-d").as_deref() != Some(managed.as_str()) {
        return CoreIdentityProof::Unprovable(format!(
            "peer process {pid} does not carry the managed -d {managed}"
        ));
    }
    let Some(config) = argument_value(&cmdline, "-f") else {
        return CoreIdentityProof::Unprovable(format!(
            "peer process {pid} carries no explicit -f managed config"
        ));
    };
    if !Path::new(&config).starts_with(managed_state_dir) {
        return CoreIdentityProof::Unprovable(format!(
            "peer process {pid} -f {config} is outside the managed state dir"
        ));
    }

    CoreIdentityProof::Proven(ManagedCoreIdentity { pid, exe, cmdline })
}

/// A managed artifact that was replaced while its process kept running shows up
/// as `<path> (deleted)`; that is exactly the orphan we have to identify, so the
/// managed path still counts as proven identity (the path cannot be occupied by
/// a foreign binary without write access to the artifact directory).
fn managed_exe_path(observed: &Path, expected: &Path) -> Option<PathBuf> {
    if observed == expected {
        return Some(observed.to_path_buf());
    }
    let raw = observed.to_string_lossy();
    let stripped = raw.strip_suffix(" (deleted)")?;
    (Path::new(stripped) == expected).then(|| PathBuf::from(stripped))
}

fn argument_value(cmdline: &[String], flag: &str) -> Option<String> {
    cmdline
        .iter()
        .position(|arg| arg == flag)
        .and_then(|index| cmdline.get(index + 1))
        .cloned()
}

fn endpoint_unix_path(endpoint: &str) -> Option<&str> {
    endpoint
        .strip_prefix("unix://")
        .or_else(|| endpoint.starts_with('/').then_some(endpoint))
}

/// SPEC 裁决 D-2：非子进程的可靠退出证明（pidfd，无复用语义）。
///
/// SIGTERM 发送成功 **不是** 证明：必须由内核句柄观察到退出。预算内未退出返回
/// `StillRunning`，不可观察返回 `Unprovable`——两者都不得当作 `Stopped`。
#[cfg(target_os = "linux")]
pub fn observe_process_exit(pid: u32, budget: Duration) -> ProcessExitProof {
    use std::os::fd::{AsRawFd, FromRawFd};

    if pid == 0 {
        return ProcessExitProof::Unprovable("pid 0 carries no identity".to_string());
    }
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
    let raw: i64 = raw;
    let pidfd = (raw >= 0).then_some(raw as i32);
    let Some(pidfd) = pidfd else {
        let error = std::io::Error::last_os_error();
        // ESRCH = the process is already gone; that is a valid exit observation.
        if error.raw_os_error() == Some(libc::ESRCH) {
            return ProcessExitProof::Exited;
        }
        return ProcessExitProof::Unprovable(format!("pidfd_open({pid}) failed: {error}"));
    };
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(pidfd) };
    let deadline = Instant::now() + budget;
    loop {
        let mut pollfd = libc::pollfd {
            fd: owned.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let slice = std::slice::from_mut(&mut pollfd);
        let result = unsafe { libc::poll(slice.as_mut_ptr(), 1, 50) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return ProcessExitProof::Unprovable(format!("poll on pidfd({pid}) failed: {error}"));
        }
        if result > 0 && pollfd.revents & libc::POLLIN != 0 {
            return ProcessExitProof::Exited;
        }
        if Instant::now() >= deadline {
            return ProcessExitProof::StillRunning;
        }
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
pub fn observe_process_exit(pid: u32, _budget: Duration) -> ProcessExitProof {
    ProcessExitProof::Unprovable(format!(
        "no stable kernel handle to observe exit of pid {pid} on this platform"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn bound_pidfd_rejects_invalid_expired_and_gone_processes() {
        assert!(BoundManagedCore::bind(0, Duration::from_secs(5)).is_err());
        assert!(BoundManagedCore::bind(std::process::id(), Duration::ZERO).is_err());
        let mut child = std::process::Command::new("/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(BoundManagedCore::bind(pid, Duration::from_secs(5)).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bound_pidfd_signals_and_observes_same_child() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let bound = BoundManagedCore::bind(child.id(), Duration::from_secs(5)).unwrap();
        assert!(bound.is_live().unwrap());
        assert_eq!(bound.terminate_and_wait(), ProcessExitProof::Exited);
        child.wait().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn expired_bound_handle_refuses_to_signal() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let bound = BoundManagedCore::bind(child.id(), Duration::from_millis(20)).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        assert!(matches!(
            bound.terminate_and_wait(),
            ProcessExitProof::Unprovable(_)
        ));
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    // pidfd 是 Linux 内核句柄语义；macOS 的 observe_process_exit 按设计返回
    // Unprovable，不能伪装成已观测退出。
    #[cfg(target_os = "linux")]
    #[test]
    fn pidfd_observes_a_real_exit_and_never_a_bare_signal() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .expect("spawn sleeper");
        let pid = child.id();
        // Process is alive: no proof yet, and a signal is not a proof either.
        assert_eq!(
            observe_process_exit(pid, Duration::from_millis(50)),
            ProcessExitProof::StillRunning
        );
        let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        assert_eq!(
            observe_process_exit(pid, Duration::from_secs(5)),
            ProcessExitProof::Exited
        );
        let _ = child.wait();
    }

    // ESRCH/pidfd 语义仅 Linux 可用；macOS 一律 Unprovable。
    #[cfg(target_os = "linux")]
    #[test]
    fn unknown_pid_is_unprovable_but_gone_pid_is_exited() {
        // A pid we never owned (init) exists but our identity probe has no proof.
        let proof = observe_process_exit(1, Duration::from_millis(10));
        assert!(
            matches!(
                proof,
                ProcessExitProof::StillRunning | ProcessExitProof::Unprovable(_)
            ),
            "pid 1 must never be reported as exited: {proof:?}"
        );
        // A definitely-free pid yields ESRCH → exited.
        let mut child = std::process::Command::new("/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert_eq!(
            observe_process_exit(pid, Duration::from_millis(10)),
            ProcessExitProof::Exited
        );
    }

    #[test]
    fn absent_socket_is_not_an_identity_claim() {
        let temp = tempfile::TempDir::new().unwrap();
        let endpoint = format!("unix://{}", temp.path().join("missing.sock").display());
        assert_eq!(
            prove_core_api_peer(&endpoint, Path::new("/bin/true"), temp.path(), None),
            CoreIdentityProof::Absent
        );
    }

    #[test]
    fn foreign_peer_is_unprovable_never_stopped() {
        // A listener owned by *this* process but whose exe is not the expected core
        // must be reported as Unprovable (identity conflict), not as Absent/Stopped.
        let temp = tempfile::TempDir::new().unwrap();
        let socket = temp.path().join("api.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let endpoint = format!("unix://{}", socket.display());
        let proof = prove_core_api_peer(
            &endpoint,
            Path::new("/bin/sleep"),
            temp.path(),
            Some(unsafe { libc::geteuid() }),
        );
        assert!(
            matches!(proof, CoreIdentityProof::Unprovable(_)),
            "foreign peer must be unprovable: {proof:?}"
        );
        drop(listener);
    }

    #[test]
    fn replaced_artifact_orphan_is_still_the_managed_core() {
        let expected = Path::new("/usr/local/lib/mihomo/mihomo");
        assert_eq!(
            managed_exe_path(
                Path::new("/usr/local/lib/mihomo/mihomo (deleted)"),
                expected
            ),
            Some(expected.to_path_buf())
        );
        assert_eq!(
            managed_exe_path(Path::new("/tmp/foreign-core"), expected),
            None
        );
    }
}
