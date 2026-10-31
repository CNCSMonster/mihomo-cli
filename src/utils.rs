use std::path::{Path, PathBuf};

#[cfg(unix)]
pub(crate) fn mode_has_setgid(mode: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        mode & libc::S_ISGID as u32 != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        mode & libc::S_ISGID != 0
    }
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub(crate) fn mode_has_setgid(_mode: u32) -> bool {
    false
}

/// SPEC P1-1 回归：sudo 场景下代际信任根必须豁免「原始用户 home」写入边界。
#[cfg(all(test, target_os = "linux"))]
#[test]
fn generation_trust_root_is_a_managed_system_write_boundary() {
    assert!(is_managed_system_runtime_path(Path::new(
        "/var/lib/mihomo-cli-generations"
    )));
    assert!(is_managed_system_runtime_path(Path::new(
        "/var/lib/mihomo-cli-generations/generations/gen-1/manifest.json"
    )));
    assert!(is_managed_system_runtime_path(Path::new(
        "/var/lib/.mihomo-cli-generations.migrating-98e243c3b6f83bce"
    )));
    assert!(is_managed_system_runtime_path(Path::new(
        "/var/lib/.mihomo-cli-generations.migrating-98e243c3b6f83bce/state.json"
    )));
    assert!(!is_managed_system_runtime_path(Path::new(
        "/var/lib/other-service/data"
    )));
    assert!(!is_managed_system_runtime_path(Path::new(
        "/var/lib/.evil-migrating-98e243c3b6f83bce"
    )));
    assert!(!is_managed_system_runtime_path(Path::new(
        "/var/lib/.mihomo-cli-generations.migrating-zz"
    )));
    assert!(!is_managed_system_runtime_path(Path::new(
        "/var/lib/.mihomo-cli-generations.migrating-12345"
    )));
    assert!(!is_managed_system_runtime_path(Path::new(
        "/home/user/.mihomo-cli-generations.migrating-98e243c3b6f83bce"
    )));
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn test_open_write_parent_dir_no_follow_for_staging_path() {
    let staging = Path::new("/var/lib/.mihomo-cli-generations.migrating-98e243c3b6f83bce");
    let (_dirfd, name) = open_write_parent_dir_no_follow(staging).unwrap();
    assert_eq!(name, ".mihomo-cli-generations.migrating-98e243c3b6f83bce");
    assert!(assert_original_user_write_allowed(staging).is_ok());
}

#[cfg(test)]
#[test]
fn test_is_migration_staging_for_root_matrix() {
    #[cfg(unix)]
    {
        let parent = Path::new("/var/lib");
        assert!(is_migration_staging_for_root(
            Path::new("/var/lib/.mihomo-cli-generations.migrating-98e243c3b6f83bce"),
            parent,
            "mihomo-cli-generations"
        ));
        assert!(is_migration_staging_for_root(
            Path::new(
                "/var/lib/.mihomo-cli-generations.migrating-98e243c3b6f83bce/generations/gen-1"
            ),
            parent,
            "mihomo-cli-generations"
        ));
        assert!(!is_migration_staging_for_root(
            Path::new("/var/lib/mihomo-cli-generations"),
            parent,
            "mihomo-cli-generations"
        ));
        assert!(!is_migration_staging_for_root(
            Path::new("/var/lib/.mihomo-cli-generations.migrating-"),
            parent,
            "mihomo-cli-generations"
        ));
        assert!(!is_migration_staging_for_root(
            Path::new("/var/lib/.mihomo-cli-generations.migrating-short"),
            parent,
            "mihomo-cli-generations"
        ));
        assert!(!is_migration_staging_for_root(
            Path::new("/var/lib/.mihomo-cli-generations.migrating-0123456789abcdefg"),
            parent,
            "mihomo-cli-generations"
        ));
        assert!(!is_migration_staging_for_root(
            Path::new("/var/lib/.other.migrating-98e243c3b6f83bce"),
            parent,
            "mihomo-cli-generations"
        ));
        assert!(!is_migration_staging_for_root(
            Path::new("/home/user/.mihomo-cli-generations.migrating-98e243c3b6f83bce"),
            parent,
            "mihomo-cli-generations"
        ));
    }
}

#[cfg(test)]
pub(crate) fn env_test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    config_dir: PathBuf,
}

impl AppPaths {
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
        }
    }

    pub fn from_system() -> Self {
        Self::new(default_config_dir())
    }

    #[cfg(test)]
    pub fn for_test(root: impl AsRef<Path>) -> Self {
        Self::new(root.as_ref().join(".config/mihomo"))
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.yaml")
    }

    pub fn rules_path(&self) -> PathBuf {
        self.config_dir.join("rules.yaml")
    }

    pub fn rules_position_path(&self) -> PathBuf {
        self.config_dir.join(".rules-position")
    }

    pub fn dns_policy_path(&self) -> PathBuf {
        self.config_dir.join("dns-policy.yaml")
    }

    pub fn dns_fake_ip_filter_path(&self) -> PathBuf {
        self.config_dir.join("dns-fake-ip-filter.yaml")
    }

    pub fn override_path(&self) -> PathBuf {
        self.config_dir.join("override.yaml")
    }

    pub fn delay_cache_path(&self) -> PathBuf {
        self.config_dir.join("delay-cache.json")
    }

    pub fn selection_state_path(&self) -> PathBuf {
        self.config_dir.join("selection-state.yaml")
    }

    pub fn selections_dir(&self) -> PathBuf {
        self.config_dir.join("selections")
    }

    pub fn selection_state_path_for_subscription(&self, id: &str) -> PathBuf {
        self.selections_dir().join(format!("{id}.yaml"))
    }

    pub fn overrides_dir(&self) -> PathBuf {
        self.config_dir.join("overrides")
    }

    pub fn groups_override_path_for_subscription(&self, id: &str) -> PathBuf {
        self.overrides_dir().join(id).join("groups.yaml")
    }

    // ── Multi-subscription paths ──

    /// ~/.config/mihomo/subscriptions/
    pub fn subscriptions_dir(&self) -> PathBuf {
        self.config_dir.join("subscriptions")
    }

    /// ~/.config/mihomo/subscriptions.yaml
    pub fn subscriptions_meta_path(&self) -> PathBuf {
        self.config_dir.join("subscriptions.yaml")
    }

    /// ~/.config/mihomo/subscriptions/active
    pub fn active_file_path(&self) -> PathBuf {
        self.subscriptions_dir().join("active")
    }

    /// ~/.config/mihomo/subscriptions/<id>.yaml
    pub fn subscription_file_path(&self, id: &str) -> PathBuf {
        self.subscriptions_dir().join(format!("{id}.yaml"))
    }
}

fn default_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MIHOMO_CLI_CONFIG_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }

    if cfg!(target_os = "windows") {
        windows_config_dir(
            std::env::var_os("APPDATA").map(PathBuf::from),
            dirs::home_dir(),
        )
    } else {
        crate::instance::planned_current_context(crate::instance::InstanceMode::User)
            .map(|ctx| ctx.paths.config_dir)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config/mihomo"))
    }
}

fn windows_config_dir(app_data: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    app_data
        .or_else(|| home.map(|home| home.join("AppData").join("Roaming")))
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("mihomo")
}

/// Resolve the default/fallback mihomo core binary path.
///
/// Delegates to `instance::planned_current_context(InstanceMode::User)` as the
/// single source of truth for per-user core paths — this eliminates the
/// duplicate path logic that drifted in the past (BUG-14: Windows path was
/// missing the `bin\` segment). `MIHOMO_CLI_MIHOMO_PATH` env override still wins.
pub fn mihomo_path() -> String {
    if let Ok(path) = std::env::var("MIHOMO_CLI_MIHOMO_PATH") {
        if !path.trim().is_empty() {
            return path;
        }
    }

    crate::instance::planned_current_context(crate::instance::InstanceMode::User)
        .map(|ctx| ctx.paths.core_binary.display().to_string())
        .unwrap_or_else(|| {
            // Last-resort fallback if instance path planning fails (e.g. unsupported OS).
            #[cfg(target_os = "windows")]
            {
                let local = dirs::data_local_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
                format!("{}\\mihomo\\bin\\mihomo.exe", local.display())
            }
            #[cfg(not(target_os = "windows"))]
            {
                let home = dirs::home_dir().unwrap_or_default();
                format!("{}/.local/bin/mihomo", home.display())
            }
        })
}

#[allow(dead_code)]
pub fn config_dir() -> String {
    AppPaths::from_system().config_dir().display().to_string()
}

/// Get the per-user core API socket directory.
/// - Linux: $XDG_RUNTIME_DIR/mihomo (standard XDG runtime location)
/// - macOS: /tmp/mihomo-$UID (v3 per-user runtime directory)
/// - Windows: not applicable (uses named pipes)
#[cfg(unix)]
pub fn socket_dir() -> String {
    #[cfg(target_os = "linux")]
    {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
            // Fallback when XDG_RUNTIME_DIR is unset (e.g. containers, non-systemd
            // sessions). Prefer the real UID; /proc/self/loginuid is -1 (4294967295)
            // in containers and other non-login contexts, so fall back to `id -u`
            // before assuming 1000.
            let uid = std::fs::read_to_string("/proc/self/loginuid")
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .filter(|u| *u != u32::MAX) // loginuid 0xFFFFFFFF = no login session
                .or_else(|| {
                    std::process::Command::new("id")
                        .arg("-u")
                        .output()
                        .ok()
                        .and_then(|out| String::from_utf8(out.stdout).ok())
                        .and_then(|s| s.trim().parse::<u32>().ok())
                })
                .unwrap_or(1000);
            format!("/run/user/{}", uid)
        });
        format!("{}/mihomo", runtime_dir)
    }
    #[cfg(target_os = "macos")]
    {
        macos_socket_dir(unsafe { libc::getuid() })
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_socket_dir(uid: u32) -> String {
    format!("/tmp/mihomo-{uid}")
}

pub fn config_path() -> String {
    AppPaths::from_system().config_path().display().to_string()
}

/// Atomically write a file: write to .tmp then rename.
/// Prevents partial writes from corrupting the file.
#[cfg(test)]
pub fn atomic_write_file(path: &str, content: &str) -> anyhow::Result<()> {
    atomic_write_bytes_impl(Path::new(path), content.as_bytes(), 0o644, None)
}

/// Atomically write bytes using a no-follow parent traversal and an exclusive,
/// no-follow temporary file.  This is used by privileged transaction writers;
/// unlike `std::fs::write`, it cannot follow a pre-planted symlink.
pub fn atomic_write_bytes_no_follow(path: &Path, bytes: &[u8], mode: u16) -> anyhow::Result<()> {
    atomic_write_bytes_impl(path, bytes, mode, None)
}

/// atomically publish a new inode with an explicit owner. System TUN
/// snapshots written by the root CLI must be readable by the `mihomo` daemon; the
/// ownership is applied to the temporary file *before* the rename, never as an
/// after-the-fact repair. Failures leave the previous file untouched.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn atomic_write_bytes_no_follow_owned(
    path: &Path,
    bytes: &[u8],
    mode: u16,
    uid: u32,
    gid: u32,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        atomic_write_bytes_impl(path, bytes, mode, Some((uid, gid)))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, bytes, mode, uid, gid);
        anyhow::bail!("owned atomic writes are only supported on Linux")
    }
}

/// Unix：父目录逐组件 O_NOFOLLOW 下降，tmp 文件使用随机名称并以
/// O_EXCL | O_NOFOLLOW 创建，renameat 同目录原子替换。
/// root 代替原始用户写入时，目标 canonical 后必须位于其 home 内（fail-closed）。
#[cfg(unix)]
fn atomic_write_bytes_impl(
    path: &Path,
    bytes: &[u8],
    mode: u16,
    owner: Option<(u32, u32)>,
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let (dirfd, name) = open_write_parent_dir_no_follow(path)?;
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
    let (fd, tmp_c) = loop {
        let tmp_name = format!(
            ".{}.tmp-{:016x}",
            name.to_string_lossy(),
            rand::random::<u64>()
        );
        let tmp_c = std::ffi::CString::new(tmp_name)
            .map_err(|err| anyhow::anyhow!("invalid temp file name: {err}"))?;
        let fd = unsafe {
            libc::openat(
                dirfd.as_raw_fd(),
                tmp_c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode as libc::c_uint,
            )
        };
        if fd >= 0 {
            break (fd, tmp_c);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(err).map_err(|err| {
                anyhow::anyhow!("Failed to create temp file for {}: {}", path.display(), err)
            });
        }
    };
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let prepare = (|| -> anyhow::Result<()> {
        file.write_all(bytes)?;
        if let Some((uid, gid)) = owner {
            if unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if let Some((uid, gid)) = owner {
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let stat = unsafe { stat.assume_init() };
            if stat.st_uid != uid
                || stat.st_gid != gid
                || stat.st_mode & libc::S_IFMT != libc::S_IFREG
                || stat.st_nlink != 1
                || stat.st_mode & 0o7777 != u32::from(mode) & 0o7777
            {
                anyhow::bail!(
                    "temp file for {} did not retain the requested owner/mode",
                    path.display()
                );
            }
        }
        file.sync_all()?;
        Ok(())
    })();
    if let Err(err) = prepare {
        drop(file);
        let _ = unsafe { libc::unlinkat(dirfd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(err);
    }
    drop(file);
    if unsafe {
        libc::renameat(
            dirfd.as_raw_fd(),
            tmp_c.as_ptr(),
            dirfd.as_raw_fd(),
            name_c.as_ptr(),
        )
    } != 0
    {
        let err = std::io::Error::last_os_error();
        let _ = unsafe { libc::unlinkat(dirfd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(err)
            .map_err(|err| anyhow::anyhow!("Failed to rename into {}: {}", path.display(), err));
    }
    if owner.is_some() && unsafe { libc::fsync(dirfd.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn atomic_write_bytes_impl(
    path: &Path,
    bytes: &[u8],
    _mode: u16,
    _owner: Option<(u32, u32)>,
) -> anyhow::Result<()> {
    crate::generation::atomic_write_file_safely(path, bytes)
        .map_err(|e| anyhow::anyhow!("Failed to atomically write {}: {}", path.display(), e))
}

pub fn atomic_write_file_for_original_user(path: &str, content: &str) -> anyhow::Result<()> {
    let path = std::path::Path::new(path);
    let parent = path.parent().unwrap_or(path);
    #[cfg(unix)]
    let parent_is_setgid = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(parent)
            .map(|metadata| mode_has_setgid(metadata.permissions().mode()))
            .unwrap_or(false)
    };
    #[cfg(not(unix))]
    let parent_is_setgid = false;

    // Per-user config is private. Linux system mode marks the config tree
    // setgid, allowing the dedicated service group read access without making
    // files visible to other users.
    let mode = if parent_is_setgid { 0o640 } else { 0o600 };
    atomic_write_bytes_impl(path, content.as_bytes(), mode, None)?;
    #[cfg(unix)]
    {
        if parent_is_setgid {
            restore_original_user_owner_preserving_group(parent)?;
            return restore_original_user_owner_preserving_group(path);
        }
    }
    restore_original_user_config_ownership(parent)?;
    restore_original_user_config_ownership(path)
}

pub fn remove_file_if_exists(path: &std::path::Path) -> anyhow::Result<()> {
    remove_file_if_exists_impl(path)
}

/// Unix：与写入侧同级的 no-follow 防护——父目录 canonical + O_NOFOLLOW 下降，
/// unlinkat 相对 dirfd 删除，root 代替原始用户时同样 fail-closed。
#[cfg(unix)]
fn remove_file_if_exists_impl(path: &Path) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    if !parent.exists() {
        return Ok(());
    }
    let (dirfd, name) = open_write_parent_dir_no_follow(path)?;
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
    if unsafe { libc::unlinkat(dirfd.as_raw_fd(), name_c.as_ptr(), 0) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err.into());
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn remove_file_if_exists_impl(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Unix：打开 `path` 的父目录（逐组件 O_NOFOLLOW），返回目录 fd 与文件名。
#[cfg(unix)]
fn open_parent_dir_no_follow(
    path: &Path,
) -> anyhow::Result<(std::os::fd::OwnedFd, std::ffi::OsString)> {
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", path.display()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("path has no parent directory: {}", path.display()))?;
    let fd = open_path_no_follow(parent)?;
    Ok((fd, file_name.to_os_string()))
}

pub(crate) fn sync_directory_no_follow(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let fd = open_directory_no_follow(path)?;
        let file = std::fs::File::from(fd);
        file.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!(
            "directory durability is unsupported on this platform: {}",
            path.display()
        )
    }
}

/// 创建目录（含缺失的中间组件）。Unix 下从最近的已存在祖先（canonical）逐组件
/// openat(O_NOFOLLOW|O_DIRECTORY) 下降，缺失时 mkdirat，拒绝穿越符号链接；
/// root 代替原始用户创建时目标必须位于其 home 内（fail-closed）。
#[cfg(unix)]
pub fn ensure_dir_all_no_follow(path: &Path) -> anyhow::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    if !path.is_absolute() {
        anyhow::bail!("ensure_dir path must be absolute: {}", path.display());
    }
    let (mut fd, relative) = if let Some((_, _, home)) = original_user_identity()? {
        if path.starts_with(&home) {
            let relative = original_user_home_relative_path(path, &home)?;
            (open_path_no_follow(&home)?, relative)
        } else {
            let mut ancestor = path;
            while !ancestor.exists() {
                ancestor = ancestor.parent().ok_or_else(|| {
                    anyhow::anyhow!("path has no existing ancestor: {}", path.display())
                })?;
            }
            let canonical_ancestor = ancestor.canonicalize()?;
            let fd = open_path_no_follow(&canonical_ancestor)?;
            let relative = path
                .strip_prefix(ancestor)
                .map_err(|err| anyhow::anyhow!("failed to relativize {}: {err}", path.display()))?
                .to_path_buf();
            (fd, relative)
        }
    } else {
        let mut ancestor = path;
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or_else(|| {
                anyhow::anyhow!("path has no existing ancestor: {}", path.display())
            })?;
        }
        let canonical_ancestor = ancestor.canonicalize()?;
        let fd = open_path_no_follow(&canonical_ancestor)?;
        let relative = path
            .strip_prefix(ancestor)
            .map_err(|err| anyhow::anyhow!("failed to relativize {}: {err}", path.display()))?
            .to_path_buf();
        (fd, relative)
    };
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            anyhow::bail!(
                "ensure_dir path contains non-normal component: {}",
                path.display()
            );
        };
        let name_c = std::ffi::CString::new(name.as_bytes())
            .map_err(|err| anyhow::anyhow!("invalid path component: {err}"))?;
        let opened = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if opened >= 0 {
            fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(opened) };
            continue;
        }
        let open_err = std::io::Error::last_os_error();
        if open_err.kind() != std::io::ErrorKind::NotFound {
            return Err(open_err).map_err(|err| {
                anyhow::anyhow!(
                    "Failed to open directory component in {}: {}",
                    path.display(),
                    err
                )
            });
        }
        #[allow(clippy::unnecessary_cast)]
        let parent_has_setgid = {
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } == 0 {
                let stat = unsafe { stat.assume_init() };
                mode_has_setgid(stat.st_mode as u32)
            } else {
                false
            }
        };
        let create_mode = if parent_has_setgid { 0o2755 } else { 0o755 };

        if unsafe { libc::mkdirat(fd.as_raw_fd(), name_c.as_ptr(), create_mode) } != 0 {
            let err = std::io::Error::last_os_error();
            // 并发创建竞争：已存在则继续走 openat
            if err.raw_os_error() != Some(libc::EEXIST) {
                return Err(err).map_err(|err| {
                    anyhow::anyhow!(
                        "Failed to create directory component in {}: {}",
                        path.display(),
                        err
                    )
                });
            }
        }
        let created = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if created < 0 {
            return Err(std::io::Error::last_os_error()).map_err(|err| {
                anyhow::anyhow!(
                    "Failed to open created directory in {}: {}",
                    path.display(),
                    err
                )
            });
        }
        if parent_has_setgid {
            let _ = unsafe { libc::fchmod(created, 0o2755) };
        }
        fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(created) };
    }
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFDIR {
        anyhow::bail!("ensure_dir target is not a directory: {}", path.display());
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn ensure_dir_all_no_follow(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    Ok(())
}

/// 以 no-follow 打开（必要时创建）文件，供锁文件等需要长期持有 fd 的场景使用。
///
/// 描述符带 `O_CLOEXEC`：锁文件被 exec 出的子进程继承后，
/// 「忘记显式解锁」会从自进程内泄漏升级成跨进程死锁（实证的 10s 白等）。
#[cfg(unix)]
pub fn open_file_create_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let (dirfd, name) = open_write_parent_dir_no_follow(path)?;
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
    let fd = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o644,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(windows)]
pub fn open_file_create_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        anyhow::bail!(
            "refusing to open non-regular or reparse-point file: {}",
            path.display()
        );
    }
    Ok(file)
}

