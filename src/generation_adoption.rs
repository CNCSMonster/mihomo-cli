//! Explicit administrator trust establishment for migrated, never-certified stores.
//! This is not proof of historical consistency, compatibility, or runtime health.
use super::*;
use generation::{
    ArtifactKind, ArtifactManifestEntry, GenerationId, GenerationManifest, GenerationState,
    GenerationStore,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const ADOPTION_SOURCE: &str = "administrator-adopted-current-v1";
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
/// The confirmed snapshot keeps exactly one in-memory copy of the approved bytes;
/// this bounds that single copy instead of the discarded recheck copies.
const MAX_APPROVED_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const HASH_CHUNK_BYTES: usize = 64 * 1024;
const WARNING: &str = "⚠ 风险提示：管理员正在为当前确切字节建立新信任；不证明历史非混合、来源可靠、兼容性或运行健康。采纳不下载、不覆盖安装文件、不停启服务。";

const FAULT_AFTER_ARTIFACTS: &str = "persist.artifacts";
const FAULT_AFTER_VALIDATION: &str = "persist.validated";
const FAULT_SYNC_GENERATION: &str = "persist.sync_generation";
const FAULT_SYNC_GENERATIONS: &str = "persist.sync_generations";
const FAULT_BEFORE_STATE: &str = "commit.before_state";
const FAULT_AFTER_STATE: &str = "commit.after_state";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Artifact {
    path: PathBuf,
    relative_path: String,
    kind: ArtifactKind,
    sha256: Option<String>,
    size_bytes: u64,
    unix_mode: Option<u32>,
    safety: String,
    #[serde(skip)]
    bytes: Option<Vec<u8>>,
}

/// An installed file the approved baseline promises for one of the five
/// program-defined roles. It is never derived from manifest-chosen paths.
#[derive(Debug)]
struct Expectation {
    path: PathBuf,
    relative_path: String,
    kind: ArtifactKind,
    sha256: Option<String>,
    size_bytes: u64,
    unix_mode: Option<u32>,
}

#[derive(Debug)]
struct ManagedArtifact {
    path: PathBuf,
    relative_path: String,
    kind: ArtifactKind,
    required: bool,
}

#[derive(Debug, Serialize)]
struct Snapshot {
    store: PathBuf,
    state: GenerationState,
    discard_pending: Option<GenerationId>,
    artifacts: Vec<Artifact>,
}

enum PreviewOutcome {
    AlreadyAdopted(GenerationId),
    Required {
        snapshot: Snapshot,
        legacy_unmigrated: bool,
    },
}

/// Test-only fault/ID injection. Thread-local so parallel tests cannot arm each
/// other, and compiled out entirely in production (no configuration backdoor).
#[cfg(test)]
mod test_hooks {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    thread_local! {
        static FAULT: RefCell<Option<&'static str>> = const { RefCell::new(None) };
        static GENERATION_ID: RefCell<Option<String>> = const { RefCell::new(None) };
        static SYNC_LOG: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn arm_fault(point: &'static str) {
        FAULT.with(|slot| *slot.borrow_mut() = Some(point));
    }

    pub(super) fn arm_generation_id(id: &str) {
        GENERATION_ID.with(|slot| *slot.borrow_mut() = Some(id.to_string()));
    }

    pub(super) fn take_generation_id() -> Option<String> {
        GENERATION_ID.with(|slot| slot.borrow_mut().take())
    }

    pub(super) fn fire(point: &str) -> anyhow::Result<()> {
        let armed = FAULT.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_deref() == Some(point) {
                slot.take();
                true
            } else {
                false
            }
        });
        anyhow::ensure!(!armed, "注入故障: {point}");
        Ok(())
    }

    pub(super) fn record_sync(path: &Path) {
        SYNC_LOG.with(|log| log.borrow_mut().push(path.to_path_buf()));
    }

    pub(super) fn take_sync_log() -> Vec<PathBuf> {
        SYNC_LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
    }
}

#[cfg(test)]
fn fire_fault(point: &str) -> anyhow::Result<()> {
    test_hooks::fire(point)
}

#[cfg(not(test))]
fn fire_fault(_point: &str) -> anyhow::Result<()> {
    Ok(())
}

fn next_generation_id() -> anyhow::Result<GenerationId> {
    #[cfg(test)]
    if let Some(raw) = test_hooks::take_generation_id() {
        return Ok(GenerationId::new(raw)?);
    }
    Ok(GenerationId::generate())
}

/// The adopt/privileged-inspect path is only certified for the Unix boundary that
/// rejects ancestor symlinks, hardlinks, and special files. Windows still lets a
/// reparse point or hardlink redirect a privileged read, so this new feature is
/// refused there until that boundary is complete.
#[cfg(windows)]
fn require_supported_platform(operation: &str) -> anyhow::Result<()> {
    anyhow::bail!(
        "{operation} 依赖 Unix 的 no-follow 路径边界；Windows 上祖先 reparse/hardlink 校验尚未补齐，已拒绝（未提权、未写入）"
    )
}

#[cfg(not(windows))]
fn require_supported_platform(_operation: &str) -> anyhow::Result<()> {
    Ok(())
}

/// Directory fsync through the shared no-follow boundary.
fn sync_directory_no_follow(path: &Path) -> anyhow::Result<()> {
    utils::sync_directory_no_follow(path)
        .with_context(|| format!("目录同步失败 {}", path.display()))
}

fn sync_dir(point: &str, path: &Path) -> anyhow::Result<()> {
    fire_fault(point)?;
    #[cfg(test)]
    test_hooks::record_sync(path);
    sync_directory_no_follow(path)
}

