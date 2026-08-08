use crate::{
    instance::ApiEndpoint,
    utils::{self, AppPaths},
};
use anyhow::Context;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Multi-subscription data structures ──

/// Subscription metadata stored in subscriptions.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionMeta {
    pub id: String,
    pub url: String,
    pub updated: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent_mode: Option<UserAgentMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UserAgentMode {
    Auto,
    Fixed,
}

/// Generate a new subscription ID: `sub-` + 8 hex chars
pub fn generate_subscription_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let hex: String = (0..4).map(|_| format!("{:02x}", rng.gen::<u8>())).collect();
    format!("sub-{hex}")
}

/// Load subscription metadata from subscriptions.yaml
pub fn load_subscriptions_at(paths: &AppPaths) -> anyhow::Result<Vec<SubscriptionMeta>> {
    let path = paths.subscriptions_meta_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;
    let subs: Vec<SubscriptionMeta> =
        serde_yaml::from_str(&content).with_context(|| "Failed to parse subscriptions.yaml")?;
    Ok(subs)
}

/// Save subscription metadata to subscriptions.yaml (atomic write)
pub fn save_subscriptions_at(paths: &AppPaths, subs: &[SubscriptionMeta]) -> anyhow::Result<()> {
    let path = paths.subscriptions_meta_path();
    std::fs::create_dir_all(paths.config_dir())?;
    let content = serde_yaml::to_string(subs)?;
    utils::atomic_write_file(&path.display().to_string(), &content)?;
    Ok(())
}

/// Get the active subscription ID from subscriptions/active
pub fn get_active_id_at(paths: &AppPaths) -> anyhow::Result<Option<String>> {
    let path = paths.active_file_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;
    let id = content.trim().to_string();
    if id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(id))
    }
}

/// Set the active subscription ID in subscriptions/active
pub fn set_active_id_at(paths: &AppPaths, id: &str) -> anyhow::Result<()> {
    let path = paths.active_file_path();
    std::fs::create_dir_all(paths.subscriptions_dir())?;
    utils::atomic_write_file(&path.display().to_string(), id)?;
    Ok(())
}

/// Find a subscription by ID
pub fn find_subscription<'a>(
    subs: &'a [SubscriptionMeta],
    id: &str,
) -> Option<&'a SubscriptionMeta> {
    subs.iter().find(|s| s.id == id)
}

// ── Phase 4: Subscription CRUD ──

/// Add a new subscription: download → save → update metadata → set active if first
pub async fn add_subscription_at(paths: &AppPaths, url: &str) -> anyhow::Result<String> {
    add_subscription_at_with_user_agent(paths, url, None, None).await
}

/// Add a subscription with optional user-agent and explicit activate control.
/// `activate`: Some(true) = force activate, Some(false) = force skip, None = auto (first only).
pub async fn add_subscription_at_with_user_agent(
    paths: &AppPaths,
    url: &str,
    user_agent: Option<String>,
    activate: Option<bool>,
) -> anyhow::Result<String> {
    // Download subscription content. Auto mode tries a small Clash-UA set and stops at first YAML.
    let result = download_sub_with_user_agent(url, user_agent.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("Cannot reach subscription URL.\n  {e}"))?;
    let content = result.content;
    let is_yaml = result.is_clash_yaml;

    // Convert if needed
    let yaml_content = if is_yaml {
        content
    } else {
        warn_raw_subscription_conversion();
        crate::log!("Converting subscription format (vmess/base64 → Clash YAML)...");
        convert_vmess_to_clash(&content)?
    };

    // Validate
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).context("Subscription content is not valid YAML")?;
    validate_subscription_yaml(&yaml)?;

    // Generate ID and save
    let id = generate_subscription_id();
    let sub_path = paths.subscription_file_path(&id);
    std::fs::create_dir_all(paths.subscriptions_dir())?;
    utils::atomic_write_file(&sub_path.display().to_string(), &yaml_content)?;

    // Update metadata
    let mut subs = load_subscriptions_at(paths)?;
    subs.push(SubscriptionMeta {
        id: id.clone(),
        url: url.to_string(),
        updated: Utc::now(),
        user_agent: user_agent.clone(),
        user_agent_mode: Some(if user_agent.is_some() {
            UserAgentMode::Fixed
        } else {
            UserAgentMode::Auto
        }),
    });
    save_subscriptions_at(paths, &subs)?;

    // Set as active based on control flag:
    // - Some(true): force activate
    // - Some(false): force skip
    // - None: auto-activate only if first subscription
    let should_activate = match activate {
        Some(v) => v,
        None => subs.len() == 1,
    };
    if should_activate {
        set_active_id_at(paths, &id)?;
    }

    crate::log!(
        "Added subscription {} with {} lines",
        id,
        yaml_content.lines().count()
    );
    Ok(id)
}

/// Remove a subscription by ID
pub fn remove_subscription_at(paths: &AppPaths, id: &str) -> anyhow::Result<()> {
    let mut subs = load_subscriptions_at(paths)?;

    // Find and remove from metadata
    let idx = subs
        .iter()
        .position(|s| s.id == id)
        .ok_or_else(|| anyhow::anyhow!("Subscription not found: {}", id))?;
    subs.remove(idx);
    save_subscriptions_at(paths, &subs)?;

    // Delete subscription file
    let sub_path = paths.subscription_file_path(id);
    if sub_path.exists() {
        std::fs::remove_file(&sub_path)?;
    }

    // Clear active if this was the active subscription
    let active = get_active_id_at(paths)?;
    if active.as_deref() == Some(id) {
        let active_path = paths.active_file_path();
        if active_path.exists() {
            std::fs::remove_file(&active_path)?;
        }
    }

    crate::log!("Removed subscription {}", id);
    Ok(())
}

/// Refresh a subscription by ID: re-download and update file
pub async fn refresh_subscription_at(paths: &AppPaths, id: &str) -> anyhow::Result<()> {
    refresh_subscription_at_with_user_agent(paths, id, None).await
}

pub async fn refresh_subscription_at_with_user_agent(
    paths: &AppPaths,
    id: &str,
    override_user_agent: Option<&str>,
) -> anyhow::Result<()> {
    let subs = load_subscriptions_at(paths)?;
    let sub = find_subscription(&subs, id)
        .ok_or_else(|| anyhow::anyhow!("Subscription not found: {}", id))?;
    let url = sub.url.clone();
    let fixed_user_agent =
        override_user_agent
            .map(str::to_string)
            .or_else(|| match sub.user_agent_mode {
                Some(UserAgentMode::Fixed) => sub.user_agent.clone(),
                _ => None,
            });

    // Download. Fixed UA uses exactly that UA; auto mode uses bounded Clash-UA negotiation.
    let result = download_sub_with_user_agent(&url, fixed_user_agent.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("Cannot reach subscription URL.\n  {e}"))?;
    let content = result.content;
    let is_yaml = result.is_clash_yaml;

    // Convert if needed
    let yaml_content = if is_yaml {
        content
    } else {
        warn_raw_subscription_conversion();
        convert_vmess_to_clash(&content)?
    };

    // Validate
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).context("Downloaded content is not valid YAML")?;
    validate_subscription_yaml(&yaml)?;

    // Overwrite subscription file
    let sub_path = paths.subscription_file_path(id);
    utils::atomic_write_file(&sub_path.display().to_string(), &yaml_content)?;

    // Update metadata timestamp
    let mut subs = subs;
    if let Some(s) = subs.iter_mut().find(|s| s.id == id) {
        s.updated = Utc::now();
    }
    save_subscriptions_at(paths, &subs)?;

    crate::log!(
        "Refreshed subscription {} with {} lines",
        id,
        yaml_content.lines().count()
    );
    Ok(())
}

/// Refresh all subscriptions
pub async fn refresh_all_at(paths: &AppPaths) -> anyhow::Result<()> {
    let subs = load_subscriptions_at(paths)?;
    for sub in &subs {
        crate::log!("Refreshing subscription {}...", sub.id);
        if let Err(e) = refresh_subscription_at(paths, &sub.id).await {
            crate::log!("Failed to refresh {}: {}", sub.id, e);
            // Continue with other subscriptions
        }
    }
    Ok(())
}

/// Switch to a subscription by ID by updating subscriptions/active.
///
/// This function intentionally does not merge config.yaml; callers that change
/// the active subscription should run `merge_user_config_checked_at()` and roll
/// back the active file if validation fails.
pub fn switch_subscription_at(paths: &AppPaths, id: &str) -> anyhow::Result<()> {
    let subs = load_subscriptions_at(paths)?;
    if find_subscription(&subs, id).is_none() {
        anyhow::bail!("Subscription not found: {}", id);
    }

    set_active_id_at(paths, id)?;

    crate::log!("Switched to subscription {}", id);
    Ok(())
}

pub fn set_subscription_user_agent_at(
    paths: &AppPaths,
    id: &str,
    user_agent: Option<String>,
) -> anyhow::Result<()> {
    let mut subs = load_subscriptions_at(paths)?;
    let sub = subs
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| anyhow::anyhow!("Subscription not found: {}", id))?;
    sub.user_agent = user_agent;
    sub.user_agent_mode = Some(if sub.user_agent.is_some() {
        UserAgentMode::Fixed
    } else {
        UserAgentMode::Auto
    });
    save_subscriptions_at(paths, &subs)
}

const SUBSCRIPTION_UA_CANDIDATES: &[(&str, &str)] = &[
    ("clash-verge", "clash-verge/v2.0.4"),
    ("clash-meta", "clash-meta/v1.19.0"),
    ("clash", "clash/v1.0.0"),
];

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub content: String,
    pub is_clash_yaml: bool,
}

#[derive(Debug, Clone)]
pub struct SubscriptionProbeResult {
    pub label: String,
    pub user_agent: Option<String>,
    pub format: String,
    pub http_status: Option<u16>,
    pub proxy_count: usize,
    pub proxy_group_count: usize,
    pub rule_count: usize,
    pub proxy_provider_count: usize,
    pub rule_provider_count: usize,
    pub bytes: usize,
    pub score: i32,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct HttpTextResponse {
    status: u16,
    body: String,
}

trait SubscriptionFetcher {
    fn get_text<'a>(
        &'a self,
        url: String,
        user_agent: Option<String>,
    ) -> futures::future::BoxFuture<'a, Result<HttpTextResponse, String>>;
}

struct ReqwestSubscriptionFetcher {
    client: reqwest::Client,
}

impl SubscriptionFetcher for ReqwestSubscriptionFetcher {
    fn get_text<'a>(
        &'a self,
        url: String,
        user_agent: Option<String>,
    ) -> futures::future::BoxFuture<'a, Result<HttpTextResponse, String>> {
        Box::pin(async move {
            let mut request = self.client.get(&url);
            if let Some(ua) = user_agent {
                request = request.header("User-Agent", ua);
            }
            let resp = request.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(HttpTextResponse { status, body })
        })
    }
}