#[cfg(all(not(unix), not(windows)))]
pub fn open_file_create_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?)
}

#[cfg(unix)]
pub fn open_regular_file_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let (parent, name) = if let Some((_, _, home)) = original_user_identity()? {
        if original_user_home_relative_path(path, &home).is_ok() {
            open_original_user_parent_dir(path, &home)?
        } else {
            open_parent_dir_no_follow(path)?
        }
    } else {
        open_parent_dir_no_follow(path)?
    };
    let name = std::ffi::CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFREG || stat.st_nlink != 1 {
        anyhow::bail!(
            "refusing to open non-regular or hard-linked file: {}",
            path.display()
        );
    }
    Ok(file)
}

/// Read a privileged artifact through the no-follow regular-file boundary and
/// reject oversized input before allocating an unbounded buffer.
pub fn read_file_no_follow_limited(path: &Path, max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;

    let mut file = open_regular_file_no_follow(path)?;
    let size = file.metadata()?.len();
    if size > max_bytes {
        anyhow::bail!(
            "refusing to read oversized artifact {} ({} bytes > {} byte limit)",
            path.display(),
            size,
            max_bytes
        );
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.by_ref().take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        anyhow::bail!(
            "artifact {} grew beyond {} byte limit while being read",
            path.display(),
            max_bytes
        );
    }
    Ok(bytes)
}

pub fn is_not_found_error(err: &anyhow::Error) -> bool {
    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
        return io_err.kind() == std::io::ErrorKind::NotFound;
    }
    if let Some(io_err) = err.root_cause().downcast_ref::<std::io::Error>() {
        return io_err.kind() == std::io::ErrorKind::NotFound;
    }
    false
}

pub fn read_file_no_follow_limited_optional(
    path: &Path,
    max_bytes: u64,
) -> anyhow::Result<Option<Vec<u8>>> {
    match read_file_no_follow_limited(path, max_bytes) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) => {
            if is_not_found_error(&err) {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}

#[cfg(windows)]
pub fn open_regular_file_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        anyhow::bail!(
            "refusing to open non-regular or reparse-point file: {}",
            path.display()
        );
    }
    Ok(file)
}

#[cfg(all(not(unix), not(windows)))]
pub fn open_regular_file_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    Ok(std::fs::File::open(path)?)
}

/// 直接写文件（非原子）并设置权限位，Unix 下全程 O_NOFOLLOW + fail-closed 护栏。
pub fn write_bytes_file_no_follow(path: &Path, bytes: &[u8], mode: u16) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;

        assert_original_user_write_allowed(path)?;
        let (dirfd, name) = open_parent_dir_no_follow(path)?;
        let name_c = std::ffi::CString::new(name.as_bytes())
            .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
        let fd = unsafe {
            libc::openat(
                dirfd.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW,
                mode as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .map_err(|err| anyhow::anyhow!("Failed to open {}: {}", path.display(), err));
        }
        // 已存在的文件不会应用创建 mode，显式 fchmod 保持语义一致
        if unsafe { libc::fchmod(fd, mode as libc::mode_t) } != 0 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(err)
                .map_err(|err| anyhow::anyhow!("Failed to chmod {}: {}", path.display(), err));
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.write_all(bytes)
            .map_err(|err| anyhow::anyhow!("Failed to write {}: {}", path.display(), err))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

fn canonical_existing_ancestor(path: &Path) -> anyhow::Result<PathBuf> {
    let mut current = path;
    while !current.exists() {
        current = current
            .parent()
            .ok_or_else(|| anyhow::anyhow!("path has no existing ancestor: {}", path.display()))?;
    }
    Ok(current.canonicalize()?)
}

/// canonical 后的目标必须位于原始用户 home 内（含 home 本身）。
/// home 内的一切本来就归该用户所有/可写，symlink 逃逸出 home 才是提权边界。
pub(crate) fn canonical_original_user_home_path(
    path: &Path,
    home: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let canonical_home = home.canonicalize()?;
    let path = canonical_existing_ancestor(path)?;
    if path == canonical_home || path.starts_with(&canonical_home) {
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

/// 系统 daemon 自有运行态目录与代际信任根，不属于原始用户 home 的写入边界。
///
/// SPEC P1-1：generation store 是 root:root 0750 的独立信任根，由特权 CLI
/// 在 sudo 场景写入；它既不在原始用户 home 内，也不是 daemon 可写目录，必须显式
/// 列入本白名单，否则所有 `sudo` 代际写入都会被 fail-closed 拒绝。
#[cfg(target_os = "linux")]
fn is_managed_system_runtime_path(path: &Path) -> bool {
    path == Path::new("/var/lib/mihomo-cli")
        || path.starts_with(Path::new("/var/lib/mihomo-cli/"))
        || path == Path::new("/var/lib/mihomo-cli-generations")
        || path.starts_with(Path::new("/var/lib/mihomo-cli-generations/"))
        || is_generation_migration_staging(path)
}

#[cfg(target_os = "linux")]
fn is_generation_migration_staging(path: &Path) -> bool {
    is_migration_staging_for_root(path, Path::new("/var/lib"), "mihomo-cli-generations")
}

#[cfg(target_os = "linux")]
fn is_managed_system_install_path(path: &Path) -> bool {
    matches!(
        path,
        path if path == Path::new("/usr/local/bin/mihomo-cli")
            || path == Path::new("/usr/local/lib/mihomo/mihomo")
    )
}

#[cfg(target_os = "macos")]
fn is_managed_system_runtime_path(path: &Path) -> bool {
    path == Path::new("/Library/Application Support/mihomo-cli")
        || path.starts_with(Path::new("/Library/Application Support/mihomo-cli/"))
        || path == Path::new("/Library/Application Support/mihomo-cli-generations")
        || path.starts_with(Path::new(
            "/Library/Application Support/mihomo-cli-generations/",
        ))
        || is_generation_migration_staging(path)
}

#[cfg(target_os = "macos")]
fn is_generation_migration_staging(path: &Path) -> bool {
    is_migration_staging_for_root(
        path,
        Path::new("/Library/Application Support"),
        "mihomo-cli-generations",
    )
}

#[cfg(unix)]
fn is_migration_staging_for_root(path: &Path, expected_parent: &Path, root_name: &str) -> bool {
    let prefix = format!(".{root_name}.migrating-");
    let Ok(relative) = path.strip_prefix(expected_parent) else {
        return false;
    };
    let Some(first_comp) = relative.components().next() else {
        return false;
    };
    let std::path::Component::Normal(comp) = first_comp else {
        return false;
    };
    let Some(comp_str) = comp.to_str() else {
        return false;
    };
    let Some(suffix) = comp_str.strip_prefix(&prefix) else {
        return false;
    };
    !suffix.is_empty() && suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(target_os = "macos")]
fn is_managed_system_install_path(path: &Path) -> bool {
    matches!(
        path,
        path if path == Path::new("/Library/Application Support/mihomo/bin/mihomo-cli")
            || path == Path::new("/Library/Application Support/mihomo/bin/mihomo")
            || path == Path::new("/usr/local/bin/mihomo-cli")
    )
}

#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn is_managed_system_runtime_path(_path: &Path) -> bool {
    false
}

#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn is_managed_system_install_path(_path: &Path) -> bool {
    false
}

/// 安全地得到原始用户 home 下的相对路径：不 canonicalize，拒绝 `..`，
/// 后续同一个 home dirfd 上逐组件 O_NOFOLLOW 打开，消除 check/use 间的路径替换窗口。
#[cfg(unix)]
fn original_user_home_relative_path(path: &Path, home: &Path) -> anyhow::Result<PathBuf> {
    let relative = path.strip_prefix(home).map_err(|_| {
        anyhow::anyhow!(
            "refusing to write outside the original user's home directory: {}",
            path.display()
        )
    })?;
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        anyhow::bail!(
            "ownership path contains non-normal component: {}",
            path.display()
        );
    }
    Ok(relative.to_path_buf())
}

/// 在可信 passwd home 的 fd 上打开目标父目录；路径判定与使用共享同一条 no-follow fd 链。
#[cfg(unix)]
fn open_original_user_parent_dir(
    path: &Path,
    home: &Path,
) -> anyhow::Result<(std::os::fd::OwnedFd, std::ffi::OsString)> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let relative = original_user_home_relative_path(path, home)?;
    let name = relative
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", path.display()))?
        .to_os_string();
    let parent = relative.parent().unwrap_or(Path::new(""));
    let mut fd = open_path_no_follow(home)?;
    for component in parent.components() {
        let std::path::Component::Normal(component) = component else {
            unreachable!()
        };
        let component = std::ffi::CString::new(component.as_bytes())
            .map_err(|err| anyhow::anyhow!("invalid path component: {err}"))?;
        let next = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                component.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if next < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(next) };
    }
    Ok((fd, name))
}

/// 为写入操作获得父目录 fd。sudo 场景锚定在 passwd home；其它场景从绝对路径安全下降。
#[cfg(unix)]
fn open_write_parent_dir_no_follow(
    path: &Path,
) -> anyhow::Result<(std::os::fd::OwnedFd, std::ffi::OsString)> {
    if is_managed_system_runtime_path(path) || is_managed_system_install_path(path) {
        open_parent_dir_no_follow(path)
    } else if let Some((_, _, home)) = original_user_identity()? {
        open_original_user_parent_dir(path, &home)
    } else {
        open_parent_dir_no_follow(path)
    }
}

/// 为显式指定的用户 home 创建目录，供管理员为其它用户授权时使用。
#[cfg(unix)]
pub fn ensure_home_path_traversable(
    path: &Path,
    home: &Path,
    _owner_gid: u32,
) -> anyhow::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let daemon_gid = unsafe {
        let group = libc::getgrnam(c"mihomo".as_ptr());
        if group.is_null() {
            anyhow::bail!("mihomo service group is missing");
        }
        (*group).gr_gid
    };
    let relative = original_user_home_relative_path(path, home)?;
    let mut fd = open_path_no_follow(home)?;
    let mut components = vec![PathBuf::from(".")];
    if let Some(parent) = relative.parent() {
        components.extend(parent.components().filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(PathBuf::from(name)),
            _ => None,
        }));
    }

    for component in components {
        let name = std::ffi::CString::new(component.as_os_str().as_bytes())?;
        let next = if component == Path::new(".") {
            fd.try_clone()?
        } else {
            let opened = unsafe {
                libc::openat(
                    fd.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if opened < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            unsafe { std::os::fd::OwnedFd::from_raw_fd(opened) }
        };

        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(next.as_raw_fd(), metadata.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let metadata = unsafe { metadata.assume_init() };
        let mode = metadata.st_mode as libc::mode_t;
        let has_group_traverse = metadata.st_gid == daemon_gid && mode & 0o010 != 0;
        let has_other_traverse = mode & 0o001 != 0;
        if !has_group_traverse && !has_other_traverse {
            let new_mode = mode
                | if metadata.st_gid == daemon_gid {
                    0o010
                } else {
                    0o001
                };
            if unsafe { libc::fchmod(next.as_raw_fd(), new_mode) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        fd = next;
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn ensure_home_path_traversable(
    _path: &Path,
    _home: &Path,
    _owner_gid: u32,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub fn ensure_dir_all_under_home_no_follow(path: &Path, home: &Path) -> anyhow::Result<()> {
    let relative = original_user_home_relative_path(path, home)?;
    ensure_dir_all_from_dirfd(open_path_no_follow(home)?, &relative, path)
}

#[cfg(unix)]
fn ensure_dir_all_from_dirfd(
    mut fd: std::os::fd::OwnedFd,
    relative: &Path,
    display_path: &Path,
) -> anyhow::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            anyhow::bail!(
                "ensure_dir path contains non-normal component: {}",
                display_path.display()
            );
        };
        let name_c = std::ffi::CString::new(name.as_bytes())
            .map_err(|err| anyhow::anyhow!("invalid path component: {err}"))?;
        let opened = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if opened >= 0 {
            fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(opened) };
            continue;
        }
        let open_err = std::io::Error::last_os_error();
        if open_err.kind() != std::io::ErrorKind::NotFound {
            return Err(open_err.into());
        }
        #[allow(clippy::unnecessary_cast)]
        let parent_has_setgid = {
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } == 0 {
                let stat = unsafe { stat.assume_init() };
                mode_has_setgid(stat.st_mode as u32)
            } else {
                false
            }
        };
        let create_mode = if parent_has_setgid { 0o2755 } else { 0o755 };

        if unsafe { libc::mkdirat(fd.as_raw_fd(), name_c.as_ptr(), create_mode) } != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let created = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if created < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if parent_has_setgid {
            let _ = unsafe { libc::fchmod(created, 0o2755) };
        }
        fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(created) };
    }
    Ok(())
}

#[cfg(unix)]
pub fn set_directory_mode_no_follow(path: &Path, mode: u16) -> anyhow::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let (parent, name) = open_write_parent_dir_no_follow(path)?;
    let name = std::ffi::CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let dir = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };

    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(dir.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };

    let mask = if (mode & 0o7000) != 0 { 0o7777 } else { 0o777 };
    let current_mode = stat.st_mode & mask;
    let target_mode = mode as libc::mode_t & mask;
    if current_mode == target_mode {
        return Ok(());
    }

    if unsafe { libc::fchmod(dir.as_raw_fd(), mode as libc::mode_t) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn set_directory_mode_no_follow(_path: &Path, _mode: u16) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn is_managed_system_state_dir_present() -> bool {
    Path::new("/var/lib/mihomo-cli").exists()
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn is_managed_system_state_dir_present() -> bool {
    false
}

/// SPEC §12.3.1: what a read-only probe of the managed system state directory can
/// actually prove. Ownership is only judgeable by an identity that can `stat` the
/// entry at all: the contract directory is `0770 root:mihomo`, so a caller outside
/// the `mihomo` group observes the top level and nothing beneath it. That asymmetry
/// is why "cannot look" must stay distinct from "is wrong" (issue ).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemStateDirVerdict {
    Healthy,
    NeedsRepair,
    /// The current identity cannot observe the asset, so no drift claim is possible.
    Undeterminable,
}

#[cfg(target_os = "linux")]
impl SystemStateDirVerdict {
    /// Confirmed drift outranks "cannot judge", which outranks healthy: the strongest
    /// evidence any single managed asset provides decides the directory.
    fn merge(self, other: Option<Self>) -> Self {
        match other {
            Some(other) if other.rank() > self.rank() => other,
            _ => self,
        }
    }

    fn rank(self) -> u8 {
        match self {
            SystemStateDirVerdict::Healthy => 0,
            SystemStateDirVerdict::Undeterminable => 1,
            SystemStateDirVerdict::NeedsRepair => 2,
        }
    }
}

/// Probe one managed asset with `lstat`. `None` means the asset is absent, which the
/// contract tolerates. The entry is deliberately never opened: opening is exactly
/// what a caller without the required identity cannot do, and that failure says
/// nothing about ownership or mode.
#[cfg(target_os = "linux")]
fn probe_managed_state_asset(
    path: &Path,
    expect_directory: bool,
    uid: libc::uid_t,
    gid: libc::gid_t,
    mode_ok: impl Fn(u32) -> bool,
) -> Option<SystemStateDirVerdict> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        // EACCES and friends: the current identity cannot see the entry.
        Err(_) => return Some(SystemStateDirVerdict::Undeterminable),
    };
    // `lstat` reports the link itself, so a symlink never satisfies either shape and
    // stays drift, preserving the no-follow contract the repair path enforces.
    let shape_ok = if expect_directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    };
    if !shape_ok
        || metadata.uid() != uid
        || metadata.gid() != gid
        || !mode_ok(metadata.mode() & 0o7777)
    {
        return Some(SystemStateDirVerdict::NeedsRepair);
    }
    Some(SystemStateDirVerdict::Healthy)
}

#[cfg(target_os = "linux")]
pub fn check_system_state_dir_needs_repair() -> SystemStateDirVerdict {
    check_system_state_dir_needs_repair_at(Path::new("/var/lib/mihomo-cli"))
}

#[cfg(target_os = "linux")]
pub(crate) fn check_system_state_dir_needs_repair_at(path: &Path) -> SystemStateDirVerdict {
    let group = unsafe { libc::getgrnam(c"mihomo".as_ptr()) };
    let user = unsafe { libc::getpwnam(c"mihomo".as_ptr()) };
    if group.is_null() || user.is_null() {
        // The service account is missing, so the install contract is broken.
        return SystemStateDirVerdict::NeedsRepair;
    }
    let uid = unsafe { (*user).pw_uid };
    let gid = unsafe { (*group).gr_gid };
    check_system_state_dir_needs_repair_with_identities(path, 0, uid, gid)
}

#[cfg(target_os = "linux")]
pub(crate) fn check_system_state_dir_needs_repair_with_identities(
    path: &Path,
    root_uid: libc::uid_t,
    uid: libc::uid_t,
    gid: libc::gid_t,
) -> SystemStateDirVerdict {
    use SystemStateDirVerdict::Healthy;

    let mut verdict =
        match probe_managed_state_asset(path, true, root_uid, gid, |mode| mode == 0o770) {
            None => return Healthy,
            Some(verdict) => verdict,
        };
    if verdict != Healthy {
        return verdict;
    }

    for asset in ["transactions", "transactions/gc", "transactions/active"] {
        verdict = verdict.merge(probe_managed_state_asset(
            &path.join(asset),
            true,
            uid,
            gid,
            transactions_dir_mode_acceptable,
        ));
    }
    verdict = verdict.merge(probe_managed_state_asset(
        &path.join("tun-config.yaml"),
        false,
        uid,
        gid,
        |mode| mode == 0o640,
    ));
    // SPEC A3 / SPEC: cache.db accepts either 0600 or 0640 (real Core persistence).
    verdict = verdict.merge(probe_managed_state_asset(
        &path.join(CACHE_DB_FILE_NAME),
        false,
        uid,
        gid,
        cache_db_mode_acceptable,
    ));
    for name in GEO_ASSET_FILE_NAMES {
        verdict = verdict.merge(probe_managed_state_asset(
            &path.join(name),
            false,
            uid,
            gid,
            geo_asset_mode_acceptable,
        ));
    }
    verdict
}