fn require_privilege(operation: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    if !service::is_process_elevated() {
        anyhow::bail!("{operation} 需要管理员身份；当前 Windows 身份未获授权，未写入");
    }
    if is_current_process_root() {
        return Ok(());
    }
    eprintln!("{operation}：请求一次管理员授权；不会停启服务或覆盖安装文件。");
    let exe = std::env::current_exe()?;
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let status = sudo_reexec_command_with(&exe, &args, !std::io::stdin().is_terminal()).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn safety(metadata: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.mode(),
            metadata.uid(),
            metadata.gid(),
            metadata.nlink(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    }
    #[cfg(not(unix))]
    {
        format!(
            "{}:{:?}:{:?}",
            metadata.len(),
            metadata.permissions(),
            metadata.modified()
        )
    }
}

/// A symlinked instance directory would redirect a privileged read somewhere the
/// administrator never approved. Only the immediate parent is checked here; the
/// full ancestor chain belongs to the shared no-follow opener.
fn ensure_managed_parent_is_not_symlink(path: &Path) -> anyhow::Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("受管路径的父目录是符号链接，拒绝读取: {}", parent.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// The paths are program-defined. Legacy manifests never select privileged read targets.
fn managed_targets(ctx: &instance::InstanceContext) -> anyhow::Result<Vec<ManagedArtifact>> {
    let root = managed_state_dir(ctx)?;
    let paths = [
        (ctx.paths.cli_binary.clone(), ArtifactKind::Daemon, true),
        (ctx.paths.core_binary.clone(), ArtifactKind::Core, true),
        (root.join("geoip.metadb"), ArtifactKind::Geoip, false),
        (root.join("GeoSite.dat"), ArtifactKind::Geosite, false),
        (root.join("Country.mmdb"), ArtifactKind::Geoip, false),
    ];
    let mut result = Vec::new();
    let mut names = std::collections::HashSet::new();
    for (path, kind, required) in paths {
        anyhow::ensure!(
            path.is_absolute(),
            "受管路径必须为绝对路径: {}",
            path.display()
        );
        let relative_path = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("非法受管路径"))?
            .to_string();
        generation::validate_artifact_path(&relative_path)?;
        anyhow::ensure!(
            names.insert(relative_path.clone()),
            "重复受管目标: {relative_path}"
        );
        result.push(ManagedArtifact {
            path,
            relative_path,
            kind,
            required,
        });
    }
    Ok(result)
}

fn hash_stream(
    file: &mut std::fs::File,
    keep_bytes: bool,
    capacity: u64,
    limit: u64,
) -> anyhow::Result<(String, u64, Option<Vec<u8>>)> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; HASH_CHUNK_BYTES];
    let mut kept = keep_bytes.then(|| Vec::with_capacity(capacity as usize));
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        anyhow::ensure!(total <= limit, "文件读取超过 {limit} 字节上限");
        hasher.update(&buffer[..read]);
        if let Some(kept) = kept.as_mut() {
            kept.extend_from_slice(&buffer[..read]);
        }
    }
    Ok((format!("{:x}", hasher.finalize()), total, kept))
}

/// Reads the five program-defined artifacts exactly once. `keep_bytes` selects
/// the single approved copy; every later recheck streams the hash and keeps only
/// stat identity.
fn read_artifacts(
    ctx: &instance::InstanceContext,
    keep_bytes: bool,
) -> anyhow::Result<Vec<Artifact>> {
    let mut result = Vec::new();
    let mut approved_total = 0u64;
    for target in managed_targets(ctx)? {
        ensure_managed_parent_is_not_symlink(&target.path)?;
        // The no-follow opener checks every ancestor, rejects hardlinks and special
        // files, and returns the original I/O error for absence/permission failures.
        let mut file = match utils::open_regular_file_no_follow(&target.path) {
            Ok(file) => file,
            Err(error) if utils::is_not_found_error(&error) && !target.required => {
                // An existing dangling symlink is not an absent Geo artifact.
                match std::fs::symlink_metadata(&target.path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                    Ok(_) => {
                        anyhow::bail!("拒绝将不安全文件记为缺失: {}", target.path.display())
                    }
                }
                result.push(Artifact {
                    path: target.path,
                    relative_path: target.relative_path,
                    kind: target.kind,
                    sha256: None,
                    size_bytes: 0,
                    unix_mode: None,
                    safety: "missing".into(),
                    bytes: None,
                });
                continue;
            }
            Err(error) => {
                return Err(error.context(format!("无法安全读取受管文件 {}", target.path.display())))
            }
        };
        let before = file.metadata()?;
        anyhow::ensure!(
            before.is_file() && before.len() <= MAX_ARTIFACT_BYTES,
            "非普通文件或文件过大: {}",
            target.path.display()
        );
        if keep_bytes {
            // Bound the single approved copy before allocating, not after.
            approved_total = approved_total
                .checked_add(before.len())
                .ok_or_else(|| anyhow::anyhow!("批准快照总字节溢出"))?;
            anyhow::ensure!(
                approved_total <= MAX_APPROVED_TOTAL_BYTES,
                "批准快照总字节超过 {} MiB 上限，无法在一次确认中保存完整字节",
                MAX_APPROVED_TOTAL_BYTES / (1024 * 1024)
            );
        }
        let (sha256, size_bytes, bytes) =
            hash_stream(&mut file, keep_bytes, before.len(), MAX_ARTIFACT_BYTES)?;
        let after = file.metadata()?;
        anyhow::ensure!(
            size_bytes == after.len() && safety(&before) == safety(&after),
            "读取期间文件变化: {}",
            target.path.display()
        );
        #[cfg(unix)]
        let unix_mode = {
            use std::os::unix::fs::PermissionsExt;
            Some(after.permissions().mode() & 0o777)
        };
        #[cfg(not(unix))]
        let unix_mode = None;
        result.push(Artifact {
            path: target.path,
            relative_path: target.relative_path,
            kind: target.kind,
            sha256: Some(sha256),
            size_bytes,
            unix_mode,
            safety: safety(&after),
            bytes,
        });
    }
    Ok(result)
}

fn is_adopted(manifest: &GenerationManifest) -> bool {
    manifest.metadata.get("adoption_source").map(String::as_str) == Some(ADOPTION_SOURCE)
}

fn eligible(
    store: &GenerationStore,
    state: &GenerationState,
    discard: Option<&GenerationId>,
) -> anyhow::Result<()> {
    let slots = [&state.active, &state.pending, &state.previous]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !slots.is_empty(),
        "没有需要采纳的迁移代际；首装不能使用 --adopt-current"
    );
    anyhow::ensure!(
        slots.iter().all(|id| store.is_generation_untrusted(id)),
        "存在可信或来源无法确认的槽位，拒绝采纳（已有可信现场不能绕过混合态闸门）"
    );
    for id in slots {
        // The untrusted marker must itself be a readable regular no-follow file.
        utils::read_file_no_follow_limited(
            &store.generation_dir(id).join(generation::UNTRUSTED_MARKER),
            1024,
        )?;
    }
    if state.pending.as_ref() != discard {
        if let Some(pending) = &state.pending {
            anyhow::bail!("迁移 pending {pending} 必须精确指定并同时撤销（本次目标 {:?} 不匹配）：mihomo-cli install --system --adopt-current --discard-pending={pending}", discard.map(GenerationId::as_str));
        }
        anyhow::bail!(
            "当前无 pending，不能指定 --discard-pending={}",
            discard.map(GenerationId::as_str).unwrap_or("")
        );
    }
    Ok(())
}

fn build_scene(
    ctx: &instance::InstanceContext,
    store: &GenerationStore,
    discard: Option<GenerationId>,
    keep_bytes: bool,
) -> anyhow::Result<Snapshot> {
    utils::verify_generation_trust_root(store.root_dir())?;
    let state = store.read_state()?;
    eligible(store, &state, discard.as_ref())?;
    Ok(Snapshot {
        store: store.root_dir().to_path_buf(),
        state,
        discard_pending: discard,
        artifacts: read_artifacts(ctx, keep_bytes)?,
    })
}