/// Backward-compatible helper. Auto mode tries a bounded Clash-UA set and stops at first YAML.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchSubscriptionReport {
    pub output_path: std::path::PathBuf,
    pub is_clash_yaml: bool,
    pub proxy_count: usize,
    pub proxy_group_count: usize,
    pub rule_count: usize,
}

pub async fn fetch_subscription_to_file(
    url: &str,
    output_path: &std::path::Path,
    user_agent: Option<&str>,
) -> anyhow::Result<FetchSubscriptionReport> {
    let result = download_sub_with_user_agent(url, user_agent)
        .await
        .map_err(|e| anyhow::anyhow!("Cannot reach subscription URL.\n  {e}"))?;
    fetch_subscription_content_to_file(result, output_path)
}

fn fetch_subscription_content_to_file(
    result: DownloadResult,
    output_path: &std::path::Path,
) -> anyhow::Result<FetchSubscriptionReport> {
    let yaml_content = if result.is_clash_yaml {
        result.content
    } else {
        crate::log!("Converting subscription format (vmess/base64/raw → Clash YAML)...");
        convert_vmess_to_clash(&result.content)?
    };

    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).context("Subscription content is not valid YAML")?;
    validate_subscription_yaml(&yaml)?;

    if let Some(parent) = output_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    utils::atomic_write_file(&output_path.display().to_string(), &yaml_content)?;

    Ok(FetchSubscriptionReport {
        output_path: output_path.to_path_buf(),
        is_clash_yaml: result.is_clash_yaml,
        proxy_count: count_sequence_field(&yaml, "proxies"),
        proxy_group_count: count_sequence_field(&yaml, "proxy-groups"),
        rule_count: count_sequence_field(&yaml, "rules"),
    })
}

fn count_sequence_field(yaml: &serde_yaml::Value, key: &str) -> usize {
    yaml.get(key)
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.len())
        .unwrap_or(0)
}

pub async fn download_sub_smart(url: &str) -> anyhow::Result<(String, bool)> {
    let result = download_sub_with_user_agent(url, None).await?;
    Ok((result.content, result.is_clash_yaml))
}

pub async fn download_sub_with_user_agent(
    url: &str,
    user_agent: Option<&str>,
) -> anyhow::Result<DownloadResult> {
    let client = crate::utils::http_client_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let fetcher = ReqwestSubscriptionFetcher { client };
    download_sub_with_user_agent_and_fetcher(url, user_agent, &fetcher).await
}

async fn download_sub_with_user_agent_and_fetcher(
    url: &str,
    user_agent: Option<&str>,
    fetcher: &impl SubscriptionFetcher,
) -> anyhow::Result<DownloadResult> {
    if let Some(ua) = user_agent {
        let clash_url = append_query_param(url, "flag=clashmeta");
        let resp = fetcher
            .get_text(clash_url, Some(ua.to_string()))
            .await
            .map_err(|e| subscription_network_error(url, e, None))?;
        let is_clash_yaml = is_clash_yaml(&resp.body);
        return Ok(DownloadResult {
            content: resp.body,
            is_clash_yaml,
        });
    }

    let clash_url = append_query_param(url, "flag=clashmeta");
    let mut first_error: Option<String> = None;

    // Auto mode is intentionally bounded and stops as soon as Clash YAML is found.
    // This avoids unnecessary requests against providers that rate-limit subscription URLs.
    for (label, ua) in SUBSCRIPTION_UA_CANDIDATES {
        match fetcher
            .get_text(clash_url.clone(), Some((*ua).to_string()))
            .await
        {
            Ok(resp) if (200..=299).contains(&resp.status) => {
                if is_clash_yaml(&resp.body) {
                    crate::log!(
                        "got Clash YAML via UA={label}, flag=clashmeta ({} lines)",
                        resp.body.lines().count()
                    );
                    return Ok(DownloadResult {
                        content: resp.body,
                        is_clash_yaml: true,
                    });
                }
                crate::log!(
                    "UA={label}, flag=clashmeta returned non-YAML format ({} bytes)",
                    resp.body.len()
                );
            }
            Ok(resp) => {
                crate::log!(
                    "UA={label}, flag=clashmeta request failed (HTTP {})",
                    resp.status
                );
            }
            Err(e) => {
                crate::log!("UA={label}, flag=clashmeta network error: {e}");
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }

    match fetcher.get_text(url.to_string(), None).await {
        Ok(resp) => {
            let is_clash_yaml = is_clash_yaml(&resp.body);
            if is_clash_yaml {
                crate::log!("got Clash YAML via bare subscription URL");
            } else {
                crate::log!(
                    "bare subscription URL returned non-YAML format ({} bytes)",
                    resp.body.len()
                );
            }
            Ok(DownloadResult {
                content: resp.body,
                is_clash_yaml,
            })
        }
        Err(e) => Err(subscription_network_error(url, e, first_error)),
    }
}

pub async fn probe_subscription_url(url: &str) -> anyhow::Result<Vec<SubscriptionProbeResult>> {
    let client = crate::utils::http_client_builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let fetcher = ReqwestSubscriptionFetcher { client };
    probe_subscription_url_with_fetcher(url, &fetcher, true).await
}

async fn probe_subscription_url_with_fetcher(
    url: &str,
    fetcher: &impl SubscriptionFetcher,
    rate_limit_delay: bool,
) -> anyhow::Result<Vec<SubscriptionProbeResult>> {
    let mut results = Vec::new();
    for (idx, (label, ua)) in SUBSCRIPTION_UA_CANDIDATES.iter().enumerate() {
        if rate_limit_delay && idx > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
        results.push(probe_once(url, label, Some(*ua), fetcher).await);
    }
    if rate_limit_delay {
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    }
    results.push(probe_once(url, "bare", None, fetcher).await);
    results.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.cmp(&b.label)));
    Ok(results)
}

async fn probe_once(
    url: &str,
    label: &str,
    user_agent: Option<&str>,
    fetcher: &impl SubscriptionFetcher,
) -> SubscriptionProbeResult {
    let request_url = if user_agent.is_some() {
        append_query_param(url, "flag=clashmeta")
    } else {
        url.to_string()
    };
    match fetcher
        .get_text(request_url, user_agent.map(str::to_string))
        .await
    {
        Ok(resp) => analyze_subscription_response(
            label.to_string(),
            user_agent.map(str::to_string),
            Some(resp.status),
            resp.body,
            None,
        ),
        Err(e) => SubscriptionProbeResult::error(label, user_agent, None, e),
    }
}

impl SubscriptionProbeResult {
    fn error(label: &str, user_agent: Option<&str>, status: Option<u16>, error: String) -> Self {
        Self {
            label: label.to_string(),
            user_agent: user_agent.map(str::to_string),
            format: "error".to_string(),
            http_status: status,
            proxy_count: 0,
            proxy_group_count: 0,
            rule_count: 0,
            proxy_provider_count: 0,
            rule_provider_count: 0,
            bytes: 0,
            score: -2000,
            error: Some(error),
        }
    }
}

fn analyze_subscription_response(
    label: String,
    user_agent: Option<String>,
    http_status: Option<u16>,
    content: String,
    error: Option<String>,
) -> SubscriptionProbeResult {
    let bytes = content.len();
    let mut result = SubscriptionProbeResult {
        label,
        user_agent,
        format: "unknown".to_string(),
        http_status,
        proxy_count: 0,
        proxy_group_count: 0,
        rule_count: 0,
        proxy_provider_count: 0,
        rule_provider_count: 0,
        bytes,
        score: 0,
        error,
    };
    if !matches!(http_status, Some(200..=299)) {
        result.score = -1000;
        result.error = Some(format!("HTTP {}", http_status.unwrap_or_default()));
        return result;
    }
    if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
        if is_clash_yaml(&content) {
            result.format = "Clash YAML".to_string();
            result.proxy_count = yaml["proxies"].as_sequence().map_or(0, |v| v.len());
            result.proxy_group_count = yaml["proxy-groups"].as_sequence().map_or(0, |v| v.len());
            result.rule_count = yaml["rules"].as_sequence().map_or(0, |v| v.len());
            result.proxy_provider_count =
                yaml["proxy-providers"].as_mapping().map_or(0, |v| v.len());
            result.rule_provider_count = yaml["rule-providers"].as_mapping().map_or(0, |v| v.len());
            result.score = 1000
                + result.proxy_count.min(300) as i32
                + (result.proxy_group_count.min(30) as i32 * 10)
                + result.rule_count.min(500) as i32
                + ((result.proxy_provider_count + result.rule_provider_count).min(20) as i32 * 20);
            return result;
        }
    }
    let raw_count = parse_lines(&content)
        .into_iter()
        .filter(|l| {
            l.starts_with("vmess://") || l.starts_with("trojan://") || l.starts_with("ss://")
        })
        .count();
    if raw_count > 0 {
        result.format = "Raw links".to_string();
        result.proxy_count = raw_count;
        result.score = 100 + raw_count.min(300) as i32;
    }
    result
}

fn subscription_network_error(
    url: &str,
    error: String,
    first_error: Option<String>,
) -> anyhow::Error {
    let detail = first_error
        .map(|first| format!("first UA attempt: {first}; final retry: {error}"))
        .unwrap_or(error);
    crate::log!("Network error: {detail}");
    crate::log!(
        "Is the URL reachable? Try: curl -I '{}'",
        utils::sanitize_url(url)
    );
    let sanitized = utils::sanitize_url(url);
    anyhow::anyhow!(
        "Network error: {detail}\n\n\
         Possible causes:\n  \
         - DNS resolution failed (DNS may be polluted)\n  \
         - Connection refused or timed out\n  \
         - TLS certificate error\n  \
         - The URL requires a proxy to reach\n  \
           → If mihomo is already running: eval \"$(mihomo-cli proxy on)\"\n  \
           → Or manually: export http_proxy=http://127.0.0.1:PORT\n  \
         - TUN mode is intercepting traffic (try: mihomo-cli tun off)\n\n\
         Verify the URL is reachable:\n  \
         curl -I '{sanitized}'\n\n\
         Workaround (if DNS is polluted):\n  \
         1. Download config on another machine:\n  \
            curl --doh-url https://dns.alidns.com/dns-query -o config.yaml '{sanitized}'\n  \
         2. Transfer to this machine and import:\n  \
            mihomo-cli config --import config.yaml"
    )
}

fn append_query_param(url: &str, param: &str) -> String {
    let separator = if url.contains('?') { "&" } else { "?" };
    format!("{url}{separator}{param}")
}

