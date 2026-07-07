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

async fn socket_request(method: &str, path: &str, body: Option<&[u8]>) -> anyhow::Result<Value> {
    crate::log!("API {} {}", method, path);

    let result = do_socket_request(method, path, body).await;
    match result {
        Err(e) => {
            anyhow::bail!(
                "Cannot reach mihomo: {e}\n  \
                 Is mihomo running? Run: mihomo-cli status\n  \
                 Socket: {}",
                socket_path()
            )
        }
        ok => ok,
    }
}

async fn do_socket_request(method: &str, path: &str, body: Option<&[u8]>) -> anyhow::Result<Value> {
    let encoded_path = percent_encode_path(path);
    let body_str = body.map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_default();
    let request = if !body_str.is_empty() {
        format!("{method} {encoded_path} HTTP/1.0\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", body_str.len(), body_str)
    } else {
        format!("{method} {encoded_path} HTTP/1.0\r\nHost: localhost\r\n\r\n")
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
        if api_get("/configs").await.is_ok() {
            return true;
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
    let path = format!("/proxies/{group}/delay?url=http://www.gstatic.com/generate_204&timeout=5000");
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
    api_put(&format!("/proxies/{group}"), serde_json::json!({"name": node})).await?;
    Ok(())
}

pub async fn get_group_nodes(group: &str) -> anyhow::Result<Vec<String>> {
    let data = api_get(&format!("/proxies/{group}")).await?;
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

pub async fn reload_configs() -> anyhow::Result<()> {
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

            if let Ok(data) = api_get("/proxies/节点选择").await {
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
            println!("  Fix: mihomo-cli restart");

            if svc_installed && !proc_alive {
                println!();
                println!("  Auto-start service is installed but the process is dead.");
                println!("  Check logs: tail -f {}", crate::utils::log_path());
            }
        }
    }

    Ok(())
}

pub async fn fetch_ip_info() -> anyhow::Result<(String, String, String)> {
    let port = get_mihomo_port();
    let proxy_url = format!("http://127.0.0.1:{port}");

    let sources: &[(&str, &str, fn(&serde_json::Value) -> Option<(String, String)>)] = &[
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

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url)?)
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    for (url, name, parse) in sources {
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
            Ok(Ok((ip, country))) => return Ok((ip, country, name.to_string())),
            _ => continue,
        }
    }

    anyhow::bail!("all IP check endpoints unreachable via proxy")
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
}
