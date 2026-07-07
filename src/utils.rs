pub fn mihomo_path() -> String {
    if cfg!(target_os = "windows") {
        let local = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
        format!("{}\\mihomo\\mihomo.exe", local.display())
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        format!("{}/.local/bin/mihomo", home.display())
    }
}

pub fn config_dir() -> String {
    if cfg!(target_os = "windows") {
        let local = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
        format!("{}\\mihomo", local.display())
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        format!("{}/.config/mihomo", home.display())
    }
}

pub fn config_path() -> String {
    format!("{}/config.yaml", config_dir())
}

pub fn start_script_path() -> String {
    format!("{}/start.sh", config_dir())
}

pub fn service_mode_path() -> String {
    format!("{}/.service-mode", config_dir())
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
    format!("{}/mihomo.log", config_dir())
}

// ── Subscription URL management ──
pub fn subscription_urls_path() -> String {
    format!("{}/.subscription-urls", config_dir())
}

/// Read all saved subscription URLs (one per line).
pub fn read_subscription_urls() -> Vec<String> {
    let path = subscription_urls_path();
    if std::path::Path::new(&path).exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let urls: Vec<String> = content.lines()
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