pub fn warn_raw_subscription_conversion() {
    eprintln!(
        "  ⚠ Subscription server did not return Clash YAML; provider preset rules, custom proxy-groups, and service-specific routing may be lost."
    );
    eprintln!(
        "  Hint: verify the subscription supports Clash format, or import a Clash YAML export directly."
    );
    eprintln!("  You can add local rules with: mihomo-cli rule add <rule>");
}

pub fn is_clash_yaml(content: &str) -> bool {
    content.contains("proxies:") || content.contains("mixed-port:") || content.contains("mode:")
}

pub fn convert_vmess_to_clash(content: &str) -> anyhow::Result<String> {
    let lines = parse_lines(content);

    let mut proxies: Vec<serde_yaml::Value> = Vec::new();

    for line in &lines {
        if line.starts_with("vmess://") {
            if let Some(proxy) = parse_vmess(line) {
                proxies.push(proxy);
            }
        } else if line.starts_with("trojan://") {
            if let Some(proxy) = parse_trojan(line) {
                proxies.push(proxy);
            }
        }
    }

    if proxies.is_empty() {
        anyhow::bail!("No proxies found in subscription");
    }

    let names: Vec<String> = proxies
        .iter()
        .filter_map(|p| p["name"].as_str().map(String::from))
        .collect();
    // let names_json = serde_json::to_string(&names)?;

    let controller_line = current_api_endpoint().controller_line();

    // Indent the serialized output for proper YAML nesting
    let proxies_yaml = indent(&serde_yaml::to_string(&proxies)?, 2);
    let names_yaml = indent(&serde_yaml::to_string(&names)?, 6);

    let config = format!(
        r#"# Generated by mihomo-cli
mode: rule
mixed-port: 7897
allow-lan: false
log-level: info
ipv6: true
{controller_line}
tun:
  enable: false
  stack: system
  auto-route: true
  auto-detect-interface: true
  dns-hijack:
    - any:53
dns:
  enable: true
  listen: 127.0.0.1:1053
  default-nameserver:
    - 198.51.100.53
    - 223.5.5.5
  enhanced-mode: fake-ip
  fake-ip-range: 28.0.0.1/8
  fake-ip-filter:
    - '+.lan'
    - '+.local'
profile:
  store-selected: true

proxies:
{proxies_yaml}

proxy-groups:
  - name: 节点选择
    type: select
    proxies:
{names_yaml}
  - name: 自动选择
    type: url-test
    proxies:
{names_yaml}
    url: http://www.gstatic.com/generate_204
    interval: 300
    tolerance: 50

rules:
  - MATCH,节点选择
"#,
        proxies_yaml = proxies_yaml,
        names_yaml = names_yaml,
    );

    Ok(config)
}

fn parse_lines(content: &str) -> Vec<String> {
    let raw = content.trim();
    // Check if it starts with protocol prefixes
    for prefix in &["vmess://", "ss://", "trojan://"] {
        if raw.starts_with(prefix) || raw.lines().any(|l| l.starts_with(prefix)) {
            return raw
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
        }
    }
    // Try base64 decode
    for pad in &["", "=", "==", "==="] {
        let b64 = format!("{raw}{pad}");
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&b64) {
            if let Ok(decoded_str) = String::from_utf8(decoded) {
                return decoded_str
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
            }
        }
    }
    raw.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn parse_vmess(line: &str) -> Option<serde_yaml::Value> {
    let b64 = line.strip_prefix("vmess://")?;
    let b64_padded = format!("{}{}", b64, "=".repeat((4 - b64.len() % 4) % 4));
    let json = base64::engine::general_purpose::STANDARD
        .decode(&b64_padded)
        .ok()?;
    let d: serde_json::Value = serde_json::from_slice(&json).ok()?;

    let name = d["ps"].as_str().unwrap_or("vmess");
    let server = d["add"].as_str().unwrap_or("");
    let port: u64 = d["port"].as_str()?.parse().ok()?;
    let uuid = d["id"].as_str().unwrap_or("");
    let aid: u64 = d["aid"].as_str()?.parse().unwrap_or(0);
    let net = d["net"].as_str().unwrap_or("tcp");
    let tls = d["tls"].as_str().unwrap_or("");

    let mut proxy = serde_yaml::Mapping::new();
    macro_rules! insert {
        ($m:expr, $k:expr, $v:expr) => {
            $m.insert(
                serde_yaml::Value::String($k.to_string()),
                serde_yaml::Value::String($v.to_string()),
            );
        };
    }

    insert!(proxy, "name", name);
    insert!(proxy, "type", "vmess");
    insert!(proxy, "server", server);
    insert!(proxy, "port", &port.to_string());
    insert!(proxy, "uuid", uuid);
    insert!(proxy, "alterId", &aid.to_string());
    insert!(proxy, "cipher", "auto");

    if tls == "tls" {
        proxy.insert(
            serde_yaml::Value::String("tls".into()),
            serde_yaml::Value::Bool(true),
        );
    }

    if net == "ws" || net == "h2" {
        insert!(proxy, "network", net);
        let mut opts = serde_yaml::Mapping::new();
        if let Some(path) = d["path"].as_str() {
            if !path.is_empty() {
                insert!(opts, "path", path);
            }
        }
        if let Some(host) = d["host"].as_str() {
            if !host.is_empty() {
                let mut headers = serde_yaml::Mapping::new();
                insert!(headers, "Host", host);
                opts.insert(
                    serde_yaml::Value::String("headers".into()),
                    serde_yaml::Value::Mapping(headers),
                );
            }
        }
        if !opts.is_empty() {
            proxy.insert(
                serde_yaml::Value::String(format!("{net}-opts")),
                serde_yaml::Value::Mapping(opts),
            );
        }
    } else if net == "grpc" {
        insert!(proxy, "network", "grpc");
        let mut opts = serde_yaml::Mapping::new();
        let svc = d["path"].as_str().unwrap_or("").trim_start_matches('/');
        if !svc.is_empty() {
            insert!(opts, "grpc-service-name", svc);
            proxy.insert(
                serde_yaml::Value::String("grpc-opts".into()),
                serde_yaml::Value::Mapping(opts),
            );
        }
    } else if !net.is_empty() {
        insert!(proxy, "network", net);
    }

    Some(serde_yaml::Value::Mapping(proxy))
}

fn parse_trojan(line: &str) -> Option<serde_yaml::Value> {
    let rest = line.strip_prefix("trojan://")?;
    let at_idx = rest.find('@')?;
    let password = &rest[..at_idx];
    let after_at = &rest[at_idx + 1..];
    let qm = after_at.find('?').unwrap_or(after_at.len());
    let host_port = &after_at[..qm];
    let (host, port_str) = host_port.rsplit_once(':')?;
    let port: u64 = port_str.parse().ok()?;

    let _query = if qm < after_at.len() {
        &after_at[qm + 1..]
    } else {
        ""
    };

    let mut proxy = serde_yaml::Mapping::new();
    macro_rules! insert {
        ($m:expr, $k:expr, $v:expr) => {
            $m.insert(
                serde_yaml::Value::String($k.to_string()),
                serde_yaml::Value::String($v.to_string()),
            );
        };
    }

    insert!(proxy, "name", "trojan");
    insert!(proxy, "type", "trojan");
    insert!(proxy, "server", host);
    insert!(proxy, "port", &port.to_string());
    insert!(proxy, "password", password);

    Some(serde_yaml::Value::Mapping(proxy))
}

#[allow(dead_code)]
pub fn save_config(content: &str) -> anyhow::Result<()> {
    let paths = AppPaths::from_system();
    let mihomo = std::path::PathBuf::from(utils::mihomo_path());
    save_config_at(&paths, content, Some(&mihomo))?;
    println!("Config saved to {}", paths.config_path().display());
    Ok(())
}

/// Save config.yaml with post-write validation and rollback.
///
/// The content is parsed, normalized, patched with the controller setting,
/// written atomically, then validated. If validation fails, the previous
/// config.yaml is restored (or the newly-created file is removed).
#[allow(dead_code)]
pub fn save_config_at(
    paths: &AppPaths,
    content: &str,
    mihomo_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    save_config_at_endpoint(paths, content, mihomo_path, &current_api_endpoint())
}

pub fn save_config_at_endpoint(
    paths: &AppPaths,
    content: &str,
    mihomo_path: Option<&std::path::Path>,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(paths.config_dir())?;
    let path = paths.config_path();
    let previous = snapshot_file(&path)?;

    let yaml: serde_yaml::Value =
        serde_yaml::from_str(content).context("Subscription content is not valid YAML")?;

    if yaml.get("proxies").is_none() && yaml.get("proxy-providers").is_none() {
        anyhow::bail!("Subscription does not contain `proxies` or `proxy-providers`");
    }

    let normalized = serde_yaml::to_string(&yaml)?;
    let content = ensure_controller_for_endpoint(&normalized, endpoint)?;

    utils::atomic_write_file(&path.display().to_string(), &content)?;

    if let Err(validation_error) = validate_config_at(paths, mihomo_path) {
        restore_file_snapshot(&path, previous)?;
        anyhow::bail!(
            "Config validation failed after save; rolled back config.yaml.
  {}",
            validation_error
        );
    }

    Ok(())
}

/// Fix the existing config file: ensure it has external-controller-unix/pipe.
/// Reads current config, applies ensure_controller_for_endpoint(), writes back.
/// Returns true if the file was modified.
#[allow(dead_code)]
pub fn fix_existing_config() -> bool {
    let paths = AppPaths::from_system();
    let mihomo = std::path::PathBuf::from(utils::mihomo_path());
    match fix_existing_config_at(&paths, Some(&mihomo)) {
        Ok(fixed) => fixed,
        Err(e) => {
            eprintln!("  Warning: Failed to fix config: {}", e);
            false
        }
    }
}

#[allow(dead_code)]
pub fn fix_existing_config_at(
    paths: &AppPaths,
    mihomo_path: Option<&std::path::Path>,
) -> anyhow::Result<bool> {
    let path = paths.config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    let fixed = ensure_controller_for_endpoint(&content, &current_api_endpoint())?;
    if fixed == content {
        return Ok(false);
    }

    let previous = Some(content);
    utils::atomic_write_file(&path.display().to_string(), &fixed)?;

    if let Err(validation_error) = validate_config_at(paths, mihomo_path) {
        restore_file_snapshot(&path, previous)?;
        anyhow::bail!(
            "Config validation failed after fix; rolled back config.yaml.
  {}",
            validation_error
        );
    }

    Ok(true)
}