/// Geo assets must stay readable for the Core without being group/other writable.
#[cfg(target_os = "linux")]
fn geo_asset_mode_acceptable(mode: u32) -> bool {
    mode & 0o400 != 0 && mode & 0o022 == 0
}

/// SPEC: the writer publishes `transactions/` as `0o2750` so children inherit the
/// service group, so setgid is the only special bit the probe may tolerate. Any other
/// special bit (setuid/sticky) still marks a confirmed drift.
#[cfg(target_os = "linux")]
fn transactions_dir_mode_acceptable(mode: u32) -> bool {
    mode & 0o777 == 0o750 && mode & 0o4000 == 0 && mode & 0o1000 == 0
}

/// repair must never normalize a qualified mode, otherwise the setgid bit
/// the writer deliberately set would be stripped on every repair pass. Only confirmed
/// drift is rewritten, and then to the writer's canonical target for that directory.
#[cfg(target_os = "linux")]
fn repair_transaction_directory(
    path: &Path,
    uid: libc::uid_t,
    gid: libc::gid_t,
    canonical_mode: u32,
) -> anyhow::Result<bool> {
    use std::os::fd::AsRawFd;

    let dir = open_directory_no_follow(path)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(dir.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    let ownership_changed = stat.st_uid != uid || stat.st_gid != gid;
    let mode_target = (!transactions_dir_mode_acceptable(stat.st_mode)).then_some(canonical_mode);
    if !ownership_changed && mode_target.is_none() {
        return Ok(false);
    }
    if ownership_changed && unsafe { libc::fchown(dir.as_raw_fd(), uid, gid) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if let Some(target) = mode_target {
        if unsafe { libc::fchmod(dir.as_raw_fd(), target as libc::mode_t) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(true)
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn check_system_state_dir_needs_repair() -> SystemStateDirVerdict {
    // The `0770 root:mihomo` state directory contract is Linux-only.
    SystemStateDirVerdict::Healthy
}

#[cfg(target_os = "linux")]
pub fn ensure_mihomo_system_state_dir() -> anyhow::Result<bool> {
    use std::os::fd::AsRawFd;

    let path = Path::new("/var/lib/mihomo-cli");
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("system Geo repair requires root privileges");
    }
    let dir = open_directory_no_follow(path).map_err(|error| {
        anyhow::anyhow!(
            "Mihomo system data directory is unavailable; reinstall the system service: {error}"
        )
    })?;
    let group = unsafe { libc::getgrnam(c"mihomo".as_ptr()) };
    let user = unsafe { libc::getpwnam(c"mihomo".as_ptr()) };
    if group.is_null() || user.is_null() {
        anyhow::bail!("mihomo service account is missing; reinstall the system service");
    }
    let uid = unsafe { (*user).pw_uid };
    let gid = unsafe { (*group).gr_gid };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(dir.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    let mut changed = stat.st_uid != 0 || stat.st_gid != gid || (stat.st_mode & 0o777) != 0o770;
    if changed
        && (unsafe { libc::fchown(dir.as_raw_fd(), 0, gid) } != 0
            || unsafe { libc::fchmod(dir.as_raw_fd(), 0o770) } != 0)
    {
        return Err(std::io::Error::last_os_error().into());
    }

    // `transactions/` is the directory the writer publishes as 2750 (setgid),
    // while gc/active are only ever ensured 0750; repair therefore keeps 2750 as the
    // canonical target for the former and 0750 for the latter, and never rewrites an
    // already-qualified mode (`transactions_dir_mode_acceptable`).
    for (name, canonical_mode) in [("transactions", 0o2750), ("transactions/gc", 0o750)] {
        let transaction_path = path.join(name);
        match std::fs::create_dir(&transaction_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let transaction_changed =
            repair_transaction_directory(&transaction_path, uid, gid, canonical_mode).map_err(
                |error| {
                    anyhow::anyhow!(
                        "Mihomo TUN transaction directory is unsafe or unavailable; reinstall the system service: {error}"
                    )
                },
            )?;
        changed |= transaction_changed;
    }

    let active_transaction_path = path.join("transactions/active");
    match repair_transaction_directory(&active_transaction_path, uid, gid, 0o750) {
        Ok(active_transaction_changed) => changed |= active_transaction_changed,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
        Err(error) => {
            return Err(anyhow::anyhow!(
                "Mihomo active TUN transaction directory is unsafe or unavailable; reinstall the system service: {error}"
            ));
        }
    }

    for name in [
        "transactions/tun-journal.json",
        "transactions/tun-candidate.yaml",
        "transactions/active/journal.json",
        "transactions/active/candidate.yaml",
        "transactions/active/recovery-target.yaml",
        "transactions/active/old-snapshot.yaml",
        "transactions/active/old-runtime.json",
    ] {
        let transaction_file_path = path.join(name);
        match open_regular_file_no_follow(&transaction_file_path) {
            Ok(transaction_file) => {
                let mut transaction_file_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                if unsafe {
                    libc::fstat(
                        transaction_file.as_raw_fd(),
                        transaction_file_stat.as_mut_ptr(),
                    )
                } != 0
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                let transaction_file_stat = unsafe { transaction_file_stat.assume_init() };
                let transaction_file_changed = transaction_file_stat.st_uid != uid
                    || transaction_file_stat.st_gid != gid
                    || (transaction_file_stat.st_mode & 0o7777) != 0o640;
                if transaction_file_changed
                    && (unsafe { libc::fchown(transaction_file.as_raw_fd(), uid, gid) } != 0
                        || unsafe { libc::fchmod(transaction_file.as_raw_fd(), 0o640) } != 0)
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                changed |= transaction_file_changed;
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "Mihomo TUN transaction file is unsafe or unavailable; reinstall the system service: {error}"
                ));
            }
        }
    }

    let snapshot_path = path.join("tun-config.yaml");
    match open_regular_file_no_follow(&snapshot_path) {
        Ok(file) => {
            let mut snapshot_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(file.as_raw_fd(), snapshot_stat.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let snapshot_stat = unsafe { snapshot_stat.assume_init() };
            let snapshot_changed = snapshot_stat.st_uid != uid
                || snapshot_stat.st_gid != gid
                || (snapshot_stat.st_mode & 0o7777) != 0o640;
            if snapshot_changed
                && (unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0
                    || unsafe { libc::fchmod(file.as_raw_fd(), 0o640) } != 0)
            {
                return Err(std::io::Error::last_os_error().into());
            }
            changed |= snapshot_changed;
        }
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
        Err(error) => {
            return Err(anyhow::anyhow!(
                "Mihomo TUN snapshot is unsafe or unavailable; reinstall the system service: {error}"
            ));
        }
    }

    // SPEC A3: the repair list now covers cache.db and the geo assets, so
    // "permissions repaired" cannot silently leave a root-owned Core persistence file.
    let mut state_file_targets: Vec<(PathBuf, u32)> =
        vec![(path.join(CACHE_DB_FILE_NAME), CACHE_DB_MODE)];
    for name in GEO_ASSET_FILE_NAMES {
        state_file_targets.push((path.join(name), GEO_ASSET_MODE as u32));
    }
    for (state_file_path, expected_mode) in state_file_targets {
        match open_regular_file_no_follow(&state_file_path) {
            Ok(file) => {
                let mut state_file_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
                if unsafe { libc::fstat(file.as_raw_fd(), state_file_stat.as_mut_ptr()) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let state_file_stat = unsafe { state_file_stat.assume_init() };
                let mode_ok = if expected_mode == CACHE_DB_MODE {
                    cache_db_mode_acceptable(state_file_stat.st_mode)
                } else {
                    geo_asset_mode_acceptable(state_file_stat.st_mode)
                };
                let state_file_changed =
                    state_file_stat.st_uid != uid || state_file_stat.st_gid != gid || !mode_ok;
                if state_file_changed
                    && (unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0
                        || unsafe { libc::fchmod(file.as_raw_fd(), expected_mode as libc::mode_t) }
                            != 0)
                {
                    return Err(std::io::Error::last_os_error().into());
                }
                changed |= state_file_changed;
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "Mihomo state file is unsafe or unavailable; reinstall the system service: {error}"
                ));
            }
        }
    }

    Ok(changed)
}

#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub fn ensure_mihomo_system_transaction_permissions(
    transaction_dir: &Path,
    candidate_path: &Path,
    journal_path: &Path,
) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;

    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("system transaction permission repair requires root privileges");
    }
    let group = unsafe { libc::getgrnam(c"mihomo".as_ptr()) };
    let user = unsafe { libc::getpwnam(c"mihomo".as_ptr()) };
    if group.is_null() || user.is_null() {
        anyhow::bail!("mihomo service account is missing; reinstall the system service");
    }
    let uid = unsafe { (*user).pw_uid };
    let gid = unsafe { (*group).gr_gid };

    let dir = open_directory_no_follow(transaction_dir)?;
    if unsafe { libc::fchown(dir.as_raw_fd(), uid, gid) } != 0
        || unsafe { libc::fchmod(dir.as_raw_fd(), 0o750) } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }

    for (path, mode) in [(candidate_path, 0o640), (journal_path, 0o660)] {
        let file = open_regular_file_no_follow(path)?;
        if unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0
            || unsafe { libc::fchmod(file.as_raw_fd(), mode) } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn ensure_mihomo_system_transaction_permissions(
    _transaction_dir: &Path,
    _candidate_path: &Path,
    _journal_path: &Path,
) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn ensure_mihomo_system_state_dir() -> anyhow::Result<bool> {
    Ok(false)
}

#[cfg(unix)]
pub fn set_file_mode_no_follow(path: &Path, mode: u16) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;

    let file = open_regular_file_no_follow(path)?;
    if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn set_file_mode_no_follow(_path: &Path, _mode: u16) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub fn is_path_in_original_user_home(path: &Path) -> anyhow::Result<bool> {
    let Some((_, _, home)) = original_user_identity()? else {
        return Ok(false);
    };
    Ok(original_user_home_relative_path(path, &home).is_ok())
}

#[cfg(not(unix))]
pub fn is_path_in_original_user_home(_path: &Path) -> anyhow::Result<bool> {
    Ok(false)
}

/// Rename through no-follow parent fd chains. Both parents and all intermediate
/// components are opened with O_NOFOLLOW before renameat publishes the move.
#[cfg(unix)]
pub fn rename_no_follow(from: &Path, to: &Path) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let from_name = from
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", from.display()))?;
    let to_name = to
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", to.display()))?;
    let (from_dirfd, _) = open_write_parent_dir_no_follow(from)?;
    let (to_dirfd, _) = open_write_parent_dir_no_follow(to)?;
    let from_c = std::ffi::CString::new(from_name.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
    let to_c = std::ffi::CString::new(to_name.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
    if unsafe {
        libc::renameat(
            from_dirfd.as_raw_fd(),
            from_c.as_ptr(),
            to_dirfd.as_raw_fd(),
            to_c.as_ptr(),
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn rename_no_follow(from: &Path, to: &Path) -> anyhow::Result<()> {
    if from.is_dir() {
        std::fs::rename(from, to).map_err(|e| {
            anyhow::anyhow!(
                "Failed to rename directory {} to {}: {}",
                from.display(),
                to.display(),
                e
            )
        })
    } else {
        crate::generation::replace_file_safely(from, to).map_err(|e| {
            anyhow::anyhow!(
                "Failed to rename file {} to {}: {}",
                from.display(),
                to.display(),
                e
            )
        })
    }
}

/// Remove a directory only if it is empty, through an fd chain that never
/// follows a symlink. This is for pre-created trust-root directories: it is
/// atomic (AT_REMOVEDIR) and fails with ENOTEMPTY instead of deleting content.
#[cfg(unix)]
pub fn remove_empty_dir_no_follow(path: &Path) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let (parent, name) = open_write_parent_dir_no_follow(path)?;
    let name_c = std::ffi::CString::new(name.as_bytes())?;
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name_c.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn remove_empty_dir_no_follow(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!(
            "refusing to remove non-directory generation root: {}",
            path.display()
        );
    }
    std::fs::remove_dir(path)?;
    Ok(())
}

/// Remove a file or directory recursively through an fd chain. All directory
/// components are opened with O_NOFOLLOW, including the final target.
#[cfg(unix)]
pub fn remove_path_no_follow(path: &Path) -> anyhow::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let (parent, name) = open_write_parent_dir_no_follow(path)?;
    let name_c = std::ffi::CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if fd >= 0 {
        let dir = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        for _ in 0..3 {
            remove_directory_contents_no_follow(&dir)?;
            if unsafe { libc::unlinkat(parent.as_raw_fd(), name_c.as_ptr(), libc::AT_REMOVEDIR) }
                == 0
            {
                return Ok(());
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ENOTEMPTY) {
                return Err(err.into());
            }
        }
        return Err(std::io::Error::from_raw_os_error(libc::ENOTEMPTY).into());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::ENOTDIR) && err.raw_os_error() != Some(libc::ELOOP) {
        if err.kind() == std::io::ErrorKind::NotFound {
            return Ok(());
        }
        return Err(err.into());
    }
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name_c.as_ptr(), 0) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err.into());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn remove_directory_contents_no_follow(dir: &std::os::fd::OwnedFd) -> anyhow::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};

    let duplicated = unsafe { libc::dup(dir.as_raw_fd()) };
    if duplicated < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let directory = unsafe { libc::fdopendir(duplicated) };
    if directory.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe { libc::close(duplicated) };
        return Err(error.into());
    }

    loop {
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        let entry = unsafe { libc::readdir(directory) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            unsafe { libc::closedir(directory) };
            if error.raw_os_error() == Some(0) {
                return Ok(());
            }
            return Err(error.into());
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let name_c = std::ffi::CString::new(name)?;
        let child = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if child >= 0 {
            let child = unsafe { std::os::fd::OwnedFd::from_raw_fd(child) };
            remove_directory_contents_no_follow(&child)?;
            if unsafe { libc::unlinkat(dir.as_raw_fd(), name_c.as_ptr(), libc::AT_REMOVEDIR) } != 0
            {
                let error = std::io::Error::last_os_error();
                unsafe { libc::closedir(directory) };
                return Err(error.into());
            }
            continue;
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENOTDIR) && error.raw_os_error() != Some(libc::ELOOP)
        {
            unsafe { libc::closedir(directory) };
            return Err(error.into());
        }
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name_c.as_ptr(), 0) } != 0 {
            let error = std::io::Error::last_os_error();
            unsafe { libc::closedir(directory) };
            return Err(error.into());
        }
    }
}

#[cfg(not(unix))]
pub fn remove_path_no_follow(path: &Path) -> anyhow::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        remove_file_if_exists(path)?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn write_bytes_file_under_home_no_follow(
    path: &Path,
    home: &Path,
    bytes: &[u8],
    mode: u16,
    uid: libc::uid_t,
    gid: libc::gid_t,
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let (dirfd, name) = open_original_user_parent_dir(path, home)?;
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
    let fd = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } != 0
        || unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    file.write_all(bytes)?;
    Ok(())
}

/// Fail-closed 写入护栏：root 代替原始用户操作时只允许 passwd home 下的正常路径。
#[cfg(unix)]
fn assert_original_user_write_allowed(path: &Path) -> anyhow::Result<()> {
    if is_managed_system_runtime_path(path) {
        return Ok(());
    }
    let Some((_, _, home)) = original_user_identity()? else {
        return Ok(());
    };
    original_user_home_relative_path(path, &home)?;
    Ok(())
}

pub fn restore_original_user_config_ownership(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    let Some((_, _, home)) = original_user_identity()?
    else {
        return Ok(());
    };
    #[cfg(not(unix))]
    let home = crate::instance::PathInputs::from_current_env().home;

    let Some(path) = canonical_original_user_home_path(path, &home)? else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let is_setgid_directory = std::fs::metadata(&path)
            .map(|metadata| metadata.is_dir() && mode_has_setgid(metadata.permissions().mode()))
            .unwrap_or(false);
        if is_setgid_directory {
            return restore_original_user_owner_preserving_group(&path);
        }
    }
    restore_original_user_ownership(&path)
}

/// Restore the invoking user's ownership without changing the file's group.
/// System-mode config directories use a setgid service group so the non-root
/// daemon can keep reading files created during a sudo re-exec.
#[cfg(unix)]
pub fn restore_original_user_owner_preserving_group(path: &Path) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;

    let Some((uid, _, home)) = original_user_identity()? else {
        return Ok(());
    };
    let Some(path) = canonical_original_user_home_path(path, &home)? else {
        return Ok(());
    };
    let fd = open_path_no_follow(&path)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    if (stat.st_mode & libc::S_IFMT) == libc::S_IFREG && stat.st_nlink != 1 {
        anyhow::bail!("refusing to chown hard-linked file: {}", path.display());
    }
    if unsafe { libc::fchown(fd.as_raw_fd(), uid, !0 as libc::gid_t) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(unix)]
pub fn open_directory_no_follow(path: &Path) -> anyhow::Result<std::os::fd::OwnedFd> {
    let fd = open_path_no_follow(path)?;
    if !std::fs::File::from(fd.try_clone()?).metadata()?.is_dir() {
        anyhow::bail!("not a directory: {}", path.display());
    }
    Ok(fd)
}

/// 以 O_NOFOLLOW 打开（必要时创建）文件用于追加写入，供下载续传等场景使用。
#[cfg(unix)]
pub fn open_append_file_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let (dirfd, name) = open_write_parent_dir_no_follow(path)?;
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
    let fd = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .map_err(|err| anyhow::anyhow!("Failed to open {}: {}", path.display(), err));
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

/// 以 O_NOFOLLOW 打开（必要时创建）文件用于覆盖写入（截断），供下载重试等场景使用。
#[cfg(unix)]
pub fn open_truncate_file_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let (dirfd, name) = open_write_parent_dir_no_follow(path)?;
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|err| anyhow::anyhow!("invalid file name: {err}"))?;
    let fd = unsafe {
        libc::openat(
            dirfd.as_raw_fd(),
            name_c.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .map_err(|err| anyhow::anyhow!("Failed to open {}: {}", path.display(), err));
    }
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(not(unix))]
pub fn open_append_file_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?)
}

#[cfg(not(unix))]
pub fn open_truncate_file_no_follow(path: &Path) -> anyhow::Result<std::fs::File> {
    Ok(std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?)
}

#[cfg(unix)]
fn open_path_no_follow(path: &Path) -> anyhow::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    if !path.is_absolute() {
        anyhow::bail!("ownership path must be absolute: {}", path.display());
    }

    // macOS exposes these root-owned compatibility paths as fixed symlinks
    // into /private. Resolve only those OS aliases; resolving the full path
    // would allow a caller-controlled intermediate symlink to bypass the
    // component-by-component O_NOFOLLOW checks below.
    #[cfg(target_os = "macos")]
    let path = normalize_macos_system_alias(path);
    #[cfg(not(target_os = "macos"))]
    let path = path.to_path_buf();

    let root = unsafe { libc::open(c"/".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    if root < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(root) };
    let components: Vec<_> = path.components().collect();
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(component) = component else {
            if matches!(component, std::path::Component::RootDir) {
                continue;
            }
            anyhow::bail!(
                "ownership path contains non-normal component: {}",
                path.display()
            );
        };
        let component = std::ffi::CString::new(component.as_bytes())
            .map_err(|err| anyhow::anyhow!("invalid path component: {err}"))?;
        let is_last = index == components.len() - 1;
        #[cfg(target_os = "linux")]
        let flags = if is_last {
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
        } else {
            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW
        };
        #[cfg(target_os = "macos")]
        let flags = if is_last {
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
        } else {
            libc::O_SEARCH | libc::O_NOFOLLOW
        };
        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | libc::O_CLOEXEC
            | if is_last { 0 } else { libc::O_DIRECTORY };
        let next = unsafe { libc::openat(fd.as_raw_fd(), component.as_ptr(), flags) };
        if next < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(next) };
    }
    Ok(fd)
}