/// The approved scene: the only snapshot that keeps the confirmed bytes.
fn snapshot(
    ctx: &instance::InstanceContext,
    store: &GenerationStore,
    discard: Option<GenerationId>,
) -> anyhow::Result<Snapshot> {
    build_scene(ctx, store, discard, true)
}

/// The streaming recheck: hash and stat identity only, never a second byte copy.
fn observe(
    ctx: &instance::InstanceContext,
    store: &GenerationStore,
    discard: Option<GenerationId>,
) -> anyhow::Result<Snapshot> {
    build_scene(ctx, store, discard, false)
}

fn summary(snapshot: &Snapshot) -> anyhow::Result<String> {
    Ok(generation::calculate_sha256_bytes(&serde_json::to_vec(
        snapshot,
    )?))
}

fn display_snapshot(snapshot: &Snapshot) {
    eprintln!("{WARNING}");
    for item in &snapshot.artifacts {
        eprintln!(
            "  {}  {} bytes  sha256={}",
            item.path.display(),
            item.size_bytes,
            item.sha256.as_deref().unwrap_or("missing（缺失）")
        );
    }
    if let Some(id) = &snapshot.discard_pending {
        eprintln!("  将同时撤销迁移候选 {id}；保留原字节，不表示已回滚。");
    }
}

fn artifacts_identical(approved: &[Artifact], observed: &[Artifact]) -> bool {
    approved.len() == observed.len()
        && approved.iter().zip(observed).all(|(approved, observed)| {
            approved.path == observed.path
                && approved.relative_path == observed.relative_path
                && approved.kind == observed.kind
                && approved.sha256 == observed.sha256
                && approved.size_bytes == observed.size_bytes
                && approved.unix_mode == observed.unix_mode
                && approved.safety == observed.safety
        })
}

fn scene_matches(approved: &Snapshot, observed: &Snapshot) -> bool {
    approved.store == observed.store
        && approved.state == observed.state
        && approved.discard_pending == observed.discard_pending
        && artifacts_identical(&approved.artifacts, &observed.artifacts)
}

fn mode_matches(expected: Option<u32>, actual: Option<u32>) -> bool {
    match expected {
        None => true,
        Some(expected) => actual.is_some_and(|actual| expected & 0o777 == actual & 0o777),
    }
}

fn existing_adoption(
    ctx: &instance::InstanceContext,
    store: &GenerationStore,
    discard: Option<&GenerationId>,
) -> anyhow::Result<Option<GenerationId>> {
    let state = store.read_state()?;
    if state.active.is_some() || state.pending.is_some() {
        return Ok(None);
    }
    let Some(id) = state.previous else {
        return Ok(None);
    };
    if store.is_generation_untrusted(&id) {
        return Ok(None);
    }
    let manifest = store.validate_generation(&id)?;
    if !is_adopted(&manifest) {
        return Ok(None);
    }
    if let Some(target) = discard {
        anyhow::ensure!(
            manifest
                .metadata
                .get("adoption_discard_pending")
                .map(String::as_str)
                == Some(target.as_str()),
            "撤销目标与已提交采纳不一致"
        );
    }
    anyhow::ensure!(installed_matches_adopted(ctx, &manifest)?, "采纳后安装态发生漂移，拒绝重新采纳；保留现场并运行 mihomo-cli doctor --system --privileged");
    Ok(Some(id))
}

fn matches_artifacts(manifest: &GenerationManifest, actual: &[Artifact]) -> bool {
    let entries = manifest.all_entries();
    let missing = manifest.missing_paths();
    entries.len() == actual.len()
        && actual.iter().all(|artifact| {
            entries.iter().any(|entry| {
                entry.relative_path == artifact.relative_path
                    && entry.kind == artifact.kind
                    && match &artifact.sha256 {
                        Some(hash) => {
                            !missing.contains(&entry.relative_path)
                                && entry.sha256 == *hash
                                && entry.size_bytes == artifact.size_bytes
                                && mode_matches(entry.unix_mode, artifact.unix_mode)
                        }
                        None => missing.contains(&entry.relative_path),
                    }
            })
        })
}

/// The five fixed roles an adopted baseline binds. Any entry, path, or declared
/// absence outside them is a foreign manifest and fails closed.
fn adopted_expectations(
    ctx: &instance::InstanceContext,
    manifest: &GenerationManifest,
) -> anyhow::Result<Vec<Expectation>> {
    anyhow::ensure!(
        is_adopted(manifest) && manifest.is_baseline(),
        "不是管理员采纳的基线清单，不能用于安装态复核"
    );
    let targets = managed_targets(ctx)?;
    let entries = manifest.all_entries();
    anyhow::ensure!(
        entries.len() == targets.len(),
        "已采纳基线包含 {} 个条目，超出程序固定的 {} 个受管角色",
        entries.len(),
        targets.len()
    );
    let missing = manifest.missing_paths();
    for name in &missing {
        anyhow::ensure!(
            targets.iter().any(|target| &target.relative_path == name),
            "已采纳基线声明了未授权的缺失路径: {name}"
        );
    }
    let mut result = Vec::new();
    for target in targets {
        let entry = entries
            .iter()
            .find(|entry| entry.relative_path == target.relative_path)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "已采纳基线未绑定受管角色 {:?} 的路径 {}",
                    target.kind,
                    target.relative_path
                )
            })?;
        anyhow::ensure!(
            entry.kind == target.kind,
            "已采纳基线中 {} 的角色 {:?} 与程序定义的 {:?} 不符",
            target.relative_path,
            entry.kind,
            target.kind
        );
        if missing.contains(&target.relative_path) {
            result.push(Expectation {
                path: target.path,
                relative_path: target.relative_path,
                kind: target.kind,
                sha256: None,
                size_bytes: 0,
                unix_mode: None,
            });
            continue;
        }
        result.push(Expectation {
            path: target.path,
            relative_path: target.relative_path,
            kind: target.kind,
            sha256: Some(entry.sha256.clone()),
            size_bytes: entry.size_bytes,
            unix_mode: entry.unix_mode,
        });
    }
    Ok(result)
}

fn adopted_matches(expected: &[Expectation], observed: &[Artifact]) -> bool {
    expected.len() == observed.len()
        && expected.iter().zip(observed).all(|(expected, observed)| {
            expected.path == observed.path
                && expected.relative_path == observed.relative_path
                && expected.kind == observed.kind
                && match (&expected.sha256, &observed.sha256) {
                    (Some(expected_hash), Some(observed_hash)) => {
                        expected_hash.eq_ignore_ascii_case(observed_hash)
                            && expected.size_bytes == observed.size_bytes
                            && mode_matches(expected.unix_mode, observed.unix_mode)
                    }
                    (None, None) => true,
                    _ => false,
                }
        })
}