pub fn fix_existing_config_at_endpoint(
    paths: &AppPaths,
    mihomo_path: Option<&std::path::Path>,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<bool> {
    let path = paths.config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    let fixed = ensure_controller_for_endpoint(&content, endpoint)?;
    if fixed == content {
        return Ok(false);
    }

    let previous = Some(content);
    utils::atomic_write_file(&path.display().to_string(), &fixed)?;

    if let Err(validation_error) = validate_config_at(paths, mihomo_path) {
        restore_file_snapshot(&path, previous)?;
        anyhow::bail!(
            "Config validation failed after fix; rolled back config.yaml.
  {}",
            validation_error
        );
    }

    Ok(true)
}

#[allow(dead_code)]
fn snapshot_file(path: &std::path::Path) -> anyhow::Result<Option<String>> {
    if path.exists() {
        Ok(Some(std::fs::read_to_string(path)?))
    } else {
        Ok(None)
    }
}

fn restore_file_snapshot(path: &std::path::Path, snapshot: Option<String>) -> anyhow::Result<()> {
    match snapshot {
        Some(content) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            utils::atomic_write_file(&path.display().to_string(), &content)?;
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

/// Add external-controller-unix/pipe if missing from the config
pub(crate) fn current_api_endpoint() -> ApiEndpoint {
    if cfg!(target_os = "windows") {
        ApiEndpoint::WindowsNamedPipe(windows_user_core_pipe_name(
            std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string()),
        ))
    } else {
        #[cfg(unix)]
        {
            unix_api_endpoint_from_socket_dir(crate::utils::socket_dir())
        }
        #[cfg(not(unix))]
        {
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("mihomo.sock"))
        }
    }
}

#[cfg(unix)]
fn unix_api_endpoint_from_socket_dir(socket_dir: impl AsRef<std::path::Path>) -> ApiEndpoint {
    ApiEndpoint::UnixSocket(socket_dir.as_ref().join("mihomo.sock"))
}

fn windows_user_core_pipe_name(username_or_sid: impl AsRef<str>) -> String {
    format!(r"\\.\pipe\mihomo-{}", username_or_sid.as_ref())
}

/// Add or repair external-controller-unix/pipe for an explicit instance endpoint.
pub(crate) fn ensure_controller_for_endpoint(
    yaml: &str,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<String> {
    let controller_line = endpoint.controller_line();

    let mut editor = crate::yaml_editor::YamlEditor::parse(yaml)
        .map_err(|e| anyhow::anyhow!("Failed to parse config.yaml: {}", e))?;

    editor
        .ensure_controller(&controller_line)
        .map_err(|e| anyhow::anyhow!("Failed to ensure controller in config.yaml: {}", e))?;

    Ok(editor.into_source())
}

#[allow(dead_code)]
pub fn check_config_exists() -> bool {
    std::path::Path::new(&utils::config_path()).exists()
}

fn indent(s: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    s.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{prefix}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Phase 3: Core generation logic ──

/// Validate that subscription YAML contains required fields
fn validate_subscription_yaml(yaml: &serde_yaml::Value) -> anyhow::Result<()> {
    if yaml.get("proxies").is_none() && yaml.get("proxy-providers").is_none() {
        anyhow::bail!(
            "Subscription does not contain `proxies` or `proxy-providers`.\n  \
             The downloaded content may not be a valid Clash subscription."
        );
    }
    Ok(())
}

/// Generate final config.yaml from subscription + user rules + DNS policies.
///
/// This is the core function that replaces older incremental editing paths.
/// It performs a full generation from source files.
#[allow(dead_code)]
pub fn generate_config_yaml(
    sub_yaml: &serde_yaml::Value,
    user_rules: &[String],
    dns_policies: &[crate::dns::DnsPolicy],
    rule_position: crate::rules::RulePosition,
) -> anyhow::Result<String> {
    generate_config_yaml_with_fake_ip_filters_for_endpoint(
        sub_yaml,
        user_rules,
        dns_policies,
        &[],
        rule_position,
        &current_api_endpoint(),
    )
}

pub fn generate_config_yaml_with_fake_ip_filters_for_endpoint(
    sub_yaml: &serde_yaml::Value,
    user_rules: &[String],
    dns_policies: &[crate::dns::DnsPolicy],
    fake_ip_filters: &[String],
    rule_position: crate::rules::RulePosition,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<String> {
    let mut config = sub_yaml.clone();
    let config_map = config
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Subscription is not a valid YAML mapping"))?;

    // 1. Merge rules
    let sub_rules = config_map
        .get("rules")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let merged_rules = match rule_position {
        crate::rules::RulePosition::Front => {
            let mut rules = user_rules.to_vec();
            rules.extend(sub_rules);
            rules
        }
        crate::rules::RulePosition::Back => {
            let mut rules = sub_rules;
            rules.extend(user_rules.iter().cloned());
            rules
        }
    };

    config_map.insert(
        serde_yaml::Value::String("rules".to_string()),
        serde_yaml::Value::Sequence(
            merged_rules
                .into_iter()
                .map(serde_yaml::Value::String)
                .collect(),
        ),
    );

    // 2. Merge dns.nameserver-policy
    if !dns_policies.is_empty() {
        let dns = config_map
            .entry(serde_yaml::Value::String("dns".to_string()))
            .or_insert(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

        let dns_map = dns
            .as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("dns is not a mapping"))?;

        let mut ns_policy = dns_map
            .get("nameserver-policy")
            .and_then(|v| v.as_mapping())
            .cloned()
            .unwrap_or_default();

        for policy in dns_policies {
            let key = if policy.match_pattern.starts_with("+.") {
                policy.match_pattern.clone()
            } else {
                format!("+{}", policy.match_pattern)
            };

            let targets: Vec<&str> = policy.target.split(',').map(|s| s.trim()).collect();
            let value = if targets.len() == 1 {
                serde_yaml::Value::String(targets[0].to_string())
            } else {
                serde_yaml::Value::Sequence(
                    targets
                        .into_iter()
                        .map(|s| serde_yaml::Value::String(s.to_string()))
                        .collect(),
                )
            };

            ns_policy.insert(serde_yaml::Value::String(key), value);
        }

        dns_map.insert(
            serde_yaml::Value::String("nameserver-policy".to_string()),
            serde_yaml::Value::Mapping(ns_policy),
        );
    }

    // 2.1. Merge dns.fake-ip-filter
    if !fake_ip_filters.is_empty() {
        let dns = config_map
            .entry(serde_yaml::Value::String("dns".to_string()))
            .or_insert(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

        let dns_map = dns
            .as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("dns is not a mapping"))?;

        let mut merged = dns_map
            .get("fake-ip-filter")
            .and_then(|v| v.as_sequence())
            .cloned()
            .unwrap_or_default();
        for filter in fake_ip_filters {
            let value = serde_yaml::Value::String(filter.clone());
            if !merged.contains(&value) {
                merged.push(value);
            }
        }
        dns_map.insert(
            serde_yaml::Value::String("fake-ip-filter".to_string()),
            serde_yaml::Value::Sequence(merged),
        );
    }

    // 2.5. Inject mixed-port if no port is configured
    let has_any_port = ["mixed-port", "port", "socks-port"]
        .iter()
        .any(|k| config_map.contains_key(serde_yaml::Value::String(k.to_string())));
    if !has_any_port {
        config_map.insert(
            serde_yaml::Value::String("mixed-port".to_string()),
            serde_yaml::Value::Number(serde_yaml::Number::from(7897)),
        );
    }

    // 3. Inject runtime-owned API controller fields last.
    inject_runtime_controller_for_endpoint(config_map, endpoint);

    // 4. Serialize
    let output = serde_yaml::to_string(&config)?;
    Ok(output)
}

#[allow(dead_code)]
fn inject_runtime_controller(config_map: &mut serde_yaml::Mapping) {
    inject_runtime_controller_for_endpoint(config_map, &current_api_endpoint());
}

fn inject_runtime_controller_for_endpoint(
    config_map: &mut serde_yaml::Mapping,
    endpoint: &ApiEndpoint,
) {
    config_map.remove(serde_yaml::Value::String("external-controller".to_string()));
    config_map.remove(serde_yaml::Value::String(
        "external-controller-unix".to_string(),
    ));
    config_map.remove(serde_yaml::Value::String(
        "external-controller-pipe".to_string(),
    ));
    config_map.remove(serde_yaml::Value::String("external-ui".to_string()));

    let (key, value) = endpoint.controller_key_and_value();
    config_map.insert(
        serde_yaml::Value::String(key.to_string()),
        serde_yaml::Value::String(value),
    );
}

fn deep_merge_yaml(base: &mut serde_yaml::Value, override_value: serde_yaml::Value) {
    match (base, override_value) {
        (serde_yaml::Value::Mapping(base_map), serde_yaml::Value::Mapping(override_map)) => {
            for (key, value) in override_map {
                match base_map.get_mut(&key) {
                    Some(existing) => deep_merge_yaml(existing, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (base_slot, override_value) => {
            *base_slot = override_value;
        }
    }
}

#[allow(dead_code)]
fn apply_override_at(paths: &AppPaths, config: &mut serde_yaml::Value) -> anyhow::Result<()> {
    apply_override_at_endpoint(paths, config, &current_api_endpoint())
}

fn apply_override_at_endpoint(
    paths: &AppPaths,
    config: &mut serde_yaml::Value,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<()> {
    let override_path = paths.override_path();
    if override_path.exists() {
        let override_content = std::fs::read_to_string(&override_path).with_context(|| {
            format!("Failed to read override.yaml: {}", override_path.display())
        })?;
        let override_yaml: serde_yaml::Value = serde_yaml::from_str(&override_content)
            .with_context(|| {
                format!("Failed to parse override.yaml: {}", override_path.display())
            })?;
        if !override_yaml.is_mapping() {
            anyhow::bail!("override.yaml must be a YAML mapping");
        }
        deep_merge_yaml(config, override_yaml);
    }

    let config_map = config
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Merged config is not a YAML mapping after override"))?;
    inject_runtime_controller_for_endpoint(config_map, endpoint);
    Ok(())
}

#[allow(dead_code)]
fn apply_override_to_config_text(paths: &AppPaths, config_content: &str) -> anyhow::Result<String> {
    apply_override_to_config_text_at_endpoint(paths, config_content, &current_api_endpoint())
}

fn apply_override_to_config_text_at_endpoint(
    paths: &AppPaths,
    config_content: &str,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<String> {
    let mut config: serde_yaml::Value = serde_yaml::from_str(config_content)
        .map_err(|e| anyhow::anyhow!("Generated config is invalid YAML before override: {}", e))?;
    apply_override_at_endpoint(paths, &mut config, endpoint)?;
    Ok(serde_yaml::to_string(&config)?)
}

/// Merge user-defined rules and DNS policies into config.yaml.
///
/// New implementation: reads from subscriptions/<active-id>.yaml and generates config.yaml.
/// Falls back to old behavior if no active subscription (for backward compatibility during transition).
pub fn merge_user_config_at(paths: &AppPaths) -> anyhow::Result<()> {
    merge_user_config_at_endpoint(paths, &current_api_endpoint())
}

pub fn merge_user_config_at_endpoint(
    paths: &AppPaths,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<()> {
    use crate::rules::{self, RulePosition};

    // Try new multi-subscription flow first
    let active_id = get_active_id_at(paths)?;

    if let Some(id) = active_id {
        // New flow: read from subscription file
        let sub_path = paths.subscription_file_path(&id);
        if !sub_path.exists() {
            anyhow::bail!(
                "Subscription file not found: {}\n  \
                 Run: mihomo-cli config --refresh  to re-download",
                sub_path.display()
            );
        }

        let sub_content = std::fs::read_to_string(&sub_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to read subscription file {}: {}\n  \
                 Run: mihomo-cli config --refresh  to re-download",
                sub_path.display(),
                e
            )
        })?;

        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(&sub_content).map_err(|e| {
            anyhow::anyhow!(
                "Subscription file {} is not valid YAML: {}\n  \
                 The file may have been manually edited with a syntax error.",
                sub_path.display(),
                e
            )
        })?;

        validate_subscription_yaml(&sub_yaml)?;

        let user_rules = if paths.rules_path().exists() {
            rules::load_rules_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };

        let dns_policies = if paths.dns_policy_path().exists() {
            crate::dns::load_policies_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };

        let fake_ip_filters = if paths.dns_fake_ip_filter_path().exists() {
            crate::dns::load_fake_ip_filters_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };

        let position = rules::get_position_at(paths).unwrap_or(RulePosition::Front);

        let config_content = generate_config_yaml_with_fake_ip_filters_for_endpoint(
            &sub_yaml,
            &user_rules,
            &dns_policies,
            &fake_ip_filters,
            position,
            endpoint,
        )?;
        let config_content =
            apply_override_to_config_text_at_endpoint(paths, &config_content, endpoint)?;

        // Validate the generated YAML
        serde_yaml::from_str::<serde_yaml::Value>(&config_content)
            .map_err(|e| anyhow::anyhow!("Generated config is invalid YAML: {}", e))?;

        utils::atomic_write_file(&paths.config_path().display().to_string(), &config_content)?;

        crate::log!(
            "Generated config from subscription {}: {} rules, {} DNS policies",
            id,
            user_rules.len(),
            dns_policies.len()
        );
    } else {
        // Legacy flow: read from config.yaml directly (backward compatibility)
        let config_path = paths.config_path();
        if !config_path.exists() {
            anyhow::bail!("No active subscription and no config.yaml found.\n  Run: mihomo-cli config --add <URL>");
        }

        let user_rules = if paths.rules_path().exists() {
            rules::load_rules_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };
        let dns_policies = if paths.dns_policy_path().exists() {
            crate::dns::load_policies_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };

        let config_content = std::fs::read_to_string(&config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read config.yaml: {}", e))?;

        let position = rules::get_position_at(paths).unwrap_or(RulePosition::Front);

        // Use the serde_yaml-validated marker editor for legacy flow
        use crate::yaml_editor::YamlEditor;
        let mut editor = YamlEditor::parse(&config_content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config.yaml: {}", e))?;

        editor
            .merge_rules(&user_rules, matches!(position, RulePosition::Front))
            .map_err(|e| anyhow::anyhow!("Failed to merge user rules: {}", e))?;

        if !dns_policies.is_empty() {
            editor
                .merge_dns_policies(&dns_policies)
                .map_err(|e| anyhow::anyhow!("Failed to merge DNS policies: {}", e))?;
        }

        let result = editor.into_source();
        let result = apply_override_to_config_text_at_endpoint(paths, &result, endpoint)?;

        serde_yaml::from_str::<serde_yaml::Value>(&result)
            .map_err(|e| anyhow::anyhow!("Merged config is invalid YAML: {}", e))?;

        if result != config_content {
            utils::atomic_write_file(&config_path.display().to_string(), &result)?;
        }

        crate::log!(
            "Legacy merge: {} rules, {} DNS policies",
            user_rules.len(),
            dns_policies.len()
        );
    }

    Ok(())
}

#[allow(dead_code)]
pub fn merge_user_config() -> anyhow::Result<()> {
    merge_user_config_at(&AppPaths::from_system())
}

/// Merge user configuration into config.yaml, then validate the result.
/// If validation fails, config.yaml is restored to its previous contents.
pub fn merge_user_config_checked_at(
    paths: &AppPaths,
    mihomo_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    merge_user_config_checked_at_endpoint(paths, mihomo_path, &current_api_endpoint())
}

pub fn merge_user_config_checked_at_endpoint(
    paths: &AppPaths,
    mihomo_path: Option<&std::path::Path>,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<()> {
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let config_path = paths.config_path();
    let previous =
        if config_path.exists() {
            Some(std::fs::read_to_string(&config_path).map_err(|e| {
                anyhow::anyhow!("Failed to read existing config before merge: {}", e)
            })?)
        } else {
            None
        };

    merge_user_config_at_endpoint(paths, endpoint)?;

    if let Err(validation_error) = validate_config_at(paths, mihomo_path) {
        match previous {
            Some(content) => {
                utils::atomic_write_file(&config_path.display().to_string(), &content)?;
            }
            None => {
                let _ = std::fs::remove_file(&config_path);
            }
        }
        anyhow::bail!(
            "Config validation failed after write; rolled back config.yaml.\n  {}",
            validation_error
        );
    }

    Ok(())
}

#[allow(dead_code)]
pub fn merge_user_config_checked() -> anyhow::Result<()> {
    let mihomo = std::path::PathBuf::from(crate::utils::mihomo_path());
    merge_user_config_checked_at(&AppPaths::from_system(), Some(&mihomo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{self, RulePosition};
    use std::collections::VecDeque;
    use tempfile::TempDir;

    fn setup_config(content: &str) -> (TempDir, AppPaths) {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), content).unwrap();
        (tmp, paths)
    }

    #[test]
    fn subscription_ua_candidates_cover_common_clash_clients() {
        let labels: Vec<&str> = SUBSCRIPTION_UA_CANDIDATES
            .iter()
            .map(|(label, _)| *label)
            .collect();
        assert_eq!(labels, vec!["clash-verge", "clash-meta", "clash"]);
        assert!(SUBSCRIPTION_UA_CANDIDATES
            .iter()
            .all(|(_, ua)| ua.contains("clash")));
    }

    #[derive(Default)]
    struct FakeSubscriptionFetcher {
        responses: std::sync::Mutex<VecDeque<Result<HttpTextResponse, String>>>,
        requests: std::sync::Mutex<Vec<(String, Option<String>)>>,
    }

    impl FakeSubscriptionFetcher {
        fn new(responses: Vec<Result<HttpTextResponse, String>>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses.into()),
                requests: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<(String, Option<String>)> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl SubscriptionFetcher for FakeSubscriptionFetcher {
        fn get_text<'a>(
            &'a self,
            url: String,
            user_agent: Option<String>,
        ) -> futures::future::BoxFuture<'a, Result<HttpTextResponse, String>> {
            Box::pin(async move {
                self.requests.lock().unwrap().push((url, user_agent));
                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| Err("no fake response".to_string()))
            })
        }
    }

    fn yaml_response(body: &str) -> Result<HttpTextResponse, String> {
        Ok(HttpTextResponse {
            status: 200,
            body: body.to_string(),
        })
    }

    #[tokio::test]
    async fn fetch_subscription_to_file_writes_only_requested_output() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("nested/config.yaml");
        let report = fetch_subscription_content_to_file(
            DownloadResult {
                content: "proxies:\n  - name: direct\n    type: direct\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n".to_string(),
                is_clash_yaml: true,
            },
            &out,
        )
        .unwrap();

        assert!(out.exists());
        assert_eq!(report.proxy_count, 1);
        assert_eq!(report.proxy_group_count, 0);
        assert_eq!(report.rule_count, 1);
        assert!(!tmp.path().join("subscriptions").exists());
        assert!(!tmp.path().join("active").exists());
        assert!(!tmp.path().join("rules.yaml").exists());
    }

    #[tokio::test]
    async fn auto_ua_negotiation_stops_after_first_clash_yaml() {
        let fetcher = FakeSubscriptionFetcher::new(vec![yaml_response(
            "proxies:\n  - name: direct\n    type: direct\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n",
        )]);

        let result =
            download_sub_with_user_agent_and_fetcher("https://example.test/sub", None, &fetcher)
                .await
                .unwrap();

        assert!(result.is_clash_yaml);
        let requests = fetcher.requests();
        assert_eq!(requests.len(), 1, "auto mode should stop after first YAML");
        assert_eq!(requests[0].1.as_deref(), Some("clash-verge/v2.0.4"));
        assert!(requests[0].0.contains("flag=clashmeta"));
    }

    #[tokio::test]
    async fn fixed_ua_uses_exactly_one_request() {
        let fetcher = FakeSubscriptionFetcher::new(vec![yaml_response(
            "proxies:\n  - name: direct\n    type: direct\nrules:\n  - MATCH,DIRECT\n",
        )]);

        let result = download_sub_with_user_agent_and_fetcher(
            "https://example.test/sub",
            Some("custom-clash/v1"),
            &fetcher,
        )
        .await
        .unwrap();

        assert!(result.is_clash_yaml);
        let requests = fetcher.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].1.as_deref(), Some("custom-clash/v1"));
    }

    #[tokio::test]
    async fn probe_scores_clash_yaml_above_raw_links() {
        let fetcher = FakeSubscriptionFetcher::new(vec![
            yaml_response("vmess://abc\nvmess://def\n"),
            yaml_response(
                "proxies:\n  - name: direct\n    type: direct\nproxy-groups:\n  - name: Proxy\n    type: select\n    proxies:\n      - direct\nrules:\n  - DOMAIN-SUFFIX,example.com,Proxy\n  - MATCH,DIRECT\n",
            ),
            yaml_response("trojan://password@example.com:443\n"),
            yaml_response("vmess://bare\n"),
        ]);

        let results =
            probe_subscription_url_with_fetcher("https://example.test/sub", &fetcher, false)
                .await
                .unwrap();

        assert_eq!(results[0].label, "clash-meta");
        assert_eq!(results[0].format, "Clash YAML");
        assert_eq!(results[0].rule_count, 2);
        assert!(results[0].score > results[1].score);
        assert_eq!(fetcher.requests().len(), 4);
    }

    #[test]
    fn append_query_param_preserves_existing_query() {
        assert_eq!(
            append_query_param("https://example.com/sub", "flag=clashmeta"),
            "https://example.com/sub?flag=clashmeta"
        );
        assert_eq!(
            append_query_param("https://example.com/sub?token=x", "flag=clashmeta"),
            "https://example.com/sub?token=x&flag=clashmeta"
        );
    }

    #[test]
    fn test_merge_user_rules_front_is_valid_yaml() {
        let (_tmp, paths) = setup_config(
            "mixed-port: 7890\nrules:\n  - DOMAIN-SUFFIX,google.com,Proxy\n  - DOMAIN-SUFFIX,github.com,DIRECT\ndns:\n  enable: true\n  enhanced-mode: fake-ip\n",
        );
        rules::save_rules_at(
            &paths,
            &[
                "DOMAIN-SUFFIX,company.com,DIRECT".to_string(),
                "IP-CIDR,10.0.0.0/8,DIRECT".to_string(),
            ],
        )
        .unwrap();

        merge_user_config_at(&paths).unwrap();

        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let rule_values = parsed["rules"].as_sequence().unwrap();
        let rules: Vec<&str> = rule_values.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(
            rules,
            vec![
                "DOMAIN-SUFFIX,company.com,DIRECT",
                "IP-CIDR,10.0.0.0/8,DIRECT",
                "DOMAIN-SUFFIX,google.com,Proxy",
                "DOMAIN-SUFFIX,github.com,DIRECT",
            ]
        );
    }

    #[test]
    fn test_merge_user_rules_back_before_next_top_level_key() {
        let (_tmp, paths) = setup_config(
            "mixed-port: 7890\nrules:\n  - DOMAIN-SUFFIX,google.com,Proxy\n  - DOMAIN-SUFFIX,github.com,DIRECT\ndns:\n  enable: true\n  enhanced-mode: fake-ip\n",
        );
        rules::set_position_at(&paths, RulePosition::Back).unwrap();
        rules::save_rules_at(&paths, &["DOMAIN-SUFFIX,back.com,DIRECT".to_string()]).unwrap();

        merge_user_config_at(&paths).unwrap();

        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let rule_values = parsed["rules"].as_sequence().unwrap();
        let rules: Vec<&str> = rule_values.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(
            rules,
            vec![
                "DOMAIN-SUFFIX,google.com,Proxy",
                "DOMAIN-SUFFIX,github.com,DIRECT",
                "DOMAIN-SUFFIX,back.com,DIRECT",
            ]
        );
    }

    #[test]
    fn test_merge_empty_rules_removes_marker_block() {
        let (_tmp, paths) = setup_config(
            "mixed-port: 7890\nrules:\n# === USER RULES START ===\n  - DOMAIN-SUFFIX,old.com,DIRECT\n# === USER RULES END ===\n  - DOMAIN-SUFFIX,google.com,Proxy\n",
        );
        rules::clear_rules_at(&paths).unwrap();

        merge_user_config_at(&paths).unwrap();

        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(!text.contains("USER RULES START"), "{text}");
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let rule_values = parsed["rules"].as_sequence().unwrap();
        let rules: Vec<&str> = rule_values.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(rules, vec!["DOMAIN-SUFFIX,google.com,Proxy"]);
    }

    // ── Failure path tests ──

    #[test]
    fn test_ensure_controller_inserts_at_top_level() {
        let yaml = "proxies:\n  - name: 节点选择\n    type: selector\n";
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from("/tmp/test.sock"));
        let result = ensure_controller_for_endpoint(yaml, &endpoint).unwrap();
        assert!(result.contains("external-controller-unix"));
        assert!(
            result.find("external-controller-unix").unwrap() < result.find("proxies:").unwrap(),
            "\n---\n{result}\n---"
        );
    }

    #[test]
    fn test_ensure_controller_malformed_input() {
        let yaml = "mixed-port: 7890\n  - invalid indent\nfoo: bar";
        // The editor accepts any serde_yaml-valid input and still repairs the controller.
        // Verify it produces a valid result
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from("/tmp/test.sock"));
        let result = ensure_controller_for_endpoint(yaml, &endpoint).unwrap();
        assert!(result.contains("external-controller-unix"));
    }

    #[test]
    fn test_ensure_controller_empty() {
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from("/tmp/test.sock"));
        let result = ensure_controller_for_endpoint("", &endpoint).unwrap();
        assert!(result.contains("external-controller-unix"));
    }

    #[test]
    #[cfg(unix)] // UnixSocket endpoint semantics; not applicable on Windows
    fn test_ensure_controller_already_has_unix() {
        #[cfg(unix)]
        let input = format!(
            "mixed-port: 7890\nexternal-controller-unix: {}/mihomo.sock\n",
            crate::utils::socket_dir()
        );
        #[cfg(not(unix))]
        let input = "mixed-port: 7890\nexternal-controller-unix: mihomo.sock\n".to_string();
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from(format!(
            "{}/mihomo.sock",
            crate::utils::socket_dir()
        )));
        let result = ensure_controller_for_endpoint(&input, &endpoint).unwrap();
        assert_eq!(result, input, "should not modify already-configured config");
    }

    #[test]
    #[cfg(unix)] // UnixSocket endpoint semantics; not applicable on Windows
    fn test_ensure_controller_replaces_wrong_unix_socket() {
        let input = "mixed-port: 7890\nexternal-controller: ''\nexternal-controller-unix: /tmp/verge/verge-mihomo.sock\n";
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from(format!(
            "{}/mihomo.sock",
            crate::utils::socket_dir()
        )));
        let result = ensure_controller_for_endpoint(input, &endpoint).unwrap();
        #[cfg(unix)]
        let expected = format!(
            "external-controller-unix: {}/mihomo.sock",
            crate::utils::socket_dir()
        );
        #[cfg(not(unix))]
        let expected = "external-controller-unix: mihomo.sock".to_string();

        assert!(result.contains(&expected), "{result}");
        assert!(!result.contains("/tmp/verge/verge-mihomo.sock"), "{result}");
    }

    #[cfg(unix)]
    #[test]
    fn unix_current_api_endpoint_joins_socket_dir_without_duplicate_separator() {
        let endpoint = unix_api_endpoint_from_socket_dir("/run/user/1000/");
        assert_eq!(
            endpoint,
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/run/user/1000/mihomo.sock"))
        );
    }

    #[test]
    fn windows_user_core_pipe_name_follows_v3_per_user_contract() {
        assert_eq!(
            windows_user_core_pipe_name("alice"),
            r"\\.\pipe\mihomo-alice"
        );
    }

    #[test]
    fn test_ensure_controller_for_endpoint_replaces_pipe_and_unix_keys() {
        let endpoint = ApiEndpoint::WindowsNamedPipe(r"\\.\pipe\mihomo-core".to_string());
        let input =
            "mixed-port: 7890\nexternal-controller-unix: /tmp/old.sock\nexternal-controller: ''\n";
        let result = ensure_controller_for_endpoint(input, &endpoint).unwrap();

        assert!(
            result.contains(r"external-controller-pipe: \\.\pipe\mihomo-core"),
            "{result}"
        );
        assert!(!result.contains("external-controller-unix"), "{result}");
        assert!(!result.contains("external-controller: ''"), "{result}");
    }

    #[test]
    fn generate_config_yaml_for_endpoint_uses_resolved_instance_endpoint() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"proxies:
  - name: direct
    type: direct
proxy-groups: []
rules: []
external-controller-unix: /tmp/stale-user.sock
external-controller: 127.0.0.1:9090
"#,
        )
        .unwrap();
        let endpoint =
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock"));

        let generated = generate_config_yaml_with_fake_ip_filters_for_endpoint(
            &sub_yaml,
            &[],
            &[],
            &[],
            crate::rules::RulePosition::Front,
            &endpoint,
        )
        .unwrap();

        assert!(generated.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!generated.contains("/tmp/stale-user.sock"));
        assert!(!generated.contains("external-controller: 127.0.0.1:9090"));
    }

    #[test]
    fn test_inject_runtime_controller_for_endpoint_removes_external_ui_and_old_endpoint() {
        let mut config: serde_yaml::Value = serde_yaml::from_str(
            r#"
mixed-port: 7890
external-controller: ''
external-controller-unix: /tmp/evil.sock
external-ui: ui
proxies: []
"#,
        )
        .unwrap();
        let map = config.as_mapping_mut().unwrap();
        inject_runtime_controller_for_endpoint(
            map,
            &ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock")),
        );

        assert!(map
            .get(serde_yaml::Value::String("external-controller".to_string()))
            .is_none());
        assert!(map
            .get(serde_yaml::Value::String("external-ui".to_string()))
            .is_none());
        assert_eq!(
            map.get(serde_yaml::Value::String(
                "external-controller-unix".to_string()
            ))
            .and_then(|v| v.as_str()),
            Some("/var/run/mihomo/mihomo.sock")
        );
    }

    #[test]
    fn save_config_at_endpoint_uses_resolved_instance_endpoint() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let endpoint =
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock"));

        save_config_at_endpoint(
            &paths,
            r#"proxies:
  - name: direct
    type: direct
external-controller-unix: /tmp/stale-user.sock
"#,
            None,
            &endpoint,
        )
        .unwrap();

        let saved = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(saved.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!saved.contains("/tmp/stale-user.sock"));
    }

    #[test]
    #[cfg(unix)] // UnixSocket endpoint semantics; not applicable on Windows
    fn test_fix_existing_config_no_controller_adds_it() {
        let content = "mixed-port: 7890\nmode: rule\n";
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from(format!(
            "{}/mihomo.sock",
            crate::utils::socket_dir()
        )));
        let result = ensure_controller_for_endpoint(content, &endpoint).unwrap();
        #[cfg(unix)]
        let expected = format!("{}/mihomo.sock", crate::utils::socket_dir());
        #[cfg(not(unix))]
        let expected = "mihomo.sock".to_string();
        assert!(result.contains(&format!("external-controller-unix: {}", expected)));
        assert!(result.contains("mixed-port: 7890"));
    }

    #[test]
    fn test_atomic_write_and_interrupt_leaves_target_untouched() {
        use std::io::Write;
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("config.yaml");
        let target_str = target.display().to_string();

        // Write original content
        std::fs::write(&target, "original").unwrap();

        // Simulate interrupt by writing .tmp but NOT renaming
        let tmp_path = format!("{}.tmp", target_str);
        let mut f = std::fs::File::create(&tmp_path).unwrap();
        f.write_all(b"new content that should not appear").unwrap();
        drop(f); // close handle without renaming

        // Now atomic_write_file should overwrite .tmp AND rename
        crate::utils::atomic_write_file(&target_str, "final content").unwrap();

        // Target should be "final content", not "original" or "new content"
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "final content");
        // .tmp should NOT exist after atomic write
        assert!(!std::path::Path::new(&tmp_path).exists());
    }

    // ── Phase 2: Multi-subscription data layer tests ──

    #[test]
    fn test_generate_subscription_id_format() {
        let id = generate_subscription_id();
        assert!(id.starts_with("sub-"));
        assert_eq!(id.len(), 12); // "sub-" + 8 hex chars
                                  // Verify hex chars
        let hex_part = &id[4..];
        assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_subscription_id_unique() {
        let ids: Vec<_> = (0..100).map(|_| generate_subscription_id()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "IDs should be unique");
    }

    #[test]
    fn test_load_subscriptions_empty() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let subs = load_subscriptions_at(&paths).unwrap();
        assert!(subs.is_empty());
    }

    #[test]
    fn test_save_and_load_subscriptions() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let subs = vec![
            SubscriptionMeta {
                id: "sub-abc12345".to_string(),
                url: "https://example.com/sub?token=xxx".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            },
            SubscriptionMeta {
                id: "sub-def67890".to_string(),
                url: "https://other.com/api/v1".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            },
        ];

        save_subscriptions_at(&paths, &subs).unwrap();

        let loaded = load_subscriptions_at(&paths).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "sub-abc12345");
        assert_eq!(loaded[1].id, "sub-def67890");
    }

    #[test]
    fn test_get_active_id_none() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let active = get_active_id_at(&paths).unwrap();
        assert!(active.is_none());
    }

    #[test]
    fn test_set_and_get_active_id() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        set_active_id_at(&paths, "sub-abc12345").unwrap();

        let active = get_active_id_at(&paths).unwrap();
        assert_eq!(active, Some("sub-abc12345".to_string()));
    }

    #[test]
    fn test_find_subscription() {
        let subs = vec![
            SubscriptionMeta {
                id: "sub-abc12345".to_string(),
                url: "https://example.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            },
            SubscriptionMeta {
                id: "sub-def67890".to_string(),
                url: "https://other.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            },
        ];

        assert!(find_subscription(&subs, "sub-abc12345").is_some());
        assert!(find_subscription(&subs, "sub-def67890").is_some());
        assert!(find_subscription(&subs, "sub-notexist").is_none());
    }

    // ── Phase 3: Core generation tests ──

    #[test]
    fn test_validate_subscription_yaml_valid() {
        let yaml: serde_yaml::Value = serde_yaml::from_str("proxies: []").unwrap();
        assert!(validate_subscription_yaml(&yaml).is_ok());
    }

    #[test]
    fn test_validate_subscription_yaml_with_proxy_providers() {
        let yaml: serde_yaml::Value = serde_yaml::from_str("proxy-providers: {}").unwrap();
        assert!(validate_subscription_yaml(&yaml).is_ok());
    }

    #[test]
    fn test_validate_subscription_yaml_invalid() {
        let yaml: serde_yaml::Value = serde_yaml::from_str("mixed-port: 7890").unwrap();
        assert!(validate_subscription_yaml(&yaml).is_err());
    }

    #[test]
    #[cfg(unix)] // platform-specific path semantics
    fn test_generate_config_yaml_basic() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
mixed-port: 7890
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#,
        )
        .unwrap();

        let result =
            generate_config_yaml(&sub_yaml, &[], &[], crate::rules::RulePosition::Front).unwrap();

        // Verify: contains external-controller-unix
        assert!(result.contains("external-controller-unix"));
        // Verify: rules preserved
        assert!(result.contains("DOMAIN-SUFFIX,google.com,Proxy"));
        // Verify: valid YAML
        let _: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
    }

    #[test]
    fn test_generate_config_yaml_with_rules_front() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#,
        )
        .unwrap();

        let user_rules = vec![
            "DOMAIN-SUFFIX,company.com,DIRECT".to_string(),
            "IP-CIDR,10.0.0.0/8,DIRECT".to_string(),
        ];

        let result = generate_config_yaml(
            &sub_yaml,
            &user_rules,
            &[],
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let rules: Vec<&str> = parsed["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        assert_eq!(
            rules,
            vec![
                "DOMAIN-SUFFIX,company.com,DIRECT",
                "IP-CIDR,10.0.0.0/8,DIRECT",
                "DOMAIN-SUFFIX,google.com,Proxy",
            ]
        );
    }

    #[test]
    fn test_generate_config_yaml_with_rules_back() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#,
        )
        .unwrap();

        let user_rules = vec!["DOMAIN-SUFFIX,company.com,DIRECT".to_string()];

        let result = generate_config_yaml(
            &sub_yaml,
            &user_rules,
            &[],
            crate::rules::RulePosition::Back,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let rules: Vec<&str> = parsed["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        assert_eq!(
            rules,
            vec![
                "DOMAIN-SUFFIX,google.com,Proxy",
                "DOMAIN-SUFFIX,company.com,DIRECT",
            ]
        );
    }

    #[test]
    fn test_generate_config_yaml_with_fake_ip_filters() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