#[cfg(target_os = "macos")]
fn normalize_macos_system_alias(path: &Path) -> PathBuf {
    for (alias, target) in [
        (Path::new("/var"), Path::new("/private/var")),
        (Path::new("/tmp"), Path::new("/private/tmp")),
        (Path::new("/etc"), Path::new("/private/etc")),
    ] {
        if let Ok(relative) = path.strip_prefix(alias) {
            return target.join(relative);
        }
    }
    path.to_path_buf()
}

#[cfg(unix)]
pub(crate) fn original_user_identity() -> anyhow::Result<Option<(libc::uid_t, libc::gid_t, PathBuf)>>
{
    if unsafe { libc::geteuid() } != 0 {
        return Ok(None);
    }

    let Some(uid) = crate::instance::PathInputs::from_current_env().uid else {
        return Ok(None);
    };
    // 直接 root（无原始用户）不存在属主恢复对象
    if uid == 0 {
        return Ok(None);
    }
    let account = unsafe { libc::getpwuid(uid) };
    if account.is_null() {
        anyhow::bail!("cannot resolve account for uid {uid}");
    }
    let account = unsafe { &*account };
    let home = unsafe { std::ffi::CStr::from_ptr(account.pw_dir) }
        .to_str()
        .map_err(|err| anyhow::anyhow!("invalid home directory for uid {uid}: {err}"))?;
    Ok(Some((uid, account.pw_gid, PathBuf::from(home))))
}

#[cfg(unix)]
pub fn restore_original_user_ownership(path: &Path) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;

    let Some((uid, gid, _)) = original_user_identity()? else {
        return Ok(());
    };
    let fd = open_path_no_follow(path)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    if (stat.st_mode & libc::S_IFMT) == libc::S_IFREG && stat.st_nlink != 1 {
        anyhow::bail!("refusing to chown hard-linked file: {}", path.display());
    }
    let result = unsafe { libc::fchown(fd.as_raw_fd(), uid, gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn restore_original_user_ownership(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
#[allow(dead_code)]
pub fn restore_original_user_owned_regular_file_under_home(path: &Path) -> anyhow::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let Some((uid, _, home)) = original_user_identity()? else {
        anyhow::bail!("configuration ownership repair requires an original sudo user");
    };
    let (parent, name) = open_original_user_parent_dir(path, &home)?;
    let name = std::ffi::CString::new(name.as_bytes())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFREG || stat.st_nlink != 1 {
        anyhow::bail!(
            "refusing to repair a non-regular or hard-linked configuration file: {}",
            path.display()
        );
    }
    if unsafe { libc::fchown(file.as_raw_fd(), uid, !0 as libc::gid_t) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn restore_original_user_owned_regular_file_under_home(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

/// Build a reqwest client that trusts the system root certificates in addition to rustls built-in.
/// This fixes compatibility with sites whose CA is trusted by the OS but not by rustls' default store.
pub fn http_client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    add_native_cert_roots(builder)
}

#[cfg(not(windows))]
fn add_native_cert_roots(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // Load system native certificates and add them as extra roots for rustls.
    let native = rustls_native_certs::load_native_certs();
    let count = native.certs.len();
    for c in native.certs {
        if let Ok(cert) = reqwest::Certificate::from_der(c.as_ref()) {
            builder = builder.add_root_certificate(cert);
        }
    }
    if !native.errors.is_empty() {
        crate::log!("native cert errors: {:?}", native.errors);
    }
    crate::log!("added {count} native certs to TLS trust store");
    builder
}

#[cfg(windows)]
fn add_native_cert_roots(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    // Windows uses reqwest native-tls/schannel, which reads the OS trust store
    // directly and avoids compiling rustls' ring provider for Windows targets.
    builder
}

/// Combine stdout and stderr from a child process output.
/// Many programs (including mihomo) write error messages to stdout, not stderr.
/// Always check both streams when diagnosing failures.
pub fn combine_output(o: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stderr = String::from_utf8_lossy(&o.stderr);
    let mut combined = String::new();
    let out = stdout.trim();
    let err = stderr.trim();
    if !out.is_empty() {
        combined.push_str(out);
    }
    if !err.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(err);
    }
    combined
}

// ── Log sanitization ──

/// Sensitive URL query parameter names (case-insensitive partial match).
const SENSITIVE_QUERY_PARAMS: &[&str] = &[
    "token",
    "key",
    "secret",
    "password",
    "auth",
    "apikey",
    "api_key",
    "access_token",
    "refresh_token",
    "sub_key",
    "sub-key",
    "subscription_key",
    "subscription-key",
    "sign",
];

/// Sanitize a URL by masking the values of sensitive query parameters.
///
/// - `?token=abc123` → `?token=***`
/// - `?api_key=xyz` → `?api_key=***`
/// - Non-sensitive params (e.g. `?format=clash`) are left intact.
/// - URLs without a query string are returned as-is.
pub fn sanitize_url(url: &str) -> String {
    // Fast path: no query string at all
    let q_pos = match url.find('?') {
        Some(p) => p,
        None => return url.to_string(),
    };

    let (prefix, query_and_frag) = url.split_at(q_pos + 1); // includes '?'
    let (query, fragment) = match query_and_frag.find('#') {
        Some(p) => (&query_and_frag[..p], &query_and_frag[p..]),
        None => (query_and_frag, ""),
    };

    let sanitized_pairs: Vec<String> = query
        .split('&')
        .map(|pair| {
            if let Some(eq_pos) = pair.find('=') {
                let name = &pair[..eq_pos];
                if SENSITIVE_QUERY_PARAMS
                    .iter()
                    .any(|sp| sp.eq_ignore_ascii_case(name))
                {
                    return format!("{name}=***");
                }
            }
            pair.to_string()
        })
        .collect();

    format!("{}{}{}", prefix, sanitized_pairs.join("&"), fragment)
}

/// Sanitize a generic string by masking values after sensitive key patterns.
///
/// Matches patterns like:
/// - `password: xxx` → `password: ***`
/// - `secret=xxx` → `secret=***`
/// - `token "xxx"` → `token "***"`
///
/// This is a best-effort heuristic for log messages, not a full parser.
#[allow(dead_code)] // Public API used by `log_debug_sensitive!` macro
/// Compare contents of two files for equality.
/// Returns false if either file cannot be opened/read or metadata differs.
pub fn file_contents_equal(path_a: &Path, path_b: &Path) -> bool {
    if path_a == path_b {
        return path_a.is_file();
    }
    let (Ok(meta_a), Ok(meta_b)) = (std::fs::metadata(path_a), std::fs::metadata(path_b)) else {
        return false;
    };
    if !meta_a.is_file() || !meta_b.is_file() || meta_a.len() != meta_b.len() {
        return false;
    }
    if meta_a.len() == 0 {
        return true;
    }
    let (Ok(mut file_a), Ok(mut file_b)) =
        (std::fs::File::open(path_a), std::fs::File::open(path_b))
    else {
        return false;
    };
    use std::io::Read;
    let mut buf_a = [0u8; 65536];
    let mut buf_b = [0u8; 65536];
    loop {
        let n_a = match file_a.read(&mut buf_a) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => return false,
        };
        let n_b = match file_b.read(&mut buf_b[..n_a]) {
            Ok(n) if n == n_a => n,
            _ => return false,
        };
        if buf_a[..n_a] != buf_b[..n_b] {
            return false;
        }
    }
    true
}

/// Compute a 64-bit FNV-1a fingerprint for a file and return a short hex string.
#[allow(dead_code)]
pub fn compute_file_fingerprint(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hash = 0xcbf29ce484222325u64;
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    Some(format!("{:016x}", hash))
}

#[allow(dead_code)]
pub fn sanitize_sensitive(s: &str) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();

    let re = RE.get_or_init(|| {
        // Match: sensitive_word followed by optional separator (: = space) then a quoted or unquoted value
        regex::Regex::new(
            r#"(?i)(token|password|passwd|secret|api[_-]?key|auth|authorization)\s*[:=]\s*(?:"([^"]*)"|'([^']*)'|(\S+))"#,
        )
        .expect("valid regex")
    });

    re.replace_all(s, |caps: &regex::Captures| {
        let key = &caps[1];
        // One of groups 2/3/4 captured the value
        let _ = caps.get(2).or_else(|| caps.get(3)).or_else(|| caps.get(4));
        format!("{key}=***")
    })
    .into_owned()
}

// ---------------- cache.db (bbolt) hygiene — SPEC A1/A2 ----------------

/// bbolt layout constants, aligned with `metacubex/bbolt@010dbbbb7a5b`
/// (`internal/common/page.go`: `PageHeaderSize`/`MetaPageFlag`, and
/// `internal/common/meta.go`: `Validate`/`Sum64`).
const BOLT_PAGE_HEADER_SIZE: usize = 16;
const BOLT_META_SIZE: usize = 64;
const BOLT_META_MAGIC: u32 = 0xED0C_DAED;
const BOLT_META_VERSION: u32 = 2;
/// Smallest readable layout: one page header followed by one meta.
const BOLT_MIN_LAYOUT_BYTES: u64 = (BOLT_PAGE_HEADER_SIZE + BOLT_META_SIZE) as u64;
/// Upstream `getPageSizeFromFirstMeta` reads a full 4KiB page.
const BOLT_PAGE_READ_BYTES: usize = 0x1000;
/// Upstream `getPageSizeFromSecondMeta` probes page 1 at `1024 << i` for `i in 0..=14`.
const BOLT_PAGE_PROBE_CANDIDATES: u32 = 15;
/// Conservative lower bound for a page size a modern Core writes. A database
/// smaller than two such pages cannot be opened by bbolt at all.
const BOLT_MIN_PAGE_SIZE: u64 = 4096;

pub const CACHE_DB_FILE_NAME: &str = "cache.db";
pub const CACHE_DB_QUARANTINE_PREFIX: &str = "cache.db.corrupt-";
pub const CACHE_DB_QUARANTINE_RETENTION: usize = 3;
/// Upper bound for `EEXIST` sequence retries when publishing a quarantine name.
pub const CACHE_DB_QUARANTINE_MAX_ATTEMPTS: u32 = 64;
/// Geo data assets the Core reads from the managed state directory.
#[cfg(target_os = "linux")]
pub const GEO_ASSET_FILE_NAMES: [&str; 2] = ["geoip.metadb", "GeoSite.dat"];
/// Owner-only read/write; the Core (service user) must be able to persist.
#[cfg(unix)]
const CACHE_DB_MODE: u32 = 0o600;

/// SPEC: cache.db permissions whitelist. Acceptable modes are strictly 0600 (owner rw)
/// or 0640 (owner rw, group r, matching upstream bbolt on Linux). Group or other write,
/// world-read, execute bits, and special bits (setuid/setgid/sticky) are rejected.
#[cfg(unix)]
pub(crate) fn cache_db_mode_acceptable(mode: u32) -> bool {
    let p = mode & 0o7777;
    p == 0o600 || p == 0o640
}
#[cfg(target_os = "linux")]
const GEO_ASSET_MODE: u16 = 0o640;

/// SPEC A1: bbolt `cache.db` classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDbState {
    Missing,
    Healthy,
    /// One of the two meta pages failed validation — bbolt self-recovers, never isolate.
    Degraded,
    /// Ownership/mode drifted from the service user; content is not judged yet.
    PermissionDrift,
    /// The current identity cannot observe the file at all, so neither ownership nor
    /// content can be judged. Never a drift claim and never grounds for repair.
    Undeterminable,
    /// Both meta pages failed validation (including valid magic with bad version/checksum).
    ContentInvalid,
    /// No valid meta in the probe range: foreign byte order, all-zero, truncated or not a regular file.
    Unrecognized,
    /// Zero length — bbolt re-initializes it as a brand new database.
    Transient,
}

impl CacheDbState {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheDbState::Missing => "missing",
            CacheDbState::Healthy => "healthy",
            CacheDbState::Degraded => "degraded",
            CacheDbState::PermissionDrift => "permission_drift",
            CacheDbState::Undeterminable => "undeterminable",
            CacheDbState::ContentInvalid => "content_invalid",
            CacheDbState::Unrecognized => "unrecognized",
            CacheDbState::Transient => "transient",
        }
    }

    /// The Core calls `os.Remove` on such a file (`initCache` on `ErrInvalid`), so it
    /// must be quarantined before the Core is allowed to open it.
    pub fn requires_quarantine(self) -> bool {
        matches!(
            self,
            CacheDbState::ContentInvalid | CacheDbState::Unrecognized
        )
    }
}

/// SPEC A1: a verdict plus the upstream openability premise (two pages and a
/// complete first-page read). A file that validates but cannot be opened is not
/// `Healthy`: bbolt's `getPageSize` would fail and `initCache` would delete it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheDbClassification {
    pub state: CacheDbState,
    pub core_openable: bool,
}

impl CacheDbClassification {
    /// True when the file must be moved aside before the Core is allowed to start.
    pub fn needs_isolation(self) -> bool {
        match self.state {
            CacheDbState::ContentInvalid | CacheDbState::Unrecognized => true,
            CacheDbState::Degraded => !self.core_openable,
            _ => false,
        }
    }

    /// the bytes are worth copying aside before any Core can open the
    /// file — either already judged unsafe (`needs_isolation`) or **not yet judged**
    /// because `PermissionDrift` short-circuits the content verdict. The unjudged
    /// side is what the copy exists for: a permission repair can hand a corrupt
    /// database to a Core that deletes it.
    pub fn needs_evidence_preservation(self) -> bool {
        self.needs_isolation() || self.state == CacheDbState::PermissionDrift
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoltMetaProbe {
    Valid {
        page_size: u32,
    },
    /// Magic absent in native byte order (all-zero, truncation, foreign layout).
    NoMagic,
    /// Magic present in the other byte order — a database written on a different endianness.
    ForeignByteOrder,
    /// Magic present but version/checksum fail — the exact `ErrInvalid` trigger.
    InvalidMeta,
}

fn bolt_meta_fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn probe_bolt_meta(buffer: &[u8], page_offset: usize) -> Option<BoltMetaProbe> {
    let start = page_offset + BOLT_PAGE_HEADER_SIZE;
    let end = start + BOLT_META_SIZE;
    if buffer.len() < end {
        return None;
    }
    let meta = &buffer[start..end];
    let mut word = [0u8; 4];
    word.copy_from_slice(&meta[0..4]);
    if u32::from_ne_bytes(word) != BOLT_META_MAGIC {
        return Some(if u32::from_be_bytes(word) == BOLT_META_MAGIC {
            BoltMetaProbe::ForeignByteOrder
        } else {
            BoltMetaProbe::NoMagic
        });
    }
    word.copy_from_slice(&meta[4..8]);
    if u32::from_ne_bytes(word) != BOLT_META_VERSION {
        return Some(BoltMetaProbe::InvalidMeta);
    }
    word.copy_from_slice(&meta[8..12]);
    let page_size = u32::from_ne_bytes(word);
    let mut checksum = [0u8; 8];
    checksum.copy_from_slice(&meta[56..64]);
    if u64::from_ne_bytes(checksum) != bolt_meta_fnv1a64(&meta[..56]) {
        return Some(BoltMetaProbe::InvalidMeta);
    }
    Some(BoltMetaProbe::Valid { page_size })
}

#[cfg(unix)]
fn open_regular_read_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(unix))]
fn open_regular_read_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn read_bolt_prefix(file: &std::fs::File, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = file;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let read = file.read(&mut buffer[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// Resolve the identity `cache.db` must be owned by. `None` means "the current
/// effective user", which is what the daemon (and the Core it spawns) runs as.
/// An unknown service user yields `None`: the ownership dimension is skipped
/// instead of guessing.
#[cfg(unix)]
fn cache_db_expected_owner(service_user: Option<&str>) -> Option<(u32, u32)> {
    let Some(name) = service_user else {
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        return Some((uid, gid));
    };
    let user_name = std::ffi::CString::new(name).ok()?;
    let user = unsafe { libc::getpwnam(user_name.as_ptr()) };
    if user.is_null() {
        return None;
    }
    let uid = unsafe { (*user).pw_uid };
    let mut gid = unsafe { (*user).pw_gid };
    let group_name = std::ffi::CString::new(name).ok()?;
    let group = unsafe { libc::getgrnam(group_name.as_ptr()) };
    if !group.is_null() {
        gid = unsafe { (*group).gr_gid };
    }
    Some((uid, gid))
}

/// SPEC A1: classify `cache.db` against the bbolt on-disk layout without
/// writing anything. Permission drift is reported before any content verdict so
/// a combined anomaly is repaired first and re-classified afterwards.
pub fn classify_cache_db(path: &Path, service_user: Option<&str>) -> CacheDbState {
    classify_cache_db_detailed(path, service_user).state
}

/// SPEC A1: classification plus the upstream openability premise. `Healthy`
/// is only claimed when bbolt could actually open the file — two pages of at
/// least `BOLT_MIN_PAGE_SIZE` and a complete first-page read. Anything else that
/// still validates degrades, so it is never handed to the Core as "fine".
pub fn classify_cache_db_detailed(
    path: &Path,
    service_user: Option<&str>,
) -> CacheDbClassification {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CacheDbClassification {
                state: CacheDbState::Missing,
                core_openable: false,
            }
        }
        // "Cannot look" is not "is wrong": only a successful stat can prove drift.
        Err(_) => {
            return CacheDbClassification {
                state: CacheDbState::Undeterminable,
                core_openable: false,
            }
        }
    };
    if !metadata.is_file() {
        return CacheDbClassification {
            state: CacheDbState::Unrecognized,
            core_openable: false,
        };
    }

    #[cfg(unix)]
    if let Some((uid, gid)) = cache_db_expected_owner(service_user) {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != uid
            || metadata.gid() != gid
            || !cache_db_mode_acceptable(metadata.mode())
        {
            return CacheDbClassification {
                state: CacheDbState::PermissionDrift,
                core_openable: false,
            };
        }
    }
    #[cfg(not(unix))]
    let _ = service_user;

    let len = metadata.len();
    if len == 0 {
        return CacheDbClassification {
            state: CacheDbState::Transient,
            core_openable: true,
        };
    }
    if len < BOLT_MIN_LAYOUT_BYTES {
        return CacheDbClassification {
            state: CacheDbState::Unrecognized,
            core_openable: false,
        };
    }

    let file = match open_regular_read_no_follow(path) {
        Ok(file) => file,
        // Ownership and mode already matched the contract above, so a failed open is
        // an identity limitation (EACCES) rather than drift: repairing could not
        // change anything. The Core runs as the contract owner and reports its own
        // failure if the file really is unopenable.
        Err(_) => {
            return CacheDbClassification {
                state: CacheDbState::Undeterminable,
                core_openable: false,
            }
        }
    };

    let first_len = BOLT_PAGE_READ_BYTES.min(usize::try_from(len).unwrap_or(usize::MAX));
    let first = read_bolt_prefix(&file, 0, first_len).unwrap_or_default();
    let first_page_complete = first.len() == BOLT_PAGE_READ_BYTES;
    let first_probe = probe_bolt_meta(&first, 0);
    let meta0_page_size = match first_probe {
        Some(BoltMetaProbe::Valid { page_size }) => Some(page_size),
        _ => None,
    };

    let mut magic_seen = matches!(first_probe, Some(BoltMetaProbe::InvalidMeta));
    let mut foreign_byte_order = matches!(first_probe, Some(BoltMetaProbe::ForeignByteOrder));
    let mut meta1_page_size = None;

    if meta0_page_size.is_none() {
        // Second meta page: the page size is not assumed to be the OS page size, so
        // probe the same bounded candidate offsets as upstream `getPageSize`.
        for index in 0..BOLT_PAGE_PROBE_CANDIDATES {
            let offset = 1024i64 << index;
            if offset >= len as i64 - 1024 {
                break;
            }
            let offset = offset as u64;
            let remaining = len.saturating_sub(offset);
            if remaining < BOLT_MIN_LAYOUT_BYTES {
                break;
            }
            let read_len =
                BOLT_PAGE_READ_BYTES.min(usize::try_from(remaining).unwrap_or(usize::MAX));
            let buffer = match read_bolt_prefix(&file, offset, read_len) {
                Ok(buffer) => buffer,
                Err(_) => continue,
            };
            match probe_bolt_meta(&buffer, 0) {
                Some(BoltMetaProbe::Valid { page_size }) => {
                    meta1_page_size = Some(page_size);
                    break;
                }
                Some(BoltMetaProbe::InvalidMeta) => magic_seen = true,
                Some(BoltMetaProbe::ForeignByteOrder) => foreign_byte_order = true,
                Some(BoltMetaProbe::NoMagic) | None => {}
            }
        }
    }

    let Some(valid_page_size) = meta0_page_size.or(meta1_page_size) else {
        if foreign_byte_order {
            crate::log!(
                "cache.db {} carries a foreign byte order; it cannot be handed to the Core",
                path.display()
            );
        }
        return CacheDbClassification {
            state: if magic_seen {
                CacheDbState::ContentInvalid
            } else {
                CacheDbState::Unrecognized
            },
            core_openable: false,
        };
    };

    // Upstream `getPageSizeFromFirstMeta` needs one complete 4KiB read, and bbolt
    // needs both meta pages mapped; a shorter file fails `getPageSize` and is then
    // `os.Remove`d by the Core's `initCache`.
    let required_len = 2 * u64::from(valid_page_size.max(BOLT_MIN_PAGE_SIZE as u32));
    let core_openable = first_page_complete && len >= required_len;
    CacheDbClassification {
        state: if meta0_page_size.is_some() && core_openable {
            CacheDbState::Healthy
        } else {
            CacheDbState::Degraded
        },
        core_openable,
    }
}

/// SPEC A2: honest permission repair — `chown`/`chmod` the service user,
/// never touch the content. Requires the kernel to allow it (root, or the
/// current owner for a mode-only drift); real errors are propagated.
pub fn repair_cache_db_permissions(
    path: &Path,
    service_user: Option<&str>,
) -> anyhow::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        let Some((uid, gid)) = cache_db_expected_owner(service_user) else {
            anyhow::bail!(
                "cannot resolve the mihomo service identity for {}",
                path.display()
            );
        };
        let file = open_regular_read_no_follow(path).map_err(|error| {
            anyhow::anyhow!(
                "cannot open {} for permission repair: {error}",
                path.display()
            )
        })?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            anyhow::bail!("{} is not a regular file", path.display());
        }
        let mut changed = false;
        if metadata.uid() != uid || metadata.gid() != gid {
            if unsafe { libc::fchown(file.as_raw_fd(), uid, gid) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            changed = true;
        }
        if !cache_db_mode_acceptable(metadata.mode()) {
            if unsafe { libc::fchmod(file.as_raw_fd(), CACHE_DB_MODE as libc::mode_t) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            changed = true;
        }
        Ok(changed)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, service_user);
        Ok(false)
    }
}

