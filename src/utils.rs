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
    #[deprecated(
        since = "0.2.0",
        note = "v1/v2 legacy marker; cleanup only, never mode resolution. Use InstanceContext instead"
    )]
    pub fn legacy_service_mode_path(&self) -> PathBuf {
        self.config_dir.join(".service-mode")
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

/// Deprecated v1/v2 mode marker path; cleanup only, never mode resolution.
#[deprecated(
    since = "0.2.0",
    note = "v1/v2 legacy marker; cleanup only. Use InstanceContext instead"
)]
pub fn legacy_service_mode_path() -> String {
    #[allow(deprecated)]
    AppPaths::from_system()
        .legacy_service_mode_path()
        .display()
        .to_string()
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
}
