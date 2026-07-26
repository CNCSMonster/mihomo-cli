use std::path::{Path, PathBuf};

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

    /// Deprecated v1/v2 mode marker path.
    ///
    /// v3 must not use this for mode resolution. It is exposed only so cleanup
    /// paths can remove stale `.service-mode` files left by older releases.
    pub fn legacy_service_mode_path(&self) -> PathBuf {
        self.config_dir.join(".service-mode")
    }

    #[allow(dead_code)]
    pub fn log_path(&self) -> PathBuf {
        self.config_dir.join("mihomo.log")
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

    pub fn override_path(&self) -> PathBuf {
        self.config_dir.join("override.yaml")
    }

    pub fn delay_cache_path(&self) -> PathBuf {
        self.config_dir.join("delay-cache.json")
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
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".config/mihomo")
    }
}

fn windows_config_dir(app_data: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    app_data
        .or_else(|| home.map(|home| home.join("AppData").join("Roaming")))
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("mihomo")
}

pub fn mihomo_path() -> String {
    if let Ok(path) = std::env::var("MIHOMO_CLI_MIHOMO_PATH") {
        if !path.trim().is_empty() {
            return path;
        }
    }

    if cfg!(target_os = "windows") {
        let local =
            dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
        format!("{}\\mihomo\\mihomo.exe", local.display())
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        format!("{}/.local/bin/mihomo", home.display())
    }
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
            // Fallback: get UID from /proc/self/loginuid or use 1000 as default
            let uid = std::fs::read_to_string("/proc/self/loginuid")
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
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

/// Deprecated v1/v2 mode marker path; cleanup only, never mode resolution.
pub fn legacy_service_mode_path() -> String {
    AppPaths::from_system()
        .legacy_service_mode_path()
        .display()
        .to_string()
}

#[allow(dead_code)]
pub fn log_path() -> String {
    AppPaths::from_system().log_path().display().to_string()
}

/// Atomically write a file: write to .tmp then rename.
/// Prevents partial writes from corrupting the file.
pub fn atomic_write_file(path: &str, content: &str) -> anyhow::Result<()> {
    let temp_path = format!("{}.tmp", path);
    std::fs::write(&temp_path, content)
        .map_err(|e| anyhow::anyhow!("Failed to write temp file {}: {}", temp_path, e))?;
    std::fs::rename(&temp_path, path)
        .map_err(|e| anyhow::anyhow!("Failed to rename {} -> {}: {}", temp_path, path, e))?;
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
}