/// Compares the current install against an adopted baseline over exactly the five
/// program-defined roles. Reads stream the hash; no full byte copy is retained,
/// and manifest-declared paths outside those roles are rejected rather than read.
pub(crate) fn installed_matches_adopted(
    ctx: &instance::InstanceContext,
    manifest: &GenerationManifest,
) -> anyhow::Result<bool> {
    let expected = adopted_expectations(ctx, manifest)?;
    let observed = read_artifacts(ctx, false)?;
    Ok(adopted_matches(&expected, &observed))
}

/// Writes only the approved byte vectors, never a second unbound read of installed
/// paths, and makes the new generation directory durable before any state change.
fn persist(
    store: &GenerationStore,
    approved: &Snapshot,
    yes: bool,
) -> anyhow::Result<GenerationId> {
    let id = next_generation_id()?;
    let dir = store.generation_dir(&id);
    // The verified, locked store owns the parent. A collision must fail, never
    // reuse an immutable generation directory (even for a random-ID collision).
    utils::ensure_dir_all_no_follow(&store.generations_dir())?;
    std::fs::create_dir(&dir)?;
    #[cfg(unix)]
    utils::set_directory_mode_no_follow(&dir, 0o755)?;
    let mut entries = Vec::new();
    let mut missing = Vec::new();
    for artifact in &approved.artifacts {
        match &artifact.bytes {
            Some(bytes) => utils::atomic_write_bytes_no_follow(
                &dir.join(&artifact.relative_path),
                bytes,
                artifact.unix_mode.unwrap_or(0o644) as u16,
            )?,
            None => {
                anyhow::ensure!(
                    artifact.sha256.is_none(),
                    "批准快照缺少已确认字节: {}",
                    artifact.path.display()
                );
                missing.push(artifact.relative_path.clone());
            }
        }
        entries.push(ArtifactManifestEntry::new(
            &artifact.relative_path,
            artifact.kind.clone(),
            artifact
                .sha256
                .as_deref()
                .unwrap_or(generation::EMPTY_SHA256),
            artifact.size_bytes,
            artifact.unix_mode,
        )?);
    }
    fire_fault(FAULT_AFTER_ARTIFACTS)?;
    let mut manifest = GenerationManifest::new(
        id.clone(),
        generation::DEFAULT_PROTOCOL_VERSION,
        entries.remove(0),
        entries.remove(0),
    );
    manifest.extra_artifacts = entries;
    manifest = manifest
        .with_metadata("kind", generation::BASELINE_KIND)
        .with_metadata("missing", missing.join(","))
        .with_metadata("adoption_source", ADOPTION_SOURCE)
        .with_metadata(
            "adoption_original_state",
            serde_json::to_string(&approved.state)?,
        )
        .with_metadata(
            "adoption_discard_pending",
            approved
                .discard_pending
                .as_ref()
                .map(GenerationId::as_str)
                .unwrap_or(""),
        )
        .with_metadata("adoption_digest", summary(approved)?)
        .with_metadata(
            "adoption_artifacts",
            serde_json::to_string(&approved.artifacts)?,
        )
        .with_metadata(
            "adoption_authority",
            format!(
                "effective_uid={:?}; original_uid={:?}",
                instance::current_uid(),
                std::env::var("SUDO_UID").ok()
            ),
        )
        .with_metadata(
            "adoption_confirmation",
            if yes { "explicit-yes" } else { "interactive-y" },
        );
    store.write_manifest(&manifest)?;
    let verified = store.validate_generation(&id)?;
    anyhow::ensure!(
        matches_artifacts(&verified, &approved.artifacts),
        "批准快照与保存的基线不一致"
    );
    fire_fault(FAULT_AFTER_VALIDATION)?;
    sync_dir(FAULT_SYNC_GENERATION, &dir)?;
    sync_dir(FAULT_SYNC_GENERATIONS, &store.generations_dir())?;
    Ok(id)
}

pub(crate) fn adopt(
    ctx: &instance::InstanceContext,
    discard: Option<&str>,
    yes: bool,
    dry_run: bool,
    json: bool,
) -> anyhow::Result<()> {
    require_supported_platform("采纳迁移现场（建立新信任）")?;
    let discard = discard.map(GenerationId::new).transpose()?;
    if dry_run {
        return preview(ctx, discard, json);
    }
    anyhow::ensure!(
        yes || (!json && std::io::stdin().is_terminal()),
        "非交互采纳必须明确传 --yes；请先 --dry-run 核对路径与 SHA-256"
    );
    require_privilege("采纳迁移现场（建立新信任）")?;
    let store = system_generation_store(ctx);
    utils::verify_generation_trust_root(store.root_dir())?;
    // Migration is copy-only; it never supplies the authorization to adopt.
    let legacy = managed_state_dir(ctx)?;
    let result = generation::migrate_legacy_store(&legacy, &store)?;
    if result == generation::MigrationOutcome::Migrated {
        eprintln!("代际存储已迁移；迁移不赋予信任，仍需核对并明确批准快照。");
    }
    match existing_adoption(ctx, &store, discard.as_ref()) {
        Ok(Some(id)) => return outcome(json, false, "already_adopted", &id, None),
        Ok(None) => {}
        Err(error) => return Err(error),
    }
    let approved = snapshot(ctx, &store, discard)?;
    display_snapshot(&approved);
    if !yes {
        use std::io::Write;
        eprint!("是否信任以上确切字节并建立回退快照？[y/N]: ");
        std::io::stderr().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        anyhow::ensure!(
            matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
            "已取消采纳，槽位及安装文件未修改"
        );
    }
    // Human interaction never holds the bounded generation lock.
    let id = commit(ctx, &store, &approved, yes)?;
    outcome(json, false, "adopted", &id, Some(&approved))
}