dns:
  enhanced-mode: fake-ip
  fake-ip-filter:
    - geosite:private
rules:
  - MATCH,DIRECT
"#,
        )
        .unwrap();
        let result = generate_config_yaml_with_fake_ip_filters_for_endpoint(
            &sub_yaml,
            &[],
            &[],
            &[
                "+.corp.example.com".to_string(),
                "geosite:private".to_string(),
            ],
            crate::rules::RulePosition::Front,
            &crate::instance::ApiEndpoint::UnixSocket(std::path::PathBuf::from("/tmp/test.sock")),
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let filters = parsed["dns"]["fake-ip-filter"].as_sequence().unwrap();
        assert!(filters.contains(&serde_yaml::Value::String("geosite:private".to_string())));
        assert!(filters.contains(&serde_yaml::Value::String("+.corp.example.com".to_string())));
        assert_eq!(
            filters
                .iter()
                .filter(|v| v.as_str() == Some("geosite:private"))
                .count(),
            1
        );
    }

    #[test]
    fn test_generate_config_yaml_with_dns_policies() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
dns:
  enable: true
  nameserver:
    - 223.5.5.5
"#,
        )
        .unwrap();

        let dns_policies = vec![
            crate::dns::DnsPolicy {
                match_pattern: "company.com".to_string(),
                target: "system".to_string(),
            },
            crate::dns::DnsPolicy {
                match_pattern: "internal.corp".to_string(),
                target: "192.0.2.53,198.51.100.53".to_string(),
            },
        ];

        let result = generate_config_yaml(
            &sub_yaml,
            &[],
            &dns_policies,
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let ns_policy = &parsed["dns"]["nameserver-policy"];

        assert_eq!(ns_policy["+company.com"].as_str().unwrap(), "system");
        assert!(ns_policy["+internal.corp"].is_sequence());
    }

    #[test]
    fn test_generate_config_yaml_dns_policy_override() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