/// Linux `renameat2(RENAME_NOREPLACE)` through the already validated parent fds.
///
/// Invoked as a raw `SYS_renameat2` syscall instead of libc's `renameat2`
/// wrapper: musl (one of the release targets in `build.sh`) does not export
/// that symbol, which broke the musl link. An `ENOSYS` kernel (or any other
/// failure) still surfaces as an error and keeps the caller's fail-closed
/// behaviour: no replaceable fallback is introduced.
#[cfg(target_os = "linux")]
fn rename_no_replace_at(
    from_dirfd: std::os::fd::RawFd,
    from: &std::ffi::CStr,
    to_dirfd: std::os::fd::RawFd,
    to: &std::ffi::CStr,
) -> std::io::Result<()> {
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            from_dirfd as libc::c_long,
            from.as_ptr() as libc::c_long,
            to_dirfd as libc::c_long,
            to.as_ptr() as libc::c_long,
            libc::RENAME_NOREPLACE as libc::c_long,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// macOS/no-`renameat2` unix fallbacks are handled in `rename_no_replace`.
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn rename_no_replace_unsupported() -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
}

/// macOS `renamex_np` resolves both operands against the CWD, so it must receive
/// the full paths, never bare file names.
#[cfg(all(unix, any(target_os = "macos", test)))]
fn rename_no_replace_c_paths(
    from: &Path,
    to: &Path,
) -> anyhow::Result<(std::ffi::CString, std::ffi::CString)> {
    use std::os::unix::ffi::OsStrExt;

    let from_c = std::ffi::CString::new(from.as_os_str().as_bytes())
        .map_err(|error| anyhow::anyhow!("invalid source path: {error}"))?;
    let to_c = std::ffi::CString::new(to.as_os_str().as_bytes())
        .map_err(|error| anyhow::anyhow!("invalid destination path: {error}"))?;
    Ok((from_c, to_c))
}

/// SPEC A2: atomic no-replace publish. There is deliberately **no**
/// replaceable fallback: when the platform primitive is unavailable the caller
/// must fail closed instead of risking evidence loss.
#[cfg(unix)]
pub fn rename_no_replace(from: &Path, to: &Path) -> anyhow::Result<()> {
    use std::os::unix::ffi::OsStrExt;

    // Both parents are opened no-follow first: the rename must not be redirected
    // through a symlinked directory, and the kernel reports missing write/search
    // permission on the real directory.
    let _from_parent = open_write_parent_dir_no_follow(from)?;
    let _to_parent = open_write_parent_dir_no_follow(to)?;

    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;

        let from_name = from
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", from.display()))?;
        let to_name = to
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("path has no file name: {}", to.display()))?;
        let from_c = std::ffi::CString::new(from_name.as_bytes())
            .map_err(|error| anyhow::anyhow!("invalid file name: {error}"))?;
        let to_c = std::ffi::CString::new(to_name.as_bytes())
            .map_err(|error| anyhow::anyhow!("invalid file name: {error}"))?;
        rename_no_replace_at(
            _from_parent.0.as_raw_fd(),
            &from_c,
            _to_parent.0.as_raw_fd(),
            &to_c,
        )?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let (from_c, to_c) = rename_no_replace_c_paths(from, to)?;
        let result = unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().into())
        }
    }

    #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
    {
        rename_no_replace_unsupported().map_err(|error| error.into())
    }
}

#[cfg(not(unix))]
pub fn rename_no_replace(from: &Path, to: &Path) -> anyhow::Result<()> {
    anyhow::bail!(
        "atomic no-replace rename is unavailable on this platform ({} -> {})",
        from.display(),
        to.display()
    )
}

fn error_is_already_exists(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .and_then(|error| error.raw_os_error())
        .is_some_and(|code| {
            #[cfg(unix)]
            {
                code == libc::EEXIST
            }
            #[cfg(not(unix))]
            {
                let _ = code;
                false
            }
        })
}

/// SPEC A2: move `cache.db` to an unforgeable `cache.db.corrupt-<ts>-<seq>`
/// name, then trim the quarantine set to `CACHE_DB_QUARANTINE_RETENTION`.
pub fn quarantine_cache_db(path: &Path) -> anyhow::Result<PathBuf> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cache.db path has no parent: {}", path.display()))?;
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let destination = publish_quarantine(
        path,
        dir,
        &timestamp,
        CACHE_DB_QUARANTINE_MAX_ATTEMPTS,
        rename_no_replace,
    )?;
    prune_cache_db_quarantine(dir);
    Ok(destination)
}

