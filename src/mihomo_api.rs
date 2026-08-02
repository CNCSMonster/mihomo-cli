use serde_json::Value;
use std::time::Duration;

#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(windows)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;
#[cfg(unix)]
use tokio::net::UnixStream;

/// Check if the socket is actually connectable (not just file exists).
/// Returns true if a connection attempt succeeds.
#[cfg(unix)]
fn socket_is_alive() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream as StdUnixStream;
        StdUnixStream::connect(socket_path()).is_ok()
    }
    #[cfg(windows)]
    {
        use std::fs::OpenOptions;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&socket_path())
            .is_ok()
    }
}

/// Build a context-aware fix suggestion message when socket/API is unreachable.
pub fn socket_fix_suggestion() -> String {
    #[cfg(not(unix))]
    {
        "  Is mihomo running? Run: mihomo-cli status".to_string()
    }
    #[cfg(unix)]
    {
        let proc_alive = mihomo_process_running();
        let sock_alive = socket_is_alive();
        let svc_installed = crate::service::service_installed();

        if proc_alive && sock_alive {
            "  Try: mihomo-cli config --fix && mihomo-cli restart".to_string()
        } else if proc_alive && !sock_alive {
            "  Config may be missing API controller. Try: mihomo-cli config --fix".to_string()
        } else if !proc_alive && svc_installed {
            "  Try: mihomo-cli restart".to_string()
        } else {
            "  Try: mihomo-cli start".to_string()
        }
    }
}