dns:
  enable: true
  nameserver-policy:
    "+.company.com": 8.8.8.8
"#,
        )
        .unwrap();

        let dns_policies = vec![crate::dns::DnsPolicy {
            match_pattern: "company.com".to_string(),
            target: "system".to_string(),
        }];

        let result = generate_config_yaml(
            &sub_yaml,
            &[],
            &dns_policies,
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        // User policy overrides subscription policy
        assert_eq!(
            parsed["dns"]["nameserver-policy"]["+company.com"]
                .as_str()
                .unwrap(),
            "system"
        );
    }

    #[test]
    fn test_generate_config_yaml_no_subscription_rules() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
mixed-port: 7890
proxies: []
"#,
        )
        .unwrap();

        let user_rules = vec!["DOMAIN-SUFFIX,company.com,DIRECT".to_string()];

        let result = generate_config_yaml(
            &sub_yaml,
            &user_rules,
            &[],
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let rules: Vec<&str> = parsed["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        assert_eq!(rules, vec!["DOMAIN-SUFFIX,company.com,DIRECT"]);
    }

    #[test]
    fn test_generate_config_yaml_no_subscription_dns() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
mixed-port: 7890
"#,
        )
        .unwrap();

        let dns_policies = vec![crate::dns::DnsPolicy {
            match_pattern: "company.com".to_string(),
            target: "system".to_string(),
        }];

        let result = generate_config_yaml(
            &sub_yaml,
            &[],
            &dns_policies,
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        assert!(parsed["dns"]["nameserver-policy"].is_mapping());
    }

    #[test]
    fn test_generate_config_yaml_injects_mixed_port_when_no_port_present() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#,
        )
        .unwrap();

        let result =
            generate_config_yaml(&sub_yaml, &[], &[], crate::rules::RulePosition::Front).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(parsed["mixed-port"].as_u64(), Some(7897));
    }

    #[test]
    fn test_generate_config_yaml_preserves_existing_port() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
