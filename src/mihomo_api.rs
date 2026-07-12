use serde_json::Value;

#[cfg(unix)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(windows)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::windows::named_pipe::ClientOptions;

/// Lightweight check: does the socket file exist?
pub fn socket_file_exists() -> bool {
    std::path::Path::new(socket_path()).exists()
}

/// Check if the socket is actually connectable (not just file exists).
/// Returns true if a connection attempt succeeds.
pub fn socket_is_alive() -> bool {
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
            .open(socket_path())
            .is_ok()
    }
}

/// Return the socket path for display purposes.
pub fn socket_path_display() -> &'static str {
    socket_path()
}

/// Build a context-aware fix suggestion message when socket/API is unreachable.
pub fn socket_fix_suggestion() -> String {
    #[cfg(not(unix))]
    {
        return "  Is mihomo running? Run: mihomo-cli status".to_string();
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

fn socket_path() -> &'static str {
    #[cfg(unix)]
    { "/tmp/verge/verge-mihomo.sock" }
    #[cfg(windows)]
    { r"\\.\pipe\mihomo" }
}

/// Percent-encode non-ASCII and reserved characters in the URL path.
fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
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
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

async fn socket_request(method: &str, path: &str, body: Option<&[u8]>) -> anyhow::Result<Value> {
    crate::log!("API {} {}", method, path);

    let result = do_socket_request(method, path, body).await;
    match result {
        Err(e) => {
            crate::log!("socket_request failed: {e}");
            let suggestion = socket_fix_suggestion();
            anyhow::bail!(
                "Cannot reach mihomo: {e}\n  \
                 Socket: {}\n\
                 {suggestion}",
                socket_path()
            )
        }
        ok => ok,
    }
}

async fn do_socket_request(method: &str, path: &str, body: Option<&[u8]>) -> anyhow::Result<Value> {
    let body_str = body.map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_default();
    let request = if !body_str.is_empty() {
        format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", body_str.len(), body_str)
    } else {
        format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\n\r\n")
    };

    #[cfg(unix)]
    let response = {
        let mut stream = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            UnixStream::connect(socket_path()),
        ).await??;
        stream.write_all(request.as_bytes()).await?;
        let mut data = Vec::new();
        let mut buf = vec![0u8; 8192];
        loop {
            let n = stream.read(&mut buf).await?;
            if n == 0 { break; }
            data.extend_from_slice(&buf[..n]);
        }
        data
    };

    #[cfg(windows)]
    let response = {
        let mut stream = ClientOptions::new()
            .open(socket_path())?;
        stream.write_all(request.as_bytes()).await?;
        let mut data = Vec::new();
        let mut buf = vec![0u8; 8192];
        loop {
            let n = stream.read(&mut buf).await?;
            if n == 0 { break; }
            data.extend_from_slice(&buf[..n]);
        }
        data
    };

    let response_str = String::from_utf8_lossy(&response);
    // Check HTTP status line before parsing body
    let status_line = response_str.lines().next().unwrap_or("");
    crate::log!("  HTTP status: {}", status_line);
    if !status_line.contains("200") && !status_line.contains("204") {
        anyhow::bail!("mihomo API returned: {}", status_line);
    }
    let body_start = response_str.find("\r\n\r\n").unwrap_or(0) + 4;
    let json_str = response_str[body_start..].trim();
    if json_str.is_empty() { return Ok(Value::Null); }
    Ok(serde_json::from_str(json_str)?)
}

async fn api_get(path: &str) -> anyhow::Result<Value> { socket_request("GET", path, None).await }
pub async fn api_put(path: &str, body: Value) -> anyhow::Result<Value> { socket_request("PUT", path, Some(&serde_json::to_vec(&body)?)).await }
pub async fn api_patch(path: &str, body: Value) -> anyhow::Result<Value> { socket_request("PATCH", path, Some(&serde_json::to_vec(&body)?)).await }
async fn api_delete(path: &str) -> anyhow::Result<Value> { socket_request("DELETE", path, None).await }

pub async fn get_all_proxies() -> anyhow::Result<Value> {
    api_get("/proxies").await
}

