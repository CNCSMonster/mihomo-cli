use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::time::Duration;

pub async fn download_mihomo() -> anyhow::Result<()> {
    let (os_name, arch_name, bin_name) = if cfg!(target_os = "windows") {
        (
            "windows",
            match std::env::consts::ARCH {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                o => o,
            },
            "mihomo.exe",
        )
    } else if cfg!(target_os = "macos") {
        (
            "darwin",
            match std::env::consts::ARCH {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                o => o,
            },
            "mihomo",
        )
    } else {
        (
            "linux",
            match std::env::consts::ARCH {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                o => o,
            },
            "mihomo",
        )
    };

    let bin_path = if cfg!(target_os = "windows") {
        let local =
            dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
        format!("{}\\mihomo\\{}", local.display(), bin_name)
    } else {
        let home = dirs::home_dir().unwrap_or_default();
        format!("{}/.local/bin/{}", home.display(), bin_name)
    };

    if Path::new(&bin_path).exists() {
        println!("mihomo already installed at {bin_path}");
        return Ok(());
    }

    let version = "v1.19.27";
    let ext = if cfg!(target_os = "windows") {
        "zip"
    } else {
        "gz"
    };
    let url = format!(
        "https://github.com/MetaCubeX/mihomo/releases/download/{version}/mihomo-{os_name}-{arch_name}-{version}.{ext}"
    );

    println!("Downloading Mihomo core...");
    crate::log!("URL: {url}");

    let parent = Path::new(&bin_path).parent().unwrap();
    std::fs::create_dir_all(parent)?;

    // Download with resume + retry
    let part_path = format!("{bin_path}.part");
    let bytes = download_with_retry(&url, &part_path).await?;

    // Decompress & install
    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::with_template("{spinner:.green} {msg}")?);
    pb.set_message("Decompressing...");

    if cfg!(target_os = "windows") {
        let cursor = std::io::Cursor::new(&bytes);
        let mut archive = zip::ZipArchive::new(cursor)?;
        let tmp = tempfile::tempdir()?;
        archive.extract(tmp.path())?;
        for entry in std::fs::read_dir(tmp.path())? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("mihomo") && name.ends_with(".exe") {
                std::fs::copy(entry.path(), &bin_path)?;
                break;
            }
        }
    } else {
        let mut decoder = GzDecoder::new(&bytes[..]);
        let mut out = std::fs::File::create(&bin_path)?;
        std::io::copy(&mut decoder, &mut out)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&bin_path)?.permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&bin_path, p)?;
        }
    }

    pb.finish_with_message("Installed");
    let _ = std::fs::remove_file(&part_path);
    println!("Installed to {bin_path}");
    Ok(())
}

async fn download_with_retry(url: &str, part_path: &str) -> anyhow::Result<Vec<u8>> {
    let max_retries = 3;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;

    let mut last_error = String::new();

    for attempt in 0..max_retries {
        if attempt > 0 {
            let delay = Duration::from_secs(1 << (attempt - 1)); // 1s, 2s, 4s
            eprintln!(
                "  Retrying in {}s... (attempt {}/{})",
                delay.as_secs(),
                attempt + 1,
                max_retries
            );
            tokio::time::sleep(delay).await;
        }

        match download_once(&client, url, part_path, attempt).await {
            Ok(bytes) => return Ok(bytes),
            Err(e) => {
                last_error = e.to_string();
                crate::log!("attempt {} failed: {}", attempt + 1, last_error);
            }
        }
    }

    anyhow::bail!(
        "Download failed after {} attempts: {}",
        max_retries,
        last_error
    )
}

async fn download_once(
    client: &reqwest::Client,
    url: &str,
    part_path: &str,
    attempt: usize,
) -> anyhow::Result<Vec<u8>> {
    let resumed_size = if attempt > 0 {
        std::fs::metadata(part_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let mut request = client.get(url).header("User-Agent", "mihomo-cli");
    if resumed_size > 0 {
        crate::log!("Resuming from byte {}", resumed_size);
        request = request.header("Range", format!("bytes={resumed_size}-"));
    }

    let resp = request.send().await?;
    let status = resp.status();

    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // Server doesn't support resume or range already satisfied — start fresh
        crate::log!("Range not satisfiable, starting fresh");
        let _ = std::fs::remove_file(part_path);
        let resp = client
            .get(url)
            .header("User-Agent", "mihomo-cli")
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("HTTP {}", resp.status());
        }
        return download_body(resp, part_path, 0).await;
    }

    if !status.is_success() {
        anyhow::bail!("HTTP {}", status);
    }

    download_body(resp, part_path, resumed_size).await
}

