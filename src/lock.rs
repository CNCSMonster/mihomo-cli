use std::cell::Cell;
use std::fs::{File, OpenOptions};
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

        let lock_path = config_dir.join(".mihomo-cli.lock");
        std::fs::create_dir_all(config_dir)?;

        #[cfg(unix)]
        {
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
                .map_err(|e| {
                    anyhow::anyhow!("Cannot open lock file {}: {}", lock_path.display(), e)
                })?;

            Self::lock_file(&file, &lock_path)?;
            Ok(ConfigLockGuard {
                _file: Some(file),
                _path: lock_path,
            })
        }
        #[cfg(windows)]
        {
            let file = Self::open_windows_exclusive_lock_file(&lock_path)?;
            Ok(ConfigLockGuard {
                _file: Some(file),
                _path: lock_path,
            })
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            let file = Self::create_exclusive_lock_file(&lock_path)?;
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
                LOCK_DEPTH.with(|depth| depth.set(0));
                anyhow::bail!("flock failed on {}: {}", lock_path.display(), err);
            }
            if Instant::now() >= deadline {
                LOCK_DEPTH.with(|depth| depth.set(0));
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
                        LOCK_DEPTH.with(|depth| depth.set(0));
                        anyhow::bail!(
                            "Another mihomo-cli instance is modifying config (timed out after {}s).\n  \
                             Please retry in a moment.",
                            LOCK_TIMEOUT.as_secs()
                        );
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(err) => {
                    LOCK_DEPTH.with(|depth| depth.set(0));
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
        )
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
                        LOCK_DEPTH.with(|depth| depth.set(0));
                        anyhow::bail!(
                            "Another mihomo-cli instance is modifying config (timed out after {}s).\n  \
                             Please retry in a moment.",
                            LOCK_TIMEOUT.as_secs()
                        );
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(err) => {
                    LOCK_DEPTH.with(|depth| depth.set(0));
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
}
