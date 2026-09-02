use crate::utils::AppPaths;
use anyhow::Result;
use chrono::Utc;
use std::path::{Path, PathBuf};

const BACKUP_ITEMS: &[&str] = &[
    "config.yaml",
    "rules.yaml",
    "dns-policy.yaml",
    "subscriptions.yaml",
    ".rules-position",
    "selections",
    "overrides",
    "subscriptions",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupReport {
    pub path: PathBuf,
    pub copied_items: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    pub restored_items: Vec<String>,
    pub safety_backup: Option<PathBuf>,
}

pub fn default_backup_dir(paths: &AppPaths) -> PathBuf {
    paths
        .config_dir()
        .join("backups")
        .join(Utc::now().format("%Y%m%d-%H%M%S").to_string())
}

pub fn backup_config(paths: &AppPaths, dest: &Path) -> Result<BackupReport> {
    let src = paths.config_dir();
    if !src.exists() {
        anyhow::bail!("Config directory does not exist: {}", src.display());
    }

    crate::utils::ensure_dir_all_no_follow(dest)?;
    let mut copied_items = Vec::new();
    for item in BACKUP_ITEMS {
        let from = src.join(item);
        if from.exists() {
            copy_path(&from, &dest.join(item))?;
            copied_items.push((*item).to_string());
        }
    }

    if copied_items.is_empty() {
        anyhow::bail!("No known config files found under {}", src.display());
    }

    Ok(BackupReport {
        path: dest.to_path_buf(),
        copied_items,
    })
}

pub fn restore_config(
    backup: &Path,
    paths: &AppPaths,
    create_safety_backup: bool,
) -> Result<RestoreReport> {
    if !backup.is_dir() {
        anyhow::bail!("Backup directory not found: {}", backup.display());
    }
    if !BACKUP_ITEMS.iter().any(|name| backup.join(name).exists()) {
        anyhow::bail!(
            "Backup does not contain known mihomo-cli config files: {}",
            backup.display()
        );
    }

    let dest = paths.config_dir();
    crate::utils::ensure_dir_all_no_follow(dest)?;

    let safety_backup = if create_safety_backup && has_existing_config(paths) {
        let safety = dest.join("backups").join(format!(
            "pre-restore-{}",
            Utc::now().format("%Y%m%d-%H%M%S")
        ));
        crate::utils::ensure_dir_all_no_follow(&safety)?;
        for name in BACKUP_ITEMS {
            let from = dest.join(name);
            if from.exists() {
                copy_path(&from, &safety.join(name))?;
            }
        }
        Some(safety)
    } else {
        None
    };

    let mut restored_items = Vec::new();
    for name in BACKUP_ITEMS {
        let from = backup.join(name);
        if from.exists() {
            copy_path(&from, &dest.join(name))?;
            restored_items.push((*name).to_string());
        }
    }

    Ok(RestoreReport {
        restored_items,
        safety_backup,
    })
}

pub fn shell_escape_path(path: &Path) -> String {
    let s = path.display().to_string();
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_/.:".contains(c))
    {
        s
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn has_existing_config(paths: &AppPaths) -> bool {
    BACKUP_ITEMS
        .iter()
        .any(|name| paths.config_dir().join(name).exists())
}

fn copy_path(from: &Path, to: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(from)?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("refusing to copy symbolic link: {}", from.display());
    }
    if metadata.is_dir() {
        if to.exists() {
            remove_dir_all_no_follow(to)?;
        }
        crate::utils::ensure_dir_all_no_follow(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_path(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = to.parent() {
            crate::utils::ensure_dir_all_no_follow(parent)?;
        }
        let content = read_regular_file_no_follow(from)?;
        crate::utils::atomic_write_file_for_original_user(&to.display().to_string(), &content)?;
    } else {
        anyhow::bail!("refusing to copy non-regular file: {}", from.display());
    }
    Ok(())
}

#[cfg(unix)]
fn read_regular_file_no_follow(path: &Path) -> Result<String> {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", path.display()))?;
    let dir = crate::utils::open_directory_no_follow(parent)?;
    let name = std::ffi::CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        anyhow::bail!("refusing to read non-regular file: {}", path.display());
    }
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(content)
}

#[cfg(not(unix))]
fn read_regular_file_no_follow(path: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(path)?)
}

fn remove_dir_all_no_follow(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!("refusing to remove symbolic link: {}", path.display());
    }
    if !metadata.is_dir() {
        anyhow::bail!("refusing to remove non-directory: {}", path.display());
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        let child_metadata = std::fs::symlink_metadata(&child)?;
        if child_metadata.file_type().is_symlink() {
            anyhow::bail!("refusing to remove symbolic link: {}", child.display());
        }
        if child_metadata.is_dir() {
            remove_dir_all_no_follow(&child)?;
        } else if child_metadata.is_file() {
            crate::utils::remove_file_if_exists(&child)?;
        } else {
            anyhow::bail!("refusing to remove non-regular file: {}", child.display());
        }
    }
    std::fs::remove_dir(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn paths(tmp: &TempDir) -> AppPaths {
        AppPaths::for_test(tmp.path())
    }

    #[test]
    fn backup_copies_known_files_and_subscriptions_dir() {
        let tmp = TempDir::new().unwrap();
        let paths = paths(&tmp);
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::write(paths.config_path(), "port: 7890\n").unwrap();
        std::fs::write(paths.rules_path(), "rules: []\n").unwrap();
        std::fs::create_dir_all(paths.selections_dir()).unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription("sub-a"),
            "selections: {}\n",
        )
        .unwrap();
        std::fs::write(paths.subscription_file_path("sub-a"), "proxies: []\n").unwrap();

        let dest = tmp.path().join("backup-out");
        let report = backup_config(&paths, &dest).unwrap();

        assert_eq!(report.path, dest);
        assert!(report.copied_items.contains(&"config.yaml".to_string()));
        assert!(report.copied_items.contains(&"rules.yaml".to_string()));
        assert!(report.copied_items.contains(&"selections".to_string()));
        assert!(report.copied_items.contains(&"subscriptions".to_string()));
        assert_eq!(
            std::fs::read_to_string(dest.join("config.yaml")).unwrap(),
            "port: 7890\n"
        );
        assert!(dest.join("subscriptions/sub-a.yaml").exists());
    }

    #[test]
    fn backup_rejects_empty_config_dir() {
        let tmp = TempDir::new().unwrap();
        let paths = paths(&tmp);
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let err = backup_config(&paths, &tmp.path().join("backup-out")).unwrap_err();
        assert!(err.to_string().contains("No known config files"));
    }

    #[test]
    fn restore_overwrites_files_and_creates_safety_backup() {
        let tmp = TempDir::new().unwrap();
        let paths = paths(&tmp);
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), "port: 7890\n").unwrap();

        let backup = tmp.path().join("backup-in");
        std::fs::create_dir_all(backup.join("subscriptions")).unwrap();
        std::fs::write(backup.join("config.yaml"), "port: 7891\n").unwrap();
        std::fs::write(backup.join("subscriptions/active"), "sub-b").unwrap();

        let report = restore_config(&backup, &paths, true).unwrap();

        assert!(report.safety_backup.is_some());
        let safety = report.safety_backup.unwrap();
        assert_eq!(
            std::fs::read_to_string(safety.join("config.yaml")).unwrap(),
            "port: 7890\n"
        );
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            "port: 7891\n"
        );
        assert_eq!(
            std::fs::read_to_string(paths.active_file_path()).unwrap(),
            "sub-b"
        );
        assert!(report.restored_items.contains(&"config.yaml".to_string()));
        assert!(report.restored_items.contains(&"subscriptions".to_string()));
    }

    #[test]
    fn restore_rejects_unknown_backup_dir() {
        let tmp = TempDir::new().unwrap();
        let paths = paths(&tmp);
        let backup = tmp.path().join("backup-in");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("random.txt"), "x").unwrap();

        let err = restore_config(&backup, &paths, false).unwrap_err();
        assert!(err.to_string().contains("does not contain known"));
    }

    #[test]
    fn shell_escape_quotes_spaces_and_single_quotes() {
        assert_eq!(shell_escape_path(Path::new("/tmp/a b")), "'/tmp/a b'");
        assert_eq!(shell_escape_path(Path::new("/tmp/a'b")), "'/tmp/a'\\''b'");
    }
}
