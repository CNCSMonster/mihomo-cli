use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static TUN_ENABLED: AtomicBool = AtomicBool::new(false);

fn argument_value(args: &[String], flag: &str) -> Option<PathBuf> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .map(PathBuf::from)
}

fn config_path(args: &[String]) -> Option<PathBuf> {
    argument_value(args, "-f").or_else(|| argument_value(args, "-d").map(|dir| dir.join("config.yaml")))
}

fn socket_path(config: &Path) -> Result<PathBuf, String> {
    let content = fs::read_to_string(config)
        .map_err(|error| format!("cannot read {}: {error}", config.display()))?;
    content
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == "external-controller-unix").then(|| {
                PathBuf::from(value.trim().trim_start_matches("unix://"))
            })
        })
        .ok_or_else(|| "external-controller-unix is missing".to_string())
}

fn tun_enabled_in_config(config: &Path) -> Result<bool, String> {
    let content = fs::read_to_string(config)
        .map_err(|error| format!("cannot read {}: {error}", config.display()))?;
    let mut in_tun = false;
    for line in content.lines() {
        if line.starts_with("tun:") {
            in_tun = true;
            continue;
        }
        if in_tun
            && !line
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
            && !line.trim().is_empty()
        {
            break;
        }
        if in_tun && line.trim() == "enable: true" {
            return Ok(true);
        }
    }
    Ok(false)
}

fn record_validation(args: &[String]) {
    if let Ok(path) = std::env::var("MIHOMO_FAKE_VALIDATION_LOG") {
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{}", args.join(" "));
        }
    }
}

fn record_argv(args: &[String]) {
    let _ = fs::write("/var/run/mihomo/fake-core-argv", format!("{}\n", args.join("\n")));
}

fn read_request(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + length {
                break;
            }
        }
    }
    Ok(request)
}

fn handle(mut stream: UnixStream) -> std::io::Result<()> {
    let request = read_request(&mut stream)?;
    let request_text = String::from_utf8_lossy(&request);
    if request_text.starts_with("PUT /proxies/") {
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/var/run/mihomo/fake-core-selection-puts")
        {
            let line = request_text.lines().next().unwrap_or("").to_string();
            let body = request_text.rsplit("\r\n\r\n").next().unwrap_or("").to_string();
            let _ = writeln!(file, "{line} {body}");
        }
    }
    if request_text.starts_with("PATCH /configs ") {
        if request_text.contains("\"enable\":false") {
            TUN_ENABLED.store(false, Ordering::SeqCst);
        } else if request_text.contains("\"enable\":true") {
            TUN_ENABLED.store(true, Ordering::SeqCst);
        }
        stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    }

    if request_text.starts_with("GET /proxies/Test ") {
        let body = r#"{"type":"Selector","now":"DIRECT","all":["DIRECT"]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )?;
        return Ok(());
    }

    if request_text.starts_with("GET /proxies ") {
        let body = r#"{"proxies":{"Test":{"type":"Selector","now":"DIRECT","all":["DIRECT"]}}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )?;
        return Ok(());
    }

    let enabled = TUN_ENABLED.load(Ordering::SeqCst);
    let body = format!("{{\"mixed-port\":7890,\"tun\":{{\"enable\":{enabled}}}}}");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-v" || arg == "--version") {
        println!("Mihomo Meta fake-container-test");
        return Ok(());
    }
    if args.iter().any(|arg| arg == "-t") {
        record_validation(&args);
        let config = config_path(&args).ok_or_else(|| "validation config is missing".to_string())?;
        let content = fs::read_to_string(&config)
            .map_err(|error| format!("cannot validate {}: {error}", config.display()))?;
        if content.contains("not-a-real-proxy-type") || content.contains("simulated validation failure") {
            return Err("configuration validation failed: invalid proxy type".to_string());
        }
        println!("configuration file {} test is successful", config.display());
        return Ok(());
    }

    let config = config_path(&args).ok_or_else(|| "-f config path is required".to_string())?;
    TUN_ENABLED.store(tun_enabled_in_config(&config)?, Ordering::SeqCst);
    let socket = socket_path(&config)?;
    record_argv(&args);
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            let _ = handle(stream);
        }
    }
    Ok(())
}
