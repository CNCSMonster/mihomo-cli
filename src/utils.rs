use std::path::{Path, PathBuf};

#[cfg(unix)]
fn mode_has_setgid(mode: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        mode & libc::S_ISGID as u32 != 0
    }
    #[cfg(not(target_os = "macos"))]
    {
        mode & libc::S_ISGID != 0
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
    atomic_write_bytes_impl(Path::new(path), content.as_bytes(), 0o644)
}

/// Atomically write bytes using a no-follow parent traversal and an exclusive,
/// no-follow temporary file.  This is used by privileged transaction writers;
/// unlike `std::fs::write`, it cannot follow a pre-planted symlink.
pub fn atomic_write_bytes_no_follow(path: &Path, bytes: &[u8], mode: u16) -> anyhow::Result<()> {
    atomic_write_bytes_impl(path, bytes, mode)
}

/// Unix：父目录 canonical 后逐组件 O_NOFOLLOW 下降，tmp 文件使用随机名称并以
/// O_EXCL | O_NOFOLLOW 创建，renameat 同目录原子替换。
/// root 代替原始用户写入时，目标 canonical 后必须位于其 home 内（fail-closed）。
#[cfg(unix)]
fn atomic_write_bytes_impl(path: &Path, bytes: &[u8], mode: u16) -> anyhow::Result<()> {
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
    if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } != 0 {
        let err = std::io::Error::last_os_error();
        drop(file);
        let _ = unsafe { libc::unlinkat(dirfd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(err).map_err(|err| {
            anyhow::anyhow!("Failed to chmod temp file for {}: {err}", path.display())
        });
    }
    if let Err(err) = file.write_all(bytes) {
        let _ = unsafe { libc::unlinkat(dirfd.as_raw_fd(), tmp_c.as_ptr(), 0) };
        return Err(err).map_err(|err| {
            anyhow::anyhow!("Failed to write temp file {}: {}", path.display(), err)
        });
    }
    file.sync_all()?;
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
    Ok(())
}

#[cfg(not(unix))]
fn atomic_write_bytes_impl(path: &Path, bytes: &[u8], _mode: u16) -> anyhow::Result<()> {
    let mut temp_path = path.as_os_str().to_os_string();
    temp_path.push(".tmp");
    std::fs::write(&temp_path, bytes).map_err(|e| {
        anyhow::anyhow!(
            "Failed to write temp file {}: {}",
            Path::new(&temp_path).display(),
            e
        )
    })?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to rename {} -> {}: {}",
            Path::new(&temp_path).display(),
            path.display(),
            e
        )
    })?;
    Ok(())
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
    atomic_write_bytes_impl(path, content.as_bytes(), mode)?;
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

/// Unix：打开 `path` 的父目录（canonical 后逐组件 O_NOFOLLOW），返回目录 fd 与文件名。
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
    let canonical_parent = parent.canonicalize().map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!("Failed to canonicalize {}: {}", parent.display(), err),
        )
    })?;
    let fd = open_path_no_follow(&canonical_parent)?;
    Ok((fd, file_name.to_os_string()))
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
        if unsafe { libc::mkdirat(fd.as_raw_fd(), name_c.as_ptr(), 0o755) } != 0 {
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
            libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW,
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
    file.read_to_end(&mut bytes)?;
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
fn canonical_original_user_home_path(path: &Path, home: &Path) -> anyhow::Result<Option<PathBuf>> {
    let canonical_home = home.canonicalize()?;
    let path = canonical_existing_ancestor(path)?;
    if path == canonical_home || path.starts_with(&canonical_home) {
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

/// 系统 daemon 自有运行态目录，不属于原始用户 home 的写入边界。
#[cfg(target_os = "linux")]
fn is_managed_system_runtime_path(path: &Path) -> bool {
    path == Path::new("/var/lib/mihomo-cli") || path.starts_with(Path::new("/var/lib/mihomo-cli/"))
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
        if unsafe { libc::mkdirat(fd.as_raw_fd(), name_c.as_ptr(), 0o755) } != 0
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

    for name in ["transactions", "transactions/gc"] {
        let transaction_path = path.join(name);
        match std::fs::create_dir(&transaction_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let transaction_dir = open_directory_no_follow(&transaction_path).map_err(|error| {
            anyhow::anyhow!(
                "Mihomo TUN transaction directory is unsafe or unavailable; reinstall the system service: {error}"
            )
        })?;
        let mut transaction_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(transaction_dir.as_raw_fd(), transaction_stat.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let transaction_stat = unsafe { transaction_stat.assume_init() };
        let transaction_changed = transaction_stat.st_uid != uid
            || transaction_stat.st_gid != gid
            || (transaction_stat.st_mode & 0o777) != 0o750;
        if transaction_changed
            && (unsafe { libc::fchown(transaction_dir.as_raw_fd(), uid, gid) } != 0
                || unsafe { libc::fchmod(transaction_dir.as_raw_fd(), 0o750) } != 0)
        {
            return Err(std::io::Error::last_os_error().into());
        }
        changed |= transaction_changed;
    }

    let active_transaction_path = path.join("transactions/active");
    match open_directory_no_follow(&active_transaction_path) {
        Ok(active_transaction_dir) => {
            let mut active_transaction_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstat(
                    active_transaction_dir.as_raw_fd(),
                    active_transaction_stat.as_mut_ptr(),
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let active_transaction_stat = unsafe { active_transaction_stat.assume_init() };
            let active_transaction_changed = active_transaction_stat.st_uid != uid
                || active_transaction_stat.st_gid != gid
                || (active_transaction_stat.st_mode & 0o777) != 0o750;
            if active_transaction_changed
                && (unsafe { libc::fchown(active_transaction_dir.as_raw_fd(), uid, gid) } != 0
                    || unsafe { libc::fchmod(active_transaction_dir.as_raw_fd(), 0o750) } != 0)
            {
                return Err(std::io::Error::last_os_error().into());
            }
            changed |= active_transaction_changed;
        }
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
                    || (transaction_file_stat.st_mode & 0o777) != 0o640;
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
                || (snapshot_stat.st_mode & 0o777) != 0o640;
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
    std::fs::rename(from, to)?;
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
    open_path_no_follow(path)
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
            libc::O_RDONLY | libc::O_NOFOLLOW
        } else {
            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW
        };
        #[cfg(target_os = "macos")]
        let flags = if is_last {
            libc::O_RDONLY | libc::O_NOFOLLOW
        } else {
            libc::O_SEARCH | libc::O_NOFOLLOW
        };
        #[cfg(all(not(target_os = "linux"), not(target_os = "macos")))]
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | if is_last { 0 } else { libc::O_DIRECTORY };
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
fn original_user_identity() -> anyhow::Result<Option<(libc::uid_t, libc::gid_t, PathBuf)>> {
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
                Some(PathBuf::from(r"C:\Users\Alice\AppData\Roaming")),
                Some(PathBuf::from(r"C:\Users\Alice")),
            ),
            PathBuf::from(r"C:\Users\Alice\AppData\Roaming").join("mihomo")
        );
    }

    #[test]
    fn windows_config_dir_falls_back_to_home_roaming() {
        assert_eq!(
            windows_config_dir(None, Some(PathBuf::from(r"C:\Users\Alice"))),
            PathBuf::from(r"C:\Users\Alice")
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
        let msg = "api_key: test-placeholder";
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
}