port: 7890
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
"#,
        )
        .unwrap();

        let result =
            generate_config_yaml(&sub_yaml, &[], &[], crate::rules::RulePosition::Front).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(parsed["port"].as_u64(), Some(7890));
        assert!(parsed["mixed-port"].is_null());
    }

    #[test]
    #[cfg(unix)] // platform-specific path semantics
    fn test_merge_user_config_new_flow() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        // Create subscription file
        let sub_content = r#"
mixed-port: 7890
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#;
        std::fs::write(paths.subscription_file_path("sub-test123"), sub_content).unwrap();
        set_active_id_at(&paths, "sub-test123").unwrap();

        // Run merge
        merge_user_config_at(&paths).unwrap();

        // Verify config.yaml was generated
        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(config.contains("external-controller-unix"));
        assert!(config.contains("DOMAIN-SUFFIX,google.com,Proxy"));
    }

    #[test]
    fn merge_user_config_legacy_flow_uses_resolved_endpoint() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.config_path(),
            "mixed-port: 7897\nexternal-controller-unix: /tmp/stale-user.sock\nrules: []\n",
        )
        .unwrap();
        let endpoint =
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock"));

        merge_user_config_at_endpoint(&paths, &endpoint).unwrap();

        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(config.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!config.contains("/tmp/stale-user.sock"));
    }

    #[test]
    fn test_merge_user_config_missing_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        set_active_id_at(&paths, "sub-nonexist").unwrap();

        let result = merge_user_config_at(&paths);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Subscription file not found"));
    }

    // ── Phase 4: Subscription CRUD tests ──

    fn setup_test_subscription(paths: &AppPaths, id: &str, content: &str) {
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::write(paths.subscription_file_path(id), content).unwrap();
    }

    #[test]
    fn test_remove_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());

        // Setup: two subscriptions
        let sub1 = "sub-aaaa1111";
        let sub2 = "sub-bbbb2222";
        setup_test_subscription(&paths, sub1, "proxies: []");
        setup_test_subscription(&paths, sub2, "proxies: []");

        let subs = vec![
            SubscriptionMeta {
                id: sub1.to_string(),
                url: "https://a.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            },
            SubscriptionMeta {
                id: sub2.to_string(),
                url: "https://b.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            },
        ];
        save_subscriptions_at(&paths, &subs).unwrap();
        set_active_id_at(&paths, sub1).unwrap();

        // Remove sub2
        remove_subscription_at(&paths, sub2).unwrap();

        // Verify
        let remaining = load_subscriptions_at(&paths).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, sub1);
        assert!(!paths.subscription_file_path(sub2).exists());
        // Active should still be sub1
        assert_eq!(get_active_id_at(&paths).unwrap(), Some(sub1.to_string()));
    }

    #[test]
    fn test_remove_active_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());

        let sub1 = "sub-aaaa1111";
        setup_test_subscription(&paths, sub1, "proxies: []");

        let subs = vec![SubscriptionMeta {
            id: sub1.to_string(),
            url: "https://a.com".to_string(),
            updated: Utc::now(),
            user_agent: None,
            user_agent_mode: Some(UserAgentMode::Auto),
        }];
        save_subscriptions_at(&paths, &subs).unwrap();
        set_active_id_at(&paths, sub1).unwrap();

        // Remove the active subscription
        remove_subscription_at(&paths, sub1).unwrap();

        // Active should be cleared
        assert_eq!(get_active_id_at(&paths).unwrap(), None);
    }

    #[test]
    fn test_remove_nonexistent_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let result = remove_subscription_at(&paths, "sub-nonexist");
        assert!(result.is_err());
    }

    #[test]
    fn test_switch_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());

        let sub1 = "sub-aaaa1111";
        let sub2 = "sub-bbbb2222";
        let content1 = "mixed-port: 7890\nproxies:\n  - name: p1\n    type: http\n    server: a.com\n    port: 443\n";
        let content2 = "mixed-port: 7891\nproxies:\n  - name: p2\n    type: http\n    server: b.com\n    port: 443\n";

        setup_test_subscription(&paths, sub1, content1);
        setup_test_subscription(&paths, sub2, content2);

        let subs = vec![
            SubscriptionMeta {
                id: sub1.to_string(),
                url: "https://a.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            },
            SubscriptionMeta {
                id: sub2.to_string(),
                url: "https://b.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            },
        ];
        save_subscriptions_at(&paths, &subs).unwrap();
        set_active_id_at(&paths, sub1).unwrap();

        // Switch to sub2
        switch_subscription_at(&paths, sub2).unwrap();

        // Verify active changed
        assert_eq!(get_active_id_at(&paths).unwrap(), Some(sub2.to_string()));

        // Caller is responsible for checked merge after switching.
        merge_user_config_checked_at(&paths, None).unwrap();
        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(config.contains("mixed-port: 7891"));
    }

    #[test]
    fn test_switch_nonexistent_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let result = switch_subscription_at(&paths, "sub-nonexist");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationReport {
    pub yaml_valid: bool,
    pub mihomo_tested: bool,
}

pub fn validate_config_at(
    paths: &crate::utils::AppPaths,
    mihomo_path: Option<&std::path::Path>,
) -> anyhow::Result<ConfigValidationReport> {
    let path = paths.config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Cannot read config: {}\n  {}", path.display(), e))?;
    let _: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Config is not valid YAML: {e}"))?;

    let Some(mihomo) = mihomo_path else {
        return Ok(ConfigValidationReport {
            yaml_valid: true,
            mihomo_tested: false,
        });
    };
    if !mihomo.exists() {
        return Ok(ConfigValidationReport {
            yaml_valid: true,
            mihomo_tested: false,
        });
    }

    let out = std::process::Command::new(mihomo)
        .args(["-t", "-d"])
        .arg(paths.config_dir())
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run mihomo -t: {e}"))?;
    if out.status.success() {
        Ok(ConfigValidationReport {
            yaml_valid: true,
            mihomo_tested: true,
        })
    } else {
        let output = crate::utils::combine_output(&out);
        anyhow::bail!(
            "mihomo -t failed:\n{}",
            output.lines().take(20).collect::<Vec<_>>().join("\n")
        )
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validate_config_accepts_valid_yaml_without_mihomo() {
        let tmp = TempDir::new().unwrap();
        let paths = crate::utils::AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), "port: 7890\nproxies: []\n").unwrap();

        let report = validate_config_at(&paths, None).unwrap();
        assert!(report.yaml_valid);
        assert!(!report.mihomo_tested);
    }

    #[test]
    fn validate_config_rejects_invalid_yaml() {
        let tmp = TempDir::new().unwrap();
        let paths = crate::utils::AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), "port: [\n").unwrap();

        let err = validate_config_at(&paths, None).unwrap_err();
        assert!(err.to_string().contains("not valid YAML"));
    }

    #[test]
    fn validate_config_reports_missing_file() {
        let tmp = TempDir::new().unwrap();
        let paths = crate::utils::AppPaths::for_test(tmp.path());

        let err = validate_config_at(&paths, None).unwrap_err();
        assert!(err.to_string().contains("Cannot read config"));
    }
}

