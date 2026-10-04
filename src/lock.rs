use std::cell::Cell;
use std::fs::File;
#[cfg(any(windows, all(not(unix), not(windows))))]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

thread_local! {
    static LOCK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

pub struct ConfigLockGuard {
    _file: Option<File>,
    _path: PathBuf,
}

impl Drop for ConfigLockGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(ref file) = self._file {
            unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_UN);
            }
        }
        #[cfg(all(not(unix), not(windows)))]
        if self._file.is_some() {
            let _ = std::fs::remove_file(&self._path);
        }
        LOCK_DEPTH.with(|depth| {
            depth.set(depth.get().saturating_sub(1));
        });
    }
}

pub struct ConfigLock;

/// Rolls back the outermost acquire's re-entrancy increment when the
/// acquisition fails before a guard exists to release it (Bug #13). On
/// success `disarm()` hands ownership of the increment to
/// `ConfigLockGuard::drop`, keeping the depth balanced exactly once.
struct DepthRollback(std::cell::Cell<bool>);

impl DepthRollback {
    fn new() -> Self {
        Self(std::cell::Cell::new(true))
    }
    fn disarm(&self) {
        self.0.set(false);
    }
}

impl Drop for DepthRollback {
    fn drop(&mut self) {
        if self.0.get() {
            LOCK_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
}

impl ConfigLock {
    pub fn acquire(config_dir: &Path) -> anyhow::Result<ConfigLockGuard> {
        // Reentrant: if this thread already holds the lock, return a no-op guard.
        let already_held = LOCK_DEPTH.with(|depth| {
            let d = depth.get();
            depth.set(d + 1);
            d
        });
        if already_held > 0 {
            return Ok(ConfigLockGuard {
                _file: None,
                _path: config_dir.join(".mihomo-cli.lock"),
            });
        }

        // Outermost acquisition: any error below must undo the increment,
        // otherwise a later acquire on this thread would take the reentrant
        // branch and return a guard that holds no OS lock.
        let rollback = DepthRollback::new();

        let lock_path = config_dir.join(".mihomo-cli.lock");
        crate::utils::ensure_dir_all_no_follow(config_dir)?;
        crate::utils::restore_original_user_config_ownership(config_dir)?;

        #[cfg(unix)]
        {
            let deadline = Instant::now() + LOCK_TIMEOUT;
            let file = loop {
                match crate::utils::open_file_create_no_follow(&lock_path) {
                    Ok(file) => break file,
                    Err(error) if crate::utils::is_not_found_error(&error) => {
                        if Instant::now() >= deadline {
                            return Err(anyhow::anyhow!(
                                "Cannot open lock file {}: {}",
                                lock_path.display(),
                                error
                            ));
                        }
                        std::thread::sleep(POLL_INTERVAL);
                    }
                    Err(error) => {
                        return Err(anyhow::anyhow!(
                            "Cannot open lock file {}: {}",
                            lock_path.display(),
                            error
                        ));
                    }
                }
            };

            crate::utils::restore_original_user_config_ownership(&lock_path)?;
            Self::lock_file(&file, &lock_path)?;
            rollback.disarm();
            Ok(ConfigLockGuard {
                _file: Some(file),
                _path: lock_path,
            })
        }
        #[cfg(windows)]
        {
            let file = Self::open_windows_exclusive_lock_file(&lock_path)?;
            rollback.disarm();
            Ok(ConfigLockGuard {
                _file: Some(file),
                _path: lock_path,
            })
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let file = Self::create_exclusive_lock_file(&lock_path)?;
            rollback.disarm();
            Ok(ConfigLockGuard {
                _file: Some(file),
                _path: lock_path,
            })
        }
    }

    #[cfg(unix)]
    fn lock_file(file: &File, lock_path: &Path) -> anyhow::Result<()> {
        let fd = file.as_raw_fd();
        let deadline = Instant::now() + LOCK_TIMEOUT;

        loop {
            let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if ret == 0 {
                return Ok(());
            }
            let err = io::Error::last_os_error();
            if err.kind() != ErrorKind::WouldBlock {
                anyhow::bail!("flock failed on {}: {}", lock_path.display(), err);
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "Another mihomo-cli instance is modifying config (timed out after {}s).\n  \
                     Please retry in a moment.",
                    LOCK_TIMEOUT.as_secs()
                );
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    #[cfg(windows)]
    fn open_windows_exclusive_lock_file(lock_path: &Path) -> anyhow::Result<File> {
        use std::os::windows::fs::OpenOptionsExt;

        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                // share_mode(0) denies read/write/delete sharing while this
                // handle is alive. Unlike create_new lock files, Windows will
                // release the lock if the process exits unexpectedly.
                .share_mode(0)
                .open(lock_path)
            {
                Ok(file) => return Ok(file),
                Err(err) if Self::is_lock_contention(&err) => {
                    if Instant::now() >= deadline {
                        anyhow::bail!(
                            "Another mihomo-cli instance is modifying config (timed out after {}s).\n  \
                             Please retry in a moment.",
                            LOCK_TIMEOUT.as_secs()
                        );
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(err) => {
                    anyhow::bail!("Cannot open lock file {}: {}", lock_path.display(), err);
                }
            }
        }
    }

    #[cfg(windows)]
    fn is_lock_contention(err: &std::io::Error) -> bool {
        matches!(
            err.kind(),
            ErrorKind::PermissionDenied | ErrorKind::AlreadyExists | ErrorKind::WouldBlock
        ) || matches!(err.raw_os_error(), Some(32 | 33))
    }

    #[cfg(all(not(unix), not(windows)))]
    fn create_exclusive_lock_file(lock_path: &Path) -> anyhow::Result<File> {
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(lock_path)
            {
                Ok(file) => return Ok(file),
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if Instant::now() >= deadline {
                        anyhow::bail!(
                            "Another mihomo-cli instance is modifying config (timed out after {}s).\n  \
                             Please retry in a moment.",
                            LOCK_TIMEOUT.as_secs()
                        );
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(err) => {
                    anyhow::bail!("Cannot create lock file {}: {}", lock_path.display(), err);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lock_acquire_and_release() {
        let tmp = TempDir::new().unwrap();
        let guard = ConfigLock::acquire(tmp.path()).unwrap();
        assert!(tmp.path().join(".mihomo-cli.lock").exists());
        drop(guard);
        // Can re-acquire after drop
        let _guard2 = ConfigLock::acquire(tmp.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn lock_blocks_second_acquirer_until_released() {
        let tmp = TempDir::new().unwrap();
        let guard = ConfigLock::acquire(tmp.path()).unwrap();

        let tmp_path = tmp.path().to_path_buf();
        let handle = std::thread::spawn(move || {
            // This should block until the first guard is dropped
            ConfigLock::acquire(&tmp_path)
        });

        // Give the thread time to start and block
        std::thread::sleep(Duration::from_millis(200));
        assert!(!handle.is_finished(), "second acquire should be blocking");

        // Release the first lock
        drop(guard);

        // Now second should complete
        let result = handle.join().unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn lock_is_reentrant_within_thread() {
        let tmp = TempDir::new().unwrap();
        let _first = ConfigLock::acquire(tmp.path()).unwrap();
        let _second = ConfigLock::acquire(tmp.path()).unwrap();
        assert!(tmp.path().join(".mihomo-cli.lock").exists());
    }

    #[test]
    fn failed_acquire_does_not_grant_a_fake_lock_on_retry() {
        // Bug #13 regression: when the outermost acquire fails before taking
        // the OS lock (here ensure_dir_all_no_follow hits ENOTDIR because a
        // path component is a regular file), the re-entrancy depth must be
        // rolled back. Otherwise the next acquire on the same thread sees
        // already_held > 0 and returns a no-op guard with no OS lock —
        // silently bypassing config serialization.
        let tmp = TempDir::new().unwrap();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let bad_dir = blocker.join("nested");

        let first = ConfigLock::acquire(&bad_dir);
        assert!(first.is_err(), "first acquire must fail");

        let second = ConfigLock::acquire(&bad_dir);
        assert!(
            second.is_err(),
            "retry after a failed acquire must not return a guard without an OS lock"
        );
    }

    #[test]
    fn failed_acquire_does_not_poison_subsequent_real_lock() {
        // After an outermost acquire fails (depth must have been rolled back),
        // the next acquire on the same thread must take a REAL OS lock —
        // observable as blocking a second thread, which a leaked reentrant
        // state would skip (fake guard) or the second thread would never see.
        let tmp = TempDir::new().unwrap();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        assert!(ConfigLock::acquire(&blocker.join("nested")).is_err());

        let valid = TempDir::new().unwrap();
        let guard = ConfigLock::acquire(valid.path()).unwrap();

        let valid_path = valid.path().to_path_buf();
        let handle = std::thread::spawn(move || ConfigLock::acquire(&valid_path));
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !handle.is_finished(),
            "acquire after a failed one must hold a real OS lock that blocks others"
        );

        drop(guard);
        assert!(handle.join().unwrap().is_ok());
    }
}