async fn download_body(
    resp: reqwest::Response,
    part_path: &str,
    offset: u64,
) -> anyhow::Result<Vec<u8>> {
    let total = resp.content_length().map(|l| l + offset);
    let pb = ProgressBar::new(total.unwrap_or(0));
    if total.is_some() {
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} {bytes}/{total_bytes} [{elapsed_precise}] {bar:30.cyan/blue} {bytes_per_sec}")?
            .progress_chars("=>-"));
    } else {
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} {bytes} [{elapsed_precise}] {bytes_per_sec}")?,
        );
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(part_path)?;

    use futures::StreamExt;
    use std::io::Write;

    let mut stream = resp.bytes_stream();
    let mut total_bytes = offset;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        file.write_all(&chunk)?;
        total_bytes += chunk.len() as u64;
        pb.set_position(total_bytes);
    }
    file.flush()?;
    pb.finish_with_message("Done");

    // Read the complete file into memory for decompression
    Ok(std::fs::read(part_path)?)
}

// ── GeoIP / GeoSite pre-download ──

const GEOIP_URL: &str =
    "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geoip.metadb";
const GEOSITE_URL: &str =
    "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/GeoSite.dat";

/// Build mirror URLs from the primary GitHub URL.
/// jsDelivr mirrors repo files via CDN (has China POPs);
/// ghproxy proxies the raw GitHub URL.
fn build_mirrors(primary: &str) -> Vec<String> {
    // jsDelivr: https://cdn.jsdelivr.net/gh/{user}/{repo}@{tag}/{file}
    let jsdelivr = primary
        .replace("https://github.com/", "https://cdn.jsdelivr.net/gh/")
        .replace("/releases/download/", "@");
    vec![
        format!("https://mirror.ghproxy.com/{primary}"),
        format!("https://ghproxy.com/{primary}"),
        jsdelivr,
    ]
}

/// Download geo files to config_dir, with mirror fallback.
/// Returns true if all succeeded; false means partial or total failure (never blocks the main flow).
pub async fn ensure_geo_files() -> bool {
    let dir = crate::utils::config_dir();
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let mut ok = true;
    let gh_token = gh_token();

    for (name, primary_url) in [("geoip.metadb", GEOIP_URL), ("GeoSite.dat", GEOSITE_URL)] {
        let dest = format!("{dir}/{name}");
        let mut urls: Vec<String> = vec![primary_url.to_string()];
        urls.extend(build_mirrors(primary_url));

        if !download_geo_with_fallback(&client, &urls, &dest, gh_token.as_deref()).await {
            eprintln!("  ⚠ Failed to download {name} — mihomo will try at startup");
            ok = false;
        }
    }
    ok
}

/// Get GitHub token from `gh auth token` for authenticated API access.
fn gh_token() -> Option<String> {
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout)
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        None
    }
}

async fn download_geo_with_fallback(
    client: &reqwest::Client,
    urls: &[String],
    dest: &str,
    gh_token: Option<&str>,
) -> bool {
    // If final file already exists, check it's a valid binary (not an HTML error page)
    if let Ok(meta) = std::fs::metadata(dest) {
        if meta.len() > 1_000_000 && is_valid_geo_file(dest) {
            let name = std::path::Path::new(dest)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            println!("  {} already exists ({} bytes), skip", name, meta.len());
            crate::log!("  {dest} already exists ({} bytes), skip", meta.len());
            return true;
        }
        if meta.len() > 1_000_000 {
            crate::log!("  {dest} exists but appears corrupt, re-downloading");
            eprintln!(
                "  {} exists but first byte is invalid, re-downloading",
                std::path::Path::new(dest)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            );
        } else if meta.len() > 0 {
            eprintln!(
                "  {} is too small ({} bytes), re-downloading",
                std::path::Path::new(dest)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                meta.len()
            );
        } else {
            eprintln!(
                "  {} is empty, re-downloading",
                std::path::Path::new(dest)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            );
        }
    } else {
        eprintln!(
            "  {} not found, downloading",
            std::path::Path::new(dest)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
    }

    let tmp = format!("{dest}.tmp");

    for (i, url) in urls.iter().enumerate() {
        // Show short label on first try, then full URL on mirrors/retries
        let short = if url.starts_with("https://github.com") {
            "(GitHub)".to_string()
        } else if url.contains("jsdelivr") {
            "(jsDelivr CDN)".to_string()
        } else if url.contains("ghproxy") {
            "(ghproxy mirror)".to_string()
        } else {
            String::new()
        };
        let label = if i == 0 {
            format!("  Downloading {}...", short)
        } else {
            format!("  Trying {} {}", short, url)
        };

        // When switching to a different mirror, discard the partial tmp file
        // from the previous URL — different servers may serve different content
        // at the same byte offset, corrupting the concatenated file.
        let resumed = if i == 0 {
            std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0)
        } else {
            let _ = std::fs::remove_file(&tmp);
            0
        };
        if resumed > 0 {
            println!("  {label} (resuming from {resumed} bytes)");
        } else {
            println!("  {label}");
        }

        // Retry same URL up to 2 times with exponential backoff
        for retry in 0..2 {
            if retry > 0 {
                let delay = std::time::Duration::from_secs(1u64 << (retry - 1)); // 1s, 2s
                eprintln!("    Retrying in {}s...", delay.as_secs());
                tokio::time::sleep(delay).await;
            }

            let resumed = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
            if try_download_geo(client, url, &tmp, resumed, gh_token).await {
                let _ = std::fs::rename(&tmp, dest);
                let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
                println!("  Saved ({} bytes)", size);
                return true;
            }
        }
    }

    // All URLs exhausted — clean up partial file
    let _ = std::fs::remove_file(&tmp);
    false
}