/// SPEC P0 /: copies `cache.db` aside as evidence **without moving
/// it or changing it**.
///
/// The upgrade pipeline must preserve the pre-upgrade bytes before any Core can
/// open (and delete) a broken database. The evidence face covers both a corrupt
/// content verdict and the unjudged overlay where `PermissionDrift` short-circuits
/// the content verdict (a drifted file may also be corrupt and the Core deletes it
/// once the permission repair lets it open the file). Unlike runtime quarantine it
/// must not mutate the live file: the copy is staged under a non-final name
/// (`.partial`), fsynced, published with a no-replace rename to
/// `cache.db.corrupt-<ts>-<seq>` and then trimmed to `K=3`.
pub fn preserve_cache_db_evidence(path: &Path) -> anyhow::Result<PathBuf> {
    use std::io::{Read, Write};

    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cache.db path has no parent: {}", path.display()))?;
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    let mut source = open_regular_file_no_follow(path)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    source
        .read_to_end(&mut bytes)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;

    // Idempotent: identical bytes already preserved (e.g. re-entry after a crash
    // between the copy and its consumer) reuse the existing artifact.
    if let Some(existing) = find_identical_evidence(dir, &bytes) {
        return Ok(existing);
    }

    let mut seq = next_quarantine_seq(dir, &timestamp);
    let mut last_error: Option<anyhow::Error> = None;
    for _ in 0..CACHE_DB_QUARANTINE_MAX_ATTEMPTS {
        let staged = dir.join(format!(
            ".{CACHE_DB_QUARANTINE_PREFIX}{timestamp}-{seq:04}.partial"
        ));
        let staged_result = (|| -> anyhow::Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = staged_result {
            return Err(anyhow::anyhow!(
                "cannot stage evidence copy for {}: {error}",
                path.display()
            ));
        }

        let destination = dir.join(format!("{CACHE_DB_QUARANTINE_PREFIX}{timestamp}-{seq:04}"));
        match rename_no_replace(&staged, &destination) {
            Ok(()) => {
                prune_cache_db_quarantine(dir);
                return Ok(destination);
            }
            Err(error) if error_is_already_exists(&error) => {
                // An unfinished copy never carries the final name.
                let _ = std::fs::remove_file(&staged);
                seq += 1;
                last_error = Some(error);
            }
            Err(error) => {
                let _ = std::fs::remove_file(&staged);
                return Err(anyhow::anyhow!(
                    "cannot publish evidence copy for {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("could not allocate an evidence name for {}", path.display())
    }))
}

/// Returns an existing quarantine artifact whose bytes equal `bytes`, so repeated
/// preservation of the same corrupt file does not burn the K=3 budget.
fn find_identical_evidence(dir: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_quarantine_artifact_name(name) {
            continue;
        }
        let candidate = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() != bytes.len() as u64 {
            continue;
        }
        if std::fs::read(&candidate).ok().as_deref() == Some(bytes) {
            return Some(candidate);
        }
    }
    None
}

/// SPEC P1-1: the generation trust root contract.
///
/// The store authenticates baselines, so it must be unreachable for writes by the
/// service account: `root:root 0750` (macOS: same unix identity/mode; Windows: the
/// installer-owned ProgramData tree) and no ancestor writable by the service
/// account or by everyone. Drift means the store cannot be trusted → refuse.
#[cfg(unix)]
pub fn verify_generation_trust_root(path: &Path) -> anyhow::Result<()> {
    verify_generation_trust_root_with(path, unsafe { libc::geteuid() }, service_account_ids())
}

#[cfg(not(unix))]
pub fn verify_generation_trust_root(_path: &Path) -> anyhow::Result<()> {
    // Windows equivalent: the root lives under the installer-ACL-protected
    // ProgramData tree (see service.rs installer metadata ACL); there is no
    // unix owner/mode to compare here.
    Ok(())
}

#[cfg(unix)]
pub(crate) fn service_account_ids() -> Option<(libc::uid_t, libc::gid_t)> {
    let user = unsafe { libc::getpwnam(c"mihomo".as_ptr()) };
    let group = unsafe { libc::getgrnam(c"mihomo".as_ptr()) };
    if user.is_null() || group.is_null() {
        return None;
    }
    Some((unsafe { (*user).pw_uid }, unsafe { (*group).gr_gid }))
}

/// Testable core of [`verify_generation_trust_root`].
#[cfg(unix)]
pub(crate) fn verify_generation_trust_root_with(
    path: &Path,
    euid: libc::uid_t,
    service_ids: Option<(libc::uid_t, libc::gid_t)>,
) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;

    if !path.exists() {
        return Ok(());
    }
    let dir = open_directory_no_follow(path).map_err(|error| {
        anyhow::anyhow!(
            "generation trust root {} is not a safe directory: {error}",
            path.display()
        )
    })?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(dir.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    let mode = stat.st_mode & 0o777;

    if euid == 0 {
        // Production: root:root 0750, exactly.
        if stat.st_uid != 0 || stat.st_gid != 0 || mode != 0o750 {
            anyhow::bail!(
                "generation trust root {} drifted from root:root 0750 (uid={} gid={} mode={mode:#o}); \
                 generation state cannot be trusted — repair ownership/mode before upgrading",
                path.display(),
                stat.st_uid,
                stat.st_gid
            );
        }
    } else {
        // Non-root invocation (isolated/test context): the service account must
        // still be unable to write, and the current identity must own it.
        if stat.st_uid != euid || mode & 0o022 != 0 {
            anyhow::bail!(
                "generation trust root {} is writable beyond its owner (uid={} mode={mode:#o})",
                path.display(),
                stat.st_uid
            );
        }
    }

    for ancestor in path.ancestors().skip(1) {
        let Ok(metadata) = std::fs::symlink_metadata(ancestor) else {
            continue;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = metadata.mode() & 0o7777;
            let sticky = mode & 0o1000 != 0;
            if mode & 0o002 != 0 && !sticky {
                anyhow::bail!(
                    "generation trust root ancestor {} is world-writable",
                    ancestor.display()
                );
            }
            if let Some((service_uid, service_gid)) = service_ids {
                let gid = metadata.gid();
                let uid = metadata.uid();
                if (mode & 0o020 != 0 && gid == service_gid)
                    || (mode & 0o200 != 0 && uid == service_uid)
                {
                    anyhow::bail!(
                        "generation trust root ancestor {} is writable by the mihomo service account",
                        ancestor.display()
                    );
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = &metadata;
        }
    }
    Ok(())
}

/// SPEC P1-1: creates the trust root with its explicit contract instead of
/// inheriting the process umask (`ensure_dir_all_no_follow` alone yields 0755).
#[cfg(unix)]
pub fn ensure_generation_trust_root(path: &Path) -> anyhow::Result<()> {
    ensure_dir_all_no_follow(path)?;
    set_directory_mode_no_follow(path, 0o750)?;
    if unsafe { libc::geteuid() } == 0 {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|error| anyhow::anyhow!("invalid trust root path: {error}"))?;
        if unsafe { libc::lchown(c_path.as_ptr(), 0, 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    verify_generation_trust_root(path)
}

#[cfg(not(unix))]
pub fn ensure_generation_trust_root(path: &Path) -> anyhow::Result<()> {
    ensure_dir_all_no_follow(path)
}

/// Bounded no-replace publish: `EEXIST` advances the sequence number, any other
/// error (including a missing rename primitive) aborts immediately.
fn publish_quarantine<Ren>(
    from: &Path,
    dir: &Path,
    timestamp: &str,
    attempts: u32,
    rename: Ren,
) -> anyhow::Result<PathBuf>
where
    Ren: Fn(&Path, &Path) -> anyhow::Result<()>,
{
    // Never reuse a sequence number: names must sort in creation order so that
    // retention always evicts the oldest artifact.
    let mut seq = next_quarantine_seq(dir, timestamp);
    for _ in 0..attempts {
        // Zero-pad so lexicographic sort == creation order (seq >= 10 would
        // otherwise sort `-10` before `-9`).
        let destination = dir.join(format!("{CACHE_DB_QUARANTINE_PREFIX}{timestamp}-{seq:04}"));
        match rename(from, &destination) {
            Ok(()) => return Ok(destination),
            Err(error) if error_is_already_exists(&error) => {
                seq += 1;
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    anyhow::bail!(
        "could not allocate an unused quarantine name for {} after {attempts} attempts",
        from.display()
    )
}

fn next_quarantine_seq(dir: &Path, timestamp: &str) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let prefix = format!("{CACHE_DB_QUARANTINE_PREFIX}{timestamp}-");
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let rest = name.to_str()?.strip_prefix(&prefix)?;
            rest.parse::<u32>().ok()
        })
        .max()
        .map(|highest| highest + 1)
        .unwrap_or(0)
}

pub fn is_quarantine_artifact_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(CACHE_DB_QUARANTINE_PREFIX) else {
        return false;
    };
    let Some((timestamp, seq)) = rest.rsplit_once('-') else {
        return false;
    };
    if timestamp.len() != 16
        || !timestamp.as_bytes()[8].eq_ignore_ascii_case(&b'T')
        || !timestamp.as_bytes()[15].eq_ignore_ascii_case(&b'Z')
    {
        return false;
    }
    if !timestamp
        .bytes()
        .enumerate()
        .all(|(index, byte)| index == 8 || index == 15 || byte.is_ascii_digit())
    {
        return false;
    }
    !seq.is_empty() && seq.bytes().all(|byte| byte.is_ascii_digit())
}

/// SPEC A2: keep `K=3` provable quarantine artifacts; the newest successful
/// quarantine triggers the eviction of the oldest ones only.
fn prune_cache_db_quarantine(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut artifacts: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            is_quarantine_artifact_name(name)
                && entry
                    .file_type()
                    .map(|kind| kind.is_file())
                    .unwrap_or(false)
        })
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    artifacts.sort();
    if artifacts.len() <= CACHE_DB_QUARANTINE_RETENTION {
        return;
    }
    let evict = artifacts.len() - CACHE_DB_QUARANTINE_RETENTION;
    for name in artifacts.into_iter().take(evict) {
        match std::fs::remove_file(dir.join(&name)) {
            Ok(()) => {
                crate::log!(
                    "quarantine artifact {} removed after retention eviction",
                    name
                );
                eprintln!(
                    "⚠ quarantine {name} removed (retention K={CACHE_DB_QUARANTINE_RETENTION})"
                );
            }
            Err(error) => {
                crate::log!("failed to evict quarantine artifact {}: {error}", name);
                eprintln!(
                    "⚠ quarantine {name} could not be removed (retention K={CACHE_DB_QUARANTINE_RETENTION}): {error}"
                );
            }
        }
    }
}

// ---------------- cache.db (bbolt) fixtures (SPEC ) ----------------
/// Build a byte image with the exact bbolt layout of
/// `metacubex/bbolt@010dbbbb7a5b`: 16-byte page headers, a 64-byte meta at
/// `page + 16`, FNV-1a 64 checksum over the first 56 meta bytes.
#[cfg(all(test, unix))]
pub(crate) fn bolt_test_db_image(page_size: u32) -> Vec<u8> {
    bolt_test_db_image_with_version(page_size, 2)
}

#[cfg(all(test, unix))]
pub(crate) fn bolt_test_db_image_with_version(page_size: u32, version: u32) -> Vec<u8> {
    fn bolt_page_image(page_size: usize, id: u64, flags: u16, payload: &[u8]) -> Vec<u8> {
        let mut page = vec![0u8; page_size];
        page[0..8].copy_from_slice(&id.to_ne_bytes());
        page[8..10].copy_from_slice(&flags.to_ne_bytes());
        page[10..12].copy_from_slice(&0u16.to_ne_bytes());
        page[12..16].copy_from_slice(&0u32.to_ne_bytes());
        page[16..16 + payload.len()].copy_from_slice(payload);
        page
    }

    fn bolt_meta_payload(page_size: u32, txid: u64, version: u32) -> Vec<u8> {
        let mut meta = vec![0u8; 64];
        meta[0..4].copy_from_slice(&0xED0C_DAEDu32.to_ne_bytes());
        meta[4..8].copy_from_slice(&version.to_ne_bytes());
        meta[8..12].copy_from_slice(&page_size.to_ne_bytes());
        // Layout per metacubex/bbolt@010dbbbb7a5b: page 0/1 = meta, page 2 =
        // freelist, page 3 = root leaf. `pgid` = next allocatable page (4).
        meta[16..24].copy_from_slice(&3u64.to_ne_bytes()); // root bucket pgid
        meta[24..32].copy_from_slice(&0u64.to_ne_bytes()); // root bucket sequence
        meta[32..40].copy_from_slice(&2u64.to_ne_bytes()); // freelist pgid
        meta[40..48].copy_from_slice(&4u64.to_ne_bytes()); // pgid (next allocatable)
        meta[48..56].copy_from_slice(&txid.to_ne_bytes());
        let checksum = bolt_meta_fnv1a64(&meta[..56]);
        meta[56..64].copy_from_slice(&checksum.to_ne_bytes());
        meta
    }

    let size = page_size as usize;
    let mut image = Vec::new();
    image.extend(bolt_page_image(
        size,
        0,
        0x04,
        &bolt_meta_payload(page_size, 1, version),
    ));
    image.extend(bolt_page_image(
        size,
        1,
        0x04,
        &bolt_meta_payload(page_size, 2, version),
    ));
    image.extend(bolt_page_image(size, 2, 0x10, &[]));
    image.extend(bolt_page_image(size, 3, 0x02, &[]));
    image
}

/// Break the checksum of the meta page at `page_offset` while keeping its magic valid.
#[cfg(all(test, unix))]
pub(crate) fn bolt_test_corrupt_meta(image: &mut [u8], page_offset: usize) {
    image[page_offset + BOLT_PAGE_HEADER_SIZE + 16] ^= 0xFF;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_socket_dir_uses_v3_uid_scoped_tmp_dir() {
        assert_eq!(macos_socket_dir(501), "/tmp/mihomo-501");
    }

    #[test]
    fn windows_config_dir_uses_appdata_roaming_per_v3() {
        assert_eq!(
            windows_config_dir(
                Some(PathBuf::from(r"C:\Users\user\AppData\Roaming")),
                Some(PathBuf::from(r"C:\Users\user"))
            ),
            PathBuf::from(r"C:\Users\user\AppData\Roaming").join("mihomo")
        );
    }

    #[test]
    fn windows_config_dir_falls_back_to_home_roaming() {
        assert_eq!(
            windows_config_dir(None, Some(PathBuf::from(r"C:\Users\user"))),
            PathBuf::from(r"C:\Users\user")
                .join("AppData")
                .join("Roaming")
                .join("mihomo")
        );
    }

    // BUG-14 回归：mihomo_path() 必须与 instance.rs planned_paths 的 core_binary
    // 一致（单一事实来源），不得有独立漂移的路径逻辑。
    #[test]
    fn mihomo_path_matches_instance_planned_user_core_binary() {
        let _guard = env_test_lock().lock().unwrap();
        let old = std::env::var_os("MIHOMO_CLI_MIHOMO_PATH");
        // 有 env override 时优先
        unsafe {
            std::env::set_var("MIHOMO_CLI_MIHOMO_PATH", "/custom/mihomo");
        }
        assert_eq!(mihomo_path(), "/custom/mihomo");

        // 无 override 时委托 instance.rs 的 user 模式 core_binary
        unsafe {
            std::env::remove_var("MIHOMO_CLI_MIHOMO_PATH");
        }
        let expected =
            crate::instance::planned_current_context(crate::instance::InstanceMode::User)
                .map(|ctx| ctx.paths.core_binary.display().to_string())
                .unwrap_or_default();
        assert_eq!(
            mihomo_path(),
            expected,
            "mihomo_path() must delegate to instance planned_paths (BUG-14)"
        );
        match old {
            Some(value) => unsafe { std::env::set_var("MIHOMO_CLI_MIHOMO_PATH", value) },
            None => unsafe { std::env::remove_var("MIHOMO_CLI_MIHOMO_PATH") },
        }
    }

    // ── sanitize_url tests ──

    #[test]
    fn sanitize_url_masks_token_param() {
        let url = "https://example.com/sub?token=abc123&format=clash";
        assert_eq!(
            sanitize_url(url),
            "https://example.com/sub?token=***&format=clash"
        );
    }

    #[test]
    fn sanitize_url_masks_api_key() {
        let url = "https://example.com/api?key=secret_value&page=1";
        assert_eq!(sanitize_url(url), "https://example.com/api?key=***&page=1");
    }

    #[test]
    fn sanitize_url_masks_password_param() {
        let url = "https://example.com/login?user=admin&password=hunter2";
        assert_eq!(
            sanitize_url(url),
            "https://example.com/login?user=admin&password=***"
        );
    }

    #[test]
    fn sanitize_url_masks_multiple_sensitive_params() {
        let url = "https://example.com/api?token=t1&api_key=k2&safe=yes";
        assert_eq!(
            sanitize_url(url),
            "https://example.com/api?token=***&api_key=***&safe=yes"
        );
    }

    #[test]
    fn sanitize_url_handles_no_query_string() {
        let url = "https://example.com/config.yaml";
        assert_eq!(sanitize_url(url), url);
    }

    #[test]
    fn sanitize_url_handles_empty_query() {
        let url = "https://example.com/?";
        assert_eq!(sanitize_url(url), url);
    }

    #[test]
    fn sanitize_url_preserves_fragment() {
        let url = "https://example.com/sub?token=secret#section";
        assert_eq!(
            sanitize_url(url),
            "https://example.com/sub?token=***#section"
        );
    }

    #[test]
    fn sanitize_url_case_insensitive_param_names() {
        let url = "https://example.com/sub?TOKEN=abc&ApiKey=xyz";
        assert_eq!(
            sanitize_url(url),
            "https://example.com/sub?TOKEN=***&ApiKey=***"
        );
    }

    #[test]
    fn sanitize_url_subscription_key() {
        let url = "https://example.com/sub?sub_key=mysub&sub-key=mysub2";
        assert_eq!(
            sanitize_url(url),
            "https://example.com/sub?sub_key=***&sub-key=***"
        );
    }

    #[test]
    fn sanitize_url_preserves_token_in_path() {
        // Only query params are masked, not path segments
        let url = "https://example.com/token/abc?format=clash";
        assert_eq!(sanitize_url(url), url);
    }

    // ── sanitize_sensitive tests ──

    #[test]
    fn sanitize_sensitive_masks_password_colon() {
        let msg = "connect failed: password: hunter2";
        assert_eq!(sanitize_sensitive(msg), "connect failed: password=***");
    }

    #[test]
    fn sanitize_sensitive_masks_secret_equals() {
        let msg = "config loaded secret=mysecret";
        assert_eq!(sanitize_sensitive(msg), "config loaded secret=***");
    }

    #[test]
    fn sanitize_sensitive_masks_quoted_value() {
        let msg = r#"auth: "bearer_token_123" ok"#;
        assert_eq!(sanitize_sensitive(msg), "auth=*** ok");
    }

    #[test]
    fn sanitize_sensitive_leaves_normal_text() {
        let msg = "subscription refreshed successfully";
        assert_eq!(sanitize_sensitive(msg), msg);
    }

    #[test]
    fn sanitize_sensitive_masks_api_key() {
        let msg = "api_key: sk-1234567890abcdef";
        assert_eq!(sanitize_sensitive(msg), "api_key=***");
    }

    #[test]
    fn canonical_original_user_home_path_allows_within_home_and_rejects_escape() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home/alice");
        let config_dir = home.join(".config/mihomo");
        let subscriptions = config_dir.join("subscriptions");
        std::fs::create_dir_all(&subscriptions).unwrap();
        let active = subscriptions.join("active");
        std::fs::write(&active, "sub-test").unwrap();
        // home 范围内但不在 .config/mihomo 下（如 cache 目录）同样允许
        let cache = home.join(".cache/mihomo-cli");
        std::fs::create_dir_all(&cache).unwrap();
        let escaped = config_dir.join("../../../../etc");

        assert!(canonical_original_user_home_path(&home, &home)
            .unwrap()
            .is_some());
        assert!(canonical_original_user_home_path(&config_dir, &home)
            .unwrap()
            .is_some());
        assert!(canonical_original_user_home_path(&active, &home)
            .unwrap()
            .is_some());
        assert!(canonical_original_user_home_path(&cache, &home)
            .unwrap()
            .is_some());
        assert!(canonical_original_user_home_path(&escaped, &home)
            .unwrap()
            .is_none());

        #[cfg(unix)]
        {
            let outside = temp.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            let escape_link = config_dir.join("escape-link");
            std::os::unix::fs::symlink(&outside, &escape_link).unwrap();
            assert!(canonical_original_user_home_path(&escape_link, &home)
                .unwrap()
                .is_none());

            // 指向 home 内部的 symlink 解析后仍在 home 内：允许（用户自己的树）
            let inner_link = home.join("inner-link");
            std::os::unix::fs::symlink(&config_dir, &inner_link).unwrap();
            assert!(canonical_original_user_home_path(&inner_link, &home)
                .unwrap()
                .is_some());
        }
    }

    #[cfg(unix)]
    #[test]
    fn open_regular_file_no_follow_supports_traverse_only_parent_directory() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let config_dir = home.join(".config/mihomo");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config = config_dir.join("config.yaml");
        std::fs::write(&config, "mixed-port: 7890\n").unwrap();

        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o111)).unwrap();
        let mut file = open_regular_file_no_follow(&config).unwrap();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut file, &mut content).unwrap();
        assert_eq!(content, "mixed-port: 7890\n");

        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn open_path_no_follow_rejects_symlink_components() {
        let temp = tempfile::tempdir().unwrap();
        let safe_dir = temp.path().join("safe");
        std::fs::create_dir(&safe_dir).unwrap();
        let safe_file = safe_dir.join("config.yaml");
        std::fs::write(&safe_file, "mixed-port: 7890").unwrap();
        assert!(open_path_no_follow(&safe_file).is_ok());

        let outside = temp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let outside_file = outside.join("config.yaml");
        std::fs::write(&outside_file, "mixed-port: 7891").unwrap();

        let final_link = safe_dir.join("final-link");
        std::os::unix::fs::symlink(&outside_file, &final_link).unwrap();
        assert!(open_path_no_follow(&final_link).is_err());

        let directory_link = temp.path().join("directory-link");
        std::os::unix::fs::symlink(&outside, &directory_link).unwrap();
        assert!(open_path_no_follow(&directory_link.join("config.yaml")).is_err());
        assert!(open_path_no_follow(&safe_dir.join("../safe/config.yaml")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn no_follow_read_and_atomic_write_reject_ancestor_links_and_fifo() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;
        let temp = tempfile::TempDir::new().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("file"), b"original").unwrap();
        let link = temp.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(open_regular_file_no_follow(&link.join("file")).is_err());
        assert!(atomic_write_bytes_no_follow(&link.join("file"), b"bad", 0o600).is_err());
        symlink(temp.path().join("missing"), temp.path().join("dangling")).unwrap();
        assert!(open_regular_file_no_follow(&temp.path().join("dangling/file")).is_err());
        assert!(
            atomic_write_bytes_no_follow(&temp.path().join("dangling/file"), b"bad", 0o600)
                .is_err()
        );
        assert!(open_regular_file_no_follow(&real.join("file")).is_ok());
        let fifo = real.join("fifo");
        let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(open_regular_file_no_follow(&fifo).is_err());
        assert_eq!(std::fs::read(real.join("file")).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_file_ignores_preplanted_legacy_tmp_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cfg");
        std::fs::create_dir(&dir).unwrap();
        let victim = temp.path().join("victim");
        std::fs::write(&victim, "original").unwrap();
        // 攻击者预置 config.yaml.tmp 符号链接指向受害文件
        std::os::unix::fs::symlink(&victim, dir.join("config.yaml.tmp")).unwrap();

        let target = dir.join("config.yaml");
        atomic_write_file(&target.display().to_string(), "replacement").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "original");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "replacement");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn atomic_write_owned_publishes_service_owner_and_mode() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("tun-config.yaml");
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        atomic_write_bytes_no_follow_owned(&target, b"tun:\n  enable: true\n", 0o640, uid, gid)
            .unwrap();
        let metadata = std::fs::metadata(&target).unwrap();
        assert_eq!(metadata.uid(), uid);
        assert_eq!(metadata.gid(), gid);
        assert_eq!(metadata.mode() & 0o7777, 0o640);
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(std::fs::read(&target).unwrap(), b"tun:\n  enable: true\n");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn atomic_write_owned_failure_keeps_previous_file() {
        use std::os::unix::fs::MetadataExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root can chown to any uid; the failure path needs an unprivileged writer.
        }
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("tun-config.yaml");
        std::fs::write(&target, b"old").unwrap();
        let before = std::fs::metadata(&target).unwrap();
        assert!(
            atomic_write_bytes_no_follow_owned(&target, b"new", 0o640, 1, 1).is_err(),
            "chown to a foreign uid must fail without privileges"
        );
        let after = std::fs::metadata(&target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(after.uid(), before.uid());
        assert_eq!(after.gid(), before.gid());
        assert_eq!(after.mode() & 0o777, before.mode() & 0o777);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn atomic_write_owned_is_not_silently_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("snapshot");
        std::fs::write(&target, b"old").unwrap();
        assert!(atomic_write_bytes_no_follow_owned(&target, b"new", 0o640, 1, 1).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"old");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_file_writes_through_real_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cfg");
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("config.yaml");

        atomic_write_file(&target.display().to_string(), "mixed-port: 7890").unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "mixed-port: 7890"
        );
        assert!(!dir.join("config.yaml.tmp").exists());

        // 旧版固定 .tmp 不参与新事务，也不能影响目标文件。
        std::fs::write(dir.join("config.yaml.tmp"), "stale").unwrap();
        atomic_write_file(&target.display().to_string(), "mixed-port: 7891").unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "mixed-port: 7891"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("config.yaml.tmp")).unwrap(),
            "stale"
        );
        let random_tmp_count = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".config.yaml.tmp-")
            })
            .count();
        assert_eq!(random_tmp_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn remove_file_if_exists_unlinks_through_real_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cfg");
        std::fs::create_dir(&dir).unwrap();
        let target = dir.join("active");
        std::fs::write(&target, "sub-1").unwrap();

        remove_file_if_exists(&target).unwrap();
        assert!(!target.exists());
        // 不存在时静默成功
        remove_file_if_exists(&target).unwrap();
        // 父目录不存在时也静默成功
        remove_file_if_exists(&temp.path().join("missing/dir/file")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rename_no_follow_moves_across_real_parent_directories() {
        let temp = tempfile::tempdir().unwrap();
        let source_parent = temp.path().join("active");
        let destination_parent = temp.path().join("gc");
        std::fs::create_dir(&source_parent).unwrap();
        std::fs::create_dir(&destination_parent).unwrap();
        let source = source_parent.join("transaction");
        let destination = destination_parent.join("completed-transaction");
        std::fs::write(&source, "journal").unwrap();

        rename_no_follow(&source, &destination).unwrap();

        assert!(!source.exists());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "journal");
    }

    #[cfg(unix)]
    #[test]
    fn remove_path_no_follow_removes_directory_containing_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let gc_dir = temp.path().join("gc");
        let target_dir = gc_dir.join("tx-123");
        std::fs::create_dir_all(&target_dir).unwrap();

        let dummy_file = temp.path().join("dummy");
        std::fs::write(&dummy_file, "data").unwrap();
        let link_inside = target_dir.join("symlink-inside");
        std::os::unix::fs::symlink(&dummy_file, &link_inside).unwrap();

        let regular_inside = target_dir.join("regular");
        std::fs::write(&regular_inside, "reg").unwrap();

        remove_path_no_follow(&target_dir).unwrap();
        assert!(!target_dir.exists());
        assert!(dummy_file.exists()); // target must not be affected
    }

    #[cfg(unix)]
    #[test]
    fn remove_empty_dir_no_follow_removes_only_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        let empty = temp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        remove_empty_dir_no_follow(&empty).unwrap();
        assert!(!empty.exists());

        let non_empty = temp.path().join("non-empty");
        std::fs::create_dir(&non_empty).unwrap();
        std::fs::write(non_empty.join("evidence"), b"keep").unwrap();
        assert!(remove_empty_dir_no_follow(&non_empty).is_err());
        assert_eq!(std::fs::read(non_empty.join("evidence")).unwrap(), b"keep");

        let target = temp.path().join("target-empty");
        std::fs::create_dir(&target).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(remove_empty_dir_no_follow(&link).is_err());
        assert!(target.exists());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn ensure_dir_all_no_follow_creates_missing_components() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("a/b/c");
        ensure_dir_all_no_follow(&nested).unwrap();
        assert!(nested.is_dir());
        // 幂等
        ensure_dir_all_no_follow(&nested).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ensure_dir_all_no_follow_rejects_file_and_dangling_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("regular");
        std::fs::write(&file, "x").unwrap();
        assert!(ensure_dir_all_no_follow(&file).is_err());

        let dangling = temp.path().join("dangling");
        std::os::unix::fs::symlink(temp.path().join("missing-target"), &dangling).unwrap();
        assert!(ensure_dir_all_no_follow(&dangling.join("child")).is_err());
    }

    /// 锁描述符必须带 `FD_CLOEXEC`，否则 fork 后的子进程 exec 仍持有锁，
    /// 只 close 不解锁的 guard 会把锁留到子进程退出。
    #[cfg(unix)]
    #[test]
    fn open_file_create_no_follow_sets_cloexec() {
        use std::os::fd::AsRawFd;

        let temp = tempfile::tempdir().unwrap();
        let file = open_file_create_no_follow(&temp.path().join("lock")).unwrap();
        let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0, "fcntl(F_GETFD) failed");
        assert_ne!(
            flags & libc::FD_CLOEXEC,
            0,
            "锁描述符缺少 FD_CLOEXEC，会被 exec 出的子进程继承"
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_file_create_no_follow_preserves_not_found_error() {
        let temp = tempfile::tempdir().unwrap();
        let error = open_file_create_no_follow(&temp.path().join("missing/lock")).unwrap_err();

        assert!(is_not_found_error(&error), "unexpected error: {error:#}");
    }

    #[cfg(unix)]
    #[test]
    fn append_truncate_no_follow_reject_preplanted_part_symlink() {
        // 下载 .part / geo 暂存文件可能被攻击者预置为符号链接：
        // append/truncate 打开必须拒绝（ELOOP），不得写入链接目标。
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cfg");
        std::fs::create_dir(&dir).unwrap();
        let victim = temp.path().join("victim");
        std::fs::write(&victim, "original").unwrap();

        for name in ["mihomo.part", "geo.part"] {
            let link = dir.join(name);
            std::os::unix::fs::symlink(&victim, &link).unwrap();
            assert!(open_append_file_no_follow(&link).is_err(), "{name} append");
            assert!(
                open_truncate_file_no_follow(&link).is_err(),
                "{name} truncate"
            );
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "original");
            std::fs::remove_file(&link).unwrap();
        }

        // 正常文件可用
        let ok = dir.join("real.part");
        let mut file = open_append_file_no_follow(&ok).unwrap();
        use std::io::Write;
        file.write_all(b"chunk").unwrap();
        drop(file);
        assert_eq!(std::fs::read_to_string(&ok).unwrap(), "chunk");
    }

    #[cfg(unix)]
    #[test]
    fn set_file_mode_no_follow_rejects_symlink_without_changing_target() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let victim = temp.path().join("victim");
        std::fs::write(&victim, "original").unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = temp.path().join("mihomo");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        assert!(set_file_mode_no_follow(&link, 0o755).is_err());
        assert_eq!(
            std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn set_file_mode_no_follow_rejects_hard_link_without_changing_target() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let victim = temp.path().join("victim");
        std::fs::write(&victim, "original").unwrap();
        std::fs::set_permissions(&victim, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = temp.path().join("mihomo");
        std::fs::hard_link(&victim, &link).unwrap();

        assert!(set_file_mode_no_follow(&link, 0o755).is_err());
        assert_eq!(
            std::fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn file_contents_equal_and_fingerprint_test() {
        let dir = tempfile::tempdir().unwrap();
        let file_a = dir.path().join("a.bin");
        let file_b = dir.path().join("b.bin");
        let file_c = dir.path().join("c.bin");
        let empty_1 = dir.path().join("e1.bin");
        let empty_2 = dir.path().join("e2.bin");

        std::fs::write(&file_a, b"hello world mihomo").unwrap();
        std::fs::write(&file_b, b"hello world mihomo").unwrap();
        std::fs::write(&file_c, b"hello world different").unwrap();
        std::fs::write(&empty_1, b"").unwrap();
        std::fs::write(&empty_2, b"").unwrap();

        assert!(file_contents_equal(&file_a, &file_b));
        assert!(file_contents_equal(&file_a, &file_a));
        assert!(file_contents_equal(&empty_1, &empty_2));
        assert!(!file_contents_equal(&file_a, &file_c));
        assert!(!file_contents_equal(&file_a, &empty_1));
        assert!(!file_contents_equal(
            &file_a,
            &dir.path().join("nonexistent")
        ));

        let fp_a = compute_file_fingerprint(&file_a);
        let fp_b = compute_file_fingerprint(&file_b);
        let fp_c = compute_file_fingerprint(&file_c);
        assert!(fp_a.is_some());
        assert_eq!(fp_a, fp_b);
        assert_ne!(fp_a, fp_c);
    }

    #[cfg(unix)]
    #[test]
    fn test_set_directory_mode_no_follow_idempotent() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let target_dir = temp.path().join("sub_dir");
        std::fs::create_dir(&target_dir).unwrap();

        // 第一次设置权限为 0750
        set_directory_mode_no_follow(&target_dir, 0o750).unwrap();
        let mode = std::fs::metadata(&target_dir).unwrap().mode();
        assert_eq!(mode & 0o7777, 0o750);

        // 第二次设置相同权限，测试幂等短路
        set_directory_mode_no_follow(&target_dir, 0o750).unwrap();
        let mode = std::fs::metadata(&target_dir).unwrap().mode();
        assert_eq!(mode & 0o7777, 0o750);

        // 测试包含 setgid 位 (0o2750) 时的幂等短路兼容性
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target_dir, std::fs::Permissions::from_mode(0o2750));
        let current_raw = std::fs::metadata(&target_dir).unwrap().mode();
        if current_raw & 0o2000 != 0 {
            // 支持 setgid 的环境下，传入 0o750 应当成功幂等短路且不破坏 setgid
            set_directory_mode_no_follow(&target_dir, 0o750).unwrap();
            let after_mode = std::fs::metadata(&target_dir).unwrap().mode();
            assert_eq!(after_mode & 0o777, 0o750);
            assert_eq!(after_mode & 0o2000, 0o2000);
        }

        // 修改为不同权限 0700
        set_directory_mode_no_follow(&target_dir, 0o700).unwrap();
        let mode = std::fs::metadata(&target_dir).unwrap().mode();
        assert_eq!(mode & 0o7777, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn test_set_directory_mode_no_follow_upgrades_to_setgid() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let target_dir = temp.path().join("sub_dir");
        std::fs::create_dir(&target_dir).unwrap();

        // 初始设置为 0o755
        set_directory_mode_no_follow(&target_dir, 0o755).unwrap();
        let mode = std::fs::metadata(&target_dir).unwrap().mode();
        assert_eq!(mode & 0o777, 0o755);

        // 提升为 0o2755 (包含 setgid 位)
        set_directory_mode_no_follow(&target_dir, 0o2755).unwrap();
        let mode_after = std::fs::metadata(&target_dir).unwrap().mode();
        assert_eq!(mode_after & 0o777, 0o755);
        assert!(mode_has_setgid(mode_after));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_check_system_state_dir_tolerates_missing_transactions() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("mihomo-cli");
        let current_uid = unsafe { libc::geteuid() };
        let current_gid = unsafe { libc::getegid() };
        let probe = || {
            check_system_state_dir_needs_repair_with_identities(
                &state_dir,
                current_uid,
                current_uid,
                current_gid,
            )
        };

        // 1. 状态目录不存在 -> Healthy（没有受管资产）
        assert_eq!(probe(), SystemStateDirVerdict::Healthy);

        // 2. 状态目录存在（模拟正确的 0o770），transactions 目录尚不存在 -> Healthy（容忍缺失）
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert_eq!(probe(), SystemStateDirVerdict::Healthy);

        // 3. transactions 存在且权限合规 (0o750) -> Healthy
        let tx = state_dir.join("transactions");
        std::fs::create_dir(&tx).unwrap();
        std::fs::set_permissions(&tx, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert_eq!(probe(), SystemStateDirVerdict::Healthy);

        // 4. transactions 存在但权限不合规 (如 0o777) -> NeedsRepair
        std::fs::set_permissions(&tx, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(probe(), SystemStateDirVerdict::NeedsRepair);
    }

    /// `transactions/` 及其 gc/active 子目录的合格集为 `{0750, 2750}`——setgid
    /// 是写入路径刻意设置的组继承位，其余特殊位（setuid/sticky）与任何权限位漂移都不合格。
    #[cfg(target_os = "linux")]
    #[test]
    fn state_dir_transactions_mode_whitelist_allows_setgid_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("mihomo-cli");
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        let probe =
            || check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid);

        let assets = ["transactions", "transactions/gc", "transactions/active"];
        for asset in assets {
            let path = state_dir.join(asset);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        }
        assert_eq!(probe(), SystemStateDirVerdict::Healthy);

        for asset in assets {
            let path = state_dir.join(asset);
            for (mode, expected) in [
                (0o750, SystemStateDirVerdict::Healthy),
                (0o2750, SystemStateDirVerdict::Healthy),
                (0o755, SystemStateDirVerdict::NeedsRepair),
                (0o700, SystemStateDirVerdict::NeedsRepair),
                (0o2755, SystemStateDirVerdict::NeedsRepair),
                (0o4750, SystemStateDirVerdict::NeedsRepair),
                (0o1750, SystemStateDirVerdict::NeedsRepair),
            ] {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
                let applied = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
                if applied != mode {
                    // 少数文件系统静默丢弃特殊位，此时该矩阵项不构成有效证据。
                    eprintln!("skip {asset} {mode:o}: kernel kept {applied:o}");
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750))
                        .unwrap();
                    continue;
                }
                assert_eq!(probe(), expected, "asset {asset} mode {mode:o}");
            }
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
        }
    }

    /// 合格态（含 setgid 2750）修复路径零改写；确证漂移才收敛到写入路径的
    /// canonical mode（`transactions/` 2750，`gc`/`active` 0750）。
    #[cfg(target_os = "linux")]
    #[test]
    fn repair_transaction_directory_keeps_qualified_mode_and_fixes_drift() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let tx = temp.path().join("transactions");
        std::fs::create_dir(&tx).unwrap();
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        let mode_of = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;

        std::fs::set_permissions(&tx, std::fs::Permissions::from_mode(0o2750)).unwrap();
        let setgid_supported = mode_of(&tx) & 0o2000 != 0;
        if setgid_supported {
            assert!(!repair_transaction_directory(&tx, uid, gid, 0o2750).unwrap());
            assert_eq!(mode_of(&tx), 0o2750, "合格的 setgid 态不得被 repair 规范化");
        }

        // 0750 也是合格态：修复不得把它「升级」成 2750（避免每次 repair 静默改权限）。
        std::fs::set_permissions(&tx, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(!repair_transaction_directory(&tx, uid, gid, 0o2750).unwrap());
        assert_eq!(mode_of(&tx), 0o750);

        // 确证漂移（0755）修复到写入路径的 canonical 目标。
        std::fs::set_permissions(&tx, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(repair_transaction_directory(&tx, uid, gid, 0o2750).unwrap());
        assert_eq!(mode_of(&tx) & 0o777, 0o750);
        if setgid_supported {
            assert_eq!(mode_of(&tx) & 0o2000, 0o2000);
        }

        // setuid/粘滞位一律不合格，修复同样清除。
        for drift in [0o4750, 0o1750] {
            std::fs::set_permissions(&tx, std::fs::Permissions::from_mode(drift)).unwrap();
            if mode_of(&tx) != drift {
                continue;
            }
            assert!(repair_transaction_directory(&tx, uid, gid, 0o2750).unwrap());
            assert_eq!(mode_of(&tx) & 0o4000, 0);
            assert_eq!(mode_of(&tx) & 0o1000, 0);
            assert_eq!(mode_of(&tx) & 0o777, 0o750);
        }

        // gc/active 的 canonical 目标是 0750（写入路径只 ensure 0750），且 2750 仍合格不改写。
        let gc = temp.path().join("gc");
        std::fs::create_dir(&gc).unwrap();
        std::fs::set_permissions(&gc, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(repair_transaction_directory(&gc, uid, gid, 0o750).unwrap());
        assert_eq!(mode_of(&gc), 0o750);
        std::fs::set_permissions(&gc, std::fs::Permissions::from_mode(0o2750)).unwrap();
        if mode_of(&gc) == 0o2750 {
            assert!(!repair_transaction_directory(&gc, uid, gid, 0o750).unwrap());
            assert_eq!(mode_of(&gc), 0o2750);
        }
    }

    /// issue ：`0770` 契约目录对组外身份 `stat` 可行、`open` 不可行，因此判据必须
    /// 建立在 `stat` 上；契约合规即 `Healthy`，且探测过程不需要任何打开权限。
    #[cfg(target_os = "linux")]
    #[test]
    fn state_dir_probe_only_needs_stat_not_open() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("mihomo-cli");
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid),
            SystemStateDirVerdict::Healthy,
            "a contract-compliant state directory must be healthy"
        );

        // `--x` 目录固定该不对称：`lstat` 成功而 `open(O_DIRECTORY)` 被拒。
        // root 绕过模式位，此时 `open` 也会成功，故仅在非 root 下断言后半段。
        let traverse_only = temp.path().join("traverse-only");
        std::fs::create_dir(&traverse_only).unwrap();
        std::fs::set_permissions(&traverse_only, std::fs::Permissions::from_mode(0o110)).unwrap();
        assert!(std::fs::symlink_metadata(&traverse_only).is_ok());
        if uid != 0 {
            assert!(open_directory_no_follow(&traverse_only).is_err());
        }
        std::fs::set_permissions(&traverse_only, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn state_dir_confirmed_drift_needs_repair() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("mihomo-cli");
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        std::fs::create_dir(&state_dir).unwrap();

        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid),
            SystemStateDirVerdict::NeedsRepair,
            "a mode that drifted from 0770 is a confirmed repair"
        );

        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        let foreign_uid = if uid == 0 { 1 } else { 0 };
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, foreign_uid, uid, gid),
            SystemStateDirVerdict::NeedsRepair,
            "a stat-ed owner that differs from root is a confirmed repair"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn state_dir_cache_db_mode_whitelist() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join("mihomo-cli");
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o770)).unwrap();

        let cache_db = state_dir.join(CACHE_DB_FILE_NAME);
        std::fs::write(&cache_db, b"db").unwrap();

        // 0600 is Healthy
        std::fs::set_permissions(&cache_db, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid),
            SystemStateDirVerdict::Healthy,
            "0600 cache.db must be healthy"
        );

        // 0640 is Healthy
        std::fs::set_permissions(&cache_db, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid),
            SystemStateDirVerdict::Healthy,
            "0640 cache.db must be healthy"
        );

        // 0644 is NeedsRepair (world readable)
        std::fs::set_permissions(&cache_db, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid),
            SystemStateDirVerdict::NeedsRepair,
            "0644 cache.db must need repair"
        );

        // 0660 is NeedsRepair (group writable)
        std::fs::set_permissions(&cache_db, std::fs::Permissions::from_mode(0o660)).unwrap();
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid),
            SystemStateDirVerdict::NeedsRepair,
            "0660 cache.db must need repair"
        );

        // 04600 is NeedsRepair (setuid bit)
        std::fs::set_permissions(&cache_db, std::fs::Permissions::from_mode(0o4600)).unwrap();
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid),
            SystemStateDirVerdict::NeedsRepair,
            "04600 cache.db (setuid) must need repair"
        );

        // Reset mode to 0640 for owner/group mismatch tests
        std::fs::set_permissions(&cache_db, std::fs::Permissions::from_mode(0o640)).unwrap();

        // Wrong UID (service user mismatch) -> NeedsRepair
        let foreign_uid = if uid == 0 { 1 } else { 0 };
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, foreign_uid, gid),
            SystemStateDirVerdict::NeedsRepair,
            "wrong UID on cache.db must need repair"
        );

        // Wrong GID (service group mismatch) -> NeedsRepair
        let foreign_gid = if gid == 0 { 1 } else { 0 };
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, foreign_gid),
            SystemStateDirVerdict::NeedsRepair,
            "wrong GID on cache.db must need repair"
        );
    }

    /// 探测本身不可观测（`stat` 也失败）时必须诚实报 `Undeterminable`，而不是索要提权。
    #[cfg(target_os = "linux")]
    #[test]
    fn state_dir_unobservable_is_undeterminable() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            // root 绕过模式位，构造不出「看不见子项」的夹具。
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("blocked");
        let state_dir = blocked.join("mihomo-cli");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        assert_eq!(
            check_system_state_dir_needs_repair_with_identities(&state_dir, uid, uid, gid),
            SystemStateDirVerdict::Undeterminable,
            "an unobservable path is not evidence of drift"
        );

        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn state_dir_verdict_merge_prefers_confirmed_drift() {
        use SystemStateDirVerdict::{Healthy, NeedsRepair, Undeterminable};

        assert_eq!(Healthy.merge(None), Healthy);
        assert_eq!(Healthy.merge(Some(Healthy)), Healthy);
        assert_eq!(Healthy.merge(Some(Undeterminable)), Undeterminable);
        assert_eq!(Undeterminable.merge(Some(Healthy)), Undeterminable);
        assert_eq!(Undeterminable.merge(Some(NeedsRepair)), NeedsRepair);
        assert_eq!(NeedsRepair.merge(Some(Undeterminable)), NeedsRepair);
    }

    #[cfg(unix)]
    mod cache_db_tests {
        use crate::utils::*;
        use std::path::Path;

        fn write_cache_db_fixture(dir: &Path, bytes: &[u8]) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;

            let path = dir.join(CACHE_DB_FILE_NAME);
            std::fs::write(&path, bytes).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(CACHE_DB_MODE))
                .unwrap();
            path
        }

        fn quarantine_artifacts(dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let name = entry.file_name().to_str().map(str::to_string)?;
                    is_quarantine_artifact_name(&name).then_some(name)
                })
                .collect();
            names.sort();
            names
        }

        #[test]
        fn bolt_fixture_matches_upstream_layout() {
            // Guards the fixture itself: page header 16 bytes, magic at page+16,
            // version 2, FNV-1a 64 checksum over bytes 0..56.
            let image = bolt_test_db_image(4096);
            assert_eq!(
                probe_bolt_meta(&image, 0),
                Some(BoltMetaProbe::Valid { page_size: 4096 })
            );
            assert_eq!(
                probe_bolt_meta(&image, 4096),
                Some(BoltMetaProbe::Valid { page_size: 4096 })
            );
        }

        #[test]
        fn cache_db_healthy_for_both_meta_valid() {
            let temp = tempfile::tempdir().unwrap();
            let path = write_cache_db_fixture(temp.path(), &bolt_test_db_image(4096));
            assert_eq!(classify_cache_db(&path, None), CacheDbState::Healthy);
        }

        #[test]
        fn cache_db_degraded_single_meta_corrupt_foreign_page_size() {
            // 8 KiB pages are not the OS page size: the second meta can only be found
            // by the bounded upstream probe.
            let temp = tempfile::tempdir().unwrap();
            let mut image = bolt_test_db_image(8192);
            bolt_test_corrupt_meta(&mut image, 0);
            let path = write_cache_db_fixture(temp.path(), &image);

            let state = classify_cache_db(&path, None);
            assert_eq!(state, CacheDbState::Degraded);
            assert!(
                !state.requires_quarantine(),
                "bbolt self-recovers a single bad meta"
            );
            assert!(quarantine_artifacts(temp.path()).is_empty());
        }

        #[test]
        fn cache_db_content_invalid_when_both_meta_checksum_fail() {
            let temp = tempfile::tempdir().unwrap();
            let mut image = bolt_test_db_image(4096);
            bolt_test_corrupt_meta(&mut image, 0);
            bolt_test_corrupt_meta(&mut image, 4096);
            let path = write_cache_db_fixture(temp.path(), &image);

            let state = classify_cache_db(&path, None);
            assert_eq!(state, CacheDbState::ContentInvalid);
            assert!(state.requires_quarantine());
        }

        #[test]
        fn cache_db_content_invalid_on_version_mismatch() {
            // Version 1 is rejected by this fork (version == 2 only), even with a
            // correct checksum — exactly the `ErrInvalid` shape the Core removes.
            let temp = tempfile::tempdir().unwrap();
            let image = bolt_test_db_image_with_version(4096, 1);
            let path = write_cache_db_fixture(temp.path(), &image);
            assert_eq!(classify_cache_db(&path, None), CacheDbState::ContentInvalid);
        }

        #[test]
        fn cache_db_unrecognized_for_all_zero_incident_shape() {
            // field shape: 16 KiB of zeros, root-owned.
            let temp = tempfile::tempdir().unwrap();
            let path = write_cache_db_fixture(temp.path(), &vec![0u8; 16 * 1024]);
            let state = classify_cache_db(&path, None);
            assert_eq!(state, CacheDbState::Unrecognized);
            assert!(state.requires_quarantine());
        }

        #[test]
        fn cache_db_transient_for_zero_length() {
            let temp = tempfile::tempdir().unwrap();
            let path = write_cache_db_fixture(temp.path(), &[]);
            let state = classify_cache_db(&path, None);
            assert_eq!(state, CacheDbState::Transient);
            assert!(!state.requires_quarantine());
        }

        #[test]
        fn cache_db_unrecognized_for_truncated_and_missing_and_non_regular() {
            let temp = tempfile::tempdir().unwrap();
            let path = write_cache_db_fixture(temp.path(), &[0u8; 40]);
            assert_eq!(classify_cache_db(&path, None), CacheDbState::Unrecognized);
            std::fs::remove_file(&path).unwrap();
            assert_eq!(classify_cache_db(&path, None), CacheDbState::Missing);
            std::fs::create_dir(&path).unwrap();
            assert_eq!(classify_cache_db(&path, None), CacheDbState::Unrecognized);
        }

        /// issue ：`stat` 都被拒时只能报 `Undeterminable`——"我读不到"不是"它不对"，
        /// 既不得声称属主/mode 漂移，也不得据此索要提权或隔离文件。
        #[test]
        fn cache_db_unobservable_path_is_undeterminable_not_drift() {
            use std::os::unix::fs::PermissionsExt;
            if unsafe { libc::geteuid() } == 0 {
                // root 绕过模式位，构造不出不可观测路径。
                return;
            }

            let temp = tempfile::tempdir().unwrap();
            let blocked = temp.path().join("blocked");
            std::fs::create_dir(&blocked).unwrap();
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
            let path = blocked.join(CACHE_DB_FILE_NAME);

            let classification = classify_cache_db_detailed(&path, None);
            assert_eq!(classification.state, CacheDbState::Undeterminable);
            assert_ne!(classification.state, CacheDbState::PermissionDrift);
            assert!(!classification.core_openable);
            assert!(!classification.needs_isolation());

            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        #[test]
        fn cache_db_permission_drift_repairs_mode_only() {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};

            let temp = tempfile::tempdir().unwrap();
            let path = write_cache_db_fixture(temp.path(), &bolt_test_db_image(4096));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let before = std::fs::read(&path).unwrap();

            assert_eq!(
                classify_cache_db(&path, None),
                CacheDbState::PermissionDrift
            );
            assert!(repair_cache_db_permissions(&path, None).unwrap());
            assert_eq!(
                std::fs::metadata(&path).unwrap().mode() & 0o777,
                CACHE_DB_MODE
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "content must not change"
            );
            assert!(
                quarantine_artifacts(temp.path()).is_empty(),
                "a permission repair must never quarantine"
            );
            assert_eq!(classify_cache_db(&path, None), CacheDbState::Healthy);
        }

        /// content corruption + permission drift. The classifier reports
        /// `PermissionDrift` (content unjudged) — the evidence copy must still be
        /// taken, must not touch the original, and must not block repair-first.
        #[test]
        fn permission_drift_with_corrupt_content_preserves_evidence_without_touching_it() {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};

            let temp = tempfile::tempdir().unwrap();
            // Version 9 is an invalid bbolt meta version -> corrupt content.
            let corrupt = bolt_test_db_image_with_version(4096, 9);
            let path = write_cache_db_fixture(temp.path(), &corrupt);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

            let classification = classify_cache_db_detailed(&path, None);
            assert_eq!(
                classification.state,
                CacheDbState::PermissionDrift,
                "permission drift must short-circuit the content verdict"
            );
            assert!(
                classification.needs_evidence_preservation(),
                "an unjudged file must be preserved"
            );
            assert!(
                !classification.needs_isolation(),
                "repair-first contract: permission drift is not quarantined"
            );
            let preserved = preserve_cache_db_evidence(&path).unwrap();
            assert_eq!(
                std::fs::read(&preserved).unwrap(),
                corrupt,
                "the evidence copy must hold the original bytes"
            );
            // The original is a copy source only: same bytes, same mode, still there.
            assert_eq!(std::fs::read(&path).unwrap(), corrupt);
            assert_eq!(
                std::fs::metadata(&path).unwrap().mode() & 0o777,
                0o644,
                "the copy must not repair or otherwise mutate the original"
            );
            assert_eq!(
                classify_cache_db(&path, None),
                CacheDbState::PermissionDrift,
                "the classification must not change because of the copy"
            );
            assert_eq!(
                quarantine_artifacts(temp.path()),
                vec![preserved.file_name().unwrap().to_str().unwrap().to_string()]
            );

            // Idempotent: re-entry reuses the artifact instead of burning K=3.
            assert_eq!(preserve_cache_db_evidence(&path).unwrap(), preserved);

            // Repair-first stays unblocked, and only now does the content verdict
            // become visible — which is exactly why the copy was needed.
            assert!(repair_cache_db_permissions(&path, None).unwrap());
            assert_eq!(
                std::fs::metadata(&path).unwrap().mode() & 0o777,
                CACHE_DB_MODE
            );
            assert_eq!(classify_cache_db(&path, None), CacheDbState::ContentInvalid);
            assert_eq!(preserve_cache_db_evidence(&path).unwrap(), preserved);
            // The predicate is the superset used by the upgrade path: judged-unsafe
            // states ask for the same copy, so switching that call site cannot
            // regress them.
            let after_repair = classify_cache_db_detailed(&path, None);
            assert!(after_repair.needs_isolation());
            assert!(after_repair.needs_evidence_preservation());
        }

        #[test]
        fn cache_db_mode_whitelist_matrix() {
            // Whitelisted acceptable modes: 0600 and 0640.
            assert!(cache_db_mode_acceptable(0o600));
            assert!(cache_db_mode_acceptable(0o640));

            // World-readable or other-accessible modes must be rejected.
            assert!(!cache_db_mode_acceptable(0o644));
            assert!(!cache_db_mode_acceptable(0o666));
            assert!(!cache_db_mode_acceptable(0o604));
            assert!(!cache_db_mode_acceptable(0o606));

            // Group-writable modes must be rejected.
            assert!(!cache_db_mode_acceptable(0o660));
            assert!(!cache_db_mode_acceptable(0o670));

            // Missing owner read/write must be rejected.
            assert!(!cache_db_mode_acceptable(0o400));
            assert!(!cache_db_mode_acceptable(0o200));
            assert!(!cache_db_mode_acceptable(0o000));
            assert!(!cache_db_mode_acceptable(0o440));

            // Executable bits must be rejected.
            assert!(!cache_db_mode_acceptable(0o700));
            assert!(!cache_db_mode_acceptable(0o750));
            assert!(!cache_db_mode_acceptable(0o755));

            // Special bits (setuid, setgid, sticky) must be rejected.
            assert!(!cache_db_mode_acceptable(0o4600));
            assert!(!cache_db_mode_acceptable(0o4640));
            assert!(!cache_db_mode_acceptable(0o2600));
            assert!(!cache_db_mode_acceptable(0o2640));
            assert!(!cache_db_mode_acceptable(0o1600));
            assert!(!cache_db_mode_acceptable(0o1640));
        }

        #[test]
        fn cache_db_mode_0640_healthy_and_not_normalized() {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};

            let temp = tempfile::tempdir().unwrap();
            let path = write_cache_db_fixture(temp.path(), &bolt_test_db_image(4096));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

            // 0640 with valid bbolt content is Healthy, not PermissionDrift.
            assert_eq!(classify_cache_db(&path, None), CacheDbState::Healthy);

            // Repairing permissions on an already acceptable 0640 file does nothing.
            // Specifically, it must not normalize 0640 to 0600.
            assert!(!repair_cache_db_permissions(&path, None).unwrap());
            assert_eq!(
                std::fs::metadata(&path).unwrap().mode() & 0o7777,
                0o640,
                "0640 mode must be preserved without normalization"
            );
        }

        #[test]
        fn cache_db_0640_corrupted_content_goes_directly_to_content_governance() {
            use std::os::unix::fs::PermissionsExt;

            let temp = tempfile::tempdir().unwrap();
            // All-zero 16 KiB file with 0640 mode (like real Core after crash/zero-fill).
            let path = write_cache_db_fixture(temp.path(), &vec![0u8; 16 * 1024]);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

            // Because 0640 is whitelisted, the classification directly inspects content
            // and detects corruption, rather than short-circuiting to PermissionDrift.
            let classification = classify_cache_db_detailed(&path, None);
            assert_eq!(classification.state, CacheDbState::Unrecognized);
            assert_ne!(classification.state, CacheDbState::PermissionDrift);
            assert!(!classification.core_openable);
            assert!(
                classification.needs_isolation(),
                "0640 corrupted cache.db must be marked for isolation directly"
            );

            // Directly quarantinable without prior permission repair.
            let destination = quarantine_cache_db(&path).unwrap();
            assert!(!path.exists());
            assert!(destination.exists());

            // Corrupt meta checksum with 0640 mode -> ContentInvalid.
            let mut bad_meta = bolt_test_db_image(4096);
            bad_meta[16 + 12] ^= 0xFF; // corrupt meta0 checksum
            bad_meta[4096 + 16 + 12] ^= 0xFF; // corrupt meta1 checksum
            let path2 = write_cache_db_fixture(temp.path(), &bad_meta);
            std::fs::set_permissions(&path2, std::fs::Permissions::from_mode(0o640)).unwrap();

            let classification2 = classify_cache_db_detailed(&path2, None);
            assert_eq!(classification2.state, CacheDbState::ContentInvalid);
            assert_ne!(classification2.state, CacheDbState::PermissionDrift);
            assert!(classification2.needs_isolation());
        }

        #[test]
        fn cache_db_permission_drift_matrix_and_repair_to_0600() {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};

            let temp = tempfile::tempdir().unwrap();
            let path = write_cache_db_fixture(temp.path(), &bolt_test_db_image(4096));

            for &bad_mode in &[0o644, 0o660, 0o755, 0o400] {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(bad_mode)).unwrap();
                assert_eq!(
                    classify_cache_db(&path, None),
                    CacheDbState::PermissionDrift,
                    "mode {bad_mode:#o} must be detected as PermissionDrift"
                );
                assert!(
                    repair_cache_db_permissions(&path, None).unwrap(),
                    "repair must succeed and report change for {bad_mode:#o}"
                );
                assert_eq!(
                    std::fs::metadata(&path).unwrap().mode() & 0o7777,
                    CACHE_DB_MODE,
                    "repair target must be 0600 for {bad_mode:#o}"
                );
                assert_eq!(classify_cache_db(&path, None), CacheDbState::Healthy);
            }
        }

        #[test]
        fn cache_db_quarantine_is_no_replace_and_retains_three() {
            let temp = tempfile::tempdir().unwrap();
            let mut quarantined_names = Vec::new();
            for _ in 0..5 {
                let path = write_cache_db_fixture(temp.path(), &vec![0u8; 16 * 1024]);
                assert_eq!(classify_cache_db(&path, None), CacheDbState::Unrecognized);
                let destination = quarantine_cache_db(&path).unwrap();
                assert!(!path.exists(), "the original name must be vacated");
                assert!(destination.exists());
                quarantined_names.push(
                    destination
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                );
            }

            let artifacts = quarantine_artifacts(temp.path());
            assert_eq!(
                artifacts.len(),
                CACHE_DB_QUARANTINE_RETENTION,
                "retention keeps exactly K=3 artifacts: {artifacts:?}"
            );
            // Newest three survive; the two oldest are evicted.
            for expected in &quarantined_names[2..] {
                assert!(
                    artifacts.contains(expected),
                    "{expected} should be retained"
                );
            }
            for evicted in &quarantined_names[..2] {
                assert!(!artifacts.contains(evicted), "{evicted} should be evicted");
            }
        }

        #[test]
        fn rename_no_replace_refuses_existing_destination() {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("cache.db");
            let destination = temp.path().join("cache.db.corrupt-20260101T000000Z-0");
            std::fs::write(&source, b"new").unwrap();
            std::fs::write(&destination, b"evidence").unwrap();

            let error = rename_no_replace(&source, &destination).unwrap_err();
            assert!(
                error_is_already_exists(&error),
                "must report EEXIST: {error}"
            );
            assert_eq!(std::fs::read(&source).unwrap(), b"new");
            assert_eq!(
                std::fs::read(&destination).unwrap(),
                b"evidence",
                "existing evidence must never be overwritten"
            );
        }

        #[test]
        fn cache_db_size_matrix_never_reports_unopenable_as_healthy() {
            let temp = tempfile::tempdir().unwrap();
            let image = bolt_test_db_image(4096);

            // 4 pages of 4096: Healthy and openable.
            let path = write_cache_db_fixture(temp.path(), &image);
            let full = classify_cache_db_detailed(&path, None);
            assert_eq!(full.state, CacheDbState::Healthy);
            assert!(full.core_openable);
            assert!(!full.needs_isolation());

            // [80, 2048]: meta0 still validates, but the 4KiB page read is incomplete.
            for len in [80usize, 512, 2048] {
                let path = write_cache_db_fixture(temp.path(), &image[..len]);
                let classification = classify_cache_db_detailed(&path, None);
                assert_ne!(
                    classification.state,
                    CacheDbState::Healthy,
                    "len={len} must never be Healthy"
                );
                assert!(!classification.core_openable, "len={len} cannot be opened");
                assert!(
                    classification.needs_isolation(),
                    "len={len} must be quarantined instead of being handed to the Core"
                );
                assert_eq!(classification.state, CacheDbState::Degraded);
            }

            // [2049, 8191]: first page reads, but two pages do not fit.
            for len in [2049usize, 4096, 6000, 8191] {
                let path = write_cache_db_fixture(temp.path(), &image[..len]);
                let classification = classify_cache_db_detailed(&path, None);
                assert_eq!(
                    classification.state,
                    CacheDbState::Degraded,
                    "len={len} must degrade, never Healthy"
                );
                assert!(!classification.core_openable, "len={len} cannot be opened");
                assert!(classification.needs_isolation());
            }

            // >= 8192 with both metas: Healthy again.
            let path = write_cache_db_fixture(temp.path(), &image[..8192]);
            let classification = classify_cache_db_detailed(&path, None);
            assert_eq!(classification.state, CacheDbState::Healthy);
            assert!(classification.core_openable);
            assert!(!classification.needs_isolation());

            // A larger page size raises the requirement: 8192-byte pages need 16 KiB.
            let wide = bolt_test_db_image(8192);
            let path = write_cache_db_fixture(temp.path(), &wide[..12000]);
            let classification = classify_cache_db_detailed(&path, None);
            assert_eq!(classification.state, CacheDbState::Degraded);
            assert!(!classification.core_openable);
            assert!(classification.needs_isolation());
        }

        #[test]
        fn macos_rename_passes_full_paths_not_basenames() {
            use std::os::unix::ffi::OsStrExt;

            let from = Path::new("/var/lib/mihomo-cli/cache.db");
            let to = Path::new("/var/lib/mihomo-cli/cache.db.corrupt-20260101T000000Z-0");
            let (from_c, to_c) = rename_no_replace_c_paths(from, to).unwrap();
            assert_eq!(
                from_c.to_bytes(),
                from.as_os_str().as_bytes(),
                "renamex_np must receive the full source path, not the basename"
            );
            assert_eq!(
                to_c.to_bytes(),
                to.as_os_str().as_bytes(),
                "renamex_np must receive the full destination path, not the basename"
            );
            assert_ne!(to_c.to_bytes(), b"cache.db.corrupt-20260101T000000Z-0");
        }

        #[test]
        fn cache_db_quarantine_sequence_is_monotonic() {
            let temp = tempfile::tempdir().unwrap();
            let mut sequences = Vec::new();
            for _ in 0..4 {
                let path = write_cache_db_fixture(temp.path(), &vec![0u8; 16 * 1024]);
                let destination = quarantine_cache_db(&path).unwrap();
                let name = destination
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                let sequence = name.rsplit_once('-').unwrap().1.parse::<u32>().unwrap();
                sequences.push(sequence);
            }
            // Sequence numbers must never be reused, even when retention frees a name.
            assert!(
                sequences.windows(2).all(|pair| pair[1] > pair[0]),
                "quarantine sequences must be strictly increasing: {sequences:?}"
            );
            assert_eq!(
                quarantine_artifacts(temp.path()).len(),
                CACHE_DB_QUARANTINE_RETENTION
            );
        }

        #[test]
        fn quarantine_publish_gives_up_after_retry_cap() {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join(CACHE_DB_FILE_NAME);
            std::fs::write(&source, b"x").unwrap();
            let attempts = std::cell::Cell::new(0u32);
            let error = publish_quarantine(
                &source,
                temp.path(),
                "20260101T000000Z",
                CACHE_DB_QUARANTINE_MAX_ATTEMPTS,
                |_, _| {
                    attempts.set(attempts.get() + 1);
                    Err(std::io::Error::from_raw_os_error(libc::EEXIST).into())
                },
            )
            .unwrap_err();
            assert_eq!(
                attempts.get(),
                CACHE_DB_QUARANTINE_MAX_ATTEMPTS,
                "EEXIST must stop at the retry cap"
            );
            assert!(error.to_string().contains("after 64 attempts"));

            // Any other error aborts immediately: no fallback publish strategy.
            let attempts = std::cell::Cell::new(0u32);
            let error = publish_quarantine(
                &source,
                temp.path(),
                "20260101T000000Z",
                CACHE_DB_QUARANTINE_MAX_ATTEMPTS,
                |_, _| {
                    attempts.set(attempts.get() + 1);
                    Err(std::io::Error::from_raw_os_error(libc::EACCES).into())
                },
            )
            .unwrap_err();
            assert_eq!(attempts.get(), 1, "non-EEXIST errors must not retry");
            assert!(!error_is_already_exists(&error));
        }

        #[test]
        fn quarantine_artifact_name_matcher_rejects_foreign_names() {
            assert!(is_quarantine_artifact_name(
                "cache.db.corrupt-20260101T000000Z-0"
            ));
            assert!(is_quarantine_artifact_name(
                "cache.db.corrupt-20260101T000000Z-12"
            ));
            assert!(!is_quarantine_artifact_name("cache.db"));
            assert!(!is_quarantine_artifact_name("cache.db.corrupt-0"));
            assert!(!is_quarantine_artifact_name(
                "cache.db.corrupt-20260101T000000Z-"
            ));
            assert!(!is_quarantine_artifact_name(
                "other.corrupt-20260101T000000Z-0"
            ));
        }

        #[test]
        fn quarantine_seq_zero_pad_preserves_lexicographic_order_at_boundary() {
            // P2-2: seq >= 10 must sort after seq 9 lexicographically.
            // Zero-padding to 4 digits ensures `-0009` < `-0010`.
            let temp = tempfile::tempdir().unwrap();
            let mut names = Vec::new();
            for _ in 0..12 {
                let path = write_cache_db_fixture(temp.path(), &vec![0u8; 16 * 1024]);
                let destination = quarantine_cache_db(&path).unwrap();
                names.push(
                    destination
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                );
            }
            let mut sorted = names.clone();
            sorted.sort();
            assert_eq!(
                names, sorted,
                "quarantine names must sort in creation order even past seq 9: {names:?}"
            );
        }
    }
}