#[cfg(test)]
mod checked_merge_tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn failing_mihomo(tmp: &TempDir) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp.path().join("mihomo-fail");
        std::fs::write(
            &path,
            "#!/usr/bin/env sh\necho simulated validation failure >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn setup_active_subscription(paths: &AppPaths) {
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::write(
            paths.subscription_file_path("sub-a"),
            r#"
proxies:
  - name: Proxy
    type: direct
proxy-groups:
  - name: ProxyGroup
    type: select
    proxies:
      - Proxy
rules:
  - MATCH,DIRECT
"#,
        )
        .unwrap();
        set_active_id_at(paths, "sub-a").unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn checked_merge_rolls_back_config_when_runtime_validation_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), "port: 7890\n").unwrap();
        setup_active_subscription(&paths);
        crate::rules::save_rules_at(&paths, &["DOMAIN-SUFFIX,example.com,DIRECT".to_string()])
            .unwrap();
        let mihomo = failing_mihomo(&tmp);

        let err = merge_user_config_checked_at(&paths, Some(&mihomo)).unwrap_err();

        assert!(err.to_string().contains("rolled back config.yaml"));
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            "port: 7890\n"
        );
    }

    #[test]
    #[cfg(unix)]
    fn save_config_rolls_back_when_runtime_validation_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.config_path(),
            "port: 7890
proxies: []
",
        )
        .unwrap();
        let mihomo = failing_mihomo(&tmp);

        let err = save_config_at(
            &paths,
            "proxies:
  - name: Proxy
    type: direct
",
            Some(&mihomo),
        )
        .unwrap_err();

        assert!(err.to_string().contains("rolled back config.yaml"));
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            "port: 7890
proxies: []
"
        );
    }

    #[test]
    #[cfg(unix)]
    fn fix_existing_config_rolls_back_when_runtime_validation_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let original = "port: 7890
proxies: []
";
        std::fs::write(paths.config_path(), original).unwrap();
        let mihomo = failing_mihomo(&tmp);

        let err = fix_existing_config_at(&paths, Some(&mihomo)).unwrap_err();

        assert!(err.to_string().contains("rolled back config.yaml"));
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            original
        );
    }

    #[test]
    fn checked_merge_succeeds_when_yaml_validation_passes_without_mihomo() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        setup_active_subscription(&paths);

        merge_user_config_checked_at(&paths, None).unwrap();

        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(config.contains("external-controller"));
        assert!(config.contains("MATCH,DIRECT") || config.contains("MATCH, DIRECT"));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionInfo {
    pub id: String,
    pub url: String,
    pub updated: DateTime<Utc>,
    pub proxy_count: usize,
    pub expire: Option<String>,
}

pub fn subscription_info_at(paths: &AppPaths, id: &str) -> anyhow::Result<SubscriptionInfo> {
    let subs = load_subscriptions_at(paths)?;
    let sub = find_subscription(&subs, id)
        .ok_or_else(|| anyhow::anyhow!("Subscription not found: {}", id))?;
    let path = paths.subscription_file_path(id);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read subscription file: {}", path.display()))?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse subscription file: {}", path.display()))?;
    let proxy_count = yaml
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.len())
        .unwrap_or(0)
        + yaml
            .get("proxy-providers")
            .and_then(|v| v.as_mapping())
            .map(|m| m.len())
            .unwrap_or(0);
    let expire = yaml
        .get("subscription-userinfo")
        .and_then(|v| v.as_str())
        .and_then(parse_expire_from_userinfo);

    Ok(SubscriptionInfo {
        id: sub.id.clone(),
        url: sub.url.clone(),
        updated: sub.updated,
        proxy_count,
        expire,
    })
}

fn parse_expire_from_userinfo(userinfo: &str) -> Option<String> {
    for part in userinfo.split(';') {
        let part = part.trim();
        let Some(value) = part.strip_prefix("expire=") else {
            continue;
        };
        if let Ok(ts) = value.parse::<i64>() {
            if let Some(dt) = DateTime::<Utc>::from_timestamp(ts, 0) {
                return Some(dt.to_rfc3339());
            }
        }
    }
    None
}

#[cfg(test)]
mod subscription_info_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn subscription_info_counts_proxies_and_parses_expire() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        let updated = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        save_subscriptions_at(
            &paths,
            &[SubscriptionMeta {
                id: "sub-a".to_string(),
                url: "file://sub-a.yaml".to_string(),
                updated,
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
            }],
        )
        .unwrap();
        std::fs::write(
            paths.subscription_file_path("sub-a"),
            r#"
subscription-userinfo: upload=0; download=0; total=100; expire=1700000000
proxies:
  - name: A
    type: direct
  - name: B
    type: direct
proxy-providers:
  p1:
    type: file
"#,
        )
        .unwrap();

        let info = subscription_info_at(&paths, "sub-a").unwrap();
        assert_eq!(info.proxy_count, 3);
        assert_eq!(info.expire, Some("2023-11-14T22:13:20+00:00".to_string()));
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn override_yaml_deep_merges_maps_and_replaces_lists() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.override_path(),
            r#"
dns:
  enhanced-mode: redir-host
  nameserver:
    - 1.1.1.1
proxy-groups:
  - name: Custom
    type: select
    proxies:
      - DIRECT
external-controller-unix: /tmp/evil.sock
"#,
        )
        .unwrap();

        let output = apply_override_to_config_text(
            &paths,
            r#"
dns:
  enable: true
  enhanced-mode: fake-ip
  nameserver:
    - 8.8.8.8
proxy-groups:
  - name: Original
    type: select
    proxies:
      - DIRECT
rules:
  - MATCH,DIRECT
"#,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&output).unwrap();

        assert_eq!(parsed["dns"]["enable"].as_bool(), Some(true));
        assert_eq!(parsed["dns"]["enhanced-mode"].as_str(), Some("redir-host"));
        assert_eq!(parsed["dns"]["nameserver"][0].as_str(), Some("1.1.1.1"));
        assert_eq!(parsed["proxy-groups"][0]["name"].as_str(), Some("Custom"));
        assert_ne!(
            parsed["external-controller-unix"].as_str(),
            Some("/tmp/evil.sock")
        );
    }

    #[test]
    fn override_yaml_must_be_mapping() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.override_path(), "- invalid\n").unwrap();

        let err = apply_override_to_config_text(&paths, "rules:\n  - MATCH,DIRECT\n").unwrap_err();
        assert!(err
            .to_string()
            .contains("override.yaml must be a YAML mapping"));
    }
}