/// Download a single geo file with resume support and progress bar.
/// Tries resumed download first; falls back to fresh download on 416 or if server ignores Range.
/// When `gh_token` is set, adds Authorization header for GitHub's authenticated API (5000 req/hr).
async fn try_download_geo(
    client: &reqwest::Client,
    url: &str,
    tmp: &str,
    resumed: u64,
    gh_token: Option<&str>,
) -> bool {
    use futures::StreamExt;
    use std::io::Write;

    // Two attempts: first with resume, then fresh if resume not supported
    for attempt in 0..2 {
        let offset = if attempt == 0 { resumed } else { 0 };
        if attempt == 1 {
            crate::log!("    retrying without resume");
            let _ = std::fs::remove_file(tmp);
        }

        let mut request = client.get(url).header("User-Agent", "mihomo-cli");
        if offset > 0 {
            request = request.header("Range", format!("bytes={offset}-"));
        }
        // GitHub authenticated API: better rate limits & routing
        if url.starts_with("https://github.com") {
            if let Some(token) = gh_token {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
        }

        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                crate::log!("    request failed: {e}");
                return false;
            }
        };

        let status = resp.status();

        // 416 = Range Not Satisfiable — restart from 0
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            continue;
        }

        if !status.is_success() {
            crate::log!("    HTTP {status}");
            return false;
        }

        let actual_offset = if offset > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
            offset
        } else {
            // Server ignored Range — start fresh
            let _ = std::fs::remove_file(tmp);
            0
        };

        // Progress bar
        let total = resp.content_length().map(|l| l + actual_offset);
        let pb = ProgressBar::new(total.unwrap_or(0));
        if total.is_some() {
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} {bytes}/{total_bytes} [{elapsed_precise}] {bar:30.cyan/blue} {bytes_per_sec}")
                    .unwrap()
                    .progress_chars("=>-"),
            );
        } else {
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} {bytes} [{elapsed_precise}] {bytes_per_sec}")
                    .unwrap(),
            );
        }

        let mut file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(tmp)
        {
            Ok(f) => f,
            Err(e) => {
                crate::log!("    cannot open {tmp}: {e}");
                pb.abandon_with_message("Failed");
                return false;
            }
        };

        let mut stream = resp.bytes_stream();
        let mut total_bytes = actual_offset;
        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    crate::log!("    stream error: {e}");
                    pb.abandon_with_message("Failed");
                    return false;
                }
            };
            if file.write_all(&chunk).is_err() {
                pb.abandon_with_message("Write error");
                return false;
            }
            total_bytes += chunk.len() as u64;
            pb.set_position(total_bytes);
        }
        let _ = file.flush();
        pb.finish_with_message("Done");
        return true;
    }

    false
}

/// Quick check: read first byte to detect HTML/JSON error pages masquerading as binary data.
fn is_valid_geo_file(path: &str) -> bool {
    use std::io::Read;
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut buf = [0u8; 1];
    if f.read_exact(&mut buf).is_err() {
        return false;
    }
    // Only reject HTML/JSON error pages.
    // - '<' = HTML error page, '{' = JSON error page
    // Note: GeoSite.dat is protobuf, first byte is 0x0A ('\n') — valid.
    // Note: geoip.metadb (MMDB) starts with 0xAB — valid.
    !matches!(buf[0], b'<' | b'{')
}