/// Read-only discovery may look at the legacy source without copying it or
/// treating any of its claims as trusted. Actual adoption always re-observes
/// the protected destination after authorization and migration.
fn read_source(ctx: &instance::InstanceContext) -> anyhow::Result<(GenerationStore, bool)> {
    let store = system_generation_store(ctx);
    match std::fs::symlink_metadata(store.root_dir()) {
        Ok(_) => Ok((store, false)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let legacy = GenerationStore::new(managed_state_dir(ctx)?);
            for path in [legacy.state_file_path(), legacy.generations_dir()] {
                match std::fs::symlink_metadata(path) {
                    Ok(_) => return Ok((legacy, true)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok((store, false))
        }
        Err(error) => Err(error.into()),
    }
}

fn preview_outcome(
    ctx: &instance::InstanceContext,
    discard: Option<GenerationId>,
) -> anyhow::Result<PreviewOutcome> {
    let (store, legacy) = read_source(ctx)?;
    let state = store.read_state()?;
    if legacy {
        anyhow::ensure!(
            [&state.active, &state.pending, &state.previous]
                .into_iter()
                .any(Option::is_some),
            "旧布局无引用槽位，不能作为迁移采纳现场"
        );
        anyhow::ensure!(
            state.pending == discard,
            "请为迁移 pending 精确指定 --discard-pending=<ID>"
        );
        return Ok(PreviewOutcome::Required {
            snapshot: Snapshot {
                store: store.root_dir().to_path_buf(),
                state,
                discard_pending: discard,
                artifacts: read_artifacts(ctx, false)?,
            },
            legacy_unmigrated: true,
        });
    }
    if is_current_process_root() {
        utils::verify_generation_trust_root(store.root_dir())?;
    }
    // An already adopted, undrifted scene is idempotent: report exactly what the
    // real command reports instead of failing the untrusted-slot gate.
    if let Some(id) = existing_adoption(ctx, &store, discard.as_ref())? {
        return Ok(PreviewOutcome::AlreadyAdopted(id));
    }
    eligible(&store, &state, discard.as_ref())?;
    Ok(PreviewOutcome::Required {
        snapshot: Snapshot {
            store: store.root_dir().to_path_buf(),
            state,
            discard_pending: discard,
            artifacts: read_artifacts(ctx, false)?,
        },
        legacy_unmigrated: false,
    })
}

fn preview(
    ctx: &instance::InstanceContext,
    discard: Option<GenerationId>,
    json: bool,
) -> anyhow::Result<()> {
    let outcome = match preview_outcome(ctx, discard) {
        Ok(outcome) => outcome,
        Err(error) if is_permission_denied(&error) => return unknown_preview(json),
        Err(error) => return Err(error),
    };
    match outcome {
        PreviewOutcome::AlreadyAdopted(id) => outcome_already(json, &id),
        PreviewOutcome::Required {
            snapshot: approved,
            legacy_unmigrated,
        } => {
            display_snapshot(&approved);
            if json {
                print_json_envelope(
                    "install",
                    serde_json::json!({"state":"adoption_required", "dry_run":true, "legacy_unmigrated":legacy_unmigrated, "snapshot":approved, "warning":WARNING}),
                    Vec::new(),
                )
            } else {
                eprintln!("预览完成，未迁移、未写入、未创建锁。确认后运行 mihomo-cli install --system --adopt-current");
                Ok(())
            }
        }
    }
}

fn commit(
    ctx: &instance::InstanceContext,
    store: &GenerationStore,
    approved: &Snapshot,
    yes: bool,
) -> anyhow::Result<GenerationId> {
    let lock = store.acquire_lock()?;
    utils::verify_generation_trust_root(store.root_dir())?;
    let current = observe(ctx, store, approved.discard_pending.clone())?;
    anyhow::ensure!(
        scene_matches(approved, &current),
        "确认后槽位或安装文件发生变化，拒绝采纳（零槽位覆盖）；请重新预览"
    );
    let id = persist(store, approved, yes)?;
    // Recheck non-cooperating Geo writers after the durable snapshot writes. Only
    // detection is promised: the committed bytes are the approved ones.
    anyhow::ensure!(
        scene_matches(
            approved,
            &observe(ctx, store, approved.discard_pending.clone())?
        ),
        "保存期间现场变化，基线未提交"
    );
    fire_fault(FAULT_BEFORE_STATE)?;
    store.consume_revoked_with_lock(&lock, Some(&id))?;
    fire_fault(FAULT_AFTER_STATE)?;
    Ok(id)
}

fn unknown_preview(json: bool) -> anyhow::Result<()> {
    if json {
        print_json_envelope(
            "install",
            serde_json::json!({"state":"unknown", "dry_run":true, "detail":"当前身份无法判定；可显式运行 mihomo-cli doctor --system --privileged"}),
            Vec::new(),
        )
    } else {
        eprintln!("当前身份无法判定，预览未请求提权、未写入。请显式运行 mihomo-cli doctor --system --privileged");
        Ok(())
    }
}

fn outcome(
    json: bool,
    dry_run: bool,
    state: &str,
    id: &GenerationId,
    snapshot: Option<&Snapshot>,
) -> anyhow::Result<()> {
    if json {
        print_json_envelope(
            "install",
            serde_json::json!({"state":state, "baseline":id, "dry_run":dry_run, "snapshot":snapshot, "warning":WARNING}),
            Vec::new(),
        )
    } else if state == "already_adopted" {
        eprintln!("当前安装字节与此前采纳的可信回退基线 {id} 一致；本次未写入、未停启服务。");
        Ok(())
    } else {
        eprintln!("已采纳当前快照为可信回退基线 {id}（{state}）；未覆盖安装文件、未停启服务。\n  Next: mihomo-cli install --system（准备候选），随后 mihomo-cli restart --system（应用更新，可能中断连接）");
        Ok(())
    }
}

fn outcome_already(json: bool, id: &GenerationId) -> anyhow::Result<()> {
    outcome(json, true, "already_adopted", id, None)
}

pub(crate) fn inspect_privileged(json: bool) -> anyhow::Result<()> {
    require_supported_platform("只读检查受保护代际状态（不修复、不迁移、不初始化）")?;
    require_privilege("只读检查受保护代际状态（不修复、不迁移、不初始化）")?;
    let ctx = instance::planned_current_context(instance::InstanceMode::System)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS"))?;
    inspect(&ctx, json)
}

fn inspect_data(ctx: &instance::InstanceContext) -> anyhow::Result<serde_json::Value> {
    let (store, legacy) = read_source(ctx)?;
    if !legacy {
        utils::verify_generation_trust_root(store.root_dir())?;
    }
    let state = store.read_state()?;
    let mut slots = Vec::new();
    let mut trust = Vec::new();
    for (slot, id) in [
        ("active", &state.active),
        ("pending", &state.pending),
        ("previous", &state.previous),
    ] {
        let Some(id) = id else {
            continue;
        };
        let manifest = store.validate_generation(id).ok();
        // A missing marker without a verifiable manifest is unknown, never trusted.
        let label = if legacy || store.is_generation_untrusted(id) {
            "untrusted"
        } else if manifest.is_some() {
            "trusted"
        } else {
            "unknown"
        };
        let matched = manifest.as_ref().and_then(|manifest| {
            if is_adopted(manifest) {
                installed_matches_adopted(ctx, manifest).ok()
            } else {
                installed_matches_manifest(ctx, &store, id, manifest).ok()
            }
        });
        trust.push(label);
        slots
            .push(serde_json::json!({"slot":slot,"id":id,"trust":label,"installed_match":matched}));
    }
    // Only a non-empty, entirely untrusted set may be pointed at --adopt-current.
    let adoption_required = !trust.is_empty() && trust.iter().all(|label| *label == "untrusted");
    Ok(
        serde_json::json!({"generation_state":state,"slots":slots,"adoption_required":adoption_required,"legacy_unmigrated":legacy,"read_only":true}),
    )
}

fn inspect(ctx: &instance::InstanceContext, json: bool) -> anyhow::Result<()> {
    let data = inspect_data(ctx)?;
    if json {
        print_json_envelope("doctor", data, Vec::new())
    } else {
        eprintln!(
            "管理员只读代际检查（未迁移、未写入、未停启）：\n{}",
            serde_json::to_string_pretty(&data)?
        );
        if data["adoption_required"] == serde_json::Value::Bool(true) {
            eprintln!("迁移记录未经认证；采纳当前完整快照需管理员显式建立新信任：mihomo-cli install --system --adopt-current（有 pending 时同时 --discard-pending=<ID>）");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (
        tempfile::TempDir,
        instance::InstanceContext,
        GenerationStore,
        GenerationId,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let inputs = instance::PathInputs {
            home: temp.path().join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(temp.path().join("run")),
            program_data: temp.path().join("ProgramData"),
            app_data: temp.path().join("AppData"),
            local_app_data: temp.path().join("Local"),
            username_or_sid: "test".into(),
        };
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.cli_binary = temp.path().join("bin/mihomo-cli");
        ctx.paths.core_binary = temp.path().join("bin/mihomo");
        ctx.paths.tun_config_file = temp.path().join("runtime/tun-config.yaml");
        ctx.paths.generation_store_root = temp.path().join("trusted");
        std::fs::create_dir_all(temp.path().join("bin")).unwrap();
        std::fs::create_dir_all(temp.path().join("runtime")).unwrap();
        std::fs::write(&ctx.paths.cli_binary, b"cli-unrecorded-current").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-unrecorded-current").unwrap();
        let store = system_generation_store(&ctx);
        store.init().unwrap();
        let legacy = GenerationId::new("legacy-unknown").unwrap();
        utils::ensure_dir_all_no_follow(&store.generation_dir(&legacy)).unwrap();
        store.mark_generation_untrusted(&legacy).unwrap();
        store
            .write_state(&GenerationState {
                active: Some(legacy.clone()),
                ..Default::default()
            })
            .unwrap();
        (temp, ctx, store, legacy)
    }

    fn write_geometry(temp: &tempfile::TempDir) {
        for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb"] {
            std::fs::write(temp.path().join("runtime").join(name), name).unwrap();
        }
    }

    #[cfg(unix)]
    fn file_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn replace_with_same_bytes(source: &Path, temp: &tempfile::TempDir) {
        let bytes = std::fs::read(source).unwrap();
        let mode = file_mode(source);
        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, bytes).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(mode)).unwrap();
        std::fs::rename(&replacement, source).unwrap();
    }

    #[test]
    fn explicit_approval_creates_complete_baseline_without_legacy_manifest_or_fingerprint() {
        let (_temp, ctx, store, legacy) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        assert_eq!(approved.artifacts.len(), 5);
        assert_eq!(
            approved
                .artifacts
                .iter()
                .filter(|a| a.sha256.is_none())
                .count(),
            3
        );
        let original = std::fs::read(&ctx.paths.cli_binary).unwrap();
        test_hooks::take_sync_log();
        let id = commit(&ctx, &store, &approved, true).unwrap();
        let state = store.read_state().unwrap();
        assert!(state.active.is_none() && state.pending.is_none());
        assert_eq!(state.previous, Some(id.clone()));
        let manifest = store.validate_generation(&id).unwrap();
        assert!(is_adopted(&manifest));
        assert_eq!(manifest.all_entries().len(), 5);
        assert_eq!(
            manifest.metadata["adoption_digest"],
            summary(&approved).unwrap()
        );
        assert!(store.is_generation_untrusted(&legacy));
        assert_eq!(std::fs::read(&ctx.paths.cli_binary).unwrap(), original);
        assert_eq!(
            existing_adoption(&ctx, &store, None).unwrap(),
            Some(id.clone())
        );
        assert!(installed_matches_adopted(&ctx, &manifest).unwrap());
        // The new generation directory is made durable before its parent entry.
        assert_eq!(
            test_hooks::take_sync_log(),
            vec![store.generation_dir(&id), store.generations_dir()]
        );
        assert!(store.cleanup_old_generations(0).unwrap().is_empty());
    }

    #[test]
    fn all_geo_files_are_program_selected_and_bound() {
        let (temp, ctx, store, _) = fixture();
        write_geometry(&temp);
        let approved = snapshot(&ctx, &store, None).unwrap();
        assert!(approved.artifacts.iter().all(|a| a.sha256.is_some()));
        let id = commit(&ctx, &store, &approved, true).unwrap();
        assert_eq!(
            store
                .validate_generation(&id)
                .unwrap()
                .missing_paths()
                .len(),
            0
        );
        std::fs::write(temp.path().join("runtime/GeoSite.dat"), "changed").unwrap();
        assert!(existing_adoption(&ctx, &store, None).is_err());
    }

    #[test]
    fn pending_only_requires_exact_combined_discard_and_never_becomes_first_install() {
        let (_temp, ctx, store, legacy) = fixture();
        let old = GenerationState {
            pending: Some(legacy.clone()),
            ..Default::default()
        };
        store.write_state(&old).unwrap();
        assert!(snapshot(&ctx, &store, None).is_err());
        assert!(snapshot(&ctx, &store, Some(GenerationId::new("wrong").unwrap())).is_err());
        assert_eq!(store.read_state().unwrap(), old);
        let approved = snapshot(&ctx, &store, Some(legacy.clone())).unwrap();
        let id = commit(&ctx, &store, &approved, true).unwrap();
        assert_eq!(store.read_state().unwrap().previous, Some(id.clone()));
        assert_eq!(
            store.validate_generation(&id).unwrap().metadata["adoption_discard_pending"],
            legacy.as_str()
        );
        assert!(store.generation_dir(&legacy).exists());
    }

    #[test]
    fn adoption_requires_every_referenced_slot_to_be_untrusted() {
        let (_temp, ctx, store, legacy) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        let trusted = persist(&store, &approved, true).unwrap();
        let cases = [
            (GenerationState::default(), false),
            (
                GenerationState {
                    active: Some(legacy.clone()),
                    ..Default::default()
                },
                true,
            ),
            (
                GenerationState {
                    pending: Some(legacy.clone()),
                    ..Default::default()
                },
                true,
            ),
            (
                GenerationState {
                    active: Some(legacy.clone()),
                    pending: Some(legacy.clone()),
                    ..Default::default()
                },
                true,
            ),
            (
                GenerationState {
                    active: Some(legacy.clone()),
                    previous: Some(trusted.clone()),
                    ..Default::default()
                },
                false,
            ),
        ];
        for (state, adoptable) in cases {
            store.write_state(&state).unwrap();
            let discard = state.pending.clone();
            assert_eq!(
                snapshot(&ctx, &store, discard.clone()).is_ok(),
                adoptable,
                "{state:?}"
            );
            assert_eq!(
                preview_outcome(&ctx, discard).is_ok(),
                adoptable,
                "{state:?}"
            );
            // A mixed or empty set must never advertise --adopt-current.
            assert_eq!(
                inspect_data(&ctx).unwrap()["adoption_required"],
                serde_json::Value::Bool(adoptable),
                "{state:?}"
            );
        }
    }

    #[test]
    fn inspect_reports_unverifiable_slots_as_unknown_not_trusted() {
        let (_temp, ctx, store, _) = fixture();
        let broken = GenerationId::new("gen-broken").unwrap();
        utils::ensure_dir_all_no_follow(&store.generation_dir(&broken)).unwrap();
        store
            .write_state(&GenerationState {
                previous: Some(broken),
                ..Default::default()
            })
            .unwrap();
        let data = inspect_data(&ctx).unwrap();
        assert_eq!(data["slots"][0]["trust"], "unknown");
        assert!(data["slots"][0]["installed_match"].is_null());
        assert_eq!(data["adoption_required"], false);
    }

    #[test]
    fn preview_reports_already_adopted_idempotently() {
        let (_temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        let id = commit(&ctx, &store, &approved, true).unwrap();
        let before = std::fs::read(store.state_file_path()).unwrap();
        let lock_before = std::fs::symlink_metadata(store.lock_file_path())
            .unwrap()
            .modified()
            .unwrap();
        for _ in 0..2 {
            match preview_outcome(&ctx, None).unwrap() {
                PreviewOutcome::AlreadyAdopted(seen) => assert_eq!(seen, id),
                PreviewOutcome::Required { .. } => {
                    panic!("已采纳且无漂移的现场不应再次要求确认")
                }
            }
        }
        assert_eq!(
            std::fs::symlink_metadata(store.lock_file_path())
                .unwrap()
                .modified()
                .unwrap(),
            lock_before
        );
        assert_eq!(std::fs::read(store.state_file_path()).unwrap(), before);
        adopt(&ctx, None, false, true, true).unwrap();
    }

    #[test]
    fn non_interactive_adoption_without_yes_never_writes() {
        let (_temp, ctx, store, _) = fixture();
        let before = store.read_state().unwrap();
        assert!(adopt(&ctx, None, false, false, true).is_err());
        assert_eq!(store.read_state().unwrap(), before);
        assert!(!store.lock_file_path().exists());
    }

    #[test]
    fn confirmation_cannot_authorize_later_bytes_or_slots() {
        let (_temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        std::fs::write(&ctx.paths.core_binary, b"changed-after-confirmation").unwrap();
        assert!(commit(&ctx, &store, &approved, true).is_err());
        assert_eq!(store.read_state().unwrap(), approved.state);
        let approved = snapshot(&ctx, &store, None).unwrap();
        store.write_state(&GenerationState::default()).unwrap();
        assert!(commit(&ctx, &store, &approved, true).is_err());
        assert!(store.read_state().unwrap().previous.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn confirmation_binds_file_identity_so_same_bytes_in_a_new_inode_are_rejected() {
        let (temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        replace_with_same_bytes(&ctx.paths.cli_binary, &temp);
        assert!(commit(&ctx, &store, &approved, true).is_err());
        assert_eq!(store.read_state().unwrap(), approved.state);
    }

    #[cfg(unix)]
    #[test]
    fn reinstall_with_identical_bytes_still_matches_but_confirmation_does_not() {
        let (temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        let id = commit(&ctx, &store, &approved, true).unwrap();
        let manifest = store.validate_generation(&id).unwrap();
        replace_with_same_bytes(&ctx.paths.cli_binary, &temp);
        // Post-install recheck binds role/path/type/nlink/hash/size/mode, not inode.
        assert!(installed_matches_adopted(&ctx, &manifest).unwrap());
        assert!(existing_adoption(&ctx, &store, None).is_ok());
        // Confirmation still binds the exact stat identity of the approved bytes.
        assert!(!artifacts_identical(
            &approved.artifacts,
            &read_artifacts(&ctx, false).unwrap()
        ));
    }

    #[test]
    fn persisted_but_uncommitted_snapshot_is_not_authority() {
        let (_temp, ctx, store, legacy) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        let orphan = persist(&store, &approved, true).unwrap();
        assert_eq!(store.read_state().unwrap(), approved.state);
        assert_eq!(existing_adoption(&ctx, &store, None).unwrap(), None);
        let id = commit(&ctx, &store, &approved, true).unwrap();
        assert_ne!(id, orphan);
        assert!(store.is_generation_untrusted(&legacy));
    }

    #[test]
    fn failed_publication_stages_leave_no_authority_and_are_retryable() {
        let (_temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();

        test_hooks::arm_generation_id("gen-half-written");
        test_hooks::arm_fault(FAULT_AFTER_ARTIFACTS);
        assert!(persist(&store, &approved, true).is_err());
        let half = GenerationId::new("gen-half-written").unwrap();
        // Artifact bytes exist, but no validated manifest was published.
        assert!(store.generation_dir(&half).exists());
        assert!(store.validate_generation(&half).is_err());
        assert_eq!(store.read_state().unwrap(), approved.state);

        test_hooks::arm_generation_id("gen-validated");
        test_hooks::arm_fault(FAULT_AFTER_VALIDATION);
        assert!(persist(&store, &approved, true).is_err());
        let validated = GenerationId::new("gen-validated").unwrap();
        // Even a fully validated B snapshot is not authority until the state commits.
        assert!(store.validate_generation(&validated).is_ok());
        assert_eq!(store.read_state().unwrap(), approved.state);

        let id = commit(&ctx, &store, &approved, true).unwrap();
        assert_eq!(store.read_state().unwrap().previous, Some(id));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_hardlinks_special_files_and_missing_binaries_are_rejected() {
        use std::os::unix::fs::symlink;
        let (temp, ctx, store, _) = fixture();
        let geo = temp.path().join("runtime/GeoSite.dat");
        symlink(temp.path().join("does-not-exist"), &geo).unwrap();
        assert!(snapshot(&ctx, &store, None).is_err());
        std::fs::remove_file(&geo).unwrap();
        std::fs::hard_link(&ctx.paths.core_binary, &geo).unwrap();
        assert!(snapshot(&ctx, &store, None).is_err());
        std::fs::remove_file(&geo).unwrap();
        let fifo = std::ffi::CString::new(geo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(snapshot(&ctx, &store, None).is_err());
        std::fs::remove_file(&geo).unwrap();
        std::fs::remove_file(&ctx.paths.core_binary).unwrap();
        assert!(snapshot(&ctx, &store, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_managed_state_directory_is_rejected() {
        use std::os::unix::fs::symlink;
        let (temp, ctx, store, _) = fixture();
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb"] {
            std::fs::write(elsewhere.join(name), name).unwrap();
        }
        let runtime = temp.path().join("runtime");
        std::fs::remove_dir(&runtime).unwrap();
        symlink(&elsewhere, &runtime).unwrap();
        assert!(snapshot(&ctx, &store, None).is_err());
        assert!(preview_outcome(&ctx, None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn mode_drift_after_adoption_is_not_a_match() {
        use std::os::unix::fs::PermissionsExt;
        let (_temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        let id = commit(&ctx, &store, &approved, true).unwrap();
        let manifest = store.validate_generation(&id).unwrap();
        assert!(installed_matches_adopted(&ctx, &manifest).unwrap());
        std::fs::set_permissions(
            &ctx.paths.cli_binary,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        assert!(!installed_matches_adopted(&ctx, &manifest).unwrap());
        assert!(existing_adoption(&ctx, &store, None).is_err());
    }

    #[test]
    fn geo_presence_drift_in_both_directions_is_not_a_match() {
        let (temp, ctx, store, _) = fixture();
        write_geometry(&temp);
        let approved = snapshot(&ctx, &store, None).unwrap();
        let id = commit(&ctx, &store, &approved, true).unwrap();
        let manifest = store.validate_generation(&id).unwrap();
        assert!(installed_matches_adopted(&ctx, &manifest).unwrap());
        std::fs::remove_file(temp.path().join("runtime/GeoSite.dat")).unwrap();
        assert!(!installed_matches_adopted(&ctx, &manifest).unwrap());

        // The opposite direction: a file declared missing at adoption must stay absent.
        let (temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        let id = commit(&ctx, &store, &approved, true).unwrap();
        let manifest = store.validate_generation(&id).unwrap();
        assert!(installed_matches_adopted(&ctx, &manifest).unwrap());
        std::fs::write(temp.path().join("runtime/Country.mmdb"), b"appeared").unwrap();
        assert!(!installed_matches_adopted(&ctx, &manifest).unwrap());
    }

    #[test]
    fn adopted_baseline_with_foreign_paths_is_rejected() {
        let (_temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        let id = commit(&ctx, &store, &approved, true).unwrap();
        let manifest = store.validate_generation(&id).unwrap();

        let mut extra = manifest.clone();
        extra.extra_artifacts.push(
            ArtifactManifestEntry::new(
                "payload.bin",
                ArtifactKind::Other("payload".into()),
                generation::EMPTY_SHA256,
                0,
                Some(0o644),
            )
            .unwrap(),
        );
        assert!(installed_matches_adopted(&ctx, &extra).is_err());

        let mut foreign_missing = manifest.clone();
        foreign_missing
            .metadata
            .insert("missing".into(), "payload.bin".into());
        assert!(installed_matches_adopted(&ctx, &foreign_missing).is_err());

        let mut renamed = manifest.clone();
        renamed.daemon.relative_path = "other-daemon".into();
        assert!(installed_matches_adopted(&ctx, &renamed).is_err());
    }

    #[test]
    fn exclusive_generation_directory_collision_is_rejected() {
        let (_temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        test_hooks::arm_generation_id("gen-collision");
        let id = persist(&store, &approved, true).unwrap();
        assert_eq!(id.as_str(), "gen-collision");
        let evidence = store.generation_dir(&id).join("evidence.txt");
        std::fs::write(&evidence, b"preserved").unwrap();
        test_hooks::arm_generation_id("gen-collision");
        assert!(persist(&store, &approved, true).is_err());
        assert_eq!(std::fs::read(&evidence).unwrap(), b"preserved");
        assert_eq!(store.read_state().unwrap(), approved.state);
    }

    #[test]
    fn directory_sync_failure_propagates_without_committing_state() {
        let (_temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();
        for point in [FAULT_SYNC_GENERATION, FAULT_SYNC_GENERATIONS] {
            test_hooks::arm_fault(point);
            assert!(commit(&ctx, &store, &approved, true).is_err());
            assert_eq!(store.read_state().unwrap(), approved.state);
        }
        // The published generation is validated before either sync; a retry works.
        let id = commit(&ctx, &store, &approved, true).unwrap();
        assert_eq!(store.read_state().unwrap().previous, Some(id.clone()));
        assert!(store.validate_generation(&id).is_ok());
    }

    #[test]
    fn commit_reports_uncertain_outcome_after_the_state_write() {
        let (_temp, ctx, store, _) = fixture();
        let approved = snapshot(&ctx, &store, None).unwrap();

        test_hooks::arm_generation_id("gen-before-state");
        test_hooks::arm_fault(FAULT_BEFORE_STATE);
        assert!(commit(&ctx, &store, &approved, true).is_err());
        let published = store.generation_dir(&GenerationId::new("gen-before-state").unwrap());
        // The bytes are published and durable, but a published snapshot is not authority.
        assert!(store
            .validate_generation(&GenerationId::new("gen-before-state").unwrap())
            .is_ok());
        assert!(published.exists());
        assert_eq!(store.read_state().unwrap(), approved.state);
        assert_eq!(existing_adoption(&ctx, &store, None).unwrap(), None);

        test_hooks::arm_generation_id("gen-after-state");
        test_hooks::arm_fault(FAULT_AFTER_STATE);
        assert!(commit(&ctx, &store, &approved, true).is_err());
        let committed = GenerationId::new("gen-after-state").unwrap();
        // The state write happened: the next run must recognize the adoption.
        assert_eq!(
            store.read_state().unwrap().previous,
            Some(committed.clone())
        );
        assert_eq!(
            existing_adoption(&ctx, &store, None).unwrap(),
            Some(committed.clone())
        );
        match preview_outcome(&ctx, None).unwrap() {
            PreviewOutcome::AlreadyAdopted(seen) => assert_eq!(seen, committed),
            PreviewOutcome::Required { .. } => panic!("已提交的采纳必须报告 already_adopted"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn directory_sync_helper_reports_missing_directory() {
        let temp = tempfile::tempdir().unwrap();
        assert!(sync_directory_no_follow(temp.path()).is_ok());
        assert!(sync_directory_no_follow(&temp.path().join("absent")).is_err());
    }

    #[test]
    fn previews_do_not_initialize_lock_or_change_slots() {
        let (_temp, ctx, store, _) = fixture();
        let before = std::fs::read(store.state_file_path()).unwrap();
        assert!(!store.lock_file_path().exists());
        adopt(&ctx, None, false, true, true).unwrap();
        assert!(!store.lock_file_path().exists());
        assert_eq!(std::fs::read(store.state_file_path()).unwrap(), before);
    }

    #[test]
    fn command_flags_keep_adoption_and_privileged_read_explicit() {
        for args in [
            vec!["mihomo-cli", "install", "--adopt-current"],
            vec!["mihomo-cli", "install", "--system", "--dry-run"],
            vec![
                "mihomo-cli",
                "install",
                "--system",
                "--adopt-current",
                "--force",
            ],
            vec!["mihomo-cli", "doctor", "--privileged"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        assert!(Cli::try_parse_from([
            "mihomo-cli",
            "install",
            "--system",
            "--adopt-current",
            "--discard-pending=P",
            "--yes"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "mihomo-cli",
            "doctor",
            "--system",
            "--privileged",
            "--json"
        ])
        .is_ok());
    }
}