/// Poll /configs until the API is ready, or timeout.
/// Returns true if ready, false if timed out.
pub async fn wait_for_api_ready(timeout_secs: u64) -> bool {
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_secs(timeout_secs);
    let mut warned_5s = false;
    let mut warned_10s = false;
    while std::time::Instant::now() < deadline {
        match api_get("/configs").await {
            Ok(_) => return true,
            Err(e) => {
                crate::log!("connection to {} failed: {e}", socket_path());
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

pub async fn list_proxies() -> anyhow::Result<()> {
    let data = api_get("/proxies").await?;
    let proxies = data["proxies"].as_object().ok_or_else(|| anyhow::anyhow!("no proxies"))?;
    let mut pairs: Vec<(&String, &Value)> = proxies.iter().collect();
    pairs.sort_by_key(|(k, _)| *k);
    for (name, p) in pairs {
        let ptype = p["type"].as_str().unwrap_or("?");
        if !["Selector", "URLTest", "Fallback"].contains(&ptype) { continue; }
        let now = p["now"].as_str().unwrap_or("-");
        println!("[{ptype:8}] {name}  →  {now}");
        if let Some(all) = p["all"].as_array() {
            for sub in all { let s = sub.as_str().unwrap_or(""); if s != now { println!("          └ {s}"); } }
        }
    }
    Ok(())
}

pub async fn delay_test(group: &str) -> anyhow::Result<()> {
    let enc = percent_encode_path(group);
    let test_url = encode_query_param("http://www.gstatic.com/generate_204");
    let path = format!("/group/{enc}/delay?url={test_url}&timeout=5000");
    let data = api_get(&path).await?;
    let obj = data.as_object().ok_or_else(|| anyhow::anyhow!("no delay data"))?;
    let mut results: Vec<(u64, String)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (name, ms) in obj {
        if let Some(ms) = ms.as_u64() { results.push((ms, name.clone())); }
        else { errors.push(name.clone()); }
    }
    results.sort_by_key(|(ms, _)| *ms);
    for (ms, name) in &results { println!("{name}: {ms}ms"); }
    for name in &errors { println!("{name}: timeout"); }
    Ok(())
}

pub async fn tun_toggle(action: Option<crate::TunAction>) -> anyhow::Result<()> {
    match action {
        Some(crate::TunAction::On) => {
            // Check if TUN is supported (Linux user mode doesn't have root)
            if cfg!(target_os = "linux") {
                let mode = crate::utils::read_service_mode();
                if mode == "user" {
                    anyhow::bail!("TUN requires root privileges on Linux.\n  Reinstall with root mode: mihomo-cli install\n  Or use: sudo mihomo-cli install (without --user)");
                }
            }
            api_patch("/configs", serde_json::json!({"tun": {"enable": true}})).await?;
            // Verify the change took effect
            let data = api_get("/configs").await?;
            if data["tun"]["enable"].as_bool().unwrap_or(false) {
                println!("TUN enabled");
            } else {
                anyhow::bail!("TUN enable failed.");
            }
        }
        Some(crate::TunAction::Off) => {
            api_patch("/configs", serde_json::json!({"tun": {"enable": false}})).await?;
            let data = api_get("/configs").await?;
            if !data["tun"]["enable"].as_bool().unwrap_or(false) {
                println!("TUN disabled");
            } else {
                anyhow::bail!("TUN disable failed");
            }
        }
        None => {
            let data = api_get("/configs").await?;
            println!("TUN is {}", if data["tun"]["enable"].as_bool().unwrap_or(false) { "enabled" } else { "disabled" });
        }
    }
    Ok(())
}

pub async fn select_proxy(group: &str, node: &str) -> anyhow::Result<()> {
    let enc = percent_encode_path(group);
    api_put(&format!("/proxies/{enc}"), serde_json::json!({"name": node})).await?;
    Ok(())
}

pub async fn get_group_nodes(group: &str) -> anyhow::Result<Vec<String>> {
    let enc = percent_encode_path(group);
    let data = api_get(&format!("/proxies/{enc}")).await?;
    let all = data["all"].as_array().ok_or_else(|| {
        anyhow::anyhow!(
            "Group '{}' not found or has no nodes.\n  Run: mihomo-cli list  (to see available groups)",
            group
        )
    })?;
    Ok(all.iter().filter_map(|v| v.as_str().map(String::from)).collect())
}

pub async fn connections(flush: bool) -> anyhow::Result<()> {
    if flush {
        api_delete("/connections").await?;
        println!("All connections closed");
        return Ok(());
    }
    let data = api_get("/connections").await?;
    let conns = data["connections"].as_array().cloned().unwrap_or_default();
    println!("Active connections: {}", conns.len());
    for c in conns {
        let meta = &c["metadata"];
        let host = meta["host"].as_str().or(meta["destinationIP"].as_str()).unwrap_or("?");
        let port = meta["destinationPort"].as_str().unwrap_or("?");
        let net = meta["network"].as_str().unwrap_or("?");
        println!("  {host}:{port} ({net})  ↑{} ↓{}", c["upload"].as_u64().unwrap_or(0), c["download"].as_u64().unwrap_or(0));
    }
    Ok(())
}

pub async fn get_config() -> anyhow::Result<Value> {
    api_get("/configs").await
}

pub async fn reload_configs() -> anyhow::Result<()> {
    // Merge user rules and DNS policies into config before reloading
    crate::config::merge_user_config()?;

    let path = crate::utils::config_path();
    api_put("/configs", serde_json::json!({"path": path})).await?;
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

pub async fn get_port() -> anyhow::Result<u64> {
    let data = api_get("/configs").await?;
    data["mixed-port"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Cannot read mixed-port from mihomo config"))
}

pub async fn status() -> anyhow::Result<()> {
    let verbose = crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed);

    println!("=== Mihomo Status ===");

    let mihomo_path = crate::utils::mihomo_path();
    let config_path = crate::utils::config_path();
    let bin_ok = std::path::Path::new(&mihomo_path).exists();
    let cfg_ok = std::path::Path::new(&config_path).exists();

    // ── Prerequisites ──
    if !bin_ok {
        println!("  mihomo binary: NOT FOUND");
        println!("  Config:        {}", if cfg_ok { "exists" } else { "missing" });
        println!();
        println!("  Fix: mihomo-cli install");
        return Ok(());
    }
    if !cfg_ok {
        println!("  mihomo binary: installed");
        println!("  Config:        NOT FOUND");
        println!();
        println!("  Fix: mihomo-cli config");
        return Ok(());
    }

    // ── Check service health ──
    let svc_installed = crate::service::service_installed();
    let proc_alive = mihomo_process_running();
    let sock_exists = socket_file_exists();

    match api_get("/configs").await {
        Ok(data) => {
            // ── Healthy ──
            let mode = data["mode"].as_str().unwrap_or("?");
            let tun = data["tun"]["enable"].as_bool().unwrap_or(false);
            let port = data["mixed-port"].as_u64().unwrap_or(0);
            let svc_mode = crate::utils::read_service_mode();
            let svc_label = if svc_mode == "root" { "root" } else { "user" };

            println!("  mihomo:        running");
            println!("  Mode:          {mode}");
            println!("  TUN:           {}", if tun { "enabled" } else { "disabled" });
            println!("  Port:          {port}");
            println!("  Service:       {svc_label}");

            if let Ok(data) = api_get(&format!("/proxies/{}", percent_encode_path("节点选择"))).await {
                println!("  Node:          {}", data["now"].as_str().unwrap_or("?"));
            }

            println!("  Auto-start:    {}", if svc_installed { "enabled" } else { "disabled" });

            println!();
            print!("  Exit IP:       ");
            match fetch_ip_info().await {
                Ok((ip, country, source)) => println!("{ip} ({country}) via {source}"),
                Err(_) => println!("unreachable"),
            }
        }
        Err(_) => {
            // ── Unhealthy — diagnose ──
            let sock_alive = socket_is_alive();
            if verbose {
                println!();
                println!("=== Diagnostics ===");
                println!("  Binary:        ✅ {}", mihomo_path);
                println!("  Config:        ✅ {}", config_path);

                // Config syntax check
                if std::path::Path::new(&mihomo_path).exists() {
                    let output = std::process::Command::new(&mihomo_path)
                        .args(["-t", "-d", &crate::utils::config_dir()])
                        .output();
                    match output {
                        Ok(o) if o.status.success() =>
                            println!("  Config syntax: ✅ valid"),
                        Ok(o) => {
                            let stderr = String::from_utf8_lossy(&o.stderr);
                            println!("  Config syntax: ❌ invalid");
                            for line in stderr.lines().take(5) {
                                println!("    {line}");
                            }
                        }
                        Err(_) => println!("  Config syntax: ⚠ cannot test (mihomo -t failed)"),
                    }
                }

                // Socket controller check (only Unix socket, not TCP)
                let config_content = std::fs::read_to_string(&config_path).unwrap_or_default();
                if config_content.contains("external-controller-unix") {
                    println!("  API controller: ✅ configured");
                } else {
                    println!("  API controller: ❌ missing → Try: mihomo-cli config --fix");
                }

                // Socket status
                println!("  Socket file:   {} {}", if sock_exists { "✅" } else { "❌" }, socket_path_display());
                println!("  Socket alive:  {}", if sock_alive { "✅" } else { "❌" });
                println!("  Process:       {}", if proc_alive { "✅ running" } else { "❌ not running" });
                println!("  Service:       {} {}",
                    if svc_installed { "✅ installed" } else { "❌ not installed" },
                    if svc_installed { format!("({})", crate::utils::read_service_mode()) } else { String::new() }
                );

                // Recent logs
                let log_path = crate::utils::log_path();
                if std::path::Path::new(&log_path).exists() {
                    println!();
                    println!("  Recent logs (tail -20):");
                    if let Ok(content) = std::fs::read_to_string(&log_path) {
                        let lines: Vec<&str> = content.lines().collect();
                        for line in lines.iter().skip(lines.len().saturating_sub(20)) {
                            println!("    {line}");
                        }
                    }
                }

                // Fix suggestion
                println!();
                println!("  Suggestion:");
                println!("{}", socket_fix_suggestion());
            } else {
                println!("  mihomo binary: {}", mihomo_path);
                println!("  Config:        {}", config_path);

                if proc_alive && !sock_exists {
                    println!();
                    println!("  ⚠  mihomo is running but the API socket is missing.");
                    println!("     The socket file was deleted (this doesn't affect proxy traffic)");
                    println!("     but CLI commands like select/list/delay won't work.");
                } else if proc_alive {
                    println!();
                    println!("  ⚠  mihomo is running but unresponsive.");
                } else {
                    println!();
                    println!("  ❌ mihomo is NOT running.");
                }

                println!();
                println!("{}", socket_fix_suggestion());

                if svc_installed && !proc_alive {
                    println!();
                    println!("  Auto-start service is installed but the process is dead.");
                    println!("  Check logs: tail -f {}", crate::utils::log_path());
                }

                println!();
                println!("  Run: mihomo-cli status -v  for diagnostics");
            }
        }
    }

    Ok(())
}

/// Result of an IP probe: exit IP, country, source endpoint name.
#[derive(Clone)]
pub struct IpProbeResult {
    pub ip: String,
    pub country: String,
    pub source: String,
}

const IP_SOURCES: &[(&str, &str, fn(&serde_json::Value) -> Option<(String, String)>)] = &[
    ("https://api.ip.sb/geoip", "api.ip.sb", |v| {
        let ip = v["ip"].as_str()?;
        let country = v["country"].as_str()?;
        Some((ip.to_string(), country.to_string()))
    }),
    ("http://ip-api.com/json?fields=query,country", "ip-api.com", |v| {
        let ip = v["query"].as_str()?;
        let country = v["country"].as_str()?;
        Some((ip.to_string(), country.to_string()))
    }),
    ("https://ifconfig.me/all.json", "ifconfig.me", |v| {
        let ip = v["ip_addr"].as_str()?;
        let country = v["country"].as_str().unwrap_or("?");
        Some((ip.to_string(), country.to_string()))
    }),
];

/// Build a reqwest client for the given probe mode.
fn build_ip_client(use_proxy: bool) -> anyhow::Result<reqwest::Client> {
    let mut builder = crate::utils::http_client_builder()
        .timeout(std::time::Duration::from_secs(5));

    if use_proxy {
        let port = get_mihomo_port();
        let proxy_url = format!("http://127.0.0.1:{port}");
        builder = builder.proxy(reqwest::Proxy::all(&proxy_url)?);
    } else {
        // Direct connection — explicitly set empty proxy to bypass env vars
        // Note: no_proxy() sets a bypass list, not "disable proxy"
        builder = builder.proxy(reqwest::Proxy::custom(|_: &reqwest::Url| None::<reqwest::Url>));
    }

    Ok(builder.build()?)
}

/// Probe exit IP through a specific path (direct or mihomo proxy).
/// If `target_url` is provided, make a request to that URL first, then check exit IP.
/// Otherwise, use IP lookup APIs directly.
async fn probe_ip(use_proxy: bool, target_url: Option<&str>) -> Option<IpProbeResult> {
    let client = match build_ip_client(use_proxy) {
        Ok(c) => c,
        Err(_) => return None,
    };

    // If target_url is provided, make a request to it first
    if let Some(url) = target_url {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            client.get(url)
                .header("User-Agent", "mihomo-cli")
                .send(),
        ).await;
        // Request completed (or timed out), now check exit IP
    }

    // Check exit IP by requesting an IP echo API
    for (url, name, parse) in IP_SOURCES {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(8),
            async {
                let resp = client.get(*url)
                    .header("User-Agent", "mihomo-cli")
                    .send()
                    .await?;
                let data: serde_json::Value = resp.json().await?;
                parse(&data).ok_or_else(|| anyhow::anyhow!("unexpected response format"))
            },
        ).await;

        match result {
            Ok(Ok((ip, country))) => {
                let source = if target_url.is_some() {
                    format!("{} (after target)", name)
                } else {
                    name.to_string()
                };
                return Some(IpProbeResult {
                    ip,
                    country,
                    source,
                });
            },
            _ => continue,
        }
    }

    None
}

/// Check if an IP address is a LAN/private address.
fn is_lan_ip(ip: &str) -> bool {
    use std::net::IpAddr;
    
    if let Ok(addr) = ip.parse::<IpAddr>() {
        match addr {
            IpAddr::V4(v4) => {
                // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 127.0.0.0/8
                v4.is_private() || v4.is_loopback() || v4.is_link_local()
            }
            IpAddr::V6(v6) => {
                // fc00::/7 (unique local), fe80::/10 (link-local), ::1 (loopback)
                v6.is_loopback() || {
                    let segments = v6.segments();
                    // fc00::/7: first 7 bits are 1111110
                    (segments[0] & 0xfe00) == 0xfc00
                    // fe80::/10: link-local
                    || (segments[0] & 0xffc0) == 0xfe80
                }
            }
        }
    } else {
        false
    }
}

/// Format the exit IP report with environment info and three probe lines.
pub fn format_ip_report(
    tun_enabled: bool,
    http_proxy_val: Option<String>,
    https_proxy_val: Option<String>,
    isp: Option<IpProbeResult>,
    now: Option<IpProbeResult>,
    via_mihomo: Option<IpProbeResult>,
) -> String {
    let mut lines = Vec::new();

    lines.push("=== Exit IP Report ===".to_string());
    lines.push(String::new());

    lines.push(format!(
        "  TUN:           {}",
        if tun_enabled { "enabled" } else { "disabled" }
    ));
    lines.push(format!(
        "  http_proxy:    {}",
        http_proxy_val.as_deref().unwrap_or("not set")
    ));
    lines.push(format!(
        "  https_proxy:   {}",
        https_proxy_val.as_deref().unwrap_or("not set")
    ));
    lines.push(String::new());

    let format_ip_line = |r: &Option<IpProbeResult>| -> String {
        match r {
            Some(r) => {
                if is_lan_ip(&r.ip) {
                    format!("{}  [LAN]", r.ip)
                } else {
                    format!("{}  {}", r.ip, r.country)
                }
            }
            None => "(unreachable)".to_string(),
        }
    };

    // ISP line: annotated if from cache
    let show_cached = matches!(&isp, Some(r) if r.source == "cached");
    let isp_label = if show_cached {
        "  ISP (cached)     "
    } else {
        "  ISP               "
    };
    lines.push(format!("{}{}", isp_label, format_ip_line(&isp)));
    lines.push(format!("  Now               {}", format_ip_line(&now)));
    lines.push(format!("  Via Mihomo       {}", format_ip_line(&via_mihomo)));

    lines.push(String::new());

    // Diagnostic message
    match (&now, &via_mihomo) {
        (Some(n), Some(v)) if n.ip == v.ip => {
            if tun_enabled {
                lines.push("  → TUN enabled: system route exits via proxy".to_string());
            } else {
                lines.push("  → System route and proxy exit are the same — proxy may not be working".to_string());
            }
        }
        (Some(_), Some(_)) => {
            if tun_enabled {
                lines.push("  → TUN enabled: system route exits via proxy node".to_string());
            }
            // TUN off + different = normal, no message needed
        }
        (None, Some(_)) => lines.push("  → System route unreachable".to_string()),
        (Some(_), None) => lines.push("  → Mihomo proxy unreachable".to_string()),
        (None, None) => {
            if tun_enabled {
                lines.push("  → Both routes unreachable — TUN may not be fully connected".to_string());
            } else {
                lines.push("  → Both routes unreachable".to_string());
            }
        }
    }

    // LAN leak detection
    for (label, result) in [("Now", &now), ("Via Mihomo", &via_mihomo)] {
        if let Some(r) = result {
            if is_lan_ip(&r.ip) {
                lines.push(format!("  → {} exits via LAN address ({})", label, r.ip));
            }
        }
    }

    lines.join("\n")
}

fn read_isp_cache(path: &str) -> Option<IpProbeResult> {
    let content = std::fs::read_to_string(path).ok()?;
    let parts: Vec<&str> = content.trim().splitn(2, '|').collect();
    if parts.len() == 2 {
        Some(IpProbeResult {
            ip: parts[0].to_string(),
            country: parts[1].to_string(),
            source: "cached".to_string(),
        })
    } else {
        None
    }
}

fn write_isp_cache(path: &str, r: &IpProbeResult) {
    let data = format!("{}|{}", r.ip, r.country);
    let _ = std::fs::write(path, data);
}

/// Probe both System route and Mihomo proxy paths, return formatted report.
/// If `target_url` is provided, probe that URL instead of IP lookup APIs.
pub async fn probe_all_ips(target_url: Option<&str>) -> String {
    // Read TUN state from mihomo API
    let tun_enabled = match api_get("/configs").await {
        Ok(data) => data["tun"]["enable"].as_bool().unwrap_or(false),
        Err(_) => false,
    };

    let http_proxy_val = std::env::var("http_proxy").ok();
    let https_proxy_val = std::env::var("https_proxy").ok();
    let cache_path = crate::utils::isp_cache_path();

    let (isp, now, via_mihomo) = if tun_enabled {
        // TUN on: ISP from cache, Now + Via Mihomo probed
        let isp = read_isp_cache(&cache_path);
        let (now, via_mihomo) = tokio::join!(
            probe_ip(false, target_url),
            probe_ip(true, target_url),
        );
        (isp, now, via_mihomo)
    } else {
        // TUN off: ISP = Now (same direct probe), cache it
        let (result, via_mihomo) = tokio::join!(
            probe_ip(false, target_url),
            probe_ip(true, target_url),
        );
        if let Some(ref r) = result {
            write_isp_cache(&cache_path, r);
        }
        let isp = result.clone();
        (isp, result, via_mihomo)
    };

    format_ip_report(tun_enabled, http_proxy_val, https_proxy_val, isp, now, via_mihomo)
}

/// Backward-compatible: probe via mihomo proxy (original behavior).
pub async fn fetch_ip_info() -> anyhow::Result<(String, String, String)> {
    match probe_ip(true, None).await {
        Some(r) => Ok((r.ip, r.country, r.source)),
        None => anyhow::bail!("all IP check endpoints unreachable via proxy"),
    }
}

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

    #[test]
    fn test_format_ip_report_tun_disabled_normal() {
        let isp = Some(IpProbeResult {
            ip: "113.113.113.113".to_string(),
            country: "China".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let now = Some(IpProbeResult {
            ip: "113.113.113.113".to_string(),
            country: "China".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let via_mihomo = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let report = format_ip_report(
            false,
            None,
            None,
            isp,
            now,
            via_mihomo,
        );
        assert!(report.contains("TUN:           disabled"));
        assert!(report.contains("http_proxy:    not set"));
        assert!(report.contains("https_proxy:   not set"));
        assert!(report.contains("ISP               113.113.113.113  China"));
        assert!(report.contains("Now               113.113.113.113  China"));
        assert!(report.contains("Via Mihomo       1.2.3.4  United States"));
    }

    #[test]
    fn test_format_ip_report_tun_enabled_with_cache() {
        let isp = Some(IpProbeResult {
            ip: "113.113.113.113".to_string(),
            country: "China".to_string(),
            source: "cached".to_string(),
        });
        let now = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let via_mihomo = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let report = format_ip_report(
            true,
            Some("127.0.0.1:7890".to_string()),
            Some("127.0.0.1:7890".to_string()),
            isp,
            now,
            via_mihomo,
        );
        assert!(report.contains("TUN:           enabled"));
        assert!(report.contains("http_proxy:    127.0.0.1:7890"));
        assert!(report.contains("ISP (cached)     113.113.113.113  China"));
        assert!(report.contains("Now               1.2.3.4  United States"));
        assert!(report.contains("TUN enabled: system route exits via proxy"));
    }

    #[test]
    fn test_format_ip_report_tun_enabled_no_cache() {
        let now = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let via_mihomo = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let report = format_ip_report(
            true,
            None,
            None,
            None,  // no ISP cache
            now,
            via_mihomo,
        );
        assert!(report.contains("ISP               (unreachable)"));
    }

    #[test]
    fn test_format_ip_report_now_unreachable() {
        let isp = Some(IpProbeResult {
            ip: "113.113.113.113".to_string(),
            country: "China".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let via_mihomo = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let report = format_ip_report(
            false,
            None,
            None,
            isp,
            None,
            via_mihomo,
        );
        assert!(report.contains("Now               (unreachable)"));
        assert!(report.contains("System route unreachable"));
    }

    #[test]
    fn test_format_ip_report_via_mihomo_unreachable() {
        let isp = Some(IpProbeResult {
            ip: "113.113.113.113".to_string(),
            country: "China".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let now = Some(IpProbeResult {
            ip: "113.113.113.113".to_string(),
            country: "China".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let report = format_ip_report(
            false,
            None,
            None,
            isp,
            now,
            None,
        );
        assert!(report.contains("Via Mihomo       (unreachable)"));
        assert!(report.contains("Mihomo proxy unreachable"));
    }

    #[test]
    fn test_format_ip_report_both_unreachable() {
        let report = format_ip_report(
            false,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(report.contains("ISP               (unreachable)"));
        assert!(report.contains("Now               (unreachable)"));
        assert!(report.contains("Via Mihomo       (unreachable)"));
        assert!(report.contains("Both routes unreachable"));
    }

    #[test]
    fn test_format_ip_report_tun_disabled_same_ip() {
        let isp = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let now = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let via_mihomo = Some(IpProbeResult {
            ip: "1.2.3.4".to_string(),
            country: "United States".to_string(),
            source: "api.ip.sb".to_string(),
        });
        let report = format_ip_report(
            false,
            Some("127.0.0.1:7890".to_string()),
            Some("127.0.0.1:7890".to_string()),
            isp,
            now,
            via_mihomo,
        );
        assert!(report.contains("http_proxy:    127.0.0.1:7890"));
        assert!(report.contains("proxy may not be working"));
    }

    // ── Failure path tests ──

    #[test]
    fn test_percent_encode_path_ascii() {
        assert_eq!(percent_encode_path("/configs"), "/configs");
        assert_eq!(percent_encode_path("/proxies/节点选择"), "/proxies/%E8%8A%82%E7%82%B9%E9%80%89%E6%8B%A9");
    }

    #[test]
    fn test_percent_encode_path_already_encoded() {
        // % should be encoded to %25 (prevents double-encoding regression)
        let already = "/group/%E8%8A%82%E7%82%B9/delay";
        assert_eq!(percent_encode_path(already), "/group/%25E8%258A%2582%25E7%2582%25B9/delay");
    }

    #[test]
    fn test_percent_encode_path_preserves_query_delimiters() {
        // ? = & are NOT in the allowlist, so they get encoded
        // Callers must construct query strings AFTER percent-encoding the path
        assert_eq!(percent_encode_path("/delay?url=test&timeout=5"), "/delay%3Furl%3Dtest%26timeout%3D5");
    }

    #[test]
    fn test_encode_query_param_url() {
        assert_eq!(encode_query_param("http://example.com"), "http%3A%2F%2Fexample.com");
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
}