/// Check if mihomo process is actually running (via pgrep/tasklist)
#[cfg(unix)]
pub fn mihomo_process_running() -> bool {
    if cfg!(target_os = "windows") {
        std::process::Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq mihomo.exe"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("mihomo"))
            .unwrap_or(false)
    } else {
        std::process::Command::new("pgrep")
            .args(["-x", "mihomo"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(unix)]
fn socket_path() -> String {
    #[cfg(unix)]
    {
        format!("{}/mihomo.sock", crate::utils::socket_dir())
    }
    #[cfg(windows)]
    {
        r"\\.\pipe\mihomo".to_string()
    }
}

/// Percent-encode non-ASCII and reserved characters in the URL path.
fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Encode a query parameter value (like JS encodeURIComponent).
/// Encodes everything except unreserved chars: A-Z a-z 0-9 - _ . ~
fn encode_query_param(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 2);
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn build_socket_http_request(method: &str, path: &str, body: Option<&[u8]>) -> Vec<u8> {
    let body_str = body
        .map(|b| String::from_utf8_lossy(b).to_string())
        .unwrap_or_default();
    let request = if !body_str.is_empty() {
        format!(
            "{method} {path} HTTP/1.0\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body_str.len(), body_str
        )
    } else {
        format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\n\r\n")
    };
    request.into_bytes()
}

fn parse_socket_http_response(response: &[u8]) -> anyhow::Result<Value> {
    let response_str = String::from_utf8_lossy(response);
    let status_line = response_str.lines().next().unwrap_or("");
    crate::log!("  HTTP status: {}", status_line);
    if !status_line.contains("200") && !status_line.contains("204") {
        anyhow::bail!("mihomo API returned: {}", status_line);
    }
    let body_start = response_str.find("\r\n\r\n").unwrap_or(0) + 4;
    let json_str = response_str[body_start..].trim();
    if json_str.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(json_str)?)
}

pub(crate) fn proxy_group_path(group: &str) -> String {
    format!("/proxies/{}", percent_encode_path(group))
}

fn proxy_select_payload(node: &str) -> Value {
    serde_json::json!({"name": node})
}

fn delay_test_path(group: &str) -> String {
    let enc = percent_encode_path(group);
    let test_url = encode_query_param("http://www.gstatic.com/generate_204");
    format!("/group/{enc}/delay?url={test_url}&timeout=5000")
}

fn reload_config_payload(path: &str) -> Value {
    serde_json::json!({"path": path})
}

#[allow(async_fn_in_trait)]
trait SocketTransport {
    async fn roundtrip(&self, socket: &str, request: &[u8]) -> anyhow::Result<Vec<u8>>;
}

#[derive(Debug, Clone, Copy)]
struct PlatformSocketTransport;

impl SocketTransport for PlatformSocketTransport {
    async fn roundtrip(&self, socket: &str, request: &[u8]) -> anyhow::Result<Vec<u8>> {
        #[cfg(unix)]
        {
            let mut stream = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                UnixStream::connect(socket),
            )
            .await??;
            stream.write_all(request).await?;
            let mut data = Vec::new();
            let mut buf = vec![0u8; 8192];
            loop {
                let n = stream.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            Ok(data)
        }

        #[cfg(windows)]
        {
            let mut stream = ClientOptions::new().open(socket)?;
            stream.write_all(request).await?;
            let mut data = Vec::new();
            let mut buf = vec![0u8; 8192];
            loop {
                let n = stream.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
            }
            Ok(data)
        }
    }
}

async fn do_socket_request_with<T: SocketTransport>(
    transport: &T,
    socket: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> anyhow::Result<Value> {
    let request = build_socket_http_request(method, path, body);
    let response = transport.roundtrip(socket, &request).await?;
    parse_socket_http_response(&response)
}

#[allow(async_fn_in_trait)]
pub(crate) trait MihomoApiClient {
    async fn get(&self, path: &str) -> anyhow::Result<Value>;
    async fn put(&self, path: &str, body: Value) -> anyhow::Result<Value>;
    async fn patch(&self, path: &str, body: Value) -> anyhow::Result<Value>;
    async fn delete(&self, path: &str) -> anyhow::Result<Value>;
}

#[derive(Debug, Clone)]
pub(crate) struct EndpointMihomoApiClient {
    endpoint: crate::instance::ApiEndpoint,
}

impl EndpointMihomoApiClient {
    pub(crate) fn new(endpoint: crate::instance::ApiEndpoint) -> Self {
        Self { endpoint }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> anyhow::Result<Value> {
        match &self.endpoint {
            crate::instance::ApiEndpoint::UnixSocket(socket) => {
                do_socket_request_with(
                    &PlatformSocketTransport,
                    &socket.display().to_string(),
                    method,
                    path,
                    body,
                )
                .await
            }
            crate::instance::ApiEndpoint::WindowsNamedPipe(pipe) => {
                do_socket_request_with(&PlatformSocketTransport, pipe, method, path, body).await
            }
        }
    }
}

impl MihomoApiClient for EndpointMihomoApiClient {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.request("GET", path, None).await
    }

    async fn put(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.request("PUT", path, Some(&serde_json::to_vec(&body)?))
            .await
    }

    async fn patch(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.request("PATCH", path, Some(&serde_json::to_vec(&body)?))
            .await
    }

    async fn delete(&self, path: &str) -> anyhow::Result<Value> {
        self.request("DELETE", path, None).await
    }
}

pub async fn api_get_at_endpoint(
    endpoint: &crate::instance::ApiEndpoint,
    path: &str,
) -> anyhow::Result<Value> {
    EndpointMihomoApiClient::new(endpoint.clone())
        .get(path)
        .await
}

/// Poll /configs until the API is ready, or timeout.
/// Returns true if ready, false if timed out.
#[allow(dead_code)]
pub async fn wait_for_api_ready_at_endpoint(
    endpoint: &crate::instance::ApiEndpoint,
    timeout_secs: u64,
) -> bool {
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_secs(timeout_secs);
    let mut warned_5s = false;
    let mut warned_10s = false;
    let client = EndpointMihomoApiClient::new(endpoint.clone());
    while std::time::Instant::now() < deadline {
        match client.get("/configs").await {
            Ok(_) => return true,
            Err(e) => {
                crate::log!("connection to {:?} failed: {e}", endpoint);
            }
        }
        let elapsed = start.elapsed().as_secs();
        if elapsed >= 10 && !warned_10s {
            eprintln!("  Still initializing (10s+)...");
            warned_10s = true;
        } else if elapsed >= 5 && !warned_5s {
            eprintln!("  Still initializing (5s+)...");
            warned_5s = true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    false
}

pub(crate) async fn list_proxies_with_client(client: &impl MihomoApiClient) -> anyhow::Result<()> {
    let data = client.get("/proxies").await?;
    let proxies = data["proxies"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("no proxies"))?;
    let mut pairs: Vec<(&String, &Value)> = proxies.iter().collect();
    pairs.sort_by_key(|(k, _)| *k);
    for (name, p) in pairs {
        let ptype = p["type"].as_str().unwrap_or("?");
        if !["Selector", "URLTest", "Fallback"].contains(&ptype) {
            continue;
        }
        let now = p["now"].as_str().unwrap_or("-");
        println!("[{ptype:8}] {name}  →  {now}");
        if let Some(all) = p["all"].as_array() {
            for sub in all {
                let s = sub.as_str().unwrap_or("");
                if s != now {
                    println!("          └ {s}");
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct DelayResult {
    pub node: String,
    pub ms: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DelayCacheEntry {
    updated_unix: i64,
    results: Vec<DelayResult>,
}

type DelayCache = std::collections::BTreeMap<String, DelayCacheEntry>;

pub(crate) async fn delay_test_with_client(
    client: &impl MihomoApiClient,
    group: &str,
    refresh: bool,
    cache_ttl: u64,
    fastest: bool,
) -> anyhow::Result<()> {
    let paths = crate::utils::AppPaths::from_system();
    let cache_key = delay_cache_key(group);
    let mut from_cache = false;
    let results = if !refresh {
        load_fresh_delay_results(&paths, &cache_key, cache_ttl)?
    } else {
        None
    };
    let results = match results {
        Some(results) => {
            from_cache = true;
            results
        }
        None => {
            let results = fetch_group_delay_results_with_client(client, group).await?;
            save_delay_results(&paths, &cache_key, results.clone())?;
            results
        }
    };

    print_delay_results(&results, from_cache);
    if fastest {
        let fastest = fastest_delay_result(&results)
            .ok_or_else(|| anyhow::anyhow!("no successful delay result to select"))?;
        select_proxy_with_client(client, group, &fastest.node).await?;
        println!(
            "Selected fastest node: {} ({}ms)",
            fastest.node,
            fastest.ms.unwrap_or_default()
        );
    }
    Ok(())
}

pub(crate) async fn fetch_group_delay_results_with_client(
    client: &impl MihomoApiClient,
    group: &str,
) -> anyhow::Result<Vec<DelayResult>> {
    let path = delay_test_path(group);
    let data = client.get(&path).await?;
    parse_delay_response(&data)
}

pub(crate) fn parse_delay_response(data: &Value) -> anyhow::Result<Vec<DelayResult>> {
    let obj = data
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("no delay data"))?;
    let mut results: Vec<DelayResult> = obj
        .iter()
        .map(|(name, ms)| DelayResult {
            node: name.clone(),
            ms: ms.as_u64(),
        })
        .collect();
    sort_delay_results(&mut results);
    Ok(results)
}

fn sort_delay_results(results: &mut [DelayResult]) {
    results.sort_by(|a, b| match (a.ms, b.ms) {
        (Some(a_ms), Some(b_ms)) => a_ms.cmp(&b_ms).then_with(|| a.node.cmp(&b.node)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.node.cmp(&b.node),
    });
}

pub(crate) fn fastest_delay_result(results: &[DelayResult]) -> Option<&DelayResult> {
    results
        .iter()
        .filter(|r| r.ms.is_some())
        .min_by_key(|r| r.ms)
}

fn print_delay_results(results: &[DelayResult], from_cache: bool) {
    if from_cache {
        println!("Using cached delay results");
    }
    for result in results {
        match result.ms {
            Some(ms) => println!("{}: {}ms", result.node, ms),
            None => println!("{}: timeout", result.node),
        }
    }
}

fn delay_cache_key(group: &str) -> String {
    format!("group:{group}:url:http://www.gstatic.com/generate_204:timeout:5000")
}

fn load_fresh_delay_results(
    paths: &crate::utils::AppPaths,
    key: &str,
    cache_ttl: u64,
) -> anyhow::Result<Option<Vec<DelayResult>>> {
    if cache_ttl == 0 {
        return Ok(None);
    }
    let cache = load_delay_cache(paths)?;
    let Some(entry) = cache.get(key) else {
        return Ok(None);
    };
    let now = chrono::Utc::now().timestamp();
    if now.saturating_sub(entry.updated_unix) <= cache_ttl as i64 {
        Ok(Some(entry.results.clone()))
    } else {
        Ok(None)
    }
}

fn save_delay_results(
    paths: &crate::utils::AppPaths,
    key: &str,
    results: Vec<DelayResult>,
) -> anyhow::Result<()> {
    let mut cache = load_delay_cache(paths)?;
    cache.insert(
        key.to_string(),
        DelayCacheEntry {
            updated_unix: chrono::Utc::now().timestamp(),
            results,
        },
    );
    std::fs::create_dir_all(paths.config_dir())?;
    let content = serde_json::to_string_pretty(&cache)?;
    crate::utils::atomic_write_file(&paths.delay_cache_path().display().to_string(), &content)?;
    Ok(())
}

fn load_delay_cache(paths: &crate::utils::AppPaths) -> anyhow::Result<DelayCache> {
    let path = paths.delay_cache_path();
    if !path.exists() {
        return Ok(DelayCache::new());
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&content)?)
}

pub(crate) async fn tun_toggle_with_client(
    client: &impl MihomoApiClient,
    action: Option<crate::TunAction>,
    stack: Option<crate::TunStack>,
    dns_hijack: Option<String>,
) -> anyhow::Result<()> {
    match action {
        Some(crate::TunAction::On) => {
            set_tun_with_client(client, true, stack.as_ref(), dns_hijack.as_deref()).await?;
            println!("TUN enabled");
            if let Some(stack) = stack {
                println!("  stack: {}", stack);
            }
            if let Some(dns_hijack) = dns_hijack {
                println!("  dns-hijack: {}", dns_hijack);
            }
        }
        Some(crate::TunAction::Off) => {
            set_tun_with_client(client, false, None, None).await?;
            println!("TUN disabled");
        }
        Some(crate::TunAction::Status) | None => {
            let data = client.get("/configs").await?;
            println!(
                "TUN is {}",
                if data["tun"]["enable"].as_bool().unwrap_or(false) {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            if let Some(stack) = data["tun"]["stack"].as_str() {
                println!("  stack: {}", stack);
            }
            if let Some(hijack) = data["tun"]["dns-hijack"].as_array() {
                let values: Vec<&str> = hijack.iter().filter_map(|v| v.as_str()).collect();
                if !values.is_empty() {
                    println!("  dns-hijack: {}", values.join(", "));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn tun_patch_payload(
    enable: bool,
    stack: Option<&crate::TunStack>,
    dns_hijack: Option<&str>,
) -> serde_json::Value {
    let mut tun = serde_json::Map::new();
    tun.insert("enable".to_string(), serde_json::Value::Bool(enable));
    if let Some(stack) = stack {
        tun.insert(
            "stack".to_string(),
            serde_json::Value::String(stack.to_string()),
        );
    }
    if let Some(dns_hijack) = dns_hijack {
        tun.insert(
            "dns-hijack".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String(dns_hijack.to_string())]),
        );
    }
    serde_json::json!({"tun": tun})
}

pub(crate) async fn set_tun_with_client(
    client: &impl MihomoApiClient,
    enable: bool,
    stack: Option<&crate::TunStack>,
    dns_hijack: Option<&str>,
) -> anyhow::Result<Value> {
    client
        .patch("/configs", tun_patch_payload(enable, stack, dns_hijack))
        .await?;
    let data = client.get("/configs").await?;
    let actual = data["tun"]["enable"].as_bool().unwrap_or(false);
    if actual != enable {
        anyhow::bail!("TUN {} failed", if enable { "enable" } else { "disable" });
    }
    Ok(data)
}

pub(crate) async fn select_proxy_with_client(
    client: &impl MihomoApiClient,
    group: &str,
    node: &str,
) -> anyhow::Result<()> {
    client
        .put(&proxy_group_path(group), proxy_select_payload(node))
        .await?;
    Ok(())
}

pub(crate) async fn get_group_nodes_with_client(
    client: &impl MihomoApiClient,
    group: &str,
) -> anyhow::Result<Vec<String>> {
    let data = client.get(&proxy_group_path(group)).await?;
    let all = data["all"].as_array().ok_or_else(|| {
        anyhow::anyhow!(
            "Group '{}' not found or has no nodes.\n  Run: mihomo-cli list  (to see available groups)",
            group
        )
    })?;
    Ok(all
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionSummary {
    pub host: String,
    pub port: String,
    pub network: String,
    pub upload: u64,
    pub download: u64,
}

pub(crate) fn parse_connections(data: &Value) -> Vec<ConnectionSummary> {
    let conns = data["connections"].as_array().cloned().unwrap_or_default();
    conns
        .into_iter()
        .map(|c| {
            let meta = &c["metadata"];
            ConnectionSummary {
                host: meta["host"]
                    .as_str()
                    .or(meta["destinationIP"].as_str())
                    .unwrap_or("?")
                    .to_string(),
                port: meta["destinationPort"].as_str().unwrap_or("?").to_string(),
                network: meta["network"].as_str().unwrap_or("?").to_string(),
                upload: c["upload"].as_u64().unwrap_or(0),
                download: c["download"].as_u64().unwrap_or(0),
            }
        })
        .collect()
}

pub(crate) async fn connections_with_client(
    client: &impl MihomoApiClient,
    flush: bool,
) -> anyhow::Result<()> {
    if flush {
        close_connections_with_client(client).await?;
        println!("All connections closed");
        return Ok(());
    }
    let conns = get_connections_with_client(client).await?;
    println!("Active connections: {}", conns.len());
    for c in conns {
        println!(
            "  {}:{} ({})  ↑{} ↓{}",
            c.host, c.port, c.network, c.upload, c.download
        );
    }
    Ok(())
}

pub(crate) async fn get_connections_with_client(
    client: &impl MihomoApiClient,
) -> anyhow::Result<Vec<ConnectionSummary>> {
    let data = client.get("/connections").await?;
    Ok(parse_connections(&data))
}

pub(crate) async fn close_connections_with_client(
    client: &impl MihomoApiClient,
) -> anyhow::Result<()> {
    client.delete("/connections").await?;
    Ok(())
}

#[allow(dead_code)]
pub(crate) async fn get_config_with_client(client: &impl MihomoApiClient) -> anyhow::Result<Value> {
    client.get("/configs").await
}

pub(crate) async fn reload_configs_with_client(
    client: &impl MihomoApiClient,
    config_path: &str,
) -> anyhow::Result<()> {
    client
        .put("/configs", reload_config_payload(config_path))
        .await?;
    Ok(())
}

/// Parse /proxies JSON into flat list: (group, node, is_current)
pub fn parse_selector_nodes(data: &Value) -> Vec<(String, String, bool)> {
    let mut result = Vec::new();
    let proxies = match data.get("proxies").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return result,
    };

    let selectable_types = ["Selector", "URLTest", "Fallback"];

    for (group_name, group) in proxies {
        let ptype = group["type"].as_str().unwrap_or("");
        if !selectable_types.contains(&ptype) {
            continue;
        }
        let now = group["now"].as_str().unwrap_or("");
        if let Some(all) = group["all"].as_array() {
            for node in all {
                let node_name = node.as_str().unwrap_or("");
                if node_name.is_empty() {
                    continue;
                }
                result.push((
                    group_name.to_string(),
                    node_name.to_string(),
                    node_name == now,
                ));
            }
        }
    }
    result
}

pub(crate) async fn get_version_with_client(
    client: &impl MihomoApiClient,
) -> anyhow::Result<String> {
    let data = client.get("/version").await?;
    data["version"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Cannot read version from mihomo API"))
}

pub(crate) async fn get_port_with_client(client: &impl MihomoApiClient) -> anyhow::Result<u16> {
    let data = client.get("/configs").await?;
    for key in ["mixed-port", "port", "socks-port"] {
        if let Some(port) = data[key].as_u64() {
            let port = u16::try_from(port)
                .map_err(|_| anyhow::anyhow!("mihomo proxy port out of range: {port}"))?;
            if key == "socks-port" {
                eprintln!("  ⚠ Only socks-port detected — HTTP proxy may not be available");
            }
            return Ok(port);
        }
    }
    anyhow::bail!("Cannot read any port (mixed-port/port/socks-port) from mihomo config")
}

#[allow(dead_code)]
fn print_proxy_probe_rule_hint(source: &str, verbose: bool) {
    let paths = crate::utils::AppPaths::from_system();
    let Ok(matched) = crate::rules::test_rule_match_at(&paths, source) else {
        return;
    };
    let Some(matched) = matched else {
        return;
    };

    if verbose {
        println!("  Probe Rule:    {}", matched.rule);
        println!("  Probe Policy:  {}", matched.policy);
    }

    if matched.policy.eq_ignore_ascii_case("DIRECT") {
        println!("  Warning:       probe source matches DIRECT; result is not a proxy-node exit");
    } else if matched.policy.eq_ignore_ascii_case("REJECT") {
        println!("  Warning:       probe source matches REJECT; proxy probe may be unreliable");
    } else if matched.policy != "节点选择" {
        println!(
            "  Note:          probe source matches policy `{}`; result reflects that policy",
            matched.policy
        );
    }
}

/// Result of an IP probe: exit IP, country, source endpoint name.
#[derive(Clone)]
#[allow(dead_code)]
pub struct IpProbeResult {
    pub ip: String,
    pub country: String,
    pub source: String,
}

#[allow(dead_code)]
type IpSourceParser = fn(&serde_json::Value) -> Option<(String, String)>;

#[allow(dead_code)]
const IP_SOURCES: &[(&str, &str, IpSourceParser)] = &[
    ("https://api.ip.sb/geoip", "api.ip.sb", |v| {
        let ip = v["ip"].as_str()?;
        let country = v["country"].as_str()?;
        Some((ip.to_string(), country.to_string()))
    }),
    (
        "http://ip-api.com/json?fields=query,country",
        "ip-api.com",
        |v| {
            let ip = v["query"].as_str()?;
            let country = v["country"].as_str()?;
            Some((ip.to_string(), country.to_string()))
        },
    ),
    ("https://ifconfig.me/all.json", "ifconfig.me", |v| {
        let ip = v["ip_addr"].as_str()?;
        let country = v["country"].as_str().unwrap_or("?");
        Some((ip.to_string(), country.to_string()))
    }),
];

#[allow(dead_code)]
async fn query_ip_source(
    client: reqwest::Client,
    url: &'static str,
    name: &'static str,
    parse: fn(&serde_json::Value) -> Option<(String, String)>,
) -> Option<IpProbeResult> {
    let resp = client
        .get(url)
        .header("User-Agent", "mihomo-cli")
        .send()
        .await
        .ok()?;
    let data: serde_json::Value = resp.json().await.ok()?;
    let (ip, country) = parse(&data)?;
    Some(IpProbeResult {
        ip,
        country,
        source: name.to_string(),
    })
}

/// Fast IP probe for `status`.
///
/// This uses hedged fallback instead of sequential fallback:
/// - source 0 starts immediately;
/// - source 1 starts after 300ms;
/// - source 2 starts after 600ms;
/// - the first successful result wins;
/// - the whole probe is bounded by `total_timeout`.
///
/// This keeps `status` useful (it still shows the current exit IP) without
/// letting one slow third-party endpoint block the whole command.
/// Build a reqwest client for the given probe mode.
#[allow(dead_code)]
fn build_ip_client_for_proxy_port(proxy_port: Option<u16>) -> anyhow::Result<reqwest::Client> {
    let mut builder =
        crate::utils::http_client_builder().timeout(std::time::Duration::from_secs(5));

    if let Some(port) = proxy_port {
        let proxy_url = format!("http://127.0.0.1:{port}");
        builder = builder.proxy(reqwest::Proxy::all(&proxy_url)?);
    } else {
        // Direct connection — explicitly set empty proxy to bypass env vars
        // Note: no_proxy() sets a bypass list, not "disable proxy"
        builder = builder.proxy(reqwest::Proxy::custom(|_: &reqwest::Url| {
            None::<reqwest::Url>
        }));
    }

    Ok(builder.build()?)
}

/// Fast status-oriented IP probe via a specific local mihomo proxy port.
pub async fn fetch_ip_info_fast_with_proxy_port(
    proxy_port: u16,
    timeout: Duration,
) -> anyhow::Result<(String, String, String)> {
    let client = build_ip_client_for_proxy_port(Some(proxy_port))?;
    let mut tasks = futures::stream::FuturesUnordered::new();
    for (idx, (url, name, parse)) in IP_SOURCES.iter().enumerate() {
        let client = client.clone();
        let url = *url;
        let name = *name;
        let parse = *parse;
        tasks.push(async move {
            if idx > 0 {
                tokio::time::sleep(Duration::from_millis((idx as u64) * 300)).await;
            }
            query_ip_source(client, url, name, parse).await
        });
    }
    let winner = async {
        use futures::stream::StreamExt;
        while let Some(result) = tasks.next().await {
            if result.is_some() {
                return result;
            }
        }
        None
    };
    match tokio::time::timeout(timeout, winner).await.ok().flatten() {
        Some(r) => Ok((r.ip, r.country, r.source)),
        None => anyhow::bail!("IP check timed out or all endpoints unreachable via proxy"),
    }
}

/// Fast IP probe without mihomo or environment proxies.
pub async fn fetch_ip_info_direct(timeout: Duration) -> anyhow::Result<(String, String, String)> {
    let client = build_ip_client_for_proxy_port(None)?;
    let mut tasks = futures::stream::FuturesUnordered::new();
    for (idx, (url, name, parse)) in IP_SOURCES.iter().enumerate() {
        let client = client.clone();
        let url = *url;
        let name = *name;
        let parse = *parse;
        tasks.push(async move {
            if idx > 0 {
                tokio::time::sleep(Duration::from_millis((idx as u64) * 300)).await;
            }
            query_ip_source(client, url, name, parse).await
        });
    }
    let winner = async {
        use futures::stream::StreamExt;
        while let Some(result) = tasks.next().await {
            if result.is_some() {
                return result;
            }
        }
        None
    };
    match tokio::time::timeout(timeout, winner).await.ok().flatten() {
        Some(r) => Ok((r.ip, r.country, r.source)),
        None => anyhow::bail!("direct IP check timed out or all endpoints unreachable"),
    }
}

#[allow(dead_code)]
fn get_mihomo_port() -> u16 {
    let path = crate::utils::config_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("mixed-port:") {
                if let Ok(port) = val.trim().parse() {
                    return port;
                }
            }
        }
    }
    7897 // fallback default
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    type RecordedCalls = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    struct FakeSocketTransport {
        response: anyhow::Result<Vec<u8>>,
        calls: RecordedCalls,
    }

    impl FakeSocketTransport {
        fn ok(response: &[u8]) -> Self {
            Self {
                response: Ok(response.to_vec()),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn err(message: &str) -> Self {
            Self {
                response: Err(anyhow::anyhow!(message.to_string())),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn calls(&self) -> Vec<(String, Vec<u8>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SocketTransport for FakeSocketTransport {
        async fn roundtrip(&self, socket: &str, request: &[u8]) -> anyhow::Result<Vec<u8>> {
            self.calls
                .lock()
                .unwrap()
                .push((socket.to_string(), request.to_vec()));
            match &self.response {
                Ok(response) => Ok(response.clone()),
                Err(error) => Err(anyhow::anyhow!(error.to_string())),
            }
        }
    }

    #[test]
    fn normal_proxies() {
        let data = json!({
            "proxies": {
                "节点选择": {
                    "type": "Selector",
                    "now": "韩国KR-HY2",
                    "all": ["自动选择", "韩国KR-HY2", "日本JP-HY2", "DIRECT"]
                },
                "自动选择": {
                    "type": "URLTest",
                    "now": "日本JP-HY2",
                    "all": ["日本JP-HY2", "新加坡SG-HY2"]
                },
                "non_selectable": {
                    "type": "Vmess",
                    "now": "x",
                    "all": ["should be ignored"]
                }
            }
        });
        let result = parse_selector_nodes(&data);
        // 节点选择: 4 nodes + 自动选择: 2 nodes = 6
        assert_eq!(result.len(), 6);

        let current: Vec<_> = result.iter().filter(|(_, _, c)| *c).collect();
        assert_eq!(current.len(), 2);
        // Sort by node name for deterministic order
        let mut current_sorted: Vec<&str> = current.iter().map(|(_, n, _)| n.as_str()).collect();
        current_sorted.sort();
        assert_eq!(current_sorted, vec!["日本JP-HY2", "韩国KR-HY2"]);
    }

    #[test]
    fn empty_proxies() {
        let data = json!({"proxies": {}});
        let result = parse_selector_nodes(&data);
        assert!(result.is_empty());
    }

    #[test]
    fn missing_proxies_key() {
        let data = json!({});
        let result = parse_selector_nodes(&data);
        assert!(result.is_empty());
    }

    #[test]
    fn only_non_selectable() {
        let data = json!({
            "proxies": {
                "vmess1": {"type": "Vmess", "now": "x", "all": ["a"]},
                "ss1": {"type": "Shadowsocks", "now": "y", "all": ["b"]}
            }
        });
        let result = parse_selector_nodes(&data);
        assert!(result.is_empty());
    }

    #[test]
    fn empty_all_array() {
        let data = json!({
            "proxies": {
                "empty_group": {
                    "type": "Selector",
                    "now": "",
                    "all": []
                }
            }
        });
        let result = parse_selector_nodes(&data);
        assert!(result.is_empty());
    }

    // ── Failure path tests ──

    #[test]
    fn test_percent_encode_path_ascii() {
        assert_eq!(percent_encode_path("/configs"), "/configs");
        assert_eq!(
            percent_encode_path("/proxies/节点选择"),
            "/proxies/%E8%8A%82%E7%82%B9%E9%80%89%E6%8B%A9"
        );
    }

    #[test]
    fn test_percent_encode_path_already_encoded() {
        // % should be encoded to %25 (prevents double-encoding regression)
        let already = "/group/%E8%8A%82%E7%82%B9/delay";
        assert_eq!(
            percent_encode_path(already),
            "/group/%25E8%258A%2582%25E7%2582%25B9/delay"
        );
    }

    #[test]
    fn test_percent_encode_path_preserves_query_delimiters() {
        // ? = & are NOT in the allowlist, so they get encoded
        // Callers must construct query strings AFTER percent-encoding the path
        assert_eq!(
            percent_encode_path("/delay?url=test&timeout=5"),
            "/delay%3Furl%3Dtest%26timeout%3D5"
        );
    }

    #[test]
    fn test_encode_query_param_url() {
        assert_eq!(
            encode_query_param("http://example.com"),
            "http%3A%2F%2Fexample.com"
        );
    }

    #[test]
    fn test_encode_query_param_special() {
        assert_eq!(encode_query_param("a b&c=d#?"), "a%20b%26c%3Dd%23%3F");
    }

    #[test]
    fn test_encode_query_param_edge_cases() {
        assert_eq!(encode_query_param(""), "");
        assert_eq!(encode_query_param("hello"), "hello");
        assert_eq!(encode_query_param("a~b.c-d_e"), "a~b.c-d_e");
    }

    #[tokio::test]
    async fn socket_request_with_transport_builds_request_and_parses_response() {
        let transport = FakeSocketTransport::ok(
            b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"mode\":\"rule\"}",
        );

        let parsed = do_socket_request_with(
            &transport,
            "/tmp/mihomo.sock",
            "PATCH",
            "/configs",
            Some(br#"{"mode":"rule"}"#),
        )
        .await
        .unwrap();

        assert_eq!(parsed["mode"].as_str(), Some("rule"));
        let calls = transport.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "/tmp/mihomo.sock");
        let request = String::from_utf8(calls[0].1.clone()).unwrap();
        assert!(request.starts_with("PATCH /configs HTTP/1.0\r\n"));
        assert!(request.contains("Content-Length: 15\r\n"));
        assert!(request.ends_with("\r\n\r\n{\"mode\":\"rule\"}"));
    }

    #[tokio::test]
    async fn socket_request_with_transport_propagates_transport_errors_before_parsing() {
        let transport = FakeSocketTransport::err("boom");

        let err = do_socket_request_with(&transport, "/tmp/mihomo.sock", "GET", "/configs", None)
            .await
            .unwrap_err()
            .to_string();

        assert_eq!(err, "boom");
        let calls = transport.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            String::from_utf8(calls[0].1.clone()).unwrap(),
            "GET /configs HTTP/1.0\r\nHost: localhost\r\n\r\n"
        );
    }

    #[test]
    fn socket_http_request_builder_includes_json_headers_and_length() {
        let request = String::from_utf8(build_socket_http_request(
            "PUT",
            "/configs",
            Some(br#"{"path":"/tmp/config.yaml"}"#),
        ))
        .unwrap();

        assert!(request.starts_with("PUT /configs HTTP/1.0\r\n"));
        assert!(request.contains("Host: localhost\r\n"));
        assert!(request.contains("Content-Type: application/json\r\n"));
        assert!(request.contains("Content-Length: 27\r\n"));
        assert!(request.ends_with("\r\n\r\n{\"path\":\"/tmp/config.yaml\"}"));
    }

    #[test]
    fn socket_http_request_builder_omits_body_headers_for_get() {
        let request =
            String::from_utf8(build_socket_http_request("GET", "/proxies", None)).unwrap();
        assert_eq!(request, "GET /proxies HTTP/1.0\r\nHost: localhost\r\n\r\n");
    }

    #[test]
    fn socket_http_response_parser_accepts_json_and_empty_204() {
        let parsed = parse_socket_http_response(
            b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"mode\":\"rule\"}\n",
        )
        .unwrap();
        assert_eq!(parsed["mode"].as_str(), Some("rule"));

        let empty = parse_socket_http_response(b"HTTP/1.0 204 No Content\r\n\r\n").unwrap();
        assert!(empty.is_null());
    }

    #[test]
    fn socket_http_response_parser_rejects_error_status_and_bad_json() {
        let err = parse_socket_http_response(b"HTTP/1.0 404 Not Found\r\n\r\n{}")
            .unwrap_err()
            .to_string();
        assert!(err.contains("404 Not Found"));

        let err = parse_socket_http_response(b"HTTP/1.0 200 OK\r\n\r\nnot-json")
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected") || err.contains("JSON"));
    }

    #[test]
    fn api_path_and_payload_builders_encode_user_supplied_names() {
        assert_eq!(
            proxy_group_path("节点选择/香港"),
            "/proxies/%E8%8A%82%E7%82%B9%E9%80%89%E6%8B%A9/%E9%A6%99%E6%B8%AF"
        );
        assert_eq!(
            proxy_select_payload("HK 01"),
            serde_json::json!({"name": "HK 01"})
        );
        assert_eq!(
            delay_test_path("节点选择"),
            "/group/%E8%8A%82%E7%82%B9%E9%80%89%E6%8B%A9/delay?url=http%3A%2F%2Fwww.gstatic.com%2Fgenerate_204&timeout=5000"
        );
        assert_eq!(
            reload_config_payload("/tmp/mihomo config/config.yaml"),
            serde_json::json!({"path": "/tmp/mihomo config/config.yaml"})
        );
    }
}

#[cfg(test)]
mod api_client_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeApiClient {
        calls: Mutex<Vec<(String, String, Value)>>,
        get_responses: Mutex<Vec<Value>>,
    }

    impl FakeApiClient {
        fn with_get_responses(responses: Vec<Value>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                get_responses: Mutex::new(responses.into_iter().rev().collect()),
            }
        }

        fn calls(&self) -> Vec<(String, String, Value)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl MihomoApiClient for FakeApiClient {
        async fn get(&self, path: &str) -> anyhow::Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push(("GET".to_string(), path.to_string(), Value::Null));
            self.get_responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| anyhow::anyhow!("no fake GET response for {path}"))
        }

        async fn put(&self, path: &str, body: Value) -> anyhow::Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push(("PUT".to_string(), path.to_string(), body));
            Ok(Value::Null)
        }

        async fn patch(&self, path: &str, body: Value) -> anyhow::Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push(("PATCH".to_string(), path.to_string(), body));
            Ok(Value::Null)
        }

        async fn delete(&self, path: &str) -> anyhow::Result<Value> {
            self.calls
                .lock()
                .unwrap()
                .push(("DELETE".to_string(), path.to_string(), Value::Null));
            Ok(Value::Null)
        }
    }

    #[tokio::test]
    async fn select_proxy_uses_encoded_group_path_and_name_payload() {
        let client = FakeApiClient::default();

        select_proxy_with_client(&client, "节点选择", "HK 01")
            .await
            .unwrap();

        assert_eq!(
            client.calls(),
            vec![(
                "PUT".to_string(),
                "/proxies/%E8%8A%82%E7%82%B9%E9%80%89%E6%8B%A9".to_string(),
                json!({"name": "HK 01"}),
            )]
        );
    }

    #[tokio::test]
    async fn get_group_nodes_reads_all_array_through_client() {
        let client = FakeApiClient::with_get_responses(vec![json!({
            "all": ["DIRECT", "HK 01", 42, "US 02"]
        })]);

        let nodes = get_group_nodes_with_client(&client, "节点选择")
            .await
            .unwrap();

        assert_eq!(nodes, vec!["DIRECT", "HK 01", "US 02"]);
        assert_eq!(client.calls()[0].0, "GET");
        assert_eq!(
            client.calls()[0].1,
            "/proxies/%E8%8A%82%E7%82%B9%E9%80%89%E6%8B%A9"
        );
    }

    #[tokio::test]
    async fn fetch_delay_results_uses_delay_endpoint_and_sorts_results() {
        let client = FakeApiClient::with_get_responses(vec![json!({
            "slow": 200,
            "fast": 20,
            "timeout": "timeout"
        })]);

        let results = fetch_group_delay_results_with_client(&client, "节点选择")
            .await
            .unwrap();

        assert_eq!(
            client.calls()[0].1,
            "/group/%E8%8A%82%E7%82%B9%E9%80%89%E6%8B%A9/delay?url=http%3A%2F%2Fwww.gstatic.com%2Fgenerate_204&timeout=5000"
        );
        assert_eq!(
            results,
            vec![
                DelayResult {
                    node: "fast".to_string(),
                    ms: Some(20)
                },
                DelayResult {
                    node: "slow".to_string(),
                    ms: Some(200)
                },
                DelayResult {
                    node: "timeout".to_string(),
                    ms: None
                },
            ]
        );
    }

    #[tokio::test]
    async fn set_tun_with_client_patches_then_verifies_config() {
        let client = FakeApiClient::with_get_responses(vec![json!({
            "tun": {"enable": true, "stack": "gvisor"}
        })]);

        let data = set_tun_with_client(
            &client,
            true,
            Some(&crate::TunStack::Gvisor),
            Some("any:53"),
        )
        .await
        .unwrap();

        assert_eq!(data["tun"]["enable"].as_bool(), Some(true));
        let calls = client.calls();
        assert_eq!(calls[0].0, "PATCH");
        assert_eq!(calls[0].1, "/configs");
        assert_eq!(calls[0].2["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(calls[0].2["tun"]["stack"].as_str(), Some("gvisor"));
        assert_eq!(calls[0].2["tun"]["dns-hijack"][0].as_str(), Some("any:53"));
        assert_eq!(
            calls[1],
            ("GET".to_string(), "/configs".to_string(), Value::Null)
        );
    }

    #[tokio::test]
    async fn set_tun_with_client_fails_when_verification_disagrees() {
        let client = FakeApiClient::with_get_responses(vec![json!({
            "tun": {"enable": false}
        })]);

        let err = set_tun_with_client(&client, true, None, None)
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("TUN enable failed"));
    }

    #[tokio::test]
    async fn get_connections_with_client_parses_host_ip_fallback_and_traffic() {
        let client = FakeApiClient::with_get_responses(vec![json!({
            "connections": [
                {
                    "metadata": {"host": "example.com", "destinationPort": "443", "network": "tcp"},
                    "upload": 10,
                    "download": 20
                },
                {
                    "metadata": {"destinationIP": "1.2.3.4", "destinationPort": "53", "network": "udp"},
                    "upload": 0,
                    "download": 5
                }
            ]
        })]);

        let conns = get_connections_with_client(&client).await.unwrap();

        assert_eq!(
            conns,
            vec![
                ConnectionSummary {
                    host: "example.com".to_string(),
                    port: "443".to_string(),
                    network: "tcp".to_string(),
                    upload: 10,
                    download: 20,
                },
                ConnectionSummary {
                    host: "1.2.3.4".to_string(),
                    port: "53".to_string(),
                    network: "udp".to_string(),
                    upload: 0,
                    download: 5,
                },
            ]
        );
        assert_eq!(
            client.calls()[0],
            ("GET".to_string(), "/connections".to_string(), Value::Null)
        );
    }

    #[tokio::test]
    async fn close_connections_with_client_deletes_connections_endpoint() {
        let client = FakeApiClient::default();

        close_connections_with_client(&client).await.unwrap();

        assert_eq!(
            client.calls(),
            vec![(
                "DELETE".to_string(),
                "/connections".to_string(),
                Value::Null
            )]
        );
    }

    #[tokio::test]
    async fn get_config_and_get_port_use_configs_endpoint() {
        let client = FakeApiClient::with_get_responses(vec![json!({"mixed-port": 7890})]);
        assert_eq!(get_port_with_client(&client).await.unwrap(), 7890);
        assert_eq!(
            client.calls()[0],
            ("GET".to_string(), "/configs".to_string(), Value::Null)
        );

        let client = FakeApiClient::with_get_responses(vec![json!({"mode": "rule"})]);
        let config = get_config_with_client(&client).await.unwrap();
        assert_eq!(config["mode"].as_str(), Some("rule"));
    }

    #[tokio::test]
    async fn get_port_falls_back_through_port_chain() {
        let client = FakeApiClient::with_get_responses(vec![json!({"port": 7891})]);
        assert_eq!(get_port_with_client(&client).await.unwrap(), 7891);

        let client = FakeApiClient::with_get_responses(vec![json!({"socks-port": 7892})]);
        assert_eq!(get_port_with_client(&client).await.unwrap(), 7892);

        let client = FakeApiClient::with_get_responses(vec![json!({"mode": "rule"})]);
        assert!(get_port_with_client(&client).await.is_err());
    }

    #[tokio::test]
    async fn get_port_rejects_out_of_range_values() {
        let client = FakeApiClient::with_get_responses(vec![json!({"mixed-port": 70000})]);
        let err = get_port_with_client(&client).await.unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[tokio::test]
    async fn reload_configs_with_client_puts_path_payload() {
        let client = FakeApiClient::default();

        reload_configs_with_client(&client, "/tmp/mihomo/config.yaml")
            .await
            .unwrap();

        assert_eq!(
            client.calls(),
            vec![(
                "PUT".to_string(),
                "/configs".to_string(),
                json!({"path": "/tmp/mihomo/config.yaml"}),
            )]
        );
    }
}

#[cfg(test)]
mod delay_tests {
    use super::{fastest_delay_result, parse_delay_response, DelayResult};
    use serde_json::json;

    #[test]
    fn parse_delay_response_sorts_successes_before_timeouts() {
        let parsed = parse_delay_response(&json!({
            "timeout-node": "timeout",
            "slow": 320,
            "fast": 80
        }))
        .unwrap();
        assert_eq!(
            parsed,
            vec![
                DelayResult {
                    node: "fast".to_string(),
                    ms: Some(80)
                },
                DelayResult {
                    node: "slow".to_string(),
                    ms: Some(320)
                },
                DelayResult {
                    node: "timeout-node".to_string(),
                    ms: None
                },
            ]
        );
    }

    #[test]
    fn fastest_delay_result_ignores_timeouts() {
        let results = vec![
            DelayResult {
                node: "timeout".to_string(),
                ms: None,
            },
            DelayResult {
                node: "slow".to_string(),
                ms: Some(200),
            },
            DelayResult {
                node: "fast".to_string(),
                ms: Some(40),
            },
        ];
        assert_eq!(fastest_delay_result(&results).unwrap().node, "fast");
    }
}

#[cfg(test)]
mod tun_tests {
    #[test]
    fn tun_patch_payload_includes_stack_and_dns_hijack() {
        let payload =
            super::tun_patch_payload(true, Some(&crate::TunStack::Gvisor), Some("any:53"));
        assert_eq!(payload["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(payload["tun"]["stack"].as_str(), Some("gvisor"));
        assert_eq!(payload["tun"]["dns-hijack"][0].as_str(), Some("any:53"));
    }

    #[test]
    fn tun_patch_payload_off_only_sets_enable_false() {
        let payload = super::tun_patch_payload(false, None, None);
        assert_eq!(payload["tun"]["enable"].as_bool(), Some(false));
        assert!(payload["tun"].get("stack").is_none());
        assert!(payload["tun"].get("dns-hijack").is_none());
    }
}
