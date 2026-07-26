use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MihomoTarget {
    pub os_name: &'static str,
    pub arch_name: String,
    pub bin_name: &'static str,
    pub archive_ext: &'static str,
}

impl MihomoTarget {
    pub(crate) fn resolve(os: &str, arch: &str) -> Self {
        let arch_name = match arch {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            other => other,
        }
        .to_string();
        match os {
            "windows" => Self {
                os_name: "windows",
                arch_name,
                bin_name: "mihomo.exe",
                archive_ext: "zip",
            },
            "macos" => Self {
                os_name: "darwin",
                arch_name,
                bin_name: "mihomo",
                archive_ext: "gz",
            },
            _ => Self {
                os_name: "linux",
                arch_name,
                bin_name: "mihomo",
                archive_ext: "gz",
            },
        }
    }

    pub(crate) fn current() -> Self {
        Self::resolve(std::env::consts::OS, std::env::consts::ARCH)
    }

    pub(crate) fn download_url(&self, version: &str) -> String {
        format!(
            "https://github.com/MetaCubeX/mihomo/releases/download/{version}/mihomo-{}-{}-{version}.{}",
            self.os_name, self.arch_name, self.archive_ext
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum BinaryFormat {
    Elf,
    MachO,
}

#[allow(dead_code)]
pub(crate) fn is_valid_binary_magic(format: BinaryFormat, magic: [u8; 4]) -> bool {
    match format {
        BinaryFormat::Elf => magic == [0x7f, b'E', b'L', b'F'],
        BinaryFormat::MachO => matches!(
            magic,
            [0xFE, 0xED, 0xFA, 0xCF]
                | [0xFE, 0xED, 0xFA, 0xCE]
                | [0xCF, 0xFA, 0xED, 0xFE]
                | [0xCE, 0xFA, 0xED, 0xFE]
        ),
    }
}

pub(crate) fn is_suspiciously_small_binary(size: u64) -> bool {
    size < 5_000_000
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeAction {
    Resume { offset: u64 },
    Fresh,
}

pub(crate) fn plan_resume_request(resumed_size: u64) -> ResumeAction {
    if resumed_size > 0 {
        ResumeAction::Resume {
            offset: resumed_size,
        }
    } else {
        ResumeAction::Fresh
    }
}

pub(crate) fn expected_final_size(
    status: u16,
    offset: u64,
    content_length: Option<u64>,
) -> Option<u64> {
    if status == 206 {
        content_length.map(|cl| cl + offset)
    } else {
        content_length
    }
}

pub(crate) fn actual_resume_offset(status: u16, requested_offset: u64) -> u64 {
    if requested_offset > 0 && status == 206 {
        requested_offset
    } else {
        0
    }
}

pub(crate) fn download_complete(total_bytes: u64, expected: Option<u64>) -> bool {
    expected.map(|e| total_bytes == e).unwrap_or(true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DownloadResponsePlan {
    UseResponse {
        actual_offset: u64,
        expected_size: Option<u64>,
        discard_existing_part: bool,
    },
    RestartFresh,
    HttpError {
        status: u16,
    },
}

pub(crate) fn plan_download_response(
    status: u16,
    requested_offset: u64,
    content_length: Option<u64>,
) -> DownloadResponsePlan {
    if status == 416 {
        return DownloadResponsePlan::RestartFresh;
    }

    if !(200..300).contains(&status) {
        return DownloadResponsePlan::HttpError { status };
    }

    let actual_offset = actual_resume_offset(status, requested_offset);
    DownloadResponsePlan::UseResponse {
        actual_offset,
        expected_size: expected_final_size(status, actual_offset, content_length),
        discard_existing_part: requested_offset > 0 && actual_offset == 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetryPlan {
    pub max_attempts: usize,
    pub attempt_index: usize,
    pub delay: Option<Duration>,
}

pub(crate) fn retry_plan(attempt_index: usize, max_attempts: usize) -> RetryPlan {
    RetryPlan {
        max_attempts,
        attempt_index,
        delay: if attempt_index == 0 {
            None
        } else {
            Some(Duration::from_secs(1 << (attempt_index - 1)))
        },
    }
}

pub(crate) fn retry_status_line(plan: RetryPlan) -> Option<String> {
    plan.delay.map(|delay| {
        format!(
            "  Retrying in {}s... (attempt {}/{})",
            delay.as_secs(),
            plan.attempt_index + 1,
            plan.max_attempts
        )
    })
}

pub(crate) fn geo_retry_delay(retry_index: usize) -> Option<Duration> {
    if retry_index == 0 {
        None
    } else {
        Some(Duration::from_secs(1u64 << (retry_index - 1)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GeoDownloadResponseDecision {
    RetryFresh,
    FailHttp {
        status: u16,
    },
    Download {
        actual_offset: u64,
        expected_final_size: Option<u64>,
        progress_total: Option<u64>,
        discard_existing_part: bool,
    },
}

fn geo_attempt_offset(attempt: usize, resumed: u64) -> u64 {
    if attempt == 0 {
        resumed
    } else {
        0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GeoRequestPlan {
    url: String,
    headers: Vec<(String, String)>,
}

fn geo_request_plan(url: &str, offset: u64, gh_token: Option<&str>) -> GeoRequestPlan {
    let mut headers = vec![("User-Agent".to_string(), "mihomo-cli".to_string())];

    if offset > 0 {
        headers.push(("Range".to_string(), format!("bytes={offset}-")));
    }

    if url.starts_with("https://github.com") {
        if let Some(token) = gh_token {
            headers.push(("Authorization".to_string(), format!("Bearer {token}")));
        }
    }

    GeoRequestPlan {
        url: url.to_string(),
        headers,
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeoPartFileAction {
    Keep,
    RemoveBeforeFreshRetry,
    RemoveBecauseRangeIgnored,
}

fn geo_part_action_before_attempt(attempt: usize) -> GeoPartFileAction {
    if attempt == 1 {
        GeoPartFileAction::RemoveBeforeFreshRetry
    } else {
        GeoPartFileAction::Keep
    }
}

fn geo_part_action_after_response(discard_existing_part: bool) -> GeoPartFileAction {
    if discard_existing_part {
        GeoPartFileAction::RemoveBecauseRangeIgnored
    } else {
        GeoPartFileAction::Keep
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProgressBarPlan {
    template: &'static str,
    progress_chars: &'static str,
}

fn progress_bar_plan(total: Option<u64>) -> ProgressBarPlan {
    if total.is_some() {
        ProgressBarPlan {
            template: "{spinner:.green} {bytes}/{total_bytes} [{elapsed_precise}] {bar:30.cyan/blue} {bytes_per_sec}",
            progress_chars: "=>-",
        }
    } else {
        ProgressBarPlan {
            template: "{spinner:.green} {bytes} [{elapsed_precise}] {bytes_per_sec}",
            progress_chars: "=>-",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOpenMode {
    Append,
    Truncate,
}

fn file_open_mode_for_download(actual_offset: u64) -> FileOpenMode {
    if actual_offset > 0 {
        FileOpenMode::Append
    } else {
        FileOpenMode::Truncate
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownloadCompletion {
    Success,
    Incomplete { got: u64, expected: u64 },
}

fn download_completion_decision(
    total_bytes: u64,
    expected_final_size: Option<u64>,
) -> DownloadCompletion {
    match expected_final_size {
        Some(expected) if !download_complete(total_bytes, Some(expected)) => {
            DownloadCompletion::Incomplete {
                got: total_bytes,
                expected,
            }
        }
        _ => DownloadCompletion::Success,
    }
}

fn plan_geo_download_response(
    status: u16,
    requested_offset: u64,
    content_length: Option<u64>,
) -> GeoDownloadResponseDecision {
    if status == 416 {
        return GeoDownloadResponseDecision::RetryFresh;
    }
    if !(200..300).contains(&status) {
        return GeoDownloadResponseDecision::FailHttp { status };
    }

    let actual_offset = actual_resume_offset(status, requested_offset);
    GeoDownloadResponseDecision::Download {
        actual_offset,
        expected_final_size: expected_final_size(status, actual_offset, content_length),
        progress_total: content_length.map(|len| len + actual_offset),
        discard_existing_part: requested_offset > 0 && actual_offset == 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DownloadProgressPlan {
    KnownTotal { total: u64 },
    UnknownTotal,
}

pub(crate) fn download_progress_plan(expected_size: Option<u64>) -> DownloadProgressPlan {
    match expected_size {
        Some(total) => DownloadProgressPlan::KnownTotal { total },
        None => DownloadProgressPlan::UnknownTotal,
    }
}

pub(crate) fn download_progress_total(plan: DownloadProgressPlan) -> u64 {
    match plan {
        DownloadProgressPlan::KnownTotal { total } => total,
        DownloadProgressPlan::UnknownTotal => 0,
    }
}

pub(crate) fn download_progress_template(plan: DownloadProgressPlan) -> &'static str {
    match plan {
        DownloadProgressPlan::KnownTotal { .. } => {
            "{spinner:.green} {bytes}/{total_bytes} [{elapsed_precise}] {bar:30.cyan/blue} {bytes_per_sec}"
        }
        DownloadProgressPlan::UnknownTotal => {
            "{spinner:.green} {bytes} [{elapsed_precise}] {bytes_per_sec}"
        }
    }
}

pub(crate) fn append_download_chunk(
    writer: &mut impl std::io::Write,
    chunk: &[u8],
    total_bytes: &mut u64,
) -> std::io::Result<()> {
    writer.write_all(chunk)?;
    *total_bytes += chunk.len() as u64;
    Ok(())
}

pub(crate) fn finalize_part_download(
    part_path: &str,
    total_bytes: u64,
    expected_size: Option<u64>,
) -> anyhow::Result<Vec<u8>> {
    if let Some(expected) = expected_size {
        if !download_complete(total_bytes, Some(expected)) {
            let _ = std::fs::remove_file(part_path);
            anyhow::bail!(
                "Download incomplete: got {total_bytes} bytes, expected {expected}.\n  \
                 The download was interrupted — please retry."
            );
        }
    }
    Ok(std::fs::read(part_path)?)
}

async fn write_download_stream<S, W, F>(
    stream: &mut S,
    writer: &mut W,
    offset: u64,
    mut on_progress: F,
) -> anyhow::Result<u64>
where
    S: futures::Stream<Item = anyhow::Result<Vec<u8>>> + Unpin,
    W: std::io::Write,
    F: FnMut(u64),
{
    use futures::StreamExt;

    let mut total_bytes = offset;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        append_download_chunk(writer, &chunk, &mut total_bytes)?;
        on_progress(total_bytes);
    }
    writer.flush()?;
    Ok(total_bytes)
}

pub(crate) fn is_probably_valid_geo_first_byte(byte: u8) -> bool {
    !matches!(byte, b'<' | b'{')
}

pub(crate) fn mihomo_test_failure_is_geo_related(output: &str) -> bool {
    let geo_related = output.to_lowercase();
    geo_related.contains("geo")
        || geo_related.contains("mmdb")
        || geo_related.contains("geosite")
        || geo_related.contains("load")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MihomoInstallPlan {
    pub bin_path: String,
    pub part_path: String,
    pub url: String,
    pub archive_ext: &'static str,
}

#[allow(dead_code)]
pub(crate) fn mihomo_bin_path(target: &MihomoTarget, home_or_local_data: &str) -> String {
    if target.os_name == "windows" {
        format!("{}\\mihomo\\{}", home_or_local_data, target.bin_name)
    } else {
        format!("{}/.local/bin/{}", home_or_local_data, target.bin_name)
    }
}

#[allow(dead_code)]
pub(crate) fn mihomo_install_plan(
    target: &MihomoTarget,
    home_or_local_data: &str,
    version: &str,
) -> MihomoInstallPlan {
    let bin_path = mihomo_bin_path(target, home_or_local_data);
    MihomoInstallPlan {
        part_path: format!("{bin_path}.part"),
        bin_path,
        url: target.download_url(version),
        archive_ext: target.archive_ext,
    }
}

pub(crate) fn archive_entry_is_mihomo_exe(name: &str) -> bool {
    let name = name.to_lowercase();
    name.starts_with("mihomo") && name.ends_with(".exe")
}

#[allow(dead_code)]
pub(crate) fn selected_windows_archive_entry<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    names
        .into_iter()
        .find(|name| archive_entry_is_mihomo_exe(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstalledBinaryCheck {
    Ok,
    RemoveAndRedownload,
}

pub(crate) fn existing_binary_action(
    binary_exists: bool,
    validation_ok: bool,
) -> InstalledBinaryCheck {
    if binary_exists && validation_ok {
        InstalledBinaryCheck::Ok
    } else {
        InstalledBinaryCheck::RemoveAndRedownload
    }
}

pub(crate) fn install_downloaded_archive(
    archive_ext: &str,
    bytes: &[u8],
    bin_path: &Path,
    min_binary_size: u64,
) -> anyhow::Result<u64> {
    if archive_ext == "zip" {
        let cursor = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor)?;
        let mut installed = false;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            if archive_entry_is_mihomo_exe(file.name()) {
                let mut out = std::fs::File::create(bin_path)?;
                std::io::copy(&mut file, &mut out)?;
                installed = true;
                break;
            }
        }
        if !installed {
            anyhow::bail!("Archive does not contain a mihomo Windows executable");
        }
    } else if archive_ext == "gz" {
        let mut decoder = GzDecoder::new(bytes);
        let mut out = std::fs::File::create(bin_path)?;
        std::io::copy(&mut decoder, &mut out)?;
    } else {
        anyhow::bail!("Unsupported mihomo archive format: {archive_ext}");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(bin_path)?.permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(bin_path, p)?;
    }

    let decompressed_size = std::fs::metadata(bin_path)?.len();
    let too_small = if min_binary_size == 5_000_000 {
        is_suspiciously_small_binary(decompressed_size)
    } else {
        decompressed_size < min_binary_size
    };
    if too_small {
        let _ = std::fs::remove_file(bin_path);
        anyhow::bail!(
            "Decompressed binary is suspiciously small ({decompressed_size} bytes) \
             — download was likely truncated.\n  \
             Expected ~20MB. Please retry."
        );
    }

    Ok(decompressed_size)
}

/// 验证 mihomo 二进制是有效的可执行文件。
///
/// 检查项：
///   1. Magic bytes（Linux ELF / macOS Mach-O）— 防止 HTML/文本文件冒充
///   2. `-v` 冒烟测试 — 捕获段错误、链接错误、架构不匹配
///
/// 错误信息区分三种场景：
///   - 文件不存在
///   - 文件存在但段错误 → "binary corruption"
///   - 文件存在但不可执行 → "permission"
#[allow(dead_code)]
pub fn validate_binary() -> anyhow::Result<()> {
    let bin = crate::utils::mihomo_path();
    validate_binary_at(std::path::Path::new(&bin))
}

pub fn validate_binary_at(path: &std::path::Path) -> anyhow::Result<()> {
    let bin = path.display().to_string();

    if !path.exists() {
        anyhow::bail!("mihomo binary not found at {bin}\n  Run: mihomo-cli install");
    }

    // Step 1: Magic bytes 检查
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let mut f = std::fs::File::open(&bin)?;
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic)?;
        if !is_valid_binary_magic(BinaryFormat::Elf, magic) {
            anyhow::bail!(
                "Binary at {bin} is not a valid ELF executable \
                 (magic: {magic:#04x?})\n\
                 This usually means the downloaded file is not a mihomo binary.\n\
                 Run: mihomo-cli update"
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        use std::io::Read;
        let mut f = std::fs::File::open(&bin)?;
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic)?;
        // Mach-O magic: 0xFEEDFACF (64-bit) or 0xFEEDFACE (32-bit), both endian
        if !is_valid_binary_magic(BinaryFormat::MachO, magic) {
            anyhow::bail!(
                "Binary at {bin} is not a valid Mach-O executable \
                 (magic: {magic:#04x?})\n\
                 This usually means the downloaded file is not a mihomo binary.\n\
                 Run: mihomo-cli update"
            );
        }
    }

    // Step 2: -v smoke test — distinguish crash from other failures
    let output = match std::process::Command::new(&bin)
        .arg("-v")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            anyhow::bail!("Binary is not executable.\n  Fix: chmod +x {bin}")
        }
        Err(e) => {
            anyhow::bail!("Cannot execute mihomo binary at {bin}: {e}")
        }
    };

    if output.status.success() {
        return Ok(());
    }

    let output_text = crate::utils::combine_output(&output);
    let code = output.status.code();

    // SIGSEGV — binary is broken
    if code.is_none() || code == Some(139) {
        anyhow::bail!(
            "Binary corruption: mihomo crashes immediately (SIGSEGV).\n\
             Path: {bin}\n\
             This is NOT a config issue — the binary itself is broken.\n\
             Run: mihomo-cli update"
        );
    }

    // Other failure
    let exit_code = code.unwrap_or(-1);
    if output_text.is_empty() {
        anyhow::bail!("Binary smoke test failed (exit {exit_code}).\n  Run: mihomo-cli update");
    } else {
        anyhow::bail!(
            "Binary smoke test failed (exit {exit_code}):\n{}",
            output_text.lines().take(3).collect::<Vec<_>>().join("\n")
        );
    }
}

pub async fn download_mihomo_to(version: Option<&str>, bin_path: &Path) -> anyhow::Result<()> {
    let target = MihomoTarget::current();
    let resolved_version = version.unwrap_or("v1.19.27");
    let part_path = format!("{}.part", bin_path.display());
    let plan = MihomoInstallPlan {
        url: target.download_url(resolved_version),
        bin_path: bin_path.display().to_string(),
        part_path,
        archive_ext: target.archive_ext,
    };
    download_mihomo_with_plan(plan).await
}

#[allow(dead_code)]
pub async fn download_mihomo(version: Option<&str>) -> anyhow::Result<()> {
    let target = MihomoTarget::current();

    let base_dir = if cfg!(target_os = "windows") {
        dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"))
            .display()
            .to_string()
    } else {
        dirs::home_dir().unwrap_or_default().display().to_string()
    };
    let resolved_version = version.unwrap_or("v1.19.27");
    let plan = mihomo_install_plan(&target, &base_dir, resolved_version);
    let bin_path = plan.bin_path.clone();

    let plan = MihomoInstallPlan {
        url: plan.url,
        bin_path,
        part_path: plan.part_path,
        archive_ext: plan.archive_ext,
    };
    download_mihomo_with_plan(plan).await
}

async fn download_mihomo_with_plan(plan: MihomoInstallPlan) -> anyhow::Result<()> {
    let bin_path = plan.bin_path.clone();
    if Path::new(&bin_path).exists() {
        println!("mihomo already installed at {bin_path}");
        // Fix permissions if missing (defense against old installer)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&bin_path) {
                if meta.permissions().mode() & 0o111 == 0 {
                    let mut p = meta.permissions();
                    p.set_mode(0o755);
                    let _ = std::fs::set_permissions(&bin_path, p);
                }
            }
        }
        // 层 4：验证已有二进制是否健康，损坏则删掉重新下载
        match existing_binary_action(true, validate_binary_at(Path::new(&bin_path)).is_ok()) {
            InstalledBinaryCheck::Ok => return Ok(()),
            InstalledBinaryCheck::RemoveAndRedownload => {
                println!("Existing binary is corrupted, re-downloading...");
                let _ = std::fs::remove_file(&bin_path);
            }
        }
    }

    println!("Downloading Mihomo core...");
    crate::log!("URL: {}", plan.url);

    let parent = Path::new(&bin_path).parent().unwrap();
    std::fs::create_dir_all(parent)?;

    // Download with resume + retry
    let bytes = download_with_retry(&plan.url, &plan.part_path).await?;

    // Download complete — clean up .part immediately so a corrupt file
    // never lingers to poison the next resume attempt.
    let _ = std::fs::remove_file(&plan.part_path);

    // Decompress & install
    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::with_template("{spinner:.green} {msg}")?);
    pb.set_message("Decompressing...");

    // 层 2：解压后检查文件大小 — mihomo 正常 ~20MB，远大于 5MB
    // 防止 gzip 流截断后解压出空壳文件却无报错
    install_downloaded_archive(plan.archive_ext, &bytes, Path::new(&bin_path), 5_000_000)?;

    // 层 3：验证新下载的二进制可用。如果失败，删除损坏文件以免脏数据残留
    if let Err(e) = validate_binary_at(Path::new(&bin_path)) {
        let _ = std::fs::remove_file(&bin_path);
        anyhow::bail!("Downloaded binary is corrupted: {e}");
    }

    pb.finish_with_message("Installed");
    println!("Installed to {bin_path}");
    Ok(())
}

trait InstallerDownloader {
    fn download_once<'a>(
        &'a mut self,
        url: &'a str,
        part_path: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<u8>>> + 'a>>;
}

struct ReqwestInstallerDownloader {
    client: reqwest::Client,
}

impl InstallerDownloader for ReqwestInstallerDownloader {
    fn download_once<'a>(
        &'a mut self,
        url: &'a str,
        part_path: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<u8>>> + 'a>> {
        Box::pin(async move { download_once(&self.client, url, part_path).await })
    }
}

async fn download_with_retry_using<D: InstallerDownloader>(
    downloader: &mut D,
    url: &str,
    part_path: &str,
    max_retries: usize,
    sleep_between_retries: bool,
) -> anyhow::Result<Vec<u8>> {
    let mut last_error = String::new();

    for attempt in 0..max_retries {
        let plan = retry_plan(attempt, max_retries);
        if let Some(line) = retry_status_line(plan) {
            eprintln!("{line}");
        }
        if sleep_between_retries {
            if let Some(delay) = plan.delay {
                tokio::time::sleep(delay).await;
            }
        }

        match downloader.download_once(url, part_path).await {
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

async fn download_with_retry(url: &str, part_path: &str) -> anyhow::Result<Vec<u8>> {
    let client = crate::utils::http_client_builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let mut downloader = ReqwestInstallerDownloader { client };
    download_with_retry_using(&mut downloader, url, part_path, 3, true).await
}

async fn download_once(
    client: &reqwest::Client,
    url: &str,
    part_path: &str,
) -> anyhow::Result<Vec<u8>> {
    let resumed_size = std::fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);

    let mut request = client.get(url).header("User-Agent", "mihomo-cli");
    if let ResumeAction::Resume { offset } = plan_resume_request(resumed_size) {
        crate::log!("Resuming from byte {}", offset);
        request = request.header("Range", format!("bytes={offset}-"));
    }

    let resp = request.send().await?;
    let status = resp.status();

    match plan_download_response(status.as_u16(), resumed_size, resp.content_length()) {
        DownloadResponsePlan::RestartFresh => {
            // Server doesn't support resume or range already satisfied — start fresh
            crate::log!("Range not satisfiable, starting fresh");
            let _ = std::fs::remove_file(part_path);
            let resp = client
                .get(url)
                .header("User-Agent", "mihomo-cli")
                .send()
                .await?;
            match plan_download_response(resp.status().as_u16(), 0, resp.content_length()) {
                DownloadResponsePlan::UseResponse {
                    actual_offset,
                    expected_size,
                    ..
                } => download_body(resp, part_path, actual_offset, expected_size).await,
                DownloadResponsePlan::HttpError { status } => anyhow::bail!("HTTP {status}"),
                DownloadResponsePlan::RestartFresh => anyhow::bail!("HTTP 416"),
            }
        }
        DownloadResponsePlan::HttpError { status } => anyhow::bail!("HTTP {status}"),
        DownloadResponsePlan::UseResponse {
            actual_offset,
            expected_size,
            discard_existing_part,
        } => {
            if discard_existing_part {
                crate::log!("Server ignored Range, starting fresh");
                let _ = std::fs::remove_file(part_path);
            }
            download_body(resp, part_path, actual_offset, expected_size).await
        }
    }
}

async fn download_body(
    resp: reqwest::Response,
    part_path: &str,
    offset: u64,
    expected_size: Option<u64>,
) -> anyhow::Result<Vec<u8>> {
    let total = expected_size;
    let progress_plan = download_progress_plan(total);
    let pb = ProgressBar::new(download_progress_total(progress_plan));
    let style = ProgressStyle::default_bar().template(download_progress_template(progress_plan))?;
    if matches!(progress_plan, DownloadProgressPlan::KnownTotal { .. }) {
        pb.set_style(style.progress_chars("=>-"));
    } else {
        pb.set_style(style);
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(part_path)?;

    use futures::StreamExt;

    let mut stream = resp.bytes_stream().map(|chunk| {
        chunk
            .map(|bytes| bytes.to_vec())
            .map_err(anyhow::Error::from)
    });
    let total_bytes = write_download_stream(&mut stream, &mut file, offset, |position| {
        pb.set_position(position);
    })
    .await?;
    pb.finish_with_message("Done");

    // 层 1：校验实际字节数与 Content-Length 一致，防止截断静默通过
    // Then read the complete file into memory for decompression.
    finalize_part_download(part_path, total_bytes, total)
}

// ── GeoIP / GeoSite pre-download ──

const GEOIP_URL: &str =
    "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geoip.metadb";
const GEOSITE_URL: &str =
    "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/GeoSite.dat";

/// Build default mirror URLs from the primary GitHub release asset URL.
/// Mirrors are tried in order after the primary GitHub URL fails.
fn build_mirrors(primary: &str) -> Vec<String> {
    vec![
        format!("https://gh-proxy.com/{primary}"),
        format!("https://mirror.ghproxy.com/{primary}"),
        format!("https://ghproxy.com/{primary}"),
    ]
}

/// Return geo file mirror URLs with optional custom proxy at front.
/// Custom proxy is a base URL prepended to the primary GitHub URL.
pub(crate) fn geo_urls_with_mirrors(primary: &str, proxy_url: Option<&str>) -> Vec<String> {
    let mut urls = vec![primary.to_string()];
    if let Some(proxy) = proxy_url {
        let base = if proxy.ends_with('/') {
            proxy.to_string()
        } else {
            format!("{proxy}/")
        };
        urls.push(format!("{base}{primary}"));
    }
    urls.extend(build_mirrors(primary));
    urls
}

/// Download geo files to config_dir, with mirror fallback.
/// Returns true if all succeeded; false means partial or total failure (never blocks the main flow).
#[allow(dead_code)]
pub fn geo_files_exist() -> bool {
    let dir = crate::utils::config_dir();
    std::path::Path::new(&format!("{dir}/geoip.metadb")).exists()
        && std::path::Path::new(&format!("{dir}/GeoSite.dat")).exists()
}

#[allow(dead_code)]
pub async fn ensure_geo_files(proxy_url: Option<&str>) -> bool {
    let dir = crate::utils::config_dir();
    ensure_geo_files_in(std::path::Path::new(&dir), proxy_url).await
}

#[allow(dead_code)]
pub async fn ensure_geo_files_in(dir_path: &std::path::Path, proxy_url: Option<&str>) -> bool {
    let dir = dir_path.display().to_string();
    let client = match crate::utils::http_client_builder()
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
        let urls = geo_urls_with_mirrors(primary_url, proxy_url);

        if !download_geo_with_fallback(&client, &urls, &dest, gh_token.as_deref()).await {
            eprintln!("  ⚠ Failed to download {name} — mihomo will try at startup");
            ok = false;
        }
    }

    // Layer 3: final validation — run mihomo -t to verify geo files are loadable
    // Only runs after ALL files downloaded, so config references are fully satisfied
    if ok && validate_geo_files(&dir) {
        crate::log!("Geo files validated by mihomo -t");
    } else if ok {
        // Validation failed — remove corrupt files so next run re-downloads
        eprintln!("  ⚠ Geo files failed mihomo validation, will retry next time");
        for name in ["geoip.metadb", "GeoSite.dat"] {
            let _ = std::fs::remove_file(format!("{dir}/{name}"));
        }
        ok = false;
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
        } else if url.contains("gh-proxy.com") {
            "(gh-proxy mirror)".to_string()
        } else if url.contains("ghproxy") {
            "(ghproxy mirror)".to_string()
        } else if !url.is_empty() {
            // Custom proxy URL (not a known mirror)
            "(proxy)".to_string()
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
            if let Some(delay) = geo_retry_delay(retry) {
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

    // Two attempts: first with resume, then fresh if resume not supported
    for attempt in 0..2 {
        let offset = geo_attempt_offset(attempt, resumed);
        if geo_part_action_before_attempt(attempt) == GeoPartFileAction::RemoveBeforeFreshRetry {
            crate::log!("    retrying without resume");
            let _ = std::fs::remove_file(tmp);
        }

        let request_plan = geo_request_plan(url, offset, gh_token);
        let mut request = client.get(&request_plan.url);
        for (name, value) in request_plan.headers {
            request = request.header(name, value);
        }

        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                crate::log!("    request failed: {e}");
                return false;
            }
        };

        let status = resp.status();
        let decision = plan_geo_download_response(status.as_u16(), offset, resp.content_length());
        let (actual_offset, expected_final_size, total) = match decision {
            GeoDownloadResponseDecision::RetryFresh => continue,
            GeoDownloadResponseDecision::FailHttp { status } => {
                crate::log!("    HTTP {status}");
                return false;
            }
            GeoDownloadResponseDecision::Download {
                actual_offset,
                expected_final_size,
                progress_total,
                discard_existing_part,
            } => {
                if geo_part_action_after_response(discard_existing_part)
                    == GeoPartFileAction::RemoveBecauseRangeIgnored
                {
                    // Server ignored Range — start fresh
                    let _ = std::fs::remove_file(tmp);
                }
                (actual_offset, expected_final_size, progress_total)
            }
        };

        // Progress bar
        let pb_plan = progress_bar_plan(total);
        let pb = ProgressBar::new(total.unwrap_or(0));
        pb.set_style(
            ProgressStyle::default_bar()
                .template(pb_plan.template)
                .unwrap()
                .progress_chars(pb_plan.progress_chars),
        );

        let open_mode = file_open_mode_for_download(actual_offset);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(matches!(open_mode, FileOpenMode::Append))
            .truncate(matches!(open_mode, FileOpenMode::Truncate))
            .open(tmp)
        {
            Ok(f) => f,
            Err(e) => {
                crate::log!("    cannot open {tmp}: {e}");
                pb.abandon_with_message("Failed");
                return false;
            }
        };

        let mut stream = resp.bytes_stream().map(|chunk| {
            chunk
                .map(|bytes| bytes.to_vec())
                .map_err(anyhow::Error::from)
        });
        let total_bytes =
            match write_download_stream(&mut stream, &mut file, actual_offset, |position| {
                pb.set_position(position);
            })
            .await
            {
                Ok(total_bytes) => total_bytes,
                Err(e) => {
                    crate::log!("    download stream failed: {e}");
                    pb.abandon_with_message("Failed");
                    return false;
                }
            };

        // Layer 1: size validation — detect truncated or oversized downloads
        match download_completion_decision(total_bytes, expected_final_size) {
            DownloadCompletion::Success => {}
            DownloadCompletion::Incomplete { got, expected } => {
                crate::log!("    size mismatch: got {got}, expected {expected}");
                pb.abandon_with_message("Incomplete");
                return false;
            }
        }

        // Layer 4: ensure data is physically written to disk
        let _ = file.sync_all();
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
    is_probably_valid_geo_first_byte(buf[0])
}

/// Layer 3 validation: run `mihomo -t` to verify geo files can actually be loaded.
/// Returns true if validation passes OR if we can't validate (mihomo/config not ready yet).
/// Returns false only when geo files are definitively corrupt.
fn validate_geo_files(config_dir: &str) -> bool {
    let mihomo = crate::utils::mihomo_path();
    if !std::path::Path::new(&mihomo).exists() {
        crate::log!("mihomo binary not found, skipping geo validation");
        return true;
    }
    let config_path = format!("{config_dir}/config.yaml");
    if !std::path::Path::new(&config_path).exists() {
        crate::log!("config.yaml not found, skipping geo validation");
        return true;
    }
    match std::process::Command::new(&mihomo)
        .args(["-t", "-d", config_dir])
        .output()
    {
        Ok(o) if !o.status.success() => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let stdout = String::from_utf8_lossy(&o.stdout);
            let output = format!("{stdout}\n{stderr}");
            // Only flag as corrupt if the error is geo-file related.
            // Other config errors (rules, ports, etc.) are not our concern here.
            if mihomo_test_failure_is_geo_related(&output) {
                crate::log!("mihomo -t geo validation failed: {output}");
                false
            } else {
                crate::log!("mihomo -t failed but not geo-related, skipping: {output}");
                true
            }
        }
        Ok(_) => true,
        Err(e) => {
            crate::log!("cannot run mihomo -t for validation: {e}");
            true
        }
    }
}

#[cfg(test)]
mod target_tests {
    use super::*;

    #[test]
    fn resolves_linux_x86_64_target() {
        let target = MihomoTarget::resolve("linux", "x86_64");
        assert_eq!(target.os_name, "linux");
        assert_eq!(target.arch_name, "amd64");
        assert_eq!(target.bin_name, "mihomo");
        assert_eq!(target.archive_ext, "gz");
        assert_eq!(
            target.download_url("v1.2.3"),
            "https://github.com/MetaCubeX/mihomo/releases/download/v1.2.3/mihomo-linux-amd64-v1.2.3.gz"
        );
    }

    #[test]
    fn resolves_macos_aarch64_as_darwin_arm64() {
        let target = MihomoTarget::resolve("macos", "aarch64");
        assert_eq!(target.os_name, "darwin");
        assert_eq!(target.arch_name, "arm64");
        assert_eq!(target.bin_name, "mihomo");
        assert_eq!(target.archive_ext, "gz");
    }

    #[test]
    fn resolves_windows_zip_exe() {
        let target = MihomoTarget::resolve("windows", "x86_64");
        assert_eq!(target.os_name, "windows");
        assert_eq!(target.arch_name, "amd64");
        assert_eq!(target.bin_name, "mihomo.exe");
        assert_eq!(target.archive_ext, "zip");
    }
    #[test]
    fn binary_magic_validation_accepts_supported_formats_only() {
        assert!(is_valid_binary_magic(
            BinaryFormat::Elf,
            [0x7f, b'E', b'L', b'F']
        ));
        assert!(!is_valid_binary_magic(BinaryFormat::Elf, *b"HTML"));

        for magic in [
            [0xFE, 0xED, 0xFA, 0xCF],
            [0xFE, 0xED, 0xFA, 0xCE],
            [0xCF, 0xFA, 0xED, 0xFE],
            [0xCE, 0xFA, 0xED, 0xFE],
        ] {
            assert!(is_valid_binary_magic(BinaryFormat::MachO, magic));
        }
        assert!(!is_valid_binary_magic(
            BinaryFormat::MachO,
            [0x7f, b'E', b'L', b'F']
        ));
    }

    #[test]
    fn binary_size_threshold_flags_truncated_downloads() {
        assert!(is_suspiciously_small_binary(0));
        assert!(is_suspiciously_small_binary(4_999_999));
        assert!(!is_suspiciously_small_binary(5_000_000));
    }

    #[test]
    fn resume_planning_uses_range_only_when_partial_file_exists() {
        assert_eq!(plan_resume_request(0), ResumeAction::Fresh);
        assert_eq!(plan_resume_request(42), ResumeAction::Resume { offset: 42 });
    }

    #[test]
    fn resume_response_planning_handles_partial_and_ignored_range() {
        assert_eq!(actual_resume_offset(206, 100), 100);
        assert_eq!(actual_resume_offset(200, 100), 0, "server ignored Range");
        assert_eq!(actual_resume_offset(200, 0), 0);

        assert_eq!(expected_final_size(206, 100, Some(900)), Some(1000));
        assert_eq!(expected_final_size(200, 100, Some(1000)), Some(1000));
        assert_eq!(expected_final_size(206, 100, None), None);

        assert!(download_complete(1000, Some(1000)));
        assert!(!download_complete(999, Some(1000)));
        assert!(download_complete(999, None));
    }

    struct FakeInstallerDownloader {
        calls: Vec<(String, String)>,
        results: std::collections::VecDeque<anyhow::Result<Vec<u8>>>,
    }

    impl InstallerDownloader for FakeInstallerDownloader {
        fn download_once<'a>(
            &'a mut self,
            url: &'a str,
            part_path: &'a str,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<Vec<u8>>> + 'a>> {
            Box::pin(async move {
                self.calls.push((url.to_string(), part_path.to_string()));
                self.results
                    .pop_front()
                    .unwrap_or_else(|| anyhow::bail!("unexpected call"))
            })
        }
    }

    #[tokio::test]
    async fn download_with_retry_using_stops_after_success() {
        let mut downloader = FakeInstallerDownloader {
            calls: Vec::new(),
            results: std::collections::VecDeque::from([
                Err(anyhow::anyhow!("temporary")),
                Ok(b"archive".to_vec()),
                Err(anyhow::anyhow!("should not be called")),
            ]),
        };

        let bytes = download_with_retry_using(
            &mut downloader,
            "https://example.test/mihomo.gz",
            "/tmp/mihomo.part",
            3,
            false,
        )
        .await
        .unwrap();

        assert_eq!(bytes, b"archive".to_vec());
        assert_eq!(
            downloader.calls,
            vec![
                (
                    "https://example.test/mihomo.gz".to_string(),
                    "/tmp/mihomo.part".to_string(),
                ),
                (
                    "https://example.test/mihomo.gz".to_string(),
                    "/tmp/mihomo.part".to_string(),
                ),
            ]
        );
    }

    #[tokio::test]
    async fn download_with_retry_using_reports_last_error_after_exhaustion() {
        let mut downloader = FakeInstallerDownloader {
            calls: Vec::new(),
            results: std::collections::VecDeque::from([
                Err(anyhow::anyhow!("first")),
                Err(anyhow::anyhow!("last")),
            ]),
        };

        let err = download_with_retry_using(&mut downloader, "url", "part", 2, false)
            .await
            .unwrap_err();

        assert_eq!(downloader.calls.len(), 2);
        assert!(
            err.to_string()
                .contains("Download failed after 2 attempts: last"),
            "error was: {err}"
        );
    }

    #[test]
    fn retry_plans_are_deterministic_and_format_status_lines() {
        assert_eq!(
            retry_plan(0, 3),
            RetryPlan {
                max_attempts: 3,
                attempt_index: 0,
                delay: None,
            }
        );
        assert_eq!(retry_plan(1, 3).delay, Some(Duration::from_secs(1)));
        assert_eq!(retry_plan(2, 3).delay, Some(Duration::from_secs(2)));
        assert_eq!(retry_status_line(retry_plan(0, 3)), None);
        assert_eq!(
            retry_status_line(retry_plan(2, 3)),
            Some("  Retrying in 2s... (attempt 3/3)".to_string())
        );
    }

    #[test]
    fn geo_part_file_actions_are_planned_for_resume_fallbacks() {
        assert_eq!(geo_part_action_before_attempt(0), GeoPartFileAction::Keep);
        assert_eq!(
            geo_part_action_before_attempt(1),
            GeoPartFileAction::RemoveBeforeFreshRetry
        );
        assert_eq!(
            geo_part_action_before_attempt(2),
            GeoPartFileAction::Keep,
            "try_download_geo only uses attempts 0 and 1; unexpected values should not remove files"
        );

        assert_eq!(
            geo_part_action_after_response(false),
            GeoPartFileAction::Keep
        );
        assert_eq!(
            geo_part_action_after_response(true),
            GeoPartFileAction::RemoveBecauseRangeIgnored
        );
    }

    #[test]
    fn progress_bar_plan_selects_template_based_on_total() {
        let with_total = progress_bar_plan(Some(1024));
        assert!(with_total.template.contains("{total_bytes}"));
        assert!(with_total.template.contains("{bar:30.cyan/blue}"));
        assert_eq!(with_total.progress_chars, "=>-");

        let without_total = progress_bar_plan(None);
        assert!(!without_total.template.contains("{total_bytes}"));
        assert!(without_total.template.contains("{bytes_per_sec}"));
        assert_eq!(without_total.progress_chars, "=>-");
    }

    #[test]
    fn file_open_mode_for_download_depends_on_resume_offset() {
        assert_eq!(
            file_open_mode_for_download(0),
            FileOpenMode::Truncate,
            "fresh download must truncate any stale .part file"
        );
        assert_eq!(
            file_open_mode_for_download(1024),
            FileOpenMode::Append,
            "resumed download must append to existing .part file"
        );
    }

    #[test]
    fn download_completion_decision_detects_incomplete_downloads() {
        assert_eq!(
            download_completion_decision(1000, Some(1000)),
            DownloadCompletion::Success
        );
        assert_eq!(
            download_completion_decision(999, Some(1000)),
            DownloadCompletion::Incomplete {
                got: 999,
                expected: 1000
            }
        );
        assert_eq!(
            download_completion_decision(1500, Some(1000)),
            DownloadCompletion::Incomplete {
                got: 1500,
                expected: 1000
            },
            "oversized downloads must be flagged as incomplete"
        );
        assert_eq!(
            download_completion_decision(999, None),
            DownloadCompletion::Success,
            "unknown total size cannot be validated"
        );
    }

    #[test]
    fn geo_request_plan_adds_only_required_headers() {
        assert_eq!(
            geo_request_plan("https://example.test/Country.mmdb", 0, None),
            GeoRequestPlan {
                url: "https://example.test/Country.mmdb".to_string(),
                headers: vec![("User-Agent".to_string(), "mihomo-cli".to_string())],
            }
        );

        assert_eq!(
            geo_request_plan("https://example.test/Country.mmdb", 42, None),
            GeoRequestPlan {
                url: "https://example.test/Country.mmdb".to_string(),
                headers: vec![
                    ("User-Agent".to_string(), "mihomo-cli".to_string()),
                    ("Range".to_string(), "bytes=42-".to_string()),
                ],
            }
        );

        assert_eq!(
            geo_request_plan(
                "https://github.com/MetaCubeX/mihomo/releases/file",
                0,
                Some("ghp_x")
            ),
            GeoRequestPlan {
                url: "https://github.com/MetaCubeX/mihomo/releases/file".to_string(),
                headers: vec![
                    ("User-Agent".to_string(), "mihomo-cli".to_string()),
                    ("Authorization".to_string(), "Bearer ghp_x".to_string()),
                ],
            }
        );

        assert_eq!(
            geo_request_plan("https://mirror.example.test/file", 10, Some("ghp_x")),
            GeoRequestPlan {
                url: "https://mirror.example.test/file".to_string(),
                headers: vec![
                    ("User-Agent".to_string(), "mihomo-cli".to_string()),
                    ("Range".to_string(), "bytes=10-".to_string()),
                ],
            },
            "GitHub token must not be sent to non-GitHub mirrors"
        );
    }

    #[test]
    fn geo_download_attempt_and_response_plans_handle_resume_and_errors() {
        assert_eq!(geo_attempt_offset(0, 123), 123);
        assert_eq!(geo_attempt_offset(1, 123), 0);

        assert_eq!(
            plan_geo_download_response(206, 100, Some(900)),
            GeoDownloadResponseDecision::Download {
                actual_offset: 100,
                expected_final_size: Some(1000),
                progress_total: Some(1000),
                discard_existing_part: false,
            }
        );
        assert_eq!(
            plan_geo_download_response(200, 100, Some(1000)),
            GeoDownloadResponseDecision::Download {
                actual_offset: 0,
                expected_final_size: Some(1000),
                progress_total: Some(1000),
                discard_existing_part: true,
            },
            "server ignored Range, so stale partial file must be discarded"
        );
        assert_eq!(
            plan_geo_download_response(416, 100, None),
            GeoDownloadResponseDecision::RetryFresh
        );
        assert_eq!(
            plan_geo_download_response(503, 0, None),
            GeoDownloadResponseDecision::FailHttp { status: 503 }
        );
    }

    #[test]
    fn geo_retry_delay_uses_same_exponential_schedule_without_first_delay() {
        assert_eq!(geo_retry_delay(0), None);
        assert_eq!(geo_retry_delay(1), Some(Duration::from_secs(1)));
        assert_eq!(geo_retry_delay(2), Some(Duration::from_secs(2)));
    }

    #[test]
    fn download_progress_plan_selects_total_and_template() {
        let known = download_progress_plan(Some(1024));
        assert_eq!(known, DownloadProgressPlan::KnownTotal { total: 1024 });
        assert_eq!(download_progress_total(known), 1024);
        assert!(download_progress_template(known).contains("{total_bytes}"));
        assert!(download_progress_template(known).contains("{bar:30.cyan/blue}"));

        let unknown = download_progress_plan(None);
        assert_eq!(unknown, DownloadProgressPlan::UnknownTotal);
        assert_eq!(download_progress_total(unknown), 0);
        assert!(!download_progress_template(unknown).contains("{total_bytes}"));
    }

    #[test]
    fn download_response_plan_handles_resume_restart_and_errors() {
        assert_eq!(
            plan_download_response(206, 100, Some(900)),
            DownloadResponsePlan::UseResponse {
                actual_offset: 100,
                expected_size: Some(1000),
                discard_existing_part: false,
            }
        );
        assert_eq!(
            plan_download_response(200, 100, Some(1000)),
            DownloadResponsePlan::UseResponse {
                actual_offset: 0,
                expected_size: Some(1000),
                discard_existing_part: true,
            },
            "when a server ignores Range, the stale .part file must be discarded before writing"
        );
        assert_eq!(
            plan_download_response(200, 0, Some(1000)),
            DownloadResponsePlan::UseResponse {
                actual_offset: 0,
                expected_size: Some(1000),
                discard_existing_part: false,
            }
        );
        assert_eq!(
            plan_download_response(416, 100, None),
            DownloadResponsePlan::RestartFresh
        );
        assert_eq!(
            plan_download_response(500, 0, None),
            DownloadResponsePlan::HttpError { status: 500 }
        );
    }

    #[tokio::test]
    async fn write_download_stream_writes_chunks_and_reports_progress() {
        let mut stream = futures::stream::iter([Ok(b"abc".to_vec()), Ok(b"defg".to_vec())]);
        let mut writer = Vec::new();
        let mut progress = Vec::new();

        let total = write_download_stream(&mut stream, &mut writer, 10, |position| {
            progress.push(position);
        })
        .await
        .unwrap();

        assert_eq!(writer, b"abcdefg");
        assert_eq!(total, 17);
        assert_eq!(progress, vec![13, 17]);
    }

    struct FailingWriter {
        written: Vec<u8>,
        fail_after: usize,
    }

    impl std::io::Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.written.len() >= self.fail_after {
                return Err(std::io::Error::other("disk full"));
            }
            let remaining = self.fail_after - self.written.len();
            let n = remaining.min(buf.len());
            self.written.extend_from_slice(&buf[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn write_download_stream_propagates_writer_errors() {
        let mut stream = futures::stream::iter([Ok(b"abc".to_vec()), Ok(b"def".to_vec())]);
        let mut writer = FailingWriter {
            written: Vec::new(),
            fail_after: 4,
        };
        let mut progress = Vec::new();

        let err = write_download_stream(&mut stream, &mut writer, 0, |position| {
            progress.push(position);
        })
        .await
        .unwrap_err();

        assert_eq!(writer.written, b"abcd");
        assert_eq!(progress, vec![3]);
        assert!(err.to_string().contains("disk full"), "error was: {err}");
    }

    #[tokio::test]
    async fn write_download_stream_stops_on_chunk_error() {
        let mut stream = futures::stream::iter([
            Ok(b"abc".to_vec()),
            Err(anyhow::anyhow!("stream failed")),
            Ok(b"ignored".to_vec()),
        ]);
        let mut writer = Vec::new();
        let mut progress = Vec::new();

        let err = write_download_stream(&mut stream, &mut writer, 0, |position| {
            progress.push(position);
        })
        .await
        .unwrap_err();

        assert_eq!(writer, b"abc");
        assert_eq!(progress, vec![3]);
        assert_eq!(err.to_string(), "stream failed");
    }

    #[test]
    fn append_download_chunk_tracks_written_total() {
        let mut bytes = Vec::new();
        let mut total = 10;

        append_download_chunk(&mut bytes, b"abc", &mut total).unwrap();
        append_download_chunk(&mut bytes, b"defg", &mut total).unwrap();

        assert_eq!(bytes, b"abcdefg");
        assert_eq!(total, 17);
    }

    #[test]
    fn finalize_part_download_reads_complete_file_and_removes_incomplete_part() {
        let tmp = tempfile::tempdir().unwrap();
        let part = tmp.path().join("mihomo.part");
        std::fs::write(&part, b"complete").unwrap();
        let part_str = part.to_str().unwrap();

        let bytes = finalize_part_download(part_str, 8, Some(8)).unwrap();
        assert_eq!(bytes, b"complete");
        assert!(part.exists());

        let bad_part = tmp.path().join("bad.part");
        std::fs::write(&bad_part, b"short").unwrap();
        let err = finalize_part_download(bad_part.to_str().unwrap(), 5, Some(8)).unwrap_err();

        assert!(
            err.to_string().contains("Download incomplete"),
            "error was: {err}"
        );
        assert!(!bad_part.exists(), "incomplete part file should be removed");
    }

    #[test]
    fn geo_mirror_plan_keeps_primary_first_then_fallbacks() {
        let urls = geo_urls_with_mirrors(GEOIP_URL, None);
        assert_eq!(urls[0], GEOIP_URL);
        assert!(urls
            .iter()
            .any(|u| u.starts_with("https://gh-proxy.com/https://github.com/")));
        assert!(urls
            .iter()
            .any(|u| u.starts_with("https://mirror.ghproxy.com/https://github.com/")));
        assert!(urls
            .iter()
            .any(|u| u.starts_with("https://ghproxy.com/https://github.com/")));
    }

    #[test]
    fn geo_mirror_plan_accepts_custom_proxy_url() {
        let proxy = "https://gitproxy.example.com/";
        let urls = geo_urls_with_mirrors(GEOIP_URL, Some(proxy));
        assert_eq!(urls[0], GEOIP_URL);
        // Custom proxy URL is inserted immediately after primary
        assert_eq!(
            urls[1],
            "https://gitproxy.example.com/https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geoip.metadb"
        );
        // Default mirrors still follow
        assert!(urls
            .iter()
            .any(|u| u.starts_with("https://mirror.ghproxy.com/")));
    }

    #[test]
    fn geo_mirror_plan_handles_proxy_without_trailing_slash() {
        let proxy = "https://gh.accelerator.io";
        let urls = geo_urls_with_mirrors(GEOIP_URL, Some(proxy));
        assert_eq!(
            urls[1],
            "https://gh.accelerator.io/https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/geoip.metadb"
        );
    }

    #[test]
    fn geo_first_byte_rejects_html_and_json_error_pages() {
        assert!(!is_probably_valid_geo_first_byte(b'<'));
        assert!(!is_probably_valid_geo_first_byte(b'{'));
        assert!(is_probably_valid_geo_first_byte(0x0A));
        assert!(is_probably_valid_geo_first_byte(0xAB));
    }

    #[test]
    fn mihomo_validation_output_classifier_only_flags_geo_failures() {
        assert!(mihomo_test_failure_is_geo_related(
            "failed to load geoip.metadb"
        ));
        assert!(mihomo_test_failure_is_geo_related("MMDB parse error"));
        assert!(mihomo_test_failure_is_geo_related("geosite data corrupt"));
        assert!(!mihomo_test_failure_is_geo_related("proxy group not found"));
        assert!(!mihomo_test_failure_is_geo_related(
            "yaml: line 2: did not find expected key"
        ));
    }

    #[test]
    fn install_plan_builds_bin_part_and_download_url() {
        let linux = MihomoTarget::resolve("linux", "x86_64");
        let plan = mihomo_install_plan(&linux, "/home/alice", "v1.2.3");
        assert_eq!(plan.bin_path, "/home/alice/.local/bin/mihomo");
        assert_eq!(plan.part_path, "/home/alice/.local/bin/mihomo.part");
        assert_eq!(plan.archive_ext, "gz");
        assert_eq!(
            plan.url,
            "https://github.com/MetaCubeX/mihomo/releases/download/v1.2.3/mihomo-linux-amd64-v1.2.3.gz"
        );

        let windows = MihomoTarget::resolve("windows", "x86_64");
        let plan = mihomo_install_plan(&windows, r"C:\Users\Alice\AppData\Local", "v9");
        assert_eq!(
            plan.bin_path,
            r"C:\Users\Alice\AppData\Local\mihomo\mihomo.exe"
        );
        assert_eq!(
            plan.part_path,
            r"C:\Users\Alice\AppData\Local\mihomo\mihomo.exe.part"
        );
        assert_eq!(plan.archive_ext, "zip");
    }

    #[test]
    fn windows_archive_entry_selection_accepts_mihomo_exe_only() {
        assert!(archive_entry_is_mihomo_exe("mihomo-windows-amd64.exe"));
        assert!(archive_entry_is_mihomo_exe("MIHOMO.EXE"));
        assert!(!archive_entry_is_mihomo_exe("clash.exe"));
        assert!(!archive_entry_is_mihomo_exe("mihomo.txt"));

        assert_eq!(
            selected_windows_archive_entry(["README.md", "mihomo-windows-amd64.exe"]),
            Some("mihomo-windows-amd64.exe")
        );
        assert_eq!(
            selected_windows_archive_entry(["README.md", "clash.exe"]),
            None
        );
    }

    #[test]
    fn existing_binary_action_uses_validation_result() {
        assert_eq!(existing_binary_action(true, true), InstalledBinaryCheck::Ok);
        assert_eq!(
            existing_binary_action(true, false),
            InstalledBinaryCheck::RemoveAndRedownload
        );
        assert_eq!(
            existing_binary_action(false, false),
            InstalledBinaryCheck::RemoveAndRedownload
        );
    }

    #[test]
    fn install_downloaded_gzip_writes_executable_and_enforces_size_floor() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("mihomo");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"fake-mihomo-binary").unwrap();
        let gz = encoder.finish().unwrap();

        let size = install_downloaded_archive("gz", &gz, &bin, 1).unwrap();

        assert_eq!(size, "fake-mihomo-binary".len() as u64);
        assert_eq!(std::fs::read(&bin).unwrap(), b"fake-mihomo-binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                std::fs::metadata(&bin).unwrap().permissions().mode() & 0o111,
                0
            );
        }

        let small_bin = tmp.path().join("mihomo-small");
        let err = install_downloaded_archive("gz", &gz, &small_bin, 1_000_000).unwrap_err();
        assert!(
            err.to_string().contains("suspiciously small"),
            "error was: {err}"
        );
        assert!(
            !small_bin.exists(),
            "small rejected binary should be removed"
        );
    }

    #[test]
    fn install_downloaded_zip_selects_mihomo_exe_without_extracting_everything() {
        use std::io::{Cursor, Write};
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("mihomo.exe");
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            zip.start_file("README.md", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"not the binary").unwrap();
            zip.start_file("mihomo-windows-amd64.exe", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"windows-binary").unwrap();
            zip.finish().unwrap();
        }

        let size = install_downloaded_archive("zip", &cursor.into_inner(), &bin, 1).unwrap();

        assert_eq!(size, "windows-binary".len() as u64);
        assert_eq!(std::fs::read(&bin).unwrap(), b"windows-binary");
        assert!(
            !tmp.path().join("README.md").exists(),
            "installer should copy the selected entry, not extract the whole archive"
        );
    }

    #[test]
    fn install_downloaded_zip_errors_when_mihomo_entry_missing() {
        use std::io::{Cursor, Write};
        use zip::write::SimpleFileOptions;

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("mihomo.exe");
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            zip.start_file("clash.exe", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"wrong-binary").unwrap();
            zip.finish().unwrap();
        }

        let err = install_downloaded_archive("zip", &cursor.into_inner(), &bin, 1).unwrap_err();

        assert!(
            err.to_string().contains("does not contain"),
            "error was: {err}"
        );
        assert!(!bin.exists());
    }
}
