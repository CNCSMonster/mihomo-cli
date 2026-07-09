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

    pub fn start_script_path(&self) -> PathBuf {
        self.config_dir.join("start.sh")
    }

    pub fn service_mode_path(&self) -> PathBuf {
        self.config_dir.join(".service-mode")
    }

    pub fn log_path(&self) -> PathBuf {
        self.config_dir.join("mihomo.log")
    }

    pub fn subscription_urls_path(&self) -> PathBuf {
        self.config_dir.join(".subscription-urls")
    }

    pub fn rules_path(&self) -> PathBuf {
        self.config_dir.join("rules.yaml")
    }

    pub fn rules_position_path(&self) -> PathBuf {
        self.config_dir.join(".rules-position")
    }

    pub fn isp_cache_path(&self) -> PathBuf {
        self.config_dir.join(".isp_cache")
    }

    pub fn dns_policy_path(&self) -> PathBuf {
        self.config_dir.join("dns-policy.yaml")
    }
}

fn default_config_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        let local = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("C:\\ProgramData"));
        local.join("mihomo")
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        home.join(".config/mihomo")
    }
}

pub fn mihomo_path() -> String {
    if cfg!(target_os = "windows") {
        let local =
            dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
        format!("{}\\mihomo\\mihomo.exe", local.display())
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        format!("{}/.local/bin/mihomo", home.display())
    }
}

pub fn config_dir() -> String {
    AppPaths::from_system().config_dir().display().to_string()
}

pub fn config_path() -> String {
    AppPaths::from_system().config_path().display().to_string()
}

pub fn start_script_path() -> String {
    AppPaths::from_system()
        .start_script_path()
        .display()
        .to_string()
}

pub fn service_mode_path() -> String {
    AppPaths::from_system()
        .service_mode_path()
        .display()
        .to_string()
}

pub fn read_service_mode() -> String {
    std::fs::read_to_string(service_mode_path())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "user".to_string())
}

pub fn write_service_mode(mode: &str) -> anyhow::Result<()> {
    std::fs::write(service_mode_path(), mode)?;
    Ok(())
}

pub fn log_path() -> String {
    AppPaths::from_system().log_path().display().to_string()
}

// ── Subscription URL management ──
pub fn subscription_urls_path() -> String {
    AppPaths::from_system()
        .subscription_urls_path()
        .display()
        .to_string()
}

/// Read all saved subscription URLs (one per line).
pub fn read_subscription_urls() -> Vec<String> {
    let path = subscription_urls_path();
    if std::path::Path::new(&path).exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let urls: Vec<String> = content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && l.starts_with("http"))
            .collect();
        if !urls.is_empty() {
            return urls;
        }
    }
    // Migrate legacy single-URL file
    let legacy = format!("{}/.subscription-url", config_dir());
    if std::path::Path::new(&legacy).exists() {
        if let Ok(content) = std::fs::read_to_string(&legacy) {
            let url = content.trim().to_string();
            if !url.is_empty() && url.starts_with("http") {
                let urls = vec![url];
                let _ = write_subscription_urls(&urls);
                let _ = std::fs::remove_file(&legacy);
                return urls;
            }
        }
    }
    vec![]
}

/// Save all subscription URLs (one per line).
pub fn write_subscription_urls(urls: &[String]) -> anyhow::Result<()> {
    let path = subscription_urls_path();
    std::fs::create_dir_all(config_dir())?;
    std::fs::write(&path, urls.join("\n"))?;
    Ok(())
}

/// Add a new subscription URL (dedup, adds to end).
pub fn add_subscription_url(url: &str) -> anyhow::Result<()> {
    let mut urls = read_subscription_urls();
    if !urls.contains(&url.to_string()) {
        urls.push(url.to_string());
        write_subscription_urls(&urls)?;
    }
    Ok(())
}

/// Remove a subscription URL by index.
pub fn remove_subscription_url(index: usize) -> anyhow::Result<()> {
    let mut urls = read_subscription_urls();
    if index < urls.len() {
        urls.remove(index);
        write_subscription_urls(&urls)?;
    }
    Ok(())
}

// ── Rule management paths ──
#[allow(dead_code)]
pub fn rules_path() -> String {
    AppPaths::from_system().rules_path().display().to_string()
}

#[allow(dead_code)]
pub fn rules_position_path() -> String {
    AppPaths::from_system()
        .rules_position_path()
        .display()
        .to_string()
}

pub fn isp_cache_path() -> String {
    AppPaths::from_system()
        .isp_cache_path()
        .display()
        .to_string()
}

#[allow(dead_code)]
pub fn dns_policy_path() -> String {
    AppPaths::from_system()
        .dns_policy_path()
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
