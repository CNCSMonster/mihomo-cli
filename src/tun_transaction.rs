use crate::instance::InstanceContext;
use crate::utils;
use serde::{Deserialize, Serialize};
use std::fs::File;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const MAX_TRANSACTION_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_TRANSACTION_JOURNAL_BYTES: u64 = 256 * 1024;
pub const MAX_TRANSACTION_EVIDENCE_BYTES: u64 = 256 * 1024;

/// §4.7：TUN **控制键**（由 `tun on/off` 事务写入或归一化的字段）。
///
/// 只有这些键从 override 中剔除；`mtu` / `device` / `route-exclude-address` 等
/// 非控制键仍按 ADR-22 作为 override 的权威取值进入 intent（review 回归-3：
/// 整块拒绝会让这些键彻底失去入口，`route-exclude-address` 更是 的推荐手段）。
///
/// 常量放在事务模块，供 promotion/freeze 的键级合并与 `config`
/// 的 override 过滤共用；`config` 仅做再导出，避免两套控制键清单。
pub(crate) const TUN_TRANSACTION_CONTROLLED_KEYS: &[&str] = &[
    "enable",
    "stack",
    "dns-hijack",
    "strict-route",
    "auto-route",
    "auto-detect-interface",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum JournalPhase {
    Prepared,
    PromotionPending,
    SnapshotPromoted,
    CoreApplied,
    RollbackPending,
    IntentCommitted,
    RolledBack,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TerminalOutcome {
    Applied,
    AppliedAfterRecovery,
    RolledBackAfterApplyFailure,
    RolledBackByUser,
    RolledBackAfterIntentConflict,
    LegacySafeCancel,
    LegacyConvergedToCurrentIntent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TransactionErrorCode {
    StaleFence,
    UnsafeArtifact,
    ArtifactRevisionMismatch,
    SnapshotConflict,
    RuntimeRevisionMismatch,
    CoreStartFailed,
    CoreStopFailed,
    ApiUnavailable,
    RuntimeTunMismatch,
    IntentConflict,
    UnsupportedLaunchSource,
    UnsupportedJournalSchema,
    LegacyRollbackEvidenceMissing,
    ObservationUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredError {
    pub code: TransactionErrorCode,
    pub stage: String,
    pub retryable: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum LaunchSource {
    SystemActiveConfig,
    SystemTunSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OldRuntimeEvidence {
    pub core_running: bool,
    pub core_identity: String,
    pub core_pid: u32,
    pub launched_revision: String,
    pub launch_source: LaunchSource,
    pub runtime_tun: bool,
    pub api_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RuntimeProofKind {
    OldRuntimeValidated,
    CandidateApplied,
    CandidateAttested,
    CandidateQuiesced,
    OldRuntimeRestored,
    LegacyRecoveryTargetApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProof {
    pub transaction_id: String,
    pub generation: u64,
    pub observed_phase: JournalPhase,
    pub proof_kind: RuntimeProofKind,
    pub core_identity: String,
    pub core_pid: u32,
    pub launched_revision: String,
    pub runtime_tun: bool,
    pub api_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeObservation {
    pub core_running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_tun: Option<bool>,
    pub api_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionFence {
    pub transaction_id: String,
    pub generation: u64,
    pub expected_phase: JournalPhase,
    pub expected_candidate_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "PascalCase")]
pub enum TransactionResponse {
    Completed(RuntimeProof),
    AlreadySatisfied(RuntimeProof),
    NotSatisfied {
        observation: RuntimeObservation,
        error: StructuredError,
    },
    Stale {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_transaction_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_generation: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_phase: Option<JournalPhase>,
    },
    EvidenceMismatch {
        observation: RuntimeObservation,
        error: StructuredError,
    },
    Unavailable {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observation: Option<RuntimeObservation>,
        error: StructuredError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemTransactionGate {
    pub schema_version: u32,
    pub transaction_id: String,
    pub generation: u64,
    pub original_uid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_intent_revision: Option<String>,
    pub candidate_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunJournal {
    pub schema_version: u32,
    pub transaction_id: String,
    pub generation: u64,
    pub phase: JournalPhase,
    pub original_uid: u32,
    pub target_runtime_tun: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_intent_revision: Option<String>,
    pub candidate_revision: String,
    /// Durable identity of the Core instance started from this transaction's
    /// candidate.  Recovery must not infer ownership from the revision alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_core_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_core_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_snapshot_revision: Option<String>,
    pub old_runtime_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_target_revision: Option<String>,

    #[serde(default)]
    pub legacy_source: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_base_revision: Option<String>,
    #[serde(default)]
    pub rollback_evidence_complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<StructuredError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TerminalOutcome>,
    /// Durable identity of the Core instance that applied a legacy recovery
    /// target.  It makes retry/AlreadySatisfied proofs idempotent and scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_core_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_core_pid: Option<u32>,
    /// SPEC B1: RFC3339 timestamp of last journal update.
    /// Used for staleness detection (>15min non-terminal = stale).
    /// Old journals missing this field = age=unknown (diagnose only, no write).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum RecoveryDirection {
    Resume,
    Abort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotClassification {
    Old,
    Candidate,
    Other,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentClassification {
    Base,
    Candidate,
    Other,
    Unreadable,
}

/// the rollback protocol can only attest the old runtime through its
/// durable evidence, so the planner needs the evidence verdict as an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OldRuntimeEvidenceState {
    /// Present and matching the journal digest.
    Present,
    /// No durable evidence file at all (deleted or never published).
    Missing,
    /// Present but unreadable, schema-invalid or digest-mismatched.
    Invalid,
}

impl OldRuntimeEvidenceState {
    pub fn describe(self) -> &'static str {
        match self {
            OldRuntimeEvidenceState::Present => "valid",
            OldRuntimeEvidenceState::Missing => "missing",
            OldRuntimeEvidenceState::Invalid => "unreadable or does not match the journal digest",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum RecoveryAction {
    NoOpTerminal,
    RepairPhaseToSnapshotPromoted,
    RetryApply,
    CommitIntent,
    FinalizeCommittedIntent,
    BeginRollback,
    ContinueRollback,
    MarkRolledBack,
    RebuildOwnerGate,
    CleanupTerminal,
    ConvergeLegacyToCurrentIntent,
    RefuseNeedsEvidence(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentCommitResult {
    Committed,
    AlreadyCandidate,
    Conflict,
}

pub fn sha256_revision(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn legacy_fnv_revision(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub fn content_revision(bytes: &[u8]) -> String {
    sha256_revision(bytes)
}

// ---------------- Paths ----------------

pub fn transactions_dir(ctx: &InstanceContext) -> PathBuf {
    ctx.paths
        .tun_config_file
        .parent()
        .map(|p| p.join("transactions"))
        .unwrap_or_else(|| ctx.paths.config_dir.join("transactions"))
}

#[allow(dead_code)]
pub fn active_config_path(ctx: &InstanceContext) -> PathBuf {
    ctx.paths
        .tun_config_file
        .parent()
        .unwrap_or_else(|| Path::new("/var/lib/mihomo-cli"))
        .join("active-config.yaml")
}

pub fn coordinator_lock_path(ctx: &InstanceContext) -> PathBuf {
    transactions_dir(ctx).join(".coordinator.lock")
}

pub fn generation_counter_path(ctx: &InstanceContext) -> PathBuf {
    transactions_dir(ctx).join("generation.counter")
}

pub fn active_dir(ctx: &InstanceContext) -> PathBuf {
    transactions_dir(ctx).join("active")
}

pub fn active_journal_path(ctx: &InstanceContext) -> PathBuf {
    active_dir(ctx).join("journal.json")
}

pub fn active_candidate_path(ctx: &InstanceContext) -> PathBuf {
    active_dir(ctx).join("candidate.yaml")
}

pub fn active_recovery_target_path(ctx: &InstanceContext) -> PathBuf {
    active_dir(ctx).join("recovery-target.yaml")
}

pub fn active_old_snapshot_path(ctx: &InstanceContext) -> PathBuf {
    active_dir(ctx).join("old-snapshot.yaml")
}

pub fn active_old_snapshot_missing_path(ctx: &InstanceContext) -> PathBuf {
    active_dir(ctx).join("old-snapshot.missing")
}

pub fn active_old_runtime_path(ctx: &InstanceContext) -> PathBuf {
    active_dir(ctx).join("old-runtime.json")
}

pub fn gc_dir(ctx: &InstanceContext) -> PathBuf {
    transactions_dir(ctx).join("gc")
}

pub fn user_gate_path(config_dir: &Path) -> PathBuf {
    config_dir.join(".system-transaction-gate.json")
}

pub fn legacy_journal_path(ctx: &InstanceContext) -> PathBuf {
    transactions_dir(ctx).join("tun-journal.json")
}

pub fn legacy_candidate_path(ctx: &InstanceContext) -> PathBuf {
    transactions_dir(ctx).join("tun-candidate.yaml")
}

// ---------------- Coordinator Lock ----------------

pub struct CoordinatorLock {
    #[cfg(any(unix, windows))]
    #[allow(dead_code)]
    file: File,
    #[cfg(all(not(unix), not(windows)))]
    _path: PathBuf,
}

impl CoordinatorLock {
    pub fn acquire(ctx: &InstanceContext) -> anyhow::Result<Self> {
        let lock_path = coordinator_lock_path(ctx);
        if let Some(parent) = lock_path.parent() {
            if !parent.exists() {
                ensure_system_dir(parent, 0o750)?;
            }
        }
        #[cfg(unix)]
        {
            let file = utils::open_file_create_no_follow(&lock_path)?;
            let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
            let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                anyhow::bail!(
                    "flock failed on coordinator lock {}: {}",
                    lock_path.display(),
                    err
                );
            }
            Ok(Self { file })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let poll_interval = std::time::Duration::from_millis(50);
            let file = loop {
                match std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .share_mode(0)
                    .open(&lock_path)
                {
                    Ok(file) => break file,
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::PermissionDenied
                                | std::io::ErrorKind::AlreadyExists
                                | std::io::ErrorKind::WouldBlock
                        ) || matches!(err.raw_os_error(), Some(32 | 33)) =>
                    {
                        if std::time::Instant::now() >= deadline {
                            anyhow::bail!(
                                "Coordinator lock timeout after 30s on {}",
                                lock_path.display()
                            );
                        }
                        std::thread::sleep(poll_interval);
                    }
                    Err(err) => {
                        anyhow::bail!(
                            "Cannot open coordinator lock {}: {}",
                            lock_path.display(),
                            err
                        );
                    }
                }
            };
            Ok(Self { file })
        }
        #[cfg(all(not(unix), not(windows)))]
        {
            Ok(Self { _path: lock_path })
        }
    }
}

#[cfg(unix)]
impl Drop for CoordinatorLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// 把持久 TUN 快照合并进待提升配置。
///
/// 不能整块替换：snapshot 中的控制键归 TUN 事务，而 `route-exclude-address`、
/// `mtu`、`device` 等非控制键属于用户 intent/override。整块替换会在
/// `restart`/`tun on` 时静默丢掉新配置刚写入的非控制键。
///
/// 语义（payload-strict）：
/// - 载荷没有 `tun` 键，或 `tun` 不是映射：整块沿用 snapshot；
/// - 双方都是映射：控制键取 snapshot；非控制键只取载荷，载荷未声明的
///   snapshot 独有非控制键被丢弃（snapshot 不是第二事实来源）。
///
/// 该函数同时被 CLI 冻结（[`merge_system_tun_snapshot_into_payload`]）与 daemon
/// promotion 使用，必须保持单实现、幂等，且输出 revision 由合并后的字节计算。
#[allow(dead_code)]
pub fn merge_snapshot_tun_block_into_config_yaml(
    config_content: &str,
    snapshot_tun: &serde_yaml::Value,
) -> anyhow::Result<(String, String)> {
    let snapshot = snapshot_tun.as_mapping().ok_or_else(|| {
        anyhow::anyhow!("active system TUN snapshot 'tun' block must be a YAML mapping")
    })?;
    let mut doc: serde_yaml::Value = serde_yaml::from_str(config_content)
        .map_err(|e| anyhow::anyhow!("system configuration YAML is invalid: {e}"))?;
    let root_map = doc
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("system configuration root must be a YAML mapping"))?;

    let tun_key = serde_yaml::Value::String("tun".to_string());
    match root_map.get_mut(&tun_key) {
        Some(serde_yaml::Value::Mapping(payload_tun)) => {
            // payload-strict：非控制键只信 intent；snapshot 独有的非控制键不补回，
            // 否则 active snapshot revision 会永久偏离 intent revision（/）。
            merge_tun_key_policy(snapshot, payload_tun, false);
        }
        Some(other) => {
            // intent 的 tun 不是映射（如 tun: null）时，不新增 promotion 失败模式：
            // 以运行快照整块替换，保持 `restart` 的恢复入口可用。
            crate::log!("intent 的 tun 块不是映射，提升时以运行快照整块替换");
            *other = serde_yaml::Value::Mapping(snapshot.clone());
        }
        None => {
            root_map.insert(tun_key, serde_yaml::Value::Mapping(snapshot.clone()));
        }
    }

    let promoted = serde_yaml::to_string(&doc).map_err(|e| {
        anyhow::anyhow!("failed to serialize promoted system configuration with TUN: {e}")
    })?;
    let revision = sha256_revision(promoted.as_bytes());
    Ok((promoted, revision))
}

/// TUN 块键级合并原语，供配置重生成（fill=true）与 snapshot promotion
/// （fill=false）共用。
///
/// - `transaction_side` 中的控制键覆盖 `intent_side`；
/// - `fill=true` 时，intent 未声明的非控制键由 transaction 侧补齐；
/// - `fill=false` 时，intent 未声明的非控制键不补回（payload-strict）。
pub(crate) fn merge_tun_key_policy(
    transaction_side: &serde_yaml::Mapping,
    intent_side: &mut serde_yaml::Mapping,
    fill_missing_noncontrolled: bool,
) {
    for (key, value) in transaction_side {
        let controlled = key
            .as_str()
            .is_some_and(|key| TUN_TRANSACTION_CONTROLLED_KEYS.contains(&key));
        let fill_missing = fill_missing_noncontrolled && !intent_side.contains_key(key);
        if controlled || fill_missing {
            intent_side.insert(key.clone(), value.clone());
        }
    }
}

/// the daemon merges the persisted TUN snapshot into the promoted
/// config. The CLI must freeze/start the same effective bytes, otherwise the
/// start receipt revision (post-merge) can never equal the frozen revision
/// (pre-merge) whenever TUN is enabled.
pub fn merge_system_tun_snapshot_into_payload(
    ctx: &InstanceContext,
    config_content: &str,
    config_revision: &str,
) -> anyhow::Result<(String, String)> {
    let snapshot_path = &ctx.paths.tun_config_file;
    let bytes = match crate::utils::read_file_no_follow_limited(
        snapshot_path,
        MAX_TRANSACTION_ARTIFACT_BYTES,
    ) {
        Ok(bytes) => bytes,
        Err(error) if crate::utils::is_not_found_error(&error) => {
            return Ok((config_content.to_string(), config_revision.to_string()))
        }
        Err(error) => {
            return Err(anyhow::anyhow!(
                "cannot read the managed TUN snapshot {}: {error}",
                snapshot_path.display()
            ))
        }
    };
    let value: serde_yaml::Value = serde_yaml::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("managed TUN snapshot YAML is invalid: {e}"))?;
    let Some(tun_block) = value.get("tun") else {
        return Ok((config_content.to_string(), config_revision.to_string()));
    };
    if tun_block.get("enable").and_then(|v| v.as_bool()) != Some(true) {
        return Ok((config_content.to_string(), config_revision.to_string()));
    }
    merge_snapshot_tun_block_into_config_yaml(config_content, tun_block)
}

pub fn parse_tun_enabled_from_bytes(bytes: &[u8]) -> anyhow::Result<bool> {
    let s = std::str::from_utf8(bytes)?;
    let val: serde_yaml::Value = serde_yaml::from_str(s)?;
    if let Some(tun) = val.get("tun") {
        if let Some(enable) = tun.get("enable").and_then(|e| e.as_bool()) {
            return Ok(enable);
        }
    }
    Ok(false)
}

fn parse_config_mapping(bytes: &[u8]) -> Option<serde_yaml::Mapping> {
    let text = std::str::from_utf8(bytes).ok()?;
    match serde_yaml::from_str::<serde_yaml::Value>(text) {
        Ok(serde_yaml::Value::Mapping(map)) if !map.is_empty() => Some(map),
        _ => None,
    }
}

/// 缺陷 1：回滚目标必须是**完整配置**（非空映射）。空文件或非映射
/// YAML 不是用户配置，写回受管快照等于把用户内容抹掉。
fn is_complete_config_mapping(bytes: &[u8]) -> bool {
    parse_config_mapping(bytes).is_some()
}

/// 该字节串是否携带 `tun` **映射块**。用于「提交不得删除用户 tun 内容」的不变量。
fn has_tun_mapping(bytes: &[u8]) -> bool {
    parse_config_mapping(bytes).is_some_and(|map| {
        map.get(serde_yaml::Value::String("tun".to_string()))
            .is_some_and(serde_yaml::Value::is_mapping)
    })
}

// ---------------- Generation Counter ----------------

pub fn read_generation(ctx: &InstanceContext) -> anyhow::Result<u64> {
    let path = generation_counter_path(ctx);
    let bytes = match utils::read_file_no_follow_limited(&path, 64) {
        Ok(bytes) => bytes,
        Err(e) => {
            if crate::utils::is_not_found_error(&e) {
                return Ok(0);
            }
            return Err(anyhow::anyhow!(
                "corrupt generation counter at {}: {}",
                path.display(),
                e
            ));
        }
    };
    let content = std::str::from_utf8(&bytes)
        .map_err(|e| anyhow::anyhow!("corrupt generation counter at {}: {}", path.display(), e))?;
    let gen = content
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow::anyhow!("corrupt generation counter at {}: {}", path.display(), e))?;
    Ok(gen)
}

pub fn next_generation(ctx: &InstanceContext) -> anyhow::Result<u64> {
    let current = read_generation(ctx)?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("generation counter overflow"))?;
    let path = generation_counter_path(ctx);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("generation counter has no parent"))?;
    ensure_system_dir(parent, 0o750)?;

    utils::atomic_write_bytes_no_follow(&path, format!("{next}\n").as_bytes(), 0o600)?;
    Ok(next)
}

// ---------------- Helper for atomic write & fsync ----------------

fn ensure_system_dir(dir: &Path, _mode: u32) -> anyhow::Result<()> {
    utils::ensure_dir_all_no_follow(dir)?;
    #[cfg(unix)]
    utils::set_directory_mode_no_follow(dir, _mode as u16)?;
    Ok(())
}

fn write_file_synced(path: &Path, bytes: &[u8], _mode: u32) -> anyhow::Result<()> {
    utils::atomic_write_bytes_no_follow(path, bytes, _mode as u16)
}

/// system TUN snapshots are written by whichever process owns the
/// transaction (sometimes the root CLI, sometimes the mihomo daemon). The daemon
/// must always be able to read the published inode, so a root writer hands the
/// new file to the fixed service identity before the rename. Non-root writers
/// (the daemon) keep the normal path; per-user mode never inherits the service
/// account.
fn write_snapshot_file(ctx: &InstanceContext, path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    #[cfg(not(target_os = "linux"))]
    let _ = ctx;
    #[cfg(target_os = "linux")]
    if ctx.mode == crate::instance::InstanceMode::System && unsafe { libc::geteuid() } == 0 {
        let (uid, gid) = utils::service_account_ids().ok_or_else(|| {
            anyhow::anyhow!("mihomo service account is missing; refusing to publish snapshot")
        })?;
        return utils::atomic_write_bytes_no_follow_owned(path, bytes, 0o640, uid, gid);
    }
    utils::atomic_write_bytes_no_follow(path, bytes, 0o640)
}

// ---------------- Read / Write Journal ----------------

pub fn read_active_journal(ctx: &InstanceContext) -> anyhow::Result<Option<TunJournal>> {
    let path = active_journal_path(ctx);
    let bytes = match utils::read_file_no_follow_limited(&path, MAX_TRANSACTION_JOURNAL_BYTES) {
        Ok(bytes) => bytes,
        Err(e) => {
            if crate::utils::is_not_found_error(&e) {
                return Ok(None);
            }
            return Err(e);
        }
    };
    let journal: TunJournal = serde_json::from_slice(&bytes)?;
    if journal.schema_version != 1 {
        anyhow::bail!(
            "unsupported active TUN journal schema {}; supported schema is 1",
            journal.schema_version
        );
    }
    Ok(Some(journal))
}

/// Validate the immutable old-runtime evidence before using it for rollback.
pub fn read_and_validate_old_runtime(
    ctx: &InstanceContext,
    journal: &TunJournal,
) -> anyhow::Result<OldRuntimeEvidence> {
    let path = active_old_runtime_path(ctx);
    let bytes = utils::read_file_no_follow_limited(&path, MAX_TRANSACTION_EVIDENCE_BYTES)?;
    if sha256_revision(&bytes) != journal.old_runtime_digest {
        anyhow::bail!("old-runtime.json digest does not match journal old_runtime_digest");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// classify the durable old-runtime evidence without propagating an
/// ENOENT (or a digest failure) to the caller; the planner turns the verdict into
/// a controlled-reset refusal instead of a bare I/O error.
pub fn classify_old_runtime_evidence(
    ctx: &InstanceContext,
    journal: &TunJournal,
) -> OldRuntimeEvidenceState {
    match read_and_validate_old_runtime(ctx, journal) {
        Ok(_) => OldRuntimeEvidenceState::Present,
        Err(error) if crate::utils::is_not_found_error(&error) => OldRuntimeEvidenceState::Missing,
        Err(_) => OldRuntimeEvidenceState::Invalid,
    }
}

/// the identity the legacy migrator writes into the synthetic
/// old-runtime artifact. It is a marker for "historical evidence unknown", not
/// an attested runtime identity.
pub const LEGACY_MIGRATION_PLACEHOLDER_IDENTITY: &str = "legacy";

/// the exact shape of the synthetic old-runtime artifact written by
/// the legacy journal migrator (``tun_transaction::check_and_migrate_legacy_journal``).
///
/// The placeholder only says "the old runtime state is unknown"; it never says
/// that the *current* Core is stopped. It is admissible as a reset precondition
/// only when every field still carries the migration value — any recorded pid,
/// revision, endpoint or timestamp means the artifact is something else and the
/// admission refuses.
pub fn is_legacy_migration_placeholder(evidence: &OldRuntimeEvidence) -> bool {
    !evidence.core_running
        && evidence.core_identity == LEGACY_MIGRATION_PLACEHOLDER_IDENTITY
        && evidence.core_pid == 0
        && evidence.launched_revision.is_empty()
        && evidence.launch_source == LaunchSource::SystemTunSnapshot
        && !evidence.runtime_tun
        && evidence.api_endpoint.is_empty()
        && evidence.recorded_at_secs.is_none()
}

/// the journal half of the legacy reset admission. Only a migrated
/// placeholder transaction qualifies — a transaction that ever recorded a
/// candidate identity/pid, completed its rollback evidence, or left
/// `RecoveryRequired` is a different shape and keeps the original (strict) path.
pub fn is_legacy_reset_journal_form(journal: &TunJournal) -> bool {
    journal.phase == JournalPhase::RecoveryRequired
        && journal.legacy_source
        && !journal.rollback_evidence_complete
        && journal.candidate_core_identity.is_none()
        && journal.candidate_core_pid.is_none()
}

/// admission, evidence half (裁决 d = 严格化 b): the migrated
/// placeholder may stand in for "is this the old runtime?" — never for "is the
/// current managed Core stopped?".
///
/// Returns a refusal reason instead of an error type so every caller (daemon
/// handler and privileged CLI) reports the same closed-form refusal. A missing,
/// unreadable or digest-mismatched artifact is a refusal, never "unknown = allow".
pub fn legacy_reset_evidence_admission(
    ctx: &InstanceContext,
    journal: &TunJournal,
) -> Result<OldRuntimeEvidence, String> {
    if !is_legacy_reset_journal_form(journal) {
        return Err(format!(
            "transaction {} is not a migrated legacy placeholder (phase {:?}, legacy_source={}, rollback_evidence_complete={}, candidate identity recorded={}, candidate pid recorded={})",
            journal.transaction_id,
            journal.phase,
            journal.legacy_source,
            journal.rollback_evidence_complete,
            journal.candidate_core_identity.is_some(),
            journal.candidate_core_pid.is_some(),
        ));
    }
    let evidence = read_and_validate_old_runtime(ctx, journal).map_err(|error| {
        format!("old-runtime evidence is unreadable or does not match the journal digest: {error}")
    })?;
    if !is_legacy_migration_placeholder(&evidence) {
        return Err(
            "old-runtime evidence is not the legacy migration placeholder (an identity, pid, revision or timestamp was recorded)"
                .to_string(),
        );
    }
    Ok(evidence)
}

pub fn runtime_matches_old_evidence(
    evidence: &OldRuntimeEvidence,
    observation: &RuntimeObservation,
) -> bool {
    evidence.core_running
        && observation.core_running
        && observation.api_ready
        && evidence.core_pid > 0
        && observation.core_pid == Some(evidence.core_pid)
        && !evidence.core_identity.is_empty()
        && observation.core_identity.as_deref() == Some(evidence.core_identity.as_str())
        && observation.launched_revision.as_deref() == Some(evidence.launched_revision.as_str())
        && observation.runtime_tun == Some(evidence.runtime_tun)
}

/// the historical PID is the process binding recorded when the
/// transaction was created, not part of the restored state. After a managed Core
/// restart the PID necessarily changes; "old runtime restored" is proven by the
/// managed identity + old revision + TUN + API on a live process. A separate
/// ownership proof must establish that the PID belongs to this managed instance.
/// This predicate never substitutes for the strict binding check above.
pub fn runtime_matches_restored_old_state(
    evidence: &OldRuntimeEvidence,
    observation: &RuntimeObservation,
) -> bool {
    evidence.core_running
        && observation.core_running
        && observation.api_ready
        && observation.core_pid.is_some_and(|pid| pid > 0)
        && !evidence.core_identity.is_empty()
        && observation.core_identity.as_deref() == Some(evidence.core_identity.as_str())
        && observation.launched_revision.as_deref() == Some(evidence.launched_revision.as_str())
        && observation.runtime_tun == Some(evidence.runtime_tun)
}

#[cfg_attr(not(unix), allow(dead_code))]
pub fn runtime_matches_candidate(journal: &TunJournal, observation: &RuntimeObservation) -> bool {
    observation.core_running
        && observation.core_pid.is_some_and(|pid| pid > 0)
        && observation
            .core_identity
            .as_deref()
            .is_some_and(|identity| !identity.is_empty())
        && journal
            .candidate_core_pid
            .is_some_and(|pid| observation.core_pid == Some(pid))
        && journal
            .candidate_core_identity
            .as_deref()
            .is_some_and(|identity| observation.core_identity.as_deref() == Some(identity))
        && observation.launched_revision.as_deref() == Some(journal.candidate_revision.as_str())
}

#[cfg_attr(not(unix), allow(dead_code))]
pub fn runtime_matches_recovery_target(
    journal: &TunJournal,
    observation: &RuntimeObservation,
    target_revision: &str,
    target_tun: bool,
) -> bool {
    observation.core_running
        && observation.api_ready
        && observation.core_pid.is_some_and(|pid| pid > 0)
        && observation
            .core_identity
            .as_deref()
            .is_some_and(|identity| !identity.is_empty())
        && observation.launched_revision.as_deref() == Some(target_revision)
        && observation.runtime_tun == Some(target_tun)
        && journal
            .recovery_core_pid
            .is_some_and(|pid| observation.core_pid == Some(pid))
        && journal
            .recovery_core_identity
            .as_deref()
            .is_some_and(|identity| observation.core_identity.as_deref() == Some(identity))
}

pub fn record_candidate_runtime(
    ctx: &InstanceContext,
    fence: &TransactionFence,
    proof: &RuntimeProof,
) -> anyhow::Result<TunJournal> {
    let _lock = CoordinatorLock::acquire(ctx)?;
    let mut journal = read_active_journal(ctx)?
        .ok_or_else(|| anyhow::anyhow!("active transaction journal is missing"))?;
    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != fence.expected_phase
        || journal.phase != JournalPhase::SnapshotPromoted
        || proof.transaction_id != journal.transaction_id
        || proof.generation != journal.generation
        || proof.observed_phase != JournalPhase::SnapshotPromoted
        || proof.proof_kind != RuntimeProofKind::CandidateApplied
        || proof.launched_revision != journal.candidate_revision
        || !proof.api_ready
        || proof.core_pid == 0
        || proof.core_identity.is_empty()
    {
        anyhow::bail!("invalid candidate runtime proof for journal fence");
    }
    journal.candidate_core_identity = Some(proof.core_identity.clone());
    journal.candidate_core_pid = Some(proof.core_pid);
    journal.updated_at = Some(now_rfc3339());
    write_active_journal(ctx, &journal)?;
    Ok(journal)
}

pub fn record_recovery_runtime(
    ctx: &InstanceContext,
    fence: &TransactionFence,
    proof: &RuntimeProof,
) -> anyhow::Result<TunJournal> {
    let _lock = CoordinatorLock::acquire(ctx)?;
    let mut journal = read_active_journal(ctx)?
        .ok_or_else(|| anyhow::anyhow!("active transaction journal is missing"))?;
    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != fence.expected_phase
        || journal.phase != JournalPhase::RecoveryRequired
        || proof.transaction_id != journal.transaction_id
        || proof.generation != journal.generation
        || proof.observed_phase != JournalPhase::RecoveryRequired
        || proof.proof_kind != RuntimeProofKind::LegacyRecoveryTargetApplied
        || proof.launched_revision
            != journal
                .recovery_target_revision
                .as_deref()
                .unwrap_or_default()
        || !proof.api_ready
        || proof.core_pid == 0
        || proof.core_identity.is_empty()
    {
        anyhow::bail!(
            "invalid legacy recovery runtime proof for journal fence: journal_phase={:?}, fence_phase={:?}, proof_phase={:?}, revision_matches={}, proof_tun={}, journal_target_tun={}, api_ready={}, core_pid_present={}, core_identity_present={}",
            journal.phase,
            fence.expected_phase,
            proof.observed_phase,
            proof.launched_revision
                == journal.recovery_target_revision.as_deref().unwrap_or_default(),
            proof.runtime_tun,
            journal.target_runtime_tun,
            proof.api_ready,
            proof.core_pid > 0,
            !proof.core_identity.is_empty(),
        );
    }
    journal.recovery_core_identity = Some(proof.core_identity.clone());
    journal.recovery_core_pid = Some(proof.core_pid);
    journal.updated_at = Some(now_rfc3339());
    write_active_journal(ctx, &journal)?;
    Ok(journal)
}

pub fn cas_update_recovery_target(
    ctx: &InstanceContext,
    fence: &TransactionFence,
    recovery_target_revision: String,
) -> anyhow::Result<TunJournal> {
    let _lock = CoordinatorLock::acquire(ctx)?;
    let mut journal = read_active_journal(ctx)?
        .ok_or_else(|| anyhow::anyhow!("active transaction journal is missing"))?;
    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != fence.expected_phase
        || journal.phase != JournalPhase::RecoveryRequired
        || !journal.legacy_source
        || journal.rollback_evidence_complete
    {
        anyhow::bail!("stale or ineligible recovery-target fence");
    }
    if journal.recovery_target_revision.as_deref() == Some(&recovery_target_revision) {
        return Ok(journal);
    }
    if journal.recovery_target_revision.is_some() {
        anyhow::bail!("recovery target revision is already fenced to another value");
    }
    journal.recovery_target_revision = Some(recovery_target_revision);
    journal.updated_at = Some(now_rfc3339());
    write_active_journal(ctx, &journal)?;
    Ok(journal)
}

pub fn write_active_journal(ctx: &InstanceContext, journal: &TunJournal) -> anyhow::Result<()> {
    let path = active_journal_path(ctx);
    let bytes = serde_json::to_vec_pretty(journal)?;
    write_file_synced(&path, &bytes, 0o640)?;
    #[cfg(target_os = "linux")]
    if ctx.mode == crate::instance::InstanceMode::System && unsafe { libc::geteuid() } == 0 {
        utils::ensure_mihomo_system_state_dir()?;
    }
    Ok(())
}

/// SPEC B1: staleness threshold for non-terminal journal phases.
pub const STALE_TRANSACTION_THRESHOLD_SECS: u64 = 15 * 60;

/// SPEC B1: compute journal age classification.
/// `Terminal` is phase-derived. `None` means a non-terminal journal whose
/// `updated_at` is missing or unparseable (age=unknown, diagnose only).
pub fn journal_age_classification(journal: &TunJournal) -> Option<JournalAge> {
    if matches!(
        journal.phase,
        JournalPhase::IntentCommitted | JournalPhase::RolledBack
    ) {
        return Some(JournalAge::Terminal);
    }
    let updated_at_str = journal.updated_at.as_deref()?;
    let updated_at = match chrono::DateTime::parse_from_rfc3339(updated_at_str) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(_) => return None,
    };
    let age_secs = (chrono::Utc::now() - updated_at).num_seconds().max(0) as u64;
    if age_secs >= STALE_TRANSACTION_THRESHOLD_SECS {
        Some(JournalAge::Stale { age_secs })
    } else {
        Some(JournalAge::Fresh { age_secs })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalAge {
    Fresh {
        age_secs: u64,
    },
    Stale {
        age_secs: u64,
    },
    /// Old journal without a usable `updated_at`: diagnose only, never write.
    Unknown,
    Terminal,
}

/// SPEC B1: return an updated_at timestamp string for the current time.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn validate_phase_transition(from: JournalPhase, to: JournalPhase) -> anyhow::Result<()> {
    let valid = match from {
        JournalPhase::Prepared => matches!(
            to,
            JournalPhase::PromotionPending
                | JournalPhase::RollbackPending
                | JournalPhase::RolledBack
                | JournalPhase::RecoveryRequired
        ),
        JournalPhase::PromotionPending => matches!(
            to,
            JournalPhase::SnapshotPromoted
                | JournalPhase::RollbackPending
                | JournalPhase::RecoveryRequired
        ),
        JournalPhase::SnapshotPromoted => matches!(
            to,
            JournalPhase::CoreApplied
                | JournalPhase::RollbackPending
                | JournalPhase::RecoveryRequired
        ),
        JournalPhase::CoreApplied => matches!(
            to,
            JournalPhase::IntentCommitted
                | JournalPhase::RollbackPending
                | JournalPhase::RecoveryRequired
        ),
        JournalPhase::RollbackPending => matches!(
            to,
            JournalPhase::RolledBack | JournalPhase::RecoveryRequired
        ),
        JournalPhase::IntentCommitted | JournalPhase::RolledBack => false,
        JournalPhase::RecoveryRequired => matches!(
            to,
            JournalPhase::PromotionPending
                | JournalPhase::SnapshotPromoted
                | JournalPhase::CoreApplied
                | JournalPhase::RollbackPending
                | JournalPhase::RolledBack
                | JournalPhase::IntentCommitted
        ),
    };
    if !valid {
        anyhow::bail!("invalid transaction phase transition from {from:?} to {to:?}");
    }
    Ok(())
}

pub fn cas_update_phase(
    ctx: &InstanceContext,
    fence: &TransactionFence,
    next_phase: JournalPhase,
    last_error: Option<StructuredError>,
    outcome: Option<TerminalOutcome>,
) -> anyhow::Result<TunJournal> {
    let _lock = CoordinatorLock::acquire(ctx)?;
    let mut journal = read_active_journal(ctx)?
        .ok_or_else(|| anyhow::anyhow!("active transaction journal is missing"))?;
    // 裁决点 3：fence 必须钉住 candidate revision。只比 phase 时，旧事务的
    // fence 可以推进**新**事务（同 transaction_id/generation 复用或残留响应），把别人的
    // 候选当自己的推进。缺失/不一致一律拒绝（fail-closed）。
    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != fence.expected_phase
        || journal.candidate_revision != fence.expected_candidate_revision
    {
        anyhow::bail!(
            "stale transaction CAS update: expected {:?}/{}/phase={:?}/candidate={}, found {:?}/{}/phase={:?}/candidate={}",
            fence.transaction_id,
            fence.generation,
            fence.expected_phase,
            fence.expected_candidate_revision,
            journal.transaction_id,
            journal.generation,
            journal.phase,
            journal.candidate_revision
        );
    }
    validate_phase_transition(journal.phase, next_phase)?;
    journal.phase = next_phase;
    if last_error.is_some() {
        journal.last_error = last_error;
    }
    if outcome.is_some() {
        journal.outcome = outcome;
    }
    // SPEC B1: update timestamp on phase change
    journal.updated_at = Some(now_rfc3339());
    write_active_journal(ctx, &journal)?;
    Ok(journal)
}

// ---------------- Prepare and Publish Active Transaction ----------------

/// 发布事务（调用方给定 base 与 candidate）。测试夹具专用：生产入口一律走
/// [`prepare_and_publish_active_transaction_capturing_intent`]（捕获与发布同一临界区）。
#[cfg(test)]
pub fn prepare_and_publish_active_transaction(
    ctx: &InstanceContext,
    original_uid: u32,
    target_runtime_tun: bool,
    base_intent_revision: String,
    candidate_bytes: &[u8],
    old_runtime_evidence: &OldRuntimeEvidence,
) -> anyhow::Result<TunJournal> {
    let _coordinator_lock = CoordinatorLock::acquire(ctx)?;
    let _config_lock = crate::lock::ConfigLock::acquire(&ctx.paths.config_dir)?;
    prepare_and_publish_active_locked(
        ctx,
        original_uid,
        target_runtime_tun,
        base_intent_revision,
        candidate_bytes,
        old_runtime_evidence,
    )
}

/// 在**同一个** CoordinatorLock → ConfigLock 临界区内捕获 intent 并发布事务。
///
/// P1-1：`base_intent_revision` 必须描述 candidate 的真实前置，且捕获与发布
/// 之间不能有并发写者可乘之隙。`build_candidate` 以同一份 intent 字节构建 candidate，
/// 因此 base 与 candidate 同源、同锁。
#[allow(dead_code)] // Convenience/test wrapper; production uses the `_with_prepare` entry.
pub fn prepare_and_publish_active_transaction_capturing_intent<F>(
    ctx: &InstanceContext,
    original_uid: u32,
    target_runtime_tun: bool,
    old_runtime_evidence: &OldRuntimeEvidence,
    build_candidate: F,
) -> anyhow::Result<TunJournal>
where
    F: FnOnce(&[u8]) -> anyhow::Result<Vec<u8>>,
{
    prepare_and_publish_active_transaction_capturing_intent_with_prepare(
        ctx,
        original_uid,
        target_runtime_tun,
        old_runtime_evidence,
        || Ok(()),
        build_candidate,
    )
}

/// 与 [`prepare_and_publish_active_transaction_capturing_intent`] 相同，
/// 但允许在**捕获 intent 之前**、且仍在 CoordinatorLock → ConfigLock 临界区内执行
/// 一次准备动作。`tun on` 用它把 override 的非控制键先收敛进 intent，保证
/// `base_intent_revision`、candidate 与后续提交看到的是同一份字节。
///
/// 准备动作必须在该函数内执行；若放到事务外，reconcile 写 intent 与捕获之间会留下
/// 并发窗口，`compare_and_commit_user_intent` 可能以 Conflict 拒绝。
pub fn prepare_and_publish_active_transaction_capturing_intent_with_prepare<F, P>(
    ctx: &InstanceContext,
    original_uid: u32,
    target_runtime_tun: bool,
    old_runtime_evidence: &OldRuntimeEvidence,
    prepare_intent: P,
    build_candidate: F,
) -> anyhow::Result<TunJournal>
where
    F: FnOnce(&[u8]) -> anyhow::Result<Vec<u8>>,
    P: FnOnce() -> anyhow::Result<()>,
{
    let _coordinator_lock = CoordinatorLock::acquire(ctx)?;
    let _config_lock = crate::lock::ConfigLock::acquire(&ctx.paths.config_dir)?;
    prepare_intent()?;
    let intent_bytes = utils::read_file_no_follow_limited(
        &ctx.paths.intent_config_file,
        MAX_TRANSACTION_ARTIFACT_BYTES,
    )?;
    let base_intent_revision = sha256_revision(&intent_bytes);
    let candidate_bytes = build_candidate(&intent_bytes)?;
    prepare_and_publish_active_locked(
        ctx,
        original_uid,
        target_runtime_tun,
        base_intent_revision,
        &candidate_bytes,
        old_runtime_evidence,
    )
}

/// 发布事务：调用方已持有 CoordinatorLock + ConfigLock 的临界区版本。
///
/// P1-1：把「捕获 intent（base revision + candidate）」与「发布事务」放进
/// **同一个**临界区，避免 main 侧「捕获后释放锁 → 再发布」之间被并发写者插入。
/// 锁序固定 CoordinatorLock → ConfigLock（与 CAS/journal 写方一致，不引入反转）。
fn prepare_and_publish_active_locked(
    ctx: &InstanceContext,
    original_uid: u32,
    target_runtime_tun: bool,
    base_intent_revision: String,
    candidate_bytes: &[u8],
    old_runtime_evidence: &OldRuntimeEvidence,
) -> anyhow::Result<TunJournal> {
    // Check if user gate already exists
    if let Some(gate) = read_user_gate(&ctx.paths.config_dir)? {
        anyhow::bail!(
            "a system transaction gate already exists for transaction {} (gen {})",
            gate.transaction_id,
            gate.generation
        );
    }

    let tx_dir = transactions_dir(ctx);
    ensure_system_dir(&tx_dir, 0o750)?;
    #[cfg(unix)]
    utils::set_directory_mode_no_follow(&tx_dir, 0o2750)?;

    let act_dir = active_dir(ctx);
    if act_dir.exists() {
        if let Some(existing) = read_active_journal(ctx)? {
            if !matches!(
                existing.phase,
                JournalPhase::IntentCommitted | JournalPhase::RolledBack
            ) {
                anyhow::bail!(
                    "an active system TUN transaction is already in progress: {} (phase: {:?})",
                    existing.transaction_id,
                    existing.phase
                );
            }
            // terminal active directory: move to gc
            let gc = gc_dir(ctx);
            ensure_system_dir(&gc, 0o750)?;
            let target = gc.join(format!(
                "{}-{}",
                existing.generation, existing.transaction_id
            ));
            utils::rename_no_follow(&act_dir, &target).map_err(|error| {
                anyhow::anyhow!(
                    "failed to archive terminal TUN transaction {}: {error}",
                    existing.transaction_id
                )
            })?;
            if let Ok(dir) = File::open(&tx_dir) {
                let _ = dir.sync_all();
            }
        } else {
            // Unparseable active dir: fail closed
            anyhow::bail!("active transaction directory exists but contains no valid journal");
        }
    }

    let generation = next_generation(ctx)?;
    let transaction_id = format!(
        "tun-{:016x}{:016x}",
        rand::random::<u64>(),
        rand::random::<u64>()
    );
    let candidate_revision = sha256_revision(candidate_bytes);

    let staging_name = format!(".prepare-{}", rand::random::<u32>());
    let staging_dir = tx_dir.join(&staging_name);
    ensure_system_dir(&staging_dir, 0o750)?;

    // 1. write candidate.yaml
    let candidate_file = staging_dir.join("candidate.yaml");
    write_file_synced(&candidate_file, candidate_bytes, 0o640)?;

    // 2. snapshot evidence
    let snapshot_path = &ctx.paths.tun_config_file;
    let old_snapshot_revision = if snapshot_path.exists() {
        let snapshot_bytes =
            utils::read_file_no_follow_limited(snapshot_path, MAX_TRANSACTION_ARTIFACT_BYTES)?;
        let rev = sha256_revision(&snapshot_bytes);
        let old_snap_file = staging_dir.join("old-snapshot.yaml");
        write_file_synced(&old_snap_file, &snapshot_bytes, 0o640)?;
        Some(rev)
    } else {
        let missing_file = staging_dir.join("old-snapshot.missing");
        write_file_synced(&missing_file, b"", 0o640)?;
        None
    };

    // 3. old-runtime.json
    let old_runtime_bytes = serde_json::to_vec_pretty(old_runtime_evidence)?;
    let old_runtime_digest = sha256_revision(&old_runtime_bytes);
    let old_runtime_file = staging_dir.join("old-runtime.json");
    write_file_synced(&old_runtime_file, &old_runtime_bytes, 0o640)?;

    // 4. journal.json
    let journal = TunJournal {
        schema_version: 1,
        transaction_id: transaction_id.clone(),
        generation,
        phase: JournalPhase::Prepared,
        original_uid,
        target_runtime_tun,
        base_intent_revision: Some(base_intent_revision.clone()),
        candidate_revision: candidate_revision.clone(),
        candidate_core_identity: None,
        candidate_core_pid: None,
        old_snapshot_revision,
        old_runtime_digest,
        recovery_target_revision: None,
        legacy_source: false,
        legacy_transaction_id: None,
        legacy_base_revision: None,
        rollback_evidence_complete: true,
        last_error: None,
        outcome: None,
        recovery_core_identity: None,
        recovery_core_pid: None,
        updated_at: Some(now_rfc3339()),
    };
    let journal_bytes = serde_json::to_vec_pretty(&journal)?;
    let journal_file = staging_dir.join("journal.json");
    write_file_synced(&journal_file, &journal_bytes, 0o640)?;

    // fsync staging dir
    if let Ok(dir) = File::open(&staging_dir) {
        let _ = dir.sync_all();
    }

    // Atomic rename staging dir to active/
    utils::rename_no_follow(&staging_dir, &act_dir)?;
    if let Ok(dir) = File::open(&tx_dir) {
        let _ = dir.sync_all();
    }

    // Write user admission gate
    let gate = SystemTransactionGate {
        schema_version: 1,
        transaction_id: transaction_id.clone(),
        generation,
        original_uid,
        base_intent_revision: Some(base_intent_revision),
        candidate_revision,
    };
    write_user_gate(&ctx.paths.config_dir, &gate, original_uid)?;

    Ok(journal)
}

// ---------------- Snapshot Promotion & Restore ----------------

pub fn promote_snapshot(ctx: &InstanceContext, journal: &TunJournal) -> anyhow::Result<()> {
    let candidate_path = active_candidate_path(ctx);
    let candidate_bytes =
        utils::read_file_no_follow_limited(&candidate_path, MAX_TRANSACTION_ARTIFACT_BYTES)?;
    if sha256_revision(&candidate_bytes) != journal.candidate_revision {
        anyhow::bail!("candidate file does not match journal candidate_revision");
    }

    let snapshot_path = &ctx.paths.tun_config_file;
    // verify current snapshot matches expected old snapshot
    if let Some(ref expected_old) = journal.old_snapshot_revision {
        if !snapshot_path.exists() {
            anyhow::bail!(
                "snapshot promotion conflict: expected old snapshot {} but file does not exist",
                expected_old
            );
        }
        let current_bytes =
            utils::read_file_no_follow_limited(snapshot_path, MAX_TRANSACTION_ARTIFACT_BYTES)?;
        if sha256_revision(&current_bytes) != *expected_old {
            anyhow::bail!("snapshot promotion conflict: current snapshot revision does not match expected old snapshot");
        }
    } else if snapshot_path.exists() {
        anyhow::bail!("snapshot promotion conflict: expected no old snapshot but file exists");
    }

    write_snapshot_file(ctx, snapshot_path, &candidate_bytes)?;
    Ok(())
}

pub fn restore_snapshot(ctx: &InstanceContext, journal: &TunJournal) -> anyhow::Result<()> {
    // SPEC B5: restore_snapshot must verify journal identity in its critical
    // section before writing, to prevent stale writes from concurrent recovery.
    let _lock = CoordinatorLock::acquire(ctx)?;
    let current_journal = read_active_journal(ctx)?
        .ok_or_else(|| anyhow::anyhow!("active transaction journal is missing"))?;

    // (from P2-1): the revision fields are part of the journal
    // identity. A caller holding a journal whose artifact revisions were
    // rewritten in place is just as stale as one whose phase moved on.
    if current_journal.transaction_id != journal.transaction_id
        || current_journal.generation != journal.generation
        || current_journal.phase != journal.phase
        || current_journal.old_snapshot_revision != journal.old_snapshot_revision
        || current_journal.candidate_revision != journal.candidate_revision
    {
        anyhow::bail!(
            "restore_snapshot: journal identity mismatch (expected {}/{}/{:?}/old={:?}/candidate={}, found {}/{}/{:?}/old={:?}/candidate={})",
            journal.transaction_id,
            journal.generation,
            journal.phase,
            journal.old_snapshot_revision,
            journal.candidate_revision,
            current_journal.transaction_id,
            current_journal.generation,
            current_journal.phase,
            current_journal.old_snapshot_revision,
            current_journal.candidate_revision
        );
    }

    let snapshot_path = &ctx.paths.tun_config_file;
    if let Some(ref expected_old) = journal.old_snapshot_revision {
        let old_snap_path = active_old_snapshot_path(ctx);
        // 缺陷 1：回滚只允许把**可证明的 base 配置**写回受管快照。
        // 原实现把 old-snapshot.yaml 原样写回，既没校验它是 journal 记录的那份
        // pre-image，也没校验它是配置：截断/空文件会被当成"base"覆盖快照，
        // 回滚于是变成了内容删除（现场即 snapshot/恢复目标缺 tun 块）。失败一律
        // 保留全部事务件并给 Fix，宁可拒绝不可删。
        let old_snap_bytes = utils::read_file_no_follow_limited(
            &old_snap_path,
            MAX_TRANSACTION_ARTIFACT_BYTES,
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "the rollback base pre-image cannot be read ({error}); the managed TUN snapshot was NOT overwritten.\n  Every transaction artifact is kept.\n  Fix: mihomo-cli restart --system"
            )
        })?;
        let old_snap_revision = sha256_revision(&old_snap_bytes);
        if old_snap_revision != *expected_old {
            anyhow::bail!(
                "the rollback base pre-image is not the journal's recorded base ({} != {}); refusing to overwrite the managed TUN snapshot.\n  Every transaction artifact is kept.\n  Fix: mihomo-cli restart --system",
                &old_snap_revision[..old_snap_revision.len().min(12)],
                &expected_old[..expected_old.len().min(12)],
            );
        }
        if !is_complete_config_mapping(&old_snap_bytes) {
            anyhow::bail!(
                "the rollback base pre-image is empty or not a YAML mapping; refusing to overwrite the managed TUN snapshot.\n  Every transaction artifact is kept.\n  Fix: mihomo-cli restart --system"
            );
        }
        write_snapshot_file(ctx, snapshot_path, &old_snap_bytes)?;
    } else {
        // Old snapshot did not exist, remove snapshot file if exists
        // 裁决点 3（缺陷 1 表）："缺失"也要有持久证据——prepare 事务时写下的
        // `old-snapshot.missing` 标记就是 manifest 的一部分。没有它就无法证明"base 本
        // 来没有快照"，删除受管快照就成了未经证明的删除。
        if !active_old_snapshot_missing_path(ctx).exists() {
            anyhow::bail!(
                "the rollback base pre-image evidence is incomplete: the journal records no old snapshot but the `old-snapshot.missing` marker is absent; refusing to delete the managed TUN snapshot.\n  Every transaction artifact is kept.\n  Fix: mihomo-cli restart --system"
            );
        }
        if snapshot_path.exists() {
            std::fs::remove_file(snapshot_path)?;
            if let Some(parent) = snapshot_path.parent() {
                if let Ok(dir) = File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
        }
    }
    Ok(())
}

// ---------------- User Intent Commit ----------------

pub fn compare_and_commit_user_intent(
    ctx: &InstanceContext,
    journal: &TunJournal,
) -> anyhow::Result<IntentCommitResult> {
    // P1-2：journal 的写方（CAS/prepare/terminal cleanup 的 journal 段）都在
    // CoordinatorLock 下，而 canonical journal 重验原先只持 ConfigLock —— 重验与 journal
    // 写入没有互斥，重验结论可能在读到写之间失真。锁序与
    // `prepare_and_publish_active_transaction` 一致：CoordinatorLock → ConfigLock。
    let _coordinator_lock = CoordinatorLock::acquire(ctx)?;
    let _lock = crate::lock::ConfigLock::acquire(&ctx.paths.config_dir)?;
    // 提交前重验 canonical journal（owner/generation/phase/candidate revision）。调用方
    // 持有的 journal 可能已被并发恢复/清理推进，此时按旧 journal 提交就是把**别人的**
    // 候选写进 intent。不一致 → 拒绝，零写入。
    let canonical = read_active_journal(ctx)?.ok_or_else(|| {
        anyhow::anyhow!(
            "no active transaction journal to commit; nothing was written.\n  Fix: mihomo-cli status"
        )
    })?;
    if canonical.transaction_id != journal.transaction_id
        || canonical.generation != journal.generation
        || canonical.phase != journal.phase
        || canonical.candidate_revision != journal.candidate_revision
    {
        anyhow::bail!(
            "the transaction journal changed under this commit (expected {:?}/{}/phase={:?}/candidate={}, found {:?}/{}/phase={:?}/candidate={}); nothing was written.\n  Fix: mihomo-cli status",
            journal.transaction_id,
            journal.generation,
            journal.phase,
            journal.candidate_revision,
            canonical.transaction_id,
            canonical.generation,
            canonical.phase,
            canonical.candidate_revision
        );
    }
    let intent_path = &ctx.paths.intent_config_file;
    let current_bytes = if intent_path.exists() {
        utils::read_file_no_follow_limited(intent_path, MAX_TRANSACTION_ARTIFACT_BYTES)?
    } else {
        Vec::new()
    };
    let current_rev = sha256_revision(&current_bytes);

    if current_rev == journal.candidate_revision {
        return Ok(IntentCommitResult::AlreadyCandidate);
    }

    // P2-2：`base_intent_revision` 缺失时**一律** Conflict。原先对
    // `legacy_source` 的免 base 放行是一个函数级契约漏洞：没有 base 摘要就无法证明
    // 当前 intent 是本事务的前置，提交即等于未经证明的覆盖（planner 目前不可达该口，
    // 但函数级契约不能依赖「当前不可达」）。
    let Some(ref base_rev) = journal.base_intent_revision else {
        return Ok(IntentCommitResult::Conflict);
    };
    if current_rev != *base_rev {
        return Ok(IntentCommitResult::Conflict);
    }

    let candidate_path = active_candidate_path(ctx);
    let candidate_bytes =
        utils::read_file_no_follow_limited(&candidate_path, MAX_TRANSACTION_ARTIFACT_BYTES)?;
    if sha256_revision(&candidate_bytes) != journal.candidate_revision {
        anyhow::bail!("candidate revision mismatch during intent commit");
    }
    // 缺陷 1：提交不得删除用户内容。residual 事务的 candidate 可能来自
    // 更早的意图快照；若它丢了当前 intent 已有的 `tun` 块，提交等于把用户的 TUN
    // 控制键整体抹掉（现场「回滚后 tun 块消失」）。宁可拒绝，保留全部事务件。
    if has_tun_mapping(&current_bytes) && !has_tun_mapping(&candidate_bytes) {
        anyhow::bail!(
            "refusing to commit: the transaction candidate would delete the `tun` block that the current intent carries.\n  Your configuration was NOT modified. Every transaction artifact is kept.\n  Fix: mihomo-cli restart --system"
        );
    }

    write_file_synced(intent_path, &candidate_bytes, 0o644)?;
    utils::restore_original_user_config_ownership(intent_path)?;

    Ok(IntentCommitResult::Committed)
}

// ---------------- User Admission Gate ----------------

pub fn write_user_gate(
    config_dir: &Path,
    gate: &SystemTransactionGate,
    _original_uid: u32,
) -> anyhow::Result<()> {
    let gate_path = user_gate_path(config_dir);
    let bytes = serde_json::to_vec_pretty(gate)?;
    write_file_synced(&gate_path, &bytes, 0o600)?;
    utils::restore_original_user_config_ownership(&gate_path)?;
    Ok(())
}

pub fn read_user_gate(config_dir: &Path) -> anyhow::Result<Option<SystemTransactionGate>> {
    let gate_path = user_gate_path(config_dir);
    let bytes = match utils::read_file_no_follow_limited(&gate_path, MAX_TRANSACTION_JOURNAL_BYTES)
    {
        Ok(bytes) => bytes,
        Err(e) => {
            if crate::utils::is_not_found_error(&e) {
                return Ok(None);
            }
            return Err(e);
        }
    };
    let gate: SystemTransactionGate = serde_json::from_slice(&bytes)?;
    Ok(Some(gate))
}

pub fn remove_user_gate(
    config_dir: &Path,
    transaction_id: &str,
    generation: u64,
) -> anyhow::Result<()> {
    let gate_path = user_gate_path(config_dir);
    if let Some(gate) = read_user_gate(config_dir)? {
        if gate.transaction_id == transaction_id && gate.generation == generation {
            let _ = std::fs::remove_file(&gate_path);
            if let Ok(dir) = File::open(config_dir) {
                let _ = dir.sync_all();
            }
        }
    }
    Ok(())
}

// SPEC B4: gate enforcement is centralized in daemon.rs
// `process_command()` with phase-specific fail-closed semantics.

/// 裁决点 1：**任何配置写入之前**，非终态事务 owner 存在就必须拒绝。
///
/// 事务窗口内的 intent 写入会让事务的 `base_intent_revision` 立刻失真：后续提交只能
/// 走 Conflict，最坏情况下残留 recovery 会用旧 candidate 覆盖用户刚写下的内容。因此
/// `override import/clear`（以及其它会改写 intent 的用户命令）在写入前调用本函数。
fn error_is_permission_denied(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
    })
}

fn permission_denied_recovery_hint_linux(state_dir: &Path) -> String {
    format!(
        "  The system state directory `{}` (usually 0770 root:mihomo) is not readable by the current identity.\n  Fix: add the current user to the `mihomo` group and sign in again (e.g. sudo usermod -aG mihomo \"$USER\"), or run this operation with the required system authorization.\n  Do not use `mihomo-cli status` as the fix; it cannot read the same directory.",
        state_dir.display()
    )
}

fn permission_denied_recovery_hint_macos(state_dir: &Path) -> String {
    format!(
        "  The protected macOS state directory `{}` (root:wheel, guarded by the system LaunchDaemon account) is not readable by the current identity.\n  Fix: retry from a managed system-authorized context (for example the system instance with sudo/administrator authorization), or repair the LaunchDaemon/state directory ownership through the supported install or system configuration.\n  Do not use `mihomo-cli status` as the fix; it cannot read the same directory.",
        state_dir.display()
    )
}

fn permission_denied_recovery_hint_windows(state_dir: &Path) -> String {
    format!(
        "  The protected Windows state directory `{}` (under %ProgramData%\\mihomo-cli, governed by the service account/DACL) is not readable by the current identity.\n  Fix: retry from an elevated/administrator or managed service context, or repair the directory DACL through the supported installer (for example: icacls \"%ProgramData%\\mihomo-cli\" /grant ...).\n  Do not use `mihomo-cli status` as the fix; it cannot read the same directory.",
        state_dir.display()
    )
}

fn permission_denied_recovery_hint_generic(state_dir: &Path) -> String {
    format!(
        "  The protected system state directory `{}` is not readable by the current identity.\n  Fix: retry from a managed system-authorized context, or repair the state directory ownership through the supported installer.\n  Do not use `mihomo-cli status` as the fix; it cannot read the same directory.",
        state_dir.display()
    )
}

fn permission_denied_recovery_hint(state_dir: &Path) -> String {
    if cfg!(target_os = "linux") {
        permission_denied_recovery_hint_linux(state_dir)
    } else if cfg!(target_os = "macos") {
        permission_denied_recovery_hint_macos(state_dir)
    } else if cfg!(target_os = "windows") {
        permission_denied_recovery_hint_windows(state_dir)
    } else {
        permission_denied_recovery_hint_generic(state_dir)
    }
}

pub fn ensure_no_non_terminal_transaction(
    ctx: &InstanceContext,
    action: &str,
) -> anyhow::Result<()> {
    if ctx.mode != crate::instance::InstanceMode::System {
        return Ok(());
    }
    let active = match read_active_journal(ctx) {
        Ok(active) => active,
        Err(error) if error_is_permission_denied(&error) => {
            let transactions = transactions_dir(ctx);
            let state_dir = transactions.parent().unwrap_or(transactions.as_path());
            anyhow::bail!(
                "cannot read the system TUN transaction journal, so `{action}` was refused before writing anything: {error}\n  Your configuration was NOT modified.\n{}",
                permission_denied_recovery_hint(state_dir)
            )
        }
        Err(error) => anyhow::bail!(
            "cannot read the transaction journal, so `{action}` was refused before writing anything: {error}\n  Every transaction artifact is kept.\n  Fix: mihomo-cli status"
        ),
    };
    if let Some(journal) = active {
        if !matches!(
            journal.phase,
            JournalPhase::IntentCommitted | JournalPhase::RolledBack
        ) {
            anyhow::bail!(
                "a system TUN transaction is in progress ({} / gen {} / phase {:?}); `{action}` was refused before writing anything.\n  Your configuration was NOT modified.\n  Finish the transaction first: mihomo-cli status, then mihomo-cli restart --system if it is stuck",
                journal.transaction_id,
                journal.generation,
                journal.phase
            );
        }
    }
    if legacy_journal_path(ctx).exists() {
        anyhow::bail!(
            "a legacy TUN transaction journal is pending; `{action}` was refused before writing anything.\n  Your configuration was NOT modified.\n  Recover it first: mihomo-cli status, then mihomo-cli restart --system"
        );
    }
    Ok(())
}

fn managed_transaction_id_is_safe(transaction_id: &str) -> bool {
    !transaction_id.is_empty()
        && transaction_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub fn validate_managed_active_transaction(
    ctx: &InstanceContext,
    journal: &TunJournal,
) -> anyhow::Result<bool> {
    let active = active_dir(ctx);
    if !active.is_dir() || !managed_transaction_id_is_safe(&journal.transaction_id) {
        return Ok(false);
    }
    for path in [
        active_journal_path(ctx),
        active_candidate_path(ctx),
        active_old_runtime_path(ctx),
    ] {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Ok(false),
        };
        if !metadata.file_type().is_file() || {
            #[cfg(unix)]
            {
                metadata.nlink() != 1
            }
            #[cfg(not(unix))]
            {
                false
            }
        } {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn quarantine_active_transaction(
    ctx: &InstanceContext,
    journal: &TunJournal,
) -> anyhow::Result<()> {
    let _lock = CoordinatorLock::acquire(ctx)?;
    let active = active_dir(ctx);
    if !active.is_dir() {
        anyhow::bail!("managed TUN transaction directory is unavailable");
    }
    let gc = gc_dir(ctx);
    ensure_system_dir(&gc, 0o750)?;
    let target = gc.join(format!(
        "reset-{}-{}",
        journal.generation, journal.transaction_id
    ));
    utils::rename_no_follow(&active, &target)?;
    if let Ok(dir) = File::open(transactions_dir(ctx)) {
        let _ = dir.sync_all();
    }
    Ok(())
}

pub fn remove_legacy_artifacts(ctx: &InstanceContext) -> anyhow::Result<()> {
    for path in [legacy_journal_path(ctx), legacy_candidate_path(ctx)] {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                std::fs::remove_file(&path)?;
            }
            Ok(_) => anyhow::bail!("legacy TUN artifact is not a regular file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn remove_managed_snapshot(ctx: &InstanceContext) -> anyhow::Result<()> {
    let path = &ctx.paths.tun_config_file;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() || {
        #[cfg(unix)]
        {
            metadata.nlink() != 1
        }
        #[cfg(not(unix))]
        {
            false
        }
    } {
        anyhow::bail!("managed TUN snapshot is not a safe regular file");
    }
    std::fs::remove_file(path)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

// ---------------- Terminal Cleanup ----------------

pub fn terminal_cleanup(
    ctx: &InstanceContext,
    transaction_id: &str,
    generation: u64,
) -> anyhow::Result<()> {
    // 1. remove user gate in user config lock
    {
        let _lock = crate::lock::ConfigLock::acquire(&ctx.paths.config_dir);
        let _ = remove_user_gate(&ctx.paths.config_dir, transaction_id, generation);
    }

    // 2. move active dir to gc in coordinator lock
    let _lock = CoordinatorLock::acquire(ctx)?;
    let act_dir = active_dir(ctx);
    if act_dir.exists() {
        if let Some(journal) = read_active_journal(ctx)? {
            if journal.transaction_id == transaction_id
                && journal.generation == generation
                && matches!(
                    journal.phase,
                    JournalPhase::IntentCommitted | JournalPhase::RolledBack
                )
            {
                let gc = gc_dir(ctx);
                ensure_system_dir(&gc, 0o750)?;
                let target = gc.join(format!("{}-{}", generation, transaction_id));
                utils::rename_no_follow(&act_dir, &target).map_err(|error| {
                    anyhow::anyhow!(
                        "failed to archive terminal TUN transaction {transaction_id}: {error}"
                    )
                })?;
                let tx_dir = transactions_dir(ctx);
                if let Ok(dir) = File::open(&tx_dir) {
                    let _ = dir.sync_all();
                }
                // asynchronously / best-effort remove gc target
                let _ = utils::remove_path_no_follow(&target);
                if journal.legacy_source {
                    // Legacy evidence is retained until the migrated transaction
                    // reaches a durable terminal phase.
                    let _ = utils::remove_file_if_exists(&legacy_journal_path(ctx));
                    let _ = utils::remove_file_if_exists(&legacy_candidate_path(ctx));
                }
            }
        }
    }
    Ok(())
}

// ---------------- Classification & Pure Recovery Planner ----------------

pub fn classify_snapshot(ctx: &InstanceContext, journal: &TunJournal) -> SnapshotClassification {
    let snapshot_path = &ctx.paths.tun_config_file;
    if !snapshot_path.exists() {
        return if journal.old_snapshot_revision.is_none() {
            SnapshotClassification::Old
        } else {
            SnapshotClassification::Missing
        };
    }
    let bytes =
        match utils::read_file_no_follow_limited(snapshot_path, MAX_TRANSACTION_ARTIFACT_BYTES) {
            Ok(b) => b,
            Err(_) => return SnapshotClassification::Unreadable,
        };
    let rev = sha256_revision(&bytes);
    if rev == journal.candidate_revision {
        SnapshotClassification::Candidate
    } else if let Some(ref old_rev) = journal.old_snapshot_revision {
        if rev == *old_rev {
            SnapshotClassification::Old
        } else {
            SnapshotClassification::Other
        }
    } else {
        SnapshotClassification::Other
    }
}

pub fn classify_intent(intent_path: &Path, journal: &TunJournal) -> IntentClassification {
    if !intent_path.exists() {
        return IntentClassification::Unreadable;
    }
    let bytes =
        match utils::read_file_no_follow_limited(intent_path, MAX_TRANSACTION_ARTIFACT_BYTES) {
            Ok(b) => b,
            Err(_) => return IntentClassification::Unreadable,
        };
    let rev = sha256_revision(&bytes);
    if rev == journal.candidate_revision {
        IntentClassification::Candidate
    } else if let Some(ref base_rev) = journal.base_intent_revision {
        if rev == *base_rev {
            IntentClassification::Base
        } else {
            IntentClassification::Other
        }
    } else if journal.legacy_source {
        if let Some(ref leg_base) = journal.legacy_base_revision {
            if legacy_fnv_revision(&bytes) == *leg_base {
                IntentClassification::Base
            } else {
                IntentClassification::Other
            }
        } else {
            IntentClassification::Other
        }
    } else {
        IntentClassification::Other
    }
}

pub fn plan_recovery(
    journal: &TunJournal,
    snapshot_cls: SnapshotClassification,
    intent_cls: IntentClassification,
    old_evidence: OldRuntimeEvidenceState,
    observation: &RuntimeObservation,
    direction: RecoveryDirection,
) -> RecoveryAction {
    if matches!(
        journal.phase,
        JournalPhase::IntentCommitted | JournalPhase::RolledBack
    ) {
        return RecoveryAction::CleanupTerminal;
    }

    match journal.phase {
        JournalPhase::Prepared => match snapshot_cls {
            SnapshotClassification::Old | SnapshotClassification::Missing => match direction {
                RecoveryDirection::Resume => RecoveryAction::RepairPhaseToSnapshotPromoted,
                RecoveryDirection::Abort => RecoveryAction::BeginRollback,
            },
            SnapshotClassification::Candidate => RecoveryAction::RepairPhaseToSnapshotPromoted,
            SnapshotClassification::Other | SnapshotClassification::Unreadable => {
                RecoveryAction::RefuseNeedsEvidence(
                    "Snapshot is an unrecoverable or unexpected revision".to_string(),
                )
            }
        },
        JournalPhase::PromotionPending => match snapshot_cls {
            SnapshotClassification::Old | SnapshotClassification::Missing => match direction {
                RecoveryDirection::Resume => RecoveryAction::RepairPhaseToSnapshotPromoted,
                RecoveryDirection::Abort => RecoveryAction::BeginRollback,
            },
            SnapshotClassification::Candidate => RecoveryAction::RepairPhaseToSnapshotPromoted,
            SnapshotClassification::Other | SnapshotClassification::Unreadable => {
                RecoveryAction::RefuseNeedsEvidence(
                    "Snapshot is an unrecoverable or unexpected revision".to_string(),
                )
            }
        },
        JournalPhase::SnapshotPromoted => match snapshot_cls {
            SnapshotClassification::Candidate => {
                let runtime_matches = observation.core_running
                    && observation.launched_revision.as_deref()
                        == Some(&journal.candidate_revision)
                    && observation.api_ready
                    && observation.runtime_tun == Some(journal.target_runtime_tun);
                if runtime_matches {
                    match intent_cls {
                        IntentClassification::Candidate => RecoveryAction::FinalizeCommittedIntent,
                        _ => RecoveryAction::CommitIntent,
                    }
                } else {
                    match direction {
                        RecoveryDirection::Resume => RecoveryAction::RetryApply,
                        RecoveryDirection::Abort => RecoveryAction::BeginRollback,
                    }
                }
            }
            SnapshotClassification::Old | SnapshotClassification::Missing => match direction {
                RecoveryDirection::Resume => RecoveryAction::RetryApply,
                RecoveryDirection::Abort => RecoveryAction::BeginRollback,
            },
            SnapshotClassification::Other | SnapshotClassification::Unreadable => {
                RecoveryAction::RefuseNeedsEvidence(
                    "Snapshot is an unrecoverable or unexpected revision".to_string(),
                )
            }
        },
        JournalPhase::CoreApplied => match intent_cls {
            IntentClassification::Candidate => match direction {
                RecoveryDirection::Resume => RecoveryAction::FinalizeCommittedIntent,
                RecoveryDirection::Abort => RecoveryAction::RefuseNeedsEvidence(
                    "commit-wins: user intent already committed as candidate, cannot abort."
                        .to_string(),
                ),
            },
            IntentClassification::Base => match direction {
                RecoveryDirection::Resume => RecoveryAction::CommitIntent,
                RecoveryDirection::Abort => RecoveryAction::BeginRollback,
            },
            IntentClassification::Other | IntentClassification::Unreadable => {
                RecoveryAction::BeginRollback
            }
        },
        JournalPhase::RollbackPending => match snapshot_cls {
            SnapshotClassification::Other | SnapshotClassification::Unreadable => {
                RecoveryAction::RefuseNeedsEvidence(
                    "Snapshot is an unrecoverable or unexpected revision".to_string(),
                )
            }
            // without the durable old-runtime evidence the rollback
            // cannot be attested, so a bare ENOENT from the copy path would leave
            // every change command blocked. Refuse into the controlled reset.
            _ if old_evidence != OldRuntimeEvidenceState::Present => {
                RecoveryAction::RefuseNeedsEvidence(format!(
                    "RollbackPending: old-runtime evidence is {}; the rollback cannot prove the old runtime. Run: mihomo-cli restart --system",
                    old_evidence.describe()
                ))
            }
            // The planner does not have authenticated old-runtime evidence
            // contents, only the verdict that they exist and validate; the
            // daemon attests the old runtime after digest validation.
            SnapshotClassification::Candidate
            | SnapshotClassification::Old
            | SnapshotClassification::Missing => RecoveryAction::ContinueRollback,
        },
        JournalPhase::RecoveryRequired => {
            // SPEC B2: RecoveryRequired no longer outputs RetryApply.
            // The only successor is explicit `mihomo-cli restart --system` controlled reset.
            // Check matrix in SPEC Section 12.10
            if journal.legacy_source && !journal.rollback_evidence_complete {
                match direction {
                    RecoveryDirection::Resume => match intent_cls {
                        IntentClassification::Base => RecoveryAction::RefuseNeedsEvidence(
                            "RecoveryRequired: explicit restart required. Run: mihomo-cli restart --system".to_string(),
                        ),
                        IntentClassification::Candidate => RecoveryAction::FinalizeCommittedIntent,
                        _ => RecoveryAction::RefuseNeedsEvidence(
                            "legacy intent does not match base or candidate".to_string(),
                        ),
                    },
                    RecoveryDirection::Abort => {
                        if snapshot_cls == SnapshotClassification::Candidate {
                            RecoveryAction::ConvergeLegacyToCurrentIntent
                        } else {
                            RecoveryAction::RefuseNeedsEvidence(
                                "legacy transaction has no durable old-snapshot evidence; refusing to delete it"
                                    .to_string(),
                            )
                        }
                    }
                }
            } else {
                match (snapshot_cls, intent_cls) {
                    (SnapshotClassification::Candidate, IntentClassification::Candidate) => {
                        match direction {
                            RecoveryDirection::Resume => {
                                if observation.core_running
                                    && observation.launched_revision.as_deref() == Some(&journal.candidate_revision)
                                    && observation.api_ready
                                    && observation.runtime_tun == Some(journal.target_runtime_tun)
                                {
                                    RecoveryAction::FinalizeCommittedIntent
                                } else {
                                    RecoveryAction::RefuseNeedsEvidence(
                                        "RecoveryRequired: explicit restart required. Run: mihomo-cli restart --system".to_string(),
                                    )
                                }
                            }
                            RecoveryDirection::Abort => RecoveryAction::RefuseNeedsEvidence(
                                "commit-wins: user intent is already candidate, cannot abort."
                                    .to_string(),
                            ),
                        }
                    }
                    (SnapshotClassification::Candidate, IntentClassification::Base) => {
                        match direction {
                            RecoveryDirection::Resume => {
                                if observation.core_running
                                    && observation.launched_revision.as_deref() == Some(&journal.candidate_revision)
                                    && observation.api_ready
                                    && observation.runtime_tun == Some(journal.target_runtime_tun)
                                {
                                    RecoveryAction::CommitIntent
                                } else {
                                    RecoveryAction::RefuseNeedsEvidence(
                                        "RecoveryRequired: explicit restart required. Run: mihomo-cli restart --system".to_string(),
                                    )
                                }
                            }
                            RecoveryDirection::Abort => RecoveryAction::BeginRollback,
                        }
                    }
                    (SnapshotClassification::Old | SnapshotClassification::Missing, IntentClassification::Base) => {
                        match direction {
                            RecoveryDirection::Resume => RecoveryAction::RefuseNeedsEvidence(
                                "RecoveryRequired: explicit restart required. Run: mihomo-cli restart --system".to_string(),
                            ),
                            RecoveryDirection::Abort => RecoveryAction::ContinueRollback,
                        }
                    }
                    (SnapshotClassification::Old | SnapshotClassification::Missing, IntentClassification::Candidate) => {
                        match direction {
                            RecoveryDirection::Resume => RecoveryAction::RefuseNeedsEvidence(
                                "RecoveryRequired: explicit restart required. Run: mihomo-cli restart --system".to_string(),
                            ),
                            RecoveryDirection::Abort => RecoveryAction::RefuseNeedsEvidence(
                                "commit-wins: user intent is already candidate, cannot abort."
                                    .to_string(),
                            ),
                        }
                    }
                    (SnapshotClassification::Old | SnapshotClassification::Missing | SnapshotClassification::Candidate, IntentClassification::Other) => {
                        RecoveryAction::BeginRollback
                    }
                    _ => RecoveryAction::RefuseNeedsEvidence(
                        "state cannot be uniquely classified; inspect diagnostics with `mihomo-cli status`."
                            .to_string(),
                    ),
                }
            }
        }
        JournalPhase::IntentCommitted | JournalPhase::RolledBack => RecoveryAction::CleanupTerminal,
    }
}

// ---------------- Legacy Migration ----------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LegacyTunJournal {
    pub transaction_id: String,
    pub base_revision: String,
    pub candidate_revision: String,
    pub state: String,
    pub candidate_path: PathBuf,
    pub snapshot_path: PathBuf,
}

pub fn check_and_migrate_legacy_journal(
    ctx: &InstanceContext,
) -> anyhow::Result<Option<TunJournal>> {
    let leg_j_path = legacy_journal_path(ctx);
    let legacy_present = match std::fs::symlink_metadata(&leg_j_path) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    };
    if !legacy_present || active_dir(ctx).exists() {
        return Ok(None);
    }

    let _coordinator_lock = CoordinatorLock::acquire(ctx)?;
    if active_dir(ctx).exists() {
        return Ok(None);
    }
    let leg_c_path = legacy_candidate_path(ctx);

    let leg_bytes =
        match utils::read_file_no_follow_limited(&leg_j_path, MAX_TRANSACTION_JOURNAL_BYTES) {
            Ok(b) => b,
            Err(e) => {
                if crate::utils::is_not_found_error(&e) {
                    return Ok(None);
                }
                return Err(anyhow::anyhow!(
                    "unreadable legacy TUN journal; recovery requires manual evidence: {}",
                    e
                ));
            }
        };

    if active_dir(ctx).exists() {
        // v10 active already exists; legacy files remain as leftover site
        return Ok(None);
    }
    let legacy_journal: LegacyTunJournal = serde_json::from_slice(&leg_bytes).map_err(|e| {
        anyhow::anyhow!(
            "unreadable legacy TUN journal; recovery requires manual evidence: {}",
            e
        )
    })?;

    let candidate_bytes =
        match utils::read_file_no_follow_limited(&leg_c_path, MAX_TRANSACTION_ARTIFACT_BYTES) {
            Ok(b) => b,
            Err(err) => {
                if crate::utils::is_not_found_error(&err) {
                    return Ok(None);
                }
                return Err(anyhow::anyhow!(
                    "unreadable legacy candidate artifact; recovery requires manual evidence: {}",
                    err
                ));
            }
        };
    if legacy_fnv_revision(&candidate_bytes) != legacy_journal.candidate_revision {
        anyhow::bail!("legacy candidate FNV revision mismatch");
    }

    let v10_candidate_revision = sha256_revision(&candidate_bytes);
    let current_snapshot_bytes = utils::read_file_no_follow_limited_optional(
        &ctx.paths.tun_config_file,
        MAX_TRANSACTION_ARTIFACT_BYTES,
    )?;
    let intent_bytes = utils::read_file_no_follow_limited_optional(
        &ctx.paths.intent_config_file,
        MAX_TRANSACTION_ARTIFACT_BYTES,
    )?;
    let base_intent_revision = intent_bytes.as_ref().and_then(|b| {
        if legacy_fnv_revision(b) == legacy_journal.base_revision {
            Some(sha256_revision(b))
        } else {
            None
        }
    });

    let original_uid = crate::instance::PathInputs::from_current_env()
        .uid
        .unwrap_or(0);

    // Migrate to staging active/
    let generation = next_generation(ctx)?;
    let transaction_id = format!("tun-migrated-{:016x}", rand::random::<u64>());
    let tx_dir = transactions_dir(ctx);
    ensure_system_dir(&tx_dir, 0o750)?;
    #[cfg(unix)]
    utils::set_directory_mode_no_follow(&tx_dir, 0o2750)?;
    let staging_dir = tx_dir.join(format!(".prepare-migrated-{}", rand::random::<u32>()));
    ensure_system_dir(&staging_dir, 0o750)?;

    write_file_synced(&staging_dir.join("candidate.yaml"), &candidate_bytes, 0o640)?;

    let old_snapshot_revision = current_snapshot_bytes.as_ref().map(|b| sha256_revision(b));
    if let Some(ref sb) = current_snapshot_bytes {
        write_file_synced(&staging_dir.join("old-snapshot.yaml"), sb, 0o640)?;
    } else {
        write_file_synced(&staging_dir.join("old-snapshot.missing"), b"", 0o640)?;
    }

    let fake_evidence = OldRuntimeEvidence {
        core_running: false,
        core_identity: "legacy".to_string(),
        core_pid: 0,
        launched_revision: String::new(),
        launch_source: LaunchSource::SystemTunSnapshot,
        runtime_tun: false,
        api_endpoint: String::new(),
        recorded_at_secs: None,
    };
    let evidence_bytes = serde_json::to_vec_pretty(&fake_evidence)?;
    let old_runtime_digest = sha256_revision(&evidence_bytes);
    write_file_synced(
        &staging_dir.join("old-runtime.json"),
        &evidence_bytes,
        0o640,
    )?;

    let target_tun = candidate_bytes.windows(11).any(|w| w == b"enable: true")
        || candidate_bytes.windows(12).any(|w| w == b"enable:  true");

    let migrated_journal = TunJournal {
        schema_version: 1,
        transaction_id: transaction_id.clone(),
        generation,
        phase: JournalPhase::RecoveryRequired,
        original_uid,
        target_runtime_tun: target_tun,
        base_intent_revision,
        candidate_revision: v10_candidate_revision.clone(),
        candidate_core_identity: None,
        candidate_core_pid: None,
        old_snapshot_revision,
        old_runtime_digest,
        recovery_target_revision: None,
        legacy_source: true,
        legacy_transaction_id: Some(legacy_journal.transaction_id),
        legacy_base_revision: Some(legacy_journal.base_revision),
        rollback_evidence_complete: false,
        last_error: None,
        outcome: None,
        recovery_core_identity: None,
        recovery_core_pid: None,
        updated_at: Some(now_rfc3339()),
    };

    let j_bytes = serde_json::to_vec_pretty(&migrated_journal)?;
    write_file_synced(&staging_dir.join("journal.json"), &j_bytes, 0o640)?;

    let act_dir = active_dir(ctx);
    utils::rename_no_follow(&staging_dir, &act_dir)?;
    if let Ok(dir) = File::open(&tx_dir) {
        let _ = dir.sync_all();
    }

    Ok(Some(migrated_journal))
}

// ---------------- Legacy Compatibility Shims (if any) ----------------

#[allow(dead_code)]
pub fn journal_path(ctx: &InstanceContext) -> PathBuf {
    active_journal_path(ctx)
}

#[allow(dead_code)]
pub fn candidate_path(ctx: &InstanceContext) -> PathBuf {
    active_candidate_path(ctx)
}

#[allow(dead_code)]
pub fn read_journal(ctx: &InstanceContext) -> anyhow::Result<Option<TunJournal>> {
    read_active_journal(ctx)
}

// ---------------- Recovery Executor Exclusivity (SPEC B5) ----------------

fn recovery_marker_path(ctx: &InstanceContext) -> PathBuf {
    transactions_dir(ctx).join("recovery-in-progress")
}

/// Recovery marker content (pid + timestamp).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryMarker {
    pub pid: u32,
    pub created_at: String, // RFC3339
}

/// RAII holder of the recovery marker. The marker is released on drop, so every
/// early return (including failures) clears it without extra bookkeeping.
pub struct RecoveryMarkerGuard<'a> {
    ctx: &'a InstanceContext,
    released: bool,
}

impl<'a> RecoveryMarkerGuard<'a> {
    fn release_inner(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let _ = release_recovery_marker(self.ctx);
    }
}

impl Drop for RecoveryMarkerGuard<'_> {
    fn drop(&mut self) {
        self.release_inner();
    }
}

/// Try to acquire the recovery marker without blocking.
/// Returns `Ok(None)` when another live process holds it — recovery must not queue.
pub fn try_acquire_recovery_marker(
    ctx: &InstanceContext,
) -> anyhow::Result<Option<RecoveryMarkerGuard<'_>>> {
    let marker_path = recovery_marker_path(ctx);
    let tx_dir = transactions_dir(ctx);

    if !tx_dir.exists() {
        utils::ensure_dir_all_no_follow(&tx_dir)?;
    }

    if let Ok(content) = utils::read_file_no_follow_limited(&marker_path, 1024) {
        if let Ok(existing) = serde_json::from_slice::<RecoveryMarker>(&content) {
            if is_process_alive(existing.pid) {
                // Live holder: never reclaim, regardless of marker age.
                // RECOVERY_MARKER_STALE_SECS only applies to dead holders.
                return Ok(None);
            }
            // Holder is dead: reclaim the marker.
            let _ = std::fs::remove_file(&marker_path);
        }
    }

    let marker = RecoveryMarker {
        pid: std::process::id(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let content = serde_json::to_vec_pretty(&marker)?;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    match options.open(&marker_path) {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(&content)?;
            file.sync_all()?;
            Ok(Some(RecoveryMarkerGuard {
                ctx,
                released: false,
            }))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Release the recovery marker, but only when this process owns it.
pub fn release_recovery_marker(ctx: &InstanceContext) -> anyhow::Result<()> {
    let marker_path = recovery_marker_path(ctx);
    if let Ok(content) = utils::read_file_no_follow_limited(&marker_path, 1024) {
        if let Ok(marker) = serde_json::from_slice::<RecoveryMarker>(&content) {
            if marker.pid == std::process::id() {
                std::fs::remove_file(&marker_path)?;
            }
        }
    }
    Ok(())
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == 0 {
            return true;
        }
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        errno != libc::ESRCH
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> (tempfile::TempDir, InstanceContext) {
        let tmp = tempfile::tempdir().unwrap();
        let inputs = crate::instance::PathInputs {
            home: tmp.path().join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(tmp.path().join("run/user/1000")),
            program_data: tmp.path().join("ProgramData"),
            app_data: tmp.path().join("AppData/Roaming"),
            local_app_data: tmp.path().join("AppData/Local"),
            username_or_sid: "alice".to_string(),
        };
        let mut ctx = InstanceContext::planned(
            crate::instance::TargetOs::Linux,
            crate::instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.config_dir = tmp.path().join("user-config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = tmp.path().join("system-data/tun-config.yaml");
        ctx.permissions = crate::instance::PermissionModel::DirectUser;
        (tmp, ctx)
    }

    fn managed_test_journal() -> TunJournal {
        TunJournal {
            schema_version: 1,
            transaction_id: "managed-reset-test".to_string(),
            generation: 7,
            phase: JournalPhase::RecoveryRequired,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: true,
            legacy_transaction_id: Some("legacy-test".to_string()),
            legacy_base_revision: None,
            rollback_evidence_complete: false,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        }
    }

    #[test]
    fn managed_active_transaction_accepts_regular_artifacts() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        let journal = managed_test_journal();
        for path in [
            active_journal_path(&ctx),
            active_candidate_path(&ctx),
            active_old_runtime_path(&ctx),
        ] {
            std::fs::write(path, b"managed").unwrap();
        }
        assert!(validate_managed_active_transaction(&ctx, &journal).unwrap());
    }

    #[test]
    fn managed_active_transaction_rejects_invalid_identity_or_artifacts() {
        let (_tmp, ctx) = test_context();
        let journal = managed_test_journal();
        assert!(!validate_managed_active_transaction(&ctx, &journal).unwrap());

        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::write(active_journal_path(&ctx), b"managed").unwrap();
        let empty_id = TunJournal {
            transaction_id: String::new(),
            ..journal.clone()
        };
        assert!(!validate_managed_active_transaction(&ctx, &empty_id).unwrap());
        let unsafe_id = TunJournal {
            transaction_id: "../escape".to_string(),
            ..journal.clone()
        };
        assert!(!validate_managed_active_transaction(&ctx, &unsafe_id).unwrap());

        #[cfg(unix)]
        {
            let target = ctx.paths.config_dir.join("target");
            std::fs::write(&target, b"target").unwrap();
            std::os::unix::fs::symlink(&target, active_candidate_path(&ctx)).unwrap();
            assert!(!validate_managed_active_transaction(&ctx, &journal).unwrap());
            std::fs::remove_file(active_candidate_path(&ctx)).unwrap();
            std::fs::write(active_candidate_path(&ctx), b"candidate").unwrap();
            let hard_link = ctx.paths.config_dir.join("hard-link");
            std::fs::hard_link(active_candidate_path(&ctx), &hard_link).unwrap();
            assert!(!validate_managed_active_transaction(&ctx, &journal).unwrap());
        }
    }

    #[test]
    fn managed_active_transaction_can_be_quarantined() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        std::fs::write(active_journal_path(&ctx), b"managed").unwrap();
        let journal = managed_test_journal();
        quarantine_active_transaction(&ctx, &journal).unwrap();
        assert!(!active_dir(&ctx).exists());
        let entries: Vec<_> = std::fs::read_dir(gc_dir(&ctx))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(entries[0]
            .to_string_lossy()
            .starts_with("reset-7-managed-reset-test"));
    }

    #[test]
    fn managed_snapshot_and_legacy_artifacts_can_be_removed() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        std::fs::write(&ctx.paths.tun_config_file, b"snapshot").unwrap();
        std::fs::create_dir_all(transactions_dir(&ctx)).unwrap();
        std::fs::write(legacy_journal_path(&ctx), b"journal").unwrap();
        std::fs::write(legacy_candidate_path(&ctx), b"candidate").unwrap();
        remove_managed_snapshot(&ctx).unwrap();
        remove_legacy_artifacts(&ctx).unwrap();
        assert!(!ctx.paths.tun_config_file.exists());
        assert!(!legacy_journal_path(&ctx).exists());
        assert!(!legacy_candidate_path(&ctx).exists());
    }

    #[cfg(unix)]
    #[test]
    fn managed_snapshot_rejects_symlink_and_hard_link() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        let target = ctx.paths.config_dir.join("snapshot-target");
        std::fs::write(&target, b"snapshot").unwrap();
        std::os::unix::fs::symlink(&target, &ctx.paths.tun_config_file).unwrap();
        assert!(remove_managed_snapshot(&ctx).is_err());
        std::fs::remove_file(&ctx.paths.tun_config_file).unwrap();
        std::fs::hard_link(&target, &ctx.paths.tun_config_file).unwrap();
        assert!(remove_managed_snapshot(&ctx).is_err());
    }

    #[test]
    fn legacy_artifacts_reject_non_regular_files() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(transactions_dir(&ctx)).unwrap();
        std::fs::create_dir(legacy_journal_path(&ctx)).unwrap();
        assert!(remove_legacy_artifacts(&ctx).is_err());
    }

    #[test]
    fn revision_computation_is_sha256() {
        let bytes = b"hello world";
        let rev = sha256_revision(bytes);
        assert_eq!(
            rev,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(content_revision(bytes), rev);
    }

    #[test]
    fn legal_phase_transitions() {
        assert!(
            validate_phase_transition(JournalPhase::Prepared, JournalPhase::PromotionPending)
                .is_ok()
        );
        assert!(validate_phase_transition(
            JournalPhase::PromotionPending,
            JournalPhase::SnapshotPromoted
        )
        .is_ok());
        assert!(validate_phase_transition(
            JournalPhase::SnapshotPromoted,
            JournalPhase::CoreApplied
        )
        .is_ok());
        assert!(validate_phase_transition(
            JournalPhase::CoreApplied,
            JournalPhase::IntentCommitted
        )
        .is_ok());
        assert!(validate_phase_transition(
            JournalPhase::CoreApplied,
            JournalPhase::RollbackPending
        )
        .is_ok());
        assert!(
            validate_phase_transition(JournalPhase::RollbackPending, JournalPhase::RolledBack)
                .is_ok()
        );
        assert!(
            validate_phase_transition(JournalPhase::Prepared, JournalPhase::IntentCommitted)
                .is_err()
        );
    }

    #[test]
    fn prepare_publish_and_cas_update_flow() {
        let (_tmp, ctx) = test_context();
        let evidence = OldRuntimeEvidence {
            core_running: true,
            core_identity: "core-1".to_string(),
            core_pid: 1234,
            launched_revision: "old-rev".to_string(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: "http://127.0.0.1:9090".to_string(),
            recorded_at_secs: Some(100),
        };
        let candidate = b"tun:\n  enable: true\n";
        let base_rev = "base-rev".to_string();

        let journal = prepare_and_publish_active_transaction(
            &ctx,
            1000,
            true,
            base_rev.clone(),
            candidate,
            &evidence,
        )
        .unwrap();

        assert_eq!(journal.phase, JournalPhase::Prepared);
        assert_eq!(journal.generation, 1);
        assert!(active_journal_path(&ctx).exists());
        assert!(active_candidate_path(&ctx).exists());
        assert!(user_gate_path(&ctx.paths.config_dir).exists());

        // CAS update to PromotionPending
        let fence = TransactionFence {
            transaction_id: journal.transaction_id.clone(),
            generation: journal.generation,
            expected_phase: JournalPhase::Prepared,
            expected_candidate_revision: journal.candidate_revision.clone(),
        };
        let updated =
            cas_update_phase(&ctx, &fence, JournalPhase::PromotionPending, None, None).unwrap();
        assert_eq!(updated.phase, JournalPhase::PromotionPending);

        // Stale CAS update rejected
        let bad_fence = TransactionFence {
            transaction_id: journal.transaction_id.clone(),
            generation: journal.generation,
            expected_phase: JournalPhase::Prepared,
            expected_candidate_revision: journal.candidate_revision.clone(),
        };
        assert!(
            cas_update_phase(&ctx, &bad_fence, JournalPhase::SnapshotPromoted, None, None).is_err()
        );
    }

    #[test]
    fn recovery_target_requires_fenced_legacy_recovery_phase() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "legacy-tx".to_string(),
            generation: 3,
            phase: JournalPhase::RecoveryRequired,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: true,
            legacy_transaction_id: Some("old-tx".to_string()),
            legacy_base_revision: Some("old-base".to_string()),
            rollback_evidence_complete: false,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        write_active_journal(&ctx, &journal).unwrap();
        let fence = TransactionFence {
            transaction_id: journal.transaction_id.clone(),
            generation: journal.generation,
            expected_phase: journal.phase,
            expected_candidate_revision: journal.candidate_revision.clone(),
        };
        let updated = cas_update_recovery_target(&ctx, &fence, "target".to_string()).unwrap();
        assert_eq!(updated.recovery_target_revision.as_deref(), Some("target"));
        assert!(cas_update_recovery_target(&ctx, &fence, "other".to_string()).is_err());
    }

    #[test]
    fn recovery_planner_commit_wins_on_candidate_intent() {
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "tx-1".to_string(),
            generation: 1,
            phase: JournalPhase::CoreApplied,
            original_uid: 1000,
            target_runtime_tun: true,
            base_intent_revision: Some("base".to_string()),
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: Some("old".to_string()),
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };

        let obs = RuntimeObservation {
            core_running: true,
            core_identity: Some("core-1".to_string()),
            core_pid: Some(100),
            launched_revision: Some("candidate".to_string()),
            runtime_tun: Some(true),
            api_ready: true,
        };

        let plan_resume = plan_recovery(
            &journal,
            SnapshotClassification::Candidate,
            IntentClassification::Candidate,
            OldRuntimeEvidenceState::Present,
            &obs,
            RecoveryDirection::Resume,
        );
        assert_eq!(plan_resume, RecoveryAction::FinalizeCommittedIntent);

        let plan_abort = plan_recovery(
            &journal,
            SnapshotClassification::Candidate,
            IntentClassification::Candidate,
            OldRuntimeEvidenceState::Present,
            &obs,
            RecoveryDirection::Abort,
        );
        assert!(matches!(plan_abort, RecoveryAction::RefuseNeedsEvidence(_)));
    }

    #[test]
    fn recovery_abort_preserves_abort_direction_when_repairing_phase() {
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "tx-repair".to_string(),
            generation: 1,
            phase: JournalPhase::Prepared,
            original_uid: 1000,
            target_runtime_tun: true,
            base_intent_revision: Some("base".to_string()),
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: Some("old".to_string()),
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        let obs = RuntimeObservation {
            core_running: false,
            core_identity: None,
            core_pid: None,
            launched_revision: None,
            runtime_tun: None,
            api_ready: false,
        };

        assert_eq!(
            plan_recovery(
                &journal,
                SnapshotClassification::Old,
                IntentClassification::Base,
                OldRuntimeEvidenceState::Present,
                &obs,
                RecoveryDirection::Abort,
            ),
            RecoveryAction::BeginRollback
        );
        assert_eq!(
            plan_recovery(
                &journal,
                SnapshotClassification::Candidate,
                IntentClassification::Base,
                OldRuntimeEvidenceState::Present,
                &obs,
                RecoveryDirection::Abort,
            ),
            RecoveryAction::RepairPhaseToSnapshotPromoted
        );
        // After phase repair the planner must still see Abort and begin rollback,
        // rather than silently taking the Resume/commit path.
        let repaired = TunJournal {
            phase: JournalPhase::SnapshotPromoted,
            ..journal
        };
        assert_eq!(
            plan_recovery(
                &repaired,
                SnapshotClassification::Candidate,
                IntentClassification::Base,
                OldRuntimeEvidenceState::Present,
                &obs,
                RecoveryDirection::Abort,
            ),
            RecoveryAction::BeginRollback
        );
    }

    #[test]
    fn legacy_prepared_without_candidate_snapshot_requires_evidence() {
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "legacy".to_string(),
            generation: 1,
            phase: JournalPhase::RecoveryRequired,
            original_uid: 1000,
            target_runtime_tun: true,
            base_intent_revision: Some("base".to_string()),
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: true,
            legacy_transaction_id: Some("old".to_string()),
            legacy_base_revision: Some("old-base".to_string()),
            rollback_evidence_complete: false,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        let obs = RuntimeObservation {
            core_running: false,
            core_identity: None,
            core_pid: None,
            launched_revision: None,
            runtime_tun: None,
            api_ready: false,
        };
        assert!(matches!(
            plan_recovery(
                &journal,
                SnapshotClassification::Old,
                IntentClassification::Base,
                OldRuntimeEvidenceState::Present,
                &obs,
                RecoveryDirection::Abort,
            ),
            RecoveryAction::RefuseNeedsEvidence(_)
        ));
    }

    /// T1: the full legacy chain — migration stamps a
    /// RecoveryRequired journal with incomplete rollback evidence, and the
    /// Resume direction refuses under Base intent without any runtime action.
    /// The refusal is the contract; only an explicit restart may take over.
    #[test]
    fn legacy_migration_recovery_required_refuses_resume_under_base_intent() {
        let (_tmp, ctx) = test_context();
        let intent_bytes = b"mixed-port: 7890\n";
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::write(&ctx.paths.intent_config_file, intent_bytes).unwrap();
        let candidate_bytes = b"tun:\n  enable: true\n";
        std::fs::create_dir_all(transactions_dir(&ctx)).unwrap();
        std::fs::write(legacy_candidate_path(&ctx), candidate_bytes).unwrap();
        let legacy_journal = serde_json::json!({
            "transaction_id": "legacy-tx",
            "base_revision": legacy_fnv_revision(intent_bytes),
            "candidate_revision": legacy_fnv_revision(candidate_bytes),
            "state": "Prepared",
            "candidate_path": legacy_candidate_path(&ctx),
            "snapshot_path": ctx.paths.tun_config_file,
        });
        std::fs::write(
            legacy_journal_path(&ctx),
            serde_json::to_vec(&legacy_journal).unwrap(),
        )
        .unwrap();

        let migrated = check_and_migrate_legacy_journal(&ctx)
            .unwrap()
            .expect("legacy journal must migrate into the active transaction");
        assert_eq!(migrated.phase, JournalPhase::RecoveryRequired);
        assert!(migrated.legacy_source);
        assert!(!migrated.rollback_evidence_complete);
        assert_eq!(migrated.legacy_transaction_id.as_deref(), Some("legacy-tx"));
        assert!(migrated.candidate_core_identity.is_none());
        assert!(migrated.candidate_core_pid.is_none());
        // The legacy evidence is retained, never rewritten by the migration.
        assert!(legacy_journal_path(&ctx).exists());
        assert!(legacy_candidate_path(&ctx).exists());
        // Migration is idempotent: the active directory blocks a second run.
        assert!(check_and_migrate_legacy_journal(&ctx).unwrap().is_none());

        let journal = read_active_journal(&ctx)
            .unwrap()
            .expect("migrated journal must be readable");
        assert_eq!(journal.phase, JournalPhase::RecoveryRequired);
        assert!(journal.legacy_source);
        assert!(!journal.rollback_evidence_complete);

        let snapshot_cls = classify_snapshot(&ctx, &journal);
        assert_eq!(snapshot_cls, SnapshotClassification::Old);
        let intent_cls = classify_intent(&ctx.paths.intent_config_file, &journal);
        assert_eq!(intent_cls, IntentClassification::Base);
        let obs = RuntimeObservation {
            core_running: true,
            core_identity: Some("/usr/local/lib/mihomo/mihomo".to_string()),
            core_pid: Some(4242),
            launched_revision: Some("runtime-rev".to_string()),
            runtime_tun: Some(false),
            api_ready: true,
        };
        assert!(matches!(
            plan_recovery(
                &journal,
                snapshot_cls.clone(),
                intent_cls.clone(),
                // A migrated legacy transaction has no durable old-runtime
                // evidence file; use the real classifier instead of assuming.
                classify_old_runtime_evidence(&ctx, &journal),
                &obs,
                RecoveryDirection::Resume,
            ),
            RecoveryAction::RefuseNeedsEvidence(_)
        ));
        // Abort has no durable old-snapshot evidence for a legacy transaction.
        assert!(matches!(
            plan_recovery(
                &journal,
                snapshot_cls,
                intent_cls,
                classify_old_runtime_evidence(&ctx, &journal),
                &obs,
                RecoveryDirection::Abort,
            ),
            RecoveryAction::RefuseNeedsEvidence(_)
        ));
    }

    #[test]
    fn old_runtime_digest_mismatch_is_rejected() {
        let (_tmp, ctx) = test_context();
        let evidence = OldRuntimeEvidence {
            core_running: true,
            core_identity: "core".to_string(),
            core_pid: 1,
            launched_revision: "old".to_string(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: "socket".to_string(),
            recorded_at_secs: None,
        };
        let bytes = serde_json::to_vec_pretty(&evidence).unwrap();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        std::fs::write(active_old_runtime_path(&ctx), &bytes).unwrap();
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "tx".to_string(),
            generation: 1,
            phase: JournalPhase::RollbackPending,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: sha256_revision(b"different"),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        assert!(read_and_validate_old_runtime(&ctx, &journal).is_err());
    }

    /// `RollbackPending` with missing or unusable old-runtime evidence
    /// must not reach the rollback copy path (bare ENOENT); it refuses into the
    /// controlled reset channel with a one-line Fix.
    #[test]
    fn rollback_pending_requires_old_runtime_evidence() {
        let (_tmp, ctx) = test_context();
        let evidence = OldRuntimeEvidence {
            core_running: true,
            core_identity: "core".to_string(),
            core_pid: 1,
            launched_revision: "old".to_string(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: "socket".to_string(),
            recorded_at_secs: None,
        };
        let evidence_bytes = serde_json::to_vec_pretty(&evidence).unwrap();
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "tx-evidence".to_string(),
            generation: 1,
            phase: JournalPhase::RollbackPending,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: sha256_revision(&evidence_bytes),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        let obs = RuntimeObservation {
            core_running: false,
            core_identity: None,
            core_pid: None,
            launched_revision: None,
            runtime_tun: None,
            api_ready: false,
        };
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();

        // 1. Evidence missing: refuse with a Fix, never ContinueRollback.
        assert_eq!(
            classify_old_runtime_evidence(&ctx, &journal),
            OldRuntimeEvidenceState::Missing
        );
        for snapshot_cls in [
            SnapshotClassification::Old,
            SnapshotClassification::Missing,
            SnapshotClassification::Candidate,
        ] {
            let action = plan_recovery(
                &journal,
                snapshot_cls.clone(),
                IntentClassification::Base,
                OldRuntimeEvidenceState::Missing,
                &obs,
                RecoveryDirection::Abort,
            );
            match action {
                RecoveryAction::RefuseNeedsEvidence(message) => {
                    assert!(
                        message.contains("mihomo-cli restart --system"),
                        "snapshot {snapshot_cls:?}: the refusal must carry the Fix: {message}"
                    );
                    assert!(
                        message.contains("missing"),
                        "the refusal must say the evidence is missing: {message}"
                    );
                }
                other => panic!("snapshot {snapshot_cls:?}: expected refusal, got {other:?}"),
            }
        }

        // 2. Evidence present and valid: the idempotent rollback protocol runs.
        std::fs::write(active_old_runtime_path(&ctx), &evidence_bytes).unwrap();
        assert_eq!(
            classify_old_runtime_evidence(&ctx, &journal),
            OldRuntimeEvidenceState::Present
        );
        assert_eq!(
            plan_recovery(
                &journal,
                SnapshotClassification::Old,
                IntentClassification::Base,
                OldRuntimeEvidenceState::Present,
                &obs,
                RecoveryDirection::Abort,
            ),
            RecoveryAction::ContinueRollback
        );

        // 3. Evidence present but digest-drifted: still a refusal (Invalid).
        std::fs::write(active_old_runtime_path(&ctx), b"{\"core_running\":false}").unwrap();
        assert_eq!(
            classify_old_runtime_evidence(&ctx, &journal),
            OldRuntimeEvidenceState::Invalid
        );
        let drifted = plan_recovery(
            &journal,
            SnapshotClassification::Old,
            IntentClassification::Base,
            classify_old_runtime_evidence(&ctx, &journal),
            &obs,
            RecoveryDirection::Abort,
        );
        match drifted {
            RecoveryAction::RefuseNeedsEvidence(message) => {
                assert!(
                    message.contains("mihomo-cli restart --system")
                        && message.contains("unreadable or does not match"),
                    "{message}"
                );
            }
            other => panic!("digest drift must refuse, got {other:?}"),
        }

        // 4. An unreadable snapshot keeps its own verdict even without evidence.
        assert_eq!(
            plan_recovery(
                &journal,
                SnapshotClassification::Unreadable,
                IntentClassification::Base,
                OldRuntimeEvidenceState::Missing,
                &obs,
                RecoveryDirection::Abort,
            ),
            RecoveryAction::RefuseNeedsEvidence(
                "Snapshot is an unrecoverable or unexpected revision".to_string()
            )
        );
    }

    #[test]
    fn old_runtime_match_requires_identity_pid_revision_tun_and_api() {
        let evidence = OldRuntimeEvidence {
            core_running: true,
            core_identity: "core-1".to_string(),
            core_pid: 123,
            launched_revision: "old".to_string(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: "socket".to_string(),
            recorded_at_secs: None,
        };
        let matching = RuntimeObservation {
            core_running: true,
            core_identity: Some("core-1".to_string()),
            core_pid: Some(123),
            launched_revision: Some("old".to_string()),
            runtime_tun: Some(false),
            api_ready: true,
        };
        assert!(runtime_matches_old_evidence(&evidence, &matching));

        for altered in [
            RuntimeObservation {
                core_identity: Some("other".to_string()),
                ..matching.clone()
            },
            RuntimeObservation {
                core_pid: Some(124),
                ..matching.clone()
            },
            RuntimeObservation {
                launched_revision: Some("candidate".to_string()),
                ..matching.clone()
            },
            RuntimeObservation {
                runtime_tun: Some(true),
                ..matching.clone()
            },
            RuntimeObservation {
                api_ready: false,
                ..matching.clone()
            },
        ] {
            assert!(!runtime_matches_old_evidence(&evidence, &altered));
        }
    }

    #[test]
    fn restored_old_state_accepts_a_new_pid_but_keeps_state_checks() {
        let evidence = OldRuntimeEvidence {
            core_running: true,
            core_identity: "core-1".to_string(),
            core_pid: 123,
            launched_revision: "old".to_string(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: true,
            api_endpoint: "socket".to_string(),
            recorded_at_secs: None,
        };
        let restarted = RuntimeObservation {
            core_running: true,
            core_identity: Some("core-1".to_string()),
            core_pid: Some(456),
            launched_revision: Some("old".to_string()),
            runtime_tun: Some(true),
            api_ready: true,
        };
        assert!(!runtime_matches_old_evidence(&evidence, &restarted));
        assert!(runtime_matches_restored_old_state(&evidence, &restarted));

        for altered in [
            RuntimeObservation {
                core_identity: Some("other".to_string()),
                ..restarted.clone()
            },
            RuntimeObservation {
                launched_revision: Some("candidate".to_string()),
                ..restarted.clone()
            },
            RuntimeObservation {
                runtime_tun: Some(false),
                ..restarted.clone()
            },
            RuntimeObservation {
                api_ready: false,
                ..restarted.clone()
            },
            RuntimeObservation {
                core_pid: None,
                ..restarted.clone()
            },
        ] {
            assert!(!runtime_matches_restored_old_state(&evidence, &altered));
        }
    }

    #[test]
    fn same_revision_unknown_core_is_not_a_candidate() {
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "tx".to_string(),
            generation: 1,
            phase: JournalPhase::RollbackPending,
            original_uid: 1000,
            target_runtime_tun: true,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: Some("/managed/mihomo".to_string()),
            candidate_core_pid: Some(42),
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        let unknown = RuntimeObservation {
            core_running: true,
            core_identity: Some("/other/mihomo".to_string()),
            core_pid: Some(99),
            launched_revision: Some("candidate".to_string()),
            runtime_tun: Some(true),
            api_ready: true,
        };
        assert!(!runtime_matches_candidate(&journal, &unknown));
    }

    #[test]
    fn restore_start_identity_and_pid_mismatch_is_rejected() {
        let evidence = OldRuntimeEvidence {
            core_running: true,
            core_identity: "/managed/mihomo".to_string(),
            core_pid: 42,
            launched_revision: "old".to_string(),
            launch_source: LaunchSource::SystemActiveConfig,
            runtime_tun: false,
            api_endpoint: "unix:///tmp/core.sock".to_string(),
            recorded_at_secs: None,
        };
        let observation = RuntimeObservation {
            core_running: true,
            core_identity: Some("/other/mihomo".to_string()),
            core_pid: Some(99),
            launched_revision: Some("old".to_string()),
            runtime_tun: Some(false),
            api_ready: true,
        };
        assert!(!runtime_matches_old_evidence(&evidence, &observation));
    }

    #[test]
    fn legacy_target_requires_recorded_identity_pid_and_exact_tun() {
        let mut journal = TunJournal {
            schema_version: 1,
            transaction_id: "legacy".to_string(),
            generation: 1,
            phase: JournalPhase::RecoveryRequired,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: Some("target".to_string()),
            legacy_source: true,
            legacy_transaction_id: Some("old".to_string()),
            legacy_base_revision: Some("base".to_string()),
            rollback_evidence_complete: false,
            last_error: None,
            outcome: None,
            recovery_core_identity: Some("/managed/mihomo".to_string()),
            recovery_core_pid: Some(42),
            updated_at: None,
        };
        let observation = RuntimeObservation {
            core_running: true,
            core_identity: Some("/managed/mihomo".to_string()),
            core_pid: Some(42),
            launched_revision: Some("target".to_string()),
            runtime_tun: Some(true),
            api_ready: true,
        };
        assert!(!runtime_matches_recovery_target(
            &journal,
            &observation,
            "target",
            false
        ));
        journal.target_runtime_tun = true;
        assert!(runtime_matches_recovery_target(
            &journal,
            &observation,
            "target",
            true
        ));
    }

    #[test]
    fn bounded_no_follow_reader_rejects_oversized_artifact() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("artifact");
        std::fs::write(&path, b"0123456789").unwrap();
        assert!(utils::read_file_no_follow_limited(&path, 9).is_err());
        assert_eq!(
            utils::read_file_no_follow_limited(&path, 10).unwrap(),
            b"0123456789"
        );
    }

    #[test]
    fn unknown_journal_schema_is_reported_as_unsupported() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        let mut journal = serde_json::to_value(TunJournal {
            schema_version: 1,
            transaction_id: "tx".to_string(),
            generation: 1,
            phase: JournalPhase::Prepared,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        })
        .unwrap();
        journal["schema_version"] = serde_json::json!(99);
        std::fs::write(
            active_journal_path(&ctx),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        let error = read_active_journal(&ctx).unwrap_err().to_string();
        assert!(error.contains("unsupported active TUN journal schema"));
        assert!(error.contains("99"));
    }

    #[test]
    fn unreadable_legacy_journal_returns_explicit_error() {
        let (_tmp, ctx) = test_context();
        let leg_j = legacy_journal_path(&ctx);
        std::fs::create_dir_all(leg_j.parent().unwrap()).unwrap();
        std::fs::write(&leg_j, b"{corrupted json").unwrap();
        let res = check_and_migrate_legacy_journal(&ctx);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("unreadable legacy TUN journal; recovery requires manual evidence"));
    }

    fn merge_test_tun(payload: &str, snapshot: &str) -> serde_yaml::Value {
        let snapshot: serde_yaml::Value = serde_yaml::from_str(snapshot).unwrap();
        let snapshot_tun = snapshot
            .get("tun")
            .cloned()
            .expect("test snapshot must contain tun");
        let (merged, revision) =
            merge_snapshot_tun_block_into_config_yaml(payload, &snapshot_tun).unwrap();
        assert_eq!(revision, sha256_revision(merged.as_bytes()));
        let doc: serde_yaml::Value = serde_yaml::from_str(&merged).unwrap();
        doc["tun"].clone()
    }

    #[test]
    fn snapshot_merge_preserves_payload_noncontrolled_keys() {
        let tun = merge_test_tun(
            "mixed-port: 7890\ntun:\n  route-exclude-address:\n  - 192.0.2.0/24\n  - 198.51.100.0/24\n  mtu: 1400\n",
            "tun:\n  enable: true\n  stack: mixed\n  auto-route: true\n  route-exclude-address:\n  - 203.0.113.0/24\n",
        );
        assert_eq!(tun["enable"].as_bool(), Some(true));
        assert_eq!(tun["stack"].as_str(), Some("mixed"));
        assert_eq!(
            tun["route-exclude-address"],
            serde_yaml::from_str::<serde_yaml::Value>("- 192.0.2.0/24\n- 198.51.100.0/24\n")
                .unwrap()
        );
        assert_eq!(tun["mtu"].as_i64(), Some(1400));
    }

    #[test]
    fn snapshot_merge_drops_snapshot_only_noncontrolled_keys_for_payload_strict() {
        let tun = merge_test_tun(
            "tun:\n  route-exclude-address:\n  - 192.0.2.0/24\n",
            "tun:\n  enable: true\n  mtu: 1400\n  route-exclude-address:\n  - 203.0.113.0/24\n",
        );
        assert_eq!(tun["enable"].as_bool(), Some(true));
        assert!(
            tun.get("mtu").is_none(),
            "snapshot-only non-controlled keys must not be re-injected: {tun:?}"
        );
    }

    #[test]
    fn snapshot_merge_keeps_explicit_empty_noncontrolled_list() {
        let tun = merge_test_tun(
            "tun:\n  route-exclude-address: []\n",
            "tun:\n  enable: true\n  route-exclude-address:\n  - 203.0.113.0/24\n",
        );
        assert_eq!(
            tun["route-exclude-address"],
            serde_yaml::Value::Sequence(Vec::new())
        );
    }

    #[test]
    fn snapshot_merge_inherits_whole_block_when_payload_has_no_tun() {
        let tun = merge_test_tun(
            "mixed-port: 7890\n",
            "tun:\n  enable: true\n  mtu: 1400\n  route-exclude-address:\n  - 192.0.2.0/24\n",
        );
        assert_eq!(tun["enable"].as_bool(), Some(true));
        assert_eq!(tun["mtu"].as_i64(), Some(1400));
        assert_eq!(
            tun["route-exclude-address"],
            serde_yaml::from_str::<serde_yaml::Value>("- 192.0.2.0/24\n").unwrap()
        );
    }

    #[test]
    fn snapshot_merge_inherits_whole_block_when_payload_tun_is_not_mapping() {
        let tun = merge_test_tun(
            "tun: null\nmixed-port: 7890\n",
            "tun:\n  enable: true\n  mtu: 1400\n  route-exclude-address:\n  - 192.0.2.0/24\n",
        );
        assert_eq!(tun["enable"].as_bool(), Some(true));
        assert_eq!(tun["mtu"].as_i64(), Some(1400));
    }

    #[test]
    fn snapshot_merge_controlled_keys_take_snapshot_values() {
        let tun = merge_test_tun(
            "tun:\n  enable: false\n  stack: gvisor\n  route-exclude-address:\n  - 192.0.2.0/24\n",
            "tun:\n  enable: true\n  stack: mixed\n  dns-hijack:\n  - any:53\n",
        );
        assert_eq!(tun["enable"].as_bool(), Some(true));
        assert_eq!(tun["stack"].as_str(), Some("mixed"));
        assert_eq!(tun["dns-hijack"][0].as_str(), Some("any:53"));
    }

    #[test]
    fn snapshot_merge_rejects_non_mapping_snapshot_tun() {
        let snapshot = serde_yaml::Value::Null;
        let error = merge_snapshot_tun_block_into_config_yaml("tun:\n  enable: true\n", &snapshot)
            .expect_err("non-mapping snapshot must fail closed");
        assert!(error.to_string().contains("must be a YAML mapping"));
    }

    #[test]
    fn system_tun_snapshot_merge_is_idempotent_and_revision_bound() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        std::fs::write(
            &ctx.paths.tun_config_file,
            b"tun:\n  enable: true\n  dns-hijack:\n  - any:53\n",
        )
        .unwrap();
        let content = "mixed-port: 7890\nmode: rule\n";
        let original_revision = sha256_revision(content.as_bytes());
        let (merged, revision) =
            merge_system_tun_snapshot_into_payload(&ctx, content, &original_revision).unwrap();
        assert!(merged.contains("tun:"));
        assert!(merged.contains("enable: true"));
        assert_eq!(revision, sha256_revision(merged.as_bytes()));
        let (merged_again, revision_again) =
            merge_system_tun_snapshot_into_payload(&ctx, &merged, &revision).unwrap();
        assert_eq!(merged_again, merged);
        assert_eq!(revision_again, revision);

        std::fs::write(&ctx.paths.tun_config_file, b"tun:\n  enable: false\n").unwrap();
        let (unchanged, unchanged_revision) =
            merge_system_tun_snapshot_into_payload(&ctx, content, &original_revision).unwrap();
        assert_eq!(unchanged, content);
        assert_eq!(unchanged_revision, original_revision);
    }

    #[test]
    fn parse_tun_enabled_from_bytes_matches_yaml() {
        assert!(parse_tun_enabled_from_bytes(
            b"tun:
  enable: true
"
        )
        .unwrap());
        assert!(!parse_tun_enabled_from_bytes(
            b"tun:
  enable: false
"
        )
        .unwrap());
        assert!(!parse_tun_enabled_from_bytes(
            b"port: 7890
"
        )
        .unwrap());
    }

    #[test]
    fn generation_counter_bounded_read() {
        let (_tmp, ctx) = test_context();
        let path = generation_counter_path(&ctx);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path, b"42
",
        )
        .unwrap();
        assert_eq!(read_generation(&ctx).unwrap(), 42);
        assert_eq!(next_generation(&ctx).unwrap(), 43);
    }
    #[cfg(unix)]
    #[test]
    fn dangling_symlink_fails_closed_in_journal_and_counter() {
        let (_tmp, ctx) = test_context();
        let missing_target = ctx.paths.config_dir.join("nonexistent-target");

        // 1. generation counter dangling symlink
        let gen_path = generation_counter_path(&ctx);
        std::fs::create_dir_all(gen_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&missing_target, &gen_path).unwrap();
        assert!(
            read_generation(&ctx).is_err(),
            "dangling generation counter must fail"
        );
        std::fs::remove_file(&gen_path).unwrap();

        // 2. legacy journal dangling symlink
        let leg_j = legacy_journal_path(&ctx);
        std::fs::create_dir_all(leg_j.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&missing_target, &leg_j).unwrap();
        assert!(
            check_and_migrate_legacy_journal(&ctx).is_err(),
            "dangling legacy journal must fail"
        );
        std::fs::remove_file(&leg_j).unwrap();

        // 3. active journal dangling symlink
        let act_j = active_journal_path(&ctx);
        std::fs::create_dir_all(act_j.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&missing_target, &act_j).unwrap();
        assert!(
            read_active_journal(&ctx).is_err(),
            "dangling active journal must fail"
        );
    }
    #[test]
    #[cfg(unix)]
    fn transaction_directory_symlink_is_rejected_before_generation_write() {
        let (_tmp, ctx) = test_context();
        let transactions = transactions_dir(&ctx);
        let outside = ctx.paths.config_dir.join("outside-transactions");
        std::fs::create_dir_all(transactions.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &transactions).unwrap();

        assert!(next_generation(&ctx).is_err());
        assert!(!outside.exists());
    }

    #[test]
    #[cfg(unix)]
    fn generation_counter_rejects_symlinks_and_dangling_links() {
        let (_tmp, ctx) = test_context();
        let path = generation_counter_path(&ctx);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        // 1. Missing file returns 0
        assert_eq!(read_generation(&ctx).unwrap(), 0);

        // 2. Dangling symlink fails
        let target = path.parent().unwrap().join("nonexistent-target");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let err = read_generation(&ctx).unwrap_err().to_string();
        assert!(err.contains("corrupt generation counter"));

        // 3. Symlink to existing file fails (no-follow refuses symlink)
        std::fs::remove_file(&path).unwrap();
        let real_file = path.parent().unwrap().join("real-gen");
        std::fs::write(&real_file, b"42\n").unwrap();
        std::os::unix::fs::symlink(&real_file, &path).unwrap();
        let err = read_generation(&ctx).unwrap_err().to_string();
        assert!(err.contains("corrupt generation counter"));

        // 4. Directory at counter path fails
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let err = read_generation(&ctx).unwrap_err().to_string();
        assert!(err.contains("corrupt generation counter"));
    }

    #[test]
    #[cfg(unix)]
    fn legacy_journal_and_candidate_reject_dangling_symlinks() {
        let (_tmp, ctx) = test_context();
        let leg_j = legacy_journal_path(&ctx);
        let leg_c = legacy_candidate_path(&ctx);
        std::fs::create_dir_all(leg_j.parent().unwrap()).unwrap();

        // 1. Dangling symlink at legacy journal path fails
        let target_j = leg_j.parent().unwrap().join("nonexistent-journal");
        std::os::unix::fs::symlink(&target_j, &leg_j).unwrap();
        let res = check_and_migrate_legacy_journal(&ctx);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("unreadable legacy TUN journal; recovery requires manual evidence"));

        // 2. Valid legacy journal, but dangling symlink at legacy candidate path fails
        std::fs::remove_file(&leg_j).unwrap();
        let legacy_json = serde_json::json!({
            "transaction_id": "legacy-tx",
            "base_revision": "base123",
            "candidate_revision": "cand123",
            "state": "Prepared",
            "candidate_path": leg_c,
            "snapshot_path": ctx.paths.tun_config_file
        });
        std::fs::write(&leg_j, serde_json::to_vec(&legacy_json).unwrap()).unwrap();

        let target_c = leg_c.parent().unwrap().join("nonexistent-candidate");
        std::os::unix::fs::symlink(&target_c, &leg_c).unwrap();
        let res = check_and_migrate_legacy_journal(&ctx);
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(
            err.contains("unreadable legacy candidate artifact; recovery requires manual evidence")
        );
    }

    // ==================== Wave 2 Session C Tests ====================

    #[test]
    fn recovery_required_phase_does_not_output_retry_apply() {
        // SPEC B2: RecoveryRequired should not output RetryApply
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "tx-recovery".to_string(),
            generation: 1,
            phase: JournalPhase::RecoveryRequired,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: Some("base".to_string()),
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: Some("old".to_string()),
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        let obs = RuntimeObservation {
            core_running: false,
            core_identity: None,
            core_pid: None,
            launched_revision: None,
            runtime_tun: None,
            api_ready: false,
        };
        let action = plan_recovery(
            &journal,
            SnapshotClassification::Old,
            IntentClassification::Base,
            OldRuntimeEvidenceState::Present,
            &obs,
            RecoveryDirection::Resume,
        );
        // Should not be RetryApply
        assert!(
            !matches!(action, RecoveryAction::RetryApply),
            "RecoveryRequired should not output RetryApply, got {:?}",
            action
        );
    }

    #[test]
    fn old_journal_without_updated_at_only_diagnoses_no_write() {
        // SPEC B1: Old journal without updated_at should only diagnose, not write
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();

        // Create a journal without updated_at field
        let mut journal = serde_json::json!({
            "schema_version": 1,
            "transaction_id": "tx-old",
            "generation": 1,
            "phase": "RollbackPending",
            "original_uid": 1000,
            "target_runtime_tun": false,
            "base_intent_revision": "base",
            "candidate_revision": "candidate",
            "old_snapshot_revision": "old",
            "old_runtime_digest": "digest",
            "rollback_evidence_complete": true
        });

        // Explicitly remove updated_at if present
        if let Some(obj) = journal.as_object_mut() {
            obj.remove("updated_at");
        }

        std::fs::write(
            active_journal_path(&ctx),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();

        // Read the journal
        let read_journal = read_active_journal(&ctx).unwrap().unwrap();

        // updated_at should be None
        assert!(
            read_journal.updated_at.is_none(),
            "Old journal should have updated_at = None"
        );

        // journal_age_classification should return None (age=unknown)
        let age = journal_age_classification(&read_journal);
        assert!(
            age.is_none(),
            "Old journal without updated_at should return None age"
        );
    }

    #[test]
    fn restore_snapshot_critical_section_can_acquire_inner_cas_guard() {
        // SPEC B5: restore_snapshot critical section should be able to acquire
        // inner CAS guard without deadlock
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();

        // Create a journal
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "tx-restore".to_string(),
            generation: 1,
            phase: JournalPhase::RollbackPending,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        write_active_journal(&ctx, &journal).unwrap();

        // Create snapshot file
        std::fs::write(&ctx.paths.tun_config_file, b"snapshot").unwrap();
        // `old_snapshot_revision: None` 必须以 prepare 期写下的
        // `old-snapshot.missing` 标记为凭据，删除受管快照才是"恢复 base 缺失态"，
        // 而不是未经证明的删除。
        std::fs::write(active_old_snapshot_missing_path(&ctx), b"").unwrap();

        // restore_snapshot should succeed without deadlock
        // This test verifies that the coordinator lock acquired in restore_snapshot
        // does not deadlock with inner operations
        let result = restore_snapshot(&ctx, &journal);
        assert!(
            result.is_ok(),
            "restore_snapshot should succeed without deadlock: {:?}",
            result
        );
    }

    // ---------------- 缺陷 1：回滚禁止删除用户内容 ----------------

    const ROLLBACK_BASE_WITH_TUN: &str = "\
mode: rule
mixed-port: 7897
tun:
  enable: true
  stack: mixed
  auto-route: true
  strict-route: true
  dns-hijack:
  - any:53
  - tcp://any:53
  route-exclude-address:
  - 224.0.0.0/4
rules:
- MATCH,DIRECT
";

    const ROLLBACK_CANDIDATE_TUN_OFF: &str = "\
mode: rule
mixed-port: 7897
tun:
  enable: false
rules:
- MATCH,DIRECT
";

    fn rollback_journal(old_snapshot_revision: Option<String>) -> TunJournal {
        TunJournal {
            schema_version: 1,
            transaction_id: "tx-041-rollback".to_string(),
            generation: 3,
            phase: JournalPhase::RollbackPending,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: Some("base".to_string()),
            candidate_revision: sha256_revision(ROLLBACK_CANDIDATE_TUN_OFF.as_bytes()),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        }
    }

    fn prepare_rollback_fixture(
        old_snapshot: Option<&[u8]>,
        journal_old_revision: Option<String>,
    ) -> (tempfile::TempDir, InstanceContext, TunJournal) {
        let (tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        let journal = rollback_journal(journal_old_revision);
        write_active_journal(&ctx, &journal).unwrap();
        if let Some(bytes) = old_snapshot {
            std::fs::write(active_old_snapshot_path(&ctx), bytes).unwrap();
        }
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        std::fs::write(&ctx.paths.tun_config_file, ROLLBACK_CANDIDATE_TUN_OFF).unwrap();
        (tmp, ctx, journal)
    }

    /// 回滚必须恢复 base 的**完整 tun 块**（逐键一致），不得出现「块消失」。
    #[test]
    fn issue041_rollback_restores_complete_tun_block() {
        let base = ROLLBACK_BASE_WITH_TUN.as_bytes();
        let (_tmp, ctx, journal) =
            prepare_rollback_fixture(Some(base), Some(sha256_revision(base)));
        // 用户 intent 与该事务无关：回滚不得写它（零写入）。
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::write(&ctx.paths.intent_config_file, ROLLBACK_BASE_WITH_TUN).unwrap();

        restore_snapshot(&ctx, &journal).expect("rollback restores the base pre-image");

        let restored = std::fs::read_to_string(&ctx.paths.tun_config_file).unwrap();
        assert_eq!(restored, ROLLBACK_BASE_WITH_TUN, "byte-identical base");
        // 完整性断言：解析后**整块 TUN map** 逐键一致（不是只看 tun:/enable 存在）。
        let doc: serde_yaml::Value = serde_yaml::from_str(&restored).unwrap();
        let expected_tun: serde_yaml::Value = serde_yaml::from_str(
            "\
enable: true
stack: mixed
auto-route: true
strict-route: true
dns-hijack:
- any:53
- tcp://any:53
route-exclude-address:
- 224.0.0.0/4
",
        )
        .unwrap();
        assert_eq!(
            doc["tun"], expected_tun,
            "the whole TUN map must survive the rollback"
        );
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap(),
            ROLLBACK_BASE_WITH_TUN,
            "the rollback must not touch user intent"
        );
    }

    /// base pre-image 是空文件（现场 recovery-target 为空）：宁可拒绝，不得把快照
    /// 截成空——这是「回滚删除用户内容」的实际形态。
    #[test]
    fn issue041_rollback_refuses_empty_base_pre_image() {
        let empty: &[u8] = b"";
        let (_tmp, ctx, journal) =
            prepare_rollback_fixture(Some(empty), Some(sha256_revision(empty)));

        let error = restore_snapshot(&ctx, &journal).expect_err("empty base must be refused");
        let message = error.to_string();
        assert!(message.contains("refusing to overwrite"), "{message}");
        assert!(message.contains("restart --system"), "{message}");
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.tun_config_file).unwrap(),
            ROLLBACK_CANDIDATE_TUN_OFF,
            "managed snapshot must be left untouched"
        );
    }

    /// base pre-image 与 journal 记录的 revision 不一致（截断/被替换）→ 拒绝。
    #[test]
    fn issue041_rollback_refuses_mismatched_base_pre_image() {
        let base = ROLLBACK_BASE_WITH_TUN.as_bytes();
        let (_tmp, ctx, journal) = prepare_rollback_fixture(
            Some(b"mode: rule\nrules:\n- MATCH,DIRECT\n"),
            Some(sha256_revision(base)),
        );

        let error = restore_snapshot(&ctx, &journal).expect_err("mismatched base must be refused");
        assert!(error
            .to_string()
            .contains("not the journal's recorded base"));
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.tun_config_file).unwrap(),
            ROLLBACK_CANDIDATE_TUN_OFF,
            "managed snapshot must be left untouched"
        );
    }

    /// （P2-1）：canonical journal 的 artifact revision 被改写、
    /// 但 transaction/generation/phase 未变时，旧句柄仍不得写快照。
    #[test]
    fn issue050_restore_rejects_rewritten_old_snapshot_revision() {
        let base = ROLLBACK_BASE_WITH_TUN.as_bytes();
        let (_tmp, ctx, journal) =
            prepare_rollback_fixture(Some(base), Some(sha256_revision(base)));
        let mut tampered = journal.clone();
        tampered.old_snapshot_revision = Some(sha256_revision(b"another base"));
        write_active_journal(&ctx, &tampered).unwrap();

        let error = restore_snapshot(&ctx, &journal)
            .expect_err("a rewritten old-snapshot revision must be rejected");
        assert!(
            error.to_string().contains("journal identity mismatch"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.tun_config_file).unwrap(),
            ROLLBACK_CANDIDATE_TUN_OFF,
            "managed snapshot must be left untouched"
        );
    }

    /// （P2-1）：candidate revision 同样属于 journal 身份。
    #[test]
    fn issue050_restore_rejects_rewritten_candidate_revision() {
        let base = ROLLBACK_BASE_WITH_TUN.as_bytes();
        let (_tmp, ctx, journal) =
            prepare_rollback_fixture(Some(base), Some(sha256_revision(base)));
        let mut tampered = journal.clone();
        tampered.candidate_revision = sha256_revision(b"another candidate");
        write_active_journal(&ctx, &tampered).unwrap();

        let error = restore_snapshot(&ctx, &journal)
            .expect_err("a rewritten candidate revision must be rejected");
        assert!(
            error.to_string().contains("journal identity mismatch"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.tun_config_file).unwrap(),
            ROLLBACK_CANDIDATE_TUN_OFF,
            "managed snapshot must be left untouched"
        );
    }

    /// base pre-image 不是 YAML 映射（例如数组/标量）→ 不是配置 → 拒绝。
    #[test]
    fn issue041_rollback_refuses_non_mapping_base_pre_image() {
        let bogus: &[u8] = b"- 224.0.0.0/4\n";
        let (_tmp, ctx, journal) =
            prepare_rollback_fixture(Some(bogus), Some(sha256_revision(bogus)));

        let error = restore_snapshot(&ctx, &journal).expect_err("non-mapping base must be refused");
        assert!(error.to_string().contains("not a YAML mapping"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.tun_config_file).unwrap(),
            ROLLBACK_CANDIDATE_TUN_OFF,
            "managed snapshot must be left untouched"
        );
    }

    /// 提交不得用丢失 `tun` 块的 candidate 覆盖仍带 tun 块的 intent。
    #[test]
    fn issue041_commit_refuses_candidate_without_tun_block() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        std::fs::write(&ctx.paths.intent_config_file, ROLLBACK_BASE_WITH_TUN).unwrap();
        std::fs::write(
            active_candidate_path(&ctx),
            "mode: rule\nrules:\n- MATCH,DIRECT\n",
        )
        .unwrap();

        let mut journal = rollback_journal(Some("old".to_string()));
        journal.phase = JournalPhase::CoreApplied;
        journal.base_intent_revision = Some(sha256_revision(ROLLBACK_BASE_WITH_TUN.as_bytes()));
        journal.candidate_revision = sha256_revision(b"mode: rule\nrules:\n- MATCH,DIRECT\n");
        write_active_journal(&ctx, &journal).unwrap();

        let error = compare_and_commit_user_intent(&ctx, &journal)
            .expect_err("a tun-dropping candidate must be refused");
        assert!(error.to_string().contains("would delete the `tun` block"));
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap(),
            ROLLBACK_BASE_WITH_TUN,
            "intent must be left untouched"
        );
    }

    /// 缺陷 1（裁决表）：base 记录为「缺失」时也必须以 prepare 期的
    /// `old-snapshot.missing` 标记为凭据；没有凭据就不许删受管快照。
    #[test]
    fn issue041_rollback_requires_missing_marker_before_deleting_snapshot() {
        let (_tmp, ctx, journal) = prepare_rollback_fixture(None, None);
        let error = restore_snapshot(&ctx, &journal)
            .expect_err("deleting the snapshot without the missing marker must be refused");
        assert!(
            error.to_string().contains("old-snapshot.missing"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.tun_config_file).unwrap(),
            ROLLBACK_CANDIDATE_TUN_OFF,
            "managed snapshot must be left untouched"
        );

        // With the prepare-time marker the base state really is "no snapshot".
        std::fs::write(active_old_snapshot_missing_path(&ctx), b"").unwrap();
        restore_snapshot(&ctx, &journal).expect("marker authorizes restoring the missing base");
        assert!(!ctx.paths.tun_config_file.exists());
    }

    /// 缺陷 1：历史冲突现场（journal base B0 / candidate C0 / 最新 intent B1）——
    /// 提交必须 Conflict（不把 C0 覆盖进 B1），且 B1 的 tun 块逐键保留。
    #[test]
    fn issue041_historical_conflict_never_overwrites_latest_intent() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();

        let base_b0 = "mode: rule\ntun:\n  enable: true\n  stack: mixed\nrules:\n- MATCH,DIRECT\n";
        let candidate_c0 = "mode: rule\ntun:\n  enable: false\nrules:\n- MATCH,DIRECT\n";
        let latest_b1 = "\
mode: rule
tun:
  enable: true
  stack: mixed
  strict-route: true
  dns-hijack:
  - any:53
  route-exclude-address:
  - 224.0.0.0/4
rules:
- MATCH,DIRECT
";
        std::fs::write(&ctx.paths.intent_config_file, latest_b1).unwrap();
        std::fs::write(active_candidate_path(&ctx), candidate_c0).unwrap();

        let mut journal = rollback_journal(Some("old".to_string()));
        journal.phase = JournalPhase::CoreApplied;
        journal.base_intent_revision = Some(sha256_revision(base_b0.as_bytes()));
        journal.candidate_revision = sha256_revision(candidate_c0.as_bytes());
        write_active_journal(&ctx, &journal).unwrap();

        let result = compare_and_commit_user_intent(&ctx, &journal).unwrap();
        assert_eq!(result, IntentCommitResult::Conflict);
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap(),
            latest_b1,
            "the stale candidate must never be written over the latest intent"
        );
        let doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap())
                .unwrap();
        assert_eq!(doc["tun"]["enable"], serde_yaml::Value::Bool(true));
        assert_eq!(
            doc["tun"]["route-exclude-address"],
            serde_yaml::from_str::<serde_yaml::Value>("- 224.0.0.0/4").unwrap()
        );
    }

    /// 缺陷 3（裁决备选 a）：核心冲突现场在两种 phase 下都只做安全动作——CoreApplied
    /// 走回滚、RollbackPending 走 ContinueRollback，二者都不写 intent、不宣称为迟到
    /// 提交（无 FinalizeCommittedIntent）。
    #[test]
    fn issue041_conflict_planner_never_commits_or_finalizes_on_external_change() {
        let journal = rollback_journal(Some("old".to_string()));
        let observation = RuntimeObservation {
            core_running: true,
            core_identity: None,
            core_pid: Some(4242),
            launched_revision: Some(journal.candidate_revision.clone()),
            runtime_tun: Some(journal.target_runtime_tun),
            api_ready: true,
        };
        let old_evidence = OldRuntimeEvidenceState::Present;

        let mut core_applied = journal.clone();
        core_applied.phase = JournalPhase::CoreApplied;
        let action = plan_recovery(
            &core_applied,
            SnapshotClassification::Candidate,
            IntentClassification::Other,
            old_evidence,
            &observation,
            RecoveryDirection::Resume,
        );
        assert_eq!(
            action,
            RecoveryAction::BeginRollback,
            "external change must not be committed (no Finalize/Commit)"
        );

        let mut rollback_pending = journal.clone();
        rollback_pending.phase = JournalPhase::RollbackPending;
        let action = plan_recovery(
            &rollback_pending,
            SnapshotClassification::Candidate,
            IntentClassification::Other,
            old_evidence,
            &observation,
            RecoveryDirection::Resume,
        );
        assert_eq!(action, RecoveryAction::ContinueRollback);
    }

    /// 缺陷 3（裁决点 3）：`cas_update_phase` 必须钉住 candidate revision——旧候选的
    /// fence 不得推进新事务（旧响应/旧 fence 不得越权）。
    #[test]
    fn issue041_cas_rejects_stale_candidate_revision() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        let journal = rollback_journal(Some("old".to_string()));
        write_active_journal(&ctx, &journal).unwrap();

        let stale_fence = TransactionFence {
            transaction_id: journal.transaction_id.clone(),
            generation: journal.generation,
            expected_phase: journal.phase,
            expected_candidate_revision: "someone-elses-candidate".to_string(),
        };
        let error = cas_update_phase(
            &ctx,
            &stale_fence,
            JournalPhase::RecoveryRequired,
            None,
            None,
        )
        .expect_err("stale candidate fence must be rejected");
        assert!(error.to_string().contains("stale transaction CAS update"));
        assert_eq!(
            read_active_journal(&ctx).unwrap().unwrap().phase,
            JournalPhase::RollbackPending,
            "journal must not advance on a stale fence"
        );
    }

    /// 缺陷 3（裁决点 2）：提交前重验 canonical journal——调用方持旧 journal（generation
    /// 已被推进）时零写入。
    #[test]
    fn issue041_commit_rejects_advanced_canonical_journal() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();
        std::fs::write(&ctx.paths.intent_config_file, ROLLBACK_BASE_WITH_TUN).unwrap();

        let mut stale = rollback_journal(Some("old".to_string()));
        stale.phase = JournalPhase::CoreApplied;
        stale.base_intent_revision = Some(sha256_revision(ROLLBACK_BASE_WITH_TUN.as_bytes()));
        stale.candidate_revision = sha256_revision(ROLLBACK_CANDIDATE_TUN_OFF.as_bytes());
        std::fs::write(
            active_candidate_path(&ctx),
            ROLLBACK_CANDIDATE_TUN_OFF.as_bytes(),
        )
        .unwrap();
        // Canonical journal moved on to a newer generation (concurrent recovery).
        std::fs::write(
            active_old_snapshot_path(&ctx),
            ROLLBACK_BASE_WITH_TUN.as_bytes(),
        )
        .unwrap();
        let mut canonical = stale.clone();
        canonical.generation = stale.generation + 1;
        write_active_journal(&ctx, &canonical).unwrap();

        let error = compare_and_commit_user_intent(&ctx, &stale)
            .expect_err("a stale journal handle must not commit");
        assert!(error
            .to_string()
            .contains("journal changed under this commit"));
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap(),
            ROLLBACK_BASE_WITH_TUN,
            "nothing may be written on a stale handle"
        );
    }

    /// 缺陷 3（裁决点 1）：非终态事务 owner 存在时，任何配置写入入口都必须拒绝。
    #[test]
    fn issue041_config_write_refused_while_transaction_owner_active() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(active_dir(&ctx)).unwrap();

        // No transaction → allowed.
        ensure_no_non_terminal_transaction(&ctx, "override import").unwrap();

        for phase in [
            JournalPhase::Prepared,
            JournalPhase::PromotionPending,
            JournalPhase::SnapshotPromoted,
            JournalPhase::CoreApplied,
            JournalPhase::RollbackPending,
            JournalPhase::RecoveryRequired,
        ] {
            let mut journal = rollback_journal(Some("old".to_string()));
            journal.phase = phase;
            write_active_journal(&ctx, &journal).unwrap();
            let error = ensure_no_non_terminal_transaction(&ctx, "override import")
                .expect_err("a non-terminal owner must block config writes");
            assert!(
                error
                    .to_string()
                    .contains("refused before writing anything"),
                "{phase:?}: {error}"
            );
        }

        // Terminal phases are not owners anymore.
        let mut terminal = rollback_journal(Some("old".to_string()));
        terminal.phase = JournalPhase::IntentCommitted;
        write_active_journal(&ctx, &terminal).unwrap();
        ensure_no_non_terminal_transaction(&ctx, "override import")
            .expect("terminal transactions do not block config writes");

        // A pending legacy journal also blocks.
        let mut committed = terminal.clone();
        committed.phase = JournalPhase::RolledBack;
        write_active_journal(&ctx, &committed).unwrap();
        std::fs::write(legacy_journal_path(&ctx), b"{}").unwrap();
        let error = ensure_no_non_terminal_transaction(&ctx, "override import")
            .expect_err("a pending legacy journal must block config writes");
        assert!(error.to_string().contains("legacy TUN transaction"));
    }

    /// Issue 064: 普通用户读不到 system 事务日志时必须 fail-closed，但错误分类
    /// 必须与日志损坏区分，并给出 Linux 组权限/授权路径，而不是把 `status` 当作修复。
    #[cfg(target_os = "linux")]
    #[test]
    fn issue064_permission_denied_message_is_actionable_and_zero_write() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root 绕过 0000 模式位，构造不出 EACCES。
        }
        let (_tmp, ctx) = test_context();
        let tx = transactions_dir(&ctx);
        std::fs::create_dir_all(&tx).unwrap();
        std::fs::set_permissions(&tx, std::fs::Permissions::from_mode(0o000)).unwrap();
        let before = std::fs::read(&ctx.paths.intent_config_file).ok();
        let error = ensure_no_non_terminal_transaction(&ctx, "override import")
            .expect_err("unreadable system transaction directory must fail closed");
        std::fs::set_permissions(&tx, std::fs::Permissions::from_mode(0o700)).unwrap();
        let message = error.to_string();
        assert!(message.contains("Permission denied"), "{message}");
        assert!(message.contains("mihomo"), "{message}");
        assert!(message.contains("usermod"), "{message}");
        assert!(message.contains("NOT modified"), "{message}");
        assert!(
            !message.contains("Fix: mihomo-cli status"),
            "status is equally unreadable and must not be suggested as the fix: {message}"
        );
        assert_eq!(std::fs::read(&ctx.paths.intent_config_file).ok(), before);
    }

    /// Issue 064 Q3: 运行期 EACCES 文案必须按平台区分，macOS/Windows 不得出现
    /// Linux 组或 `usermod` 指引；所有平台都不得把 `status` 当作修复。
    #[test]
    fn issue064_permission_denied_hint_is_platform_specific() {
        let state_dir = Path::new("/var/lib/mihomo-cli");

        let linux = permission_denied_recovery_hint_linux(state_dir);
        assert!(linux.contains("0770 root:mihomo"), "{linux}");
        assert!(linux.contains("mihomo"), "{linux}");
        assert!(linux.contains("usermod"), "{linux}");
        assert!(linux.contains("sign in again"), "{linux}");
        assert!(
            !linux.contains("Fix: mihomo-cli status"),
            "status must not be suggested as the fix: {linux}"
        );

        let macos = permission_denied_recovery_hint_macos(state_dir);
        assert!(!macos.contains("usermod"), "{macos}");
        assert!(!macos.contains("root:mihomo"), "{macos}");
        assert!(macos.contains("root:wheel"), "{macos}");
        assert!(macos.contains("LaunchDaemon"), "{macos}");
        assert!(macos.contains("authorization"), "{macos}");
        assert!(
            !macos.contains("Fix: mihomo-cli status"),
            "status must not be suggested as the fix: {macos}"
        );

        let windows = permission_denied_recovery_hint_windows(state_dir);
        assert!(!windows.contains("usermod"), "{windows}");
        assert!(!windows.contains("root:mihomo"), "{windows}");
        assert!(windows.contains("DACL"), "{windows}");
        assert!(windows.contains("%ProgramData%"), "{windows}");
        assert!(windows.contains("icacls"), "{windows}");
        assert!(
            !windows.contains("Fix: mihomo-cli status"),
            "status must not be suggested as the fix: {windows}"
        );

        let generic = permission_denied_recovery_hint_generic(state_dir);
        assert!(!generic.contains("usermod"), "{generic}");
        assert!(!generic.contains("mihomo group"), "{generic}");
        assert!(generic.contains("managed system-authorized"), "{generic}");
        assert!(
            !generic.contains("Fix: mihomo-cli status"),
            "status must not be suggested as the fix: {generic}"
        );
    }

    /// Issue 064 Q3: host 平台选择器必须与真实目标一致。
    #[test]
    fn issue064_permission_denied_hint_selects_host_platform() {
        let hint = permission_denied_recovery_hint(Path::new("/var/lib/mihomo-cli"));

        #[cfg(target_os = "linux")]
        {
            assert!(hint.contains("usermod"), "{hint}");
            assert!(hint.contains("0770 root:mihomo"), "{hint}");
        }
        #[cfg(target_os = "macos")]
        {
            assert!(!hint.contains("usermod"), "{hint}");
            assert!(hint.contains("LaunchDaemon"), "{hint}");
            assert!(hint.contains("root:wheel"), "{hint}");
        }
        #[cfg(target_os = "windows")]
        {
            assert!(!hint.contains("usermod"), "{hint}");
            assert!(hint.contains("DACL"), "{hint}");
            assert!(hint.contains("%ProgramData%"), "{hint}");
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            assert!(!hint.contains("usermod"), "{hint}");
            assert!(hint.contains("managed system-authorized"), "{hint}");
        }
    }

    /// 缺陷 3（结果诚实）：回滚失败时 phase 不允许被当作终态——planner 只能拒
    /// （受控 reset），不会返回任何"已提交/已回滚完成"的动作。
    #[test]
    fn issue041_failed_rollback_state_is_never_reported_as_terminal() {
        let journal = rollback_journal(Some("old".to_string()));
        let observation = RuntimeObservation {
            core_running: true,
            core_identity: None,
            core_pid: Some(4242),
            launched_revision: Some(journal.candidate_revision.clone()),
            runtime_tun: Some(journal.target_runtime_tun),
            api_ready: true,
        };
        let mut stuck = journal.clone();
        stuck.phase = JournalPhase::RollbackPending;
        // Base pre-image unreadable ⇒ phase classified as Other.
        let action = plan_recovery(
            &stuck,
            SnapshotClassification::Other,
            IntentClassification::Other,
            OldRuntimeEvidenceState::Present,
            &observation,
            RecoveryDirection::Resume,
        );
        assert!(
            matches!(action, RecoveryAction::RefuseNeedsEvidence(_)),
            "an unprovable rollback must refuse, got {action:?}"
        );
    }

    #[test]
    fn recovery_marker_try_acquire_conflict() {
        // SPEC B5: Second try_acquire should immediately conflict
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(transactions_dir(&ctx)).unwrap();

        // First acquire should succeed
        let first = try_acquire_recovery_marker(&ctx).unwrap();
        assert!(first.is_some(), "First try_acquire should succeed");

        // Second acquire should return None (conflict), never queue
        let second = try_acquire_recovery_marker(&ctx).unwrap();
        assert!(
            second.is_none(),
            "Second try_acquire should return None (conflict)"
        );
        assert!(recovery_marker_path(&ctx).exists());

        // RAII: dropping the guard must release the marker
        drop(first.unwrap());
        assert!(
            !recovery_marker_path(&ctx).exists(),
            "Dropping the guard must release the recovery marker"
        );

        // Now acquire should succeed again
        let third = try_acquire_recovery_marker(&ctx).unwrap();
        assert!(
            third.is_some(),
            "Third try_acquire after release should succeed"
        );

        // Explicit release through the same production helper.
        release_recovery_marker(&ctx).unwrap();
        assert!(!recovery_marker_path(&ctx).exists());
    }

    #[test]
    fn recovery_marker_release_ignores_foreign_pid() {
        // A marker written by another pid must never be removed by this process.
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(transactions_dir(&ctx)).unwrap();
        let marker_path = recovery_marker_path(&ctx);
        let foreign = RecoveryMarker {
            pid: std::process::id().wrapping_add(1),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(&marker_path, serde_json::to_vec(&foreign).unwrap()).unwrap();

        release_recovery_marker(&ctx).unwrap();
        assert!(
            marker_path.exists(),
            "release_recovery_marker must not delete a marker owned by another pid"
        );
    }

    #[test]
    fn recovery_marker_live_pid_never_reclaimed_regardless_of_age() {
        // P2-5: a live holder must never be displaced, even if the marker is
        // older than the old 600s stale threshold.
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(transactions_dir(&ctx)).unwrap();
        let marker_path = recovery_marker_path(&ctx);
        // Write a marker with our own pid (live) but an old timestamp.
        let old_marker = RecoveryMarker {
            pid: std::process::id(),
            created_at: "2020-01-01T00:00:00+00:00".to_string(),
        };
        std::fs::write(&marker_path, serde_json::to_vec(&old_marker).unwrap()).unwrap();

        let result = try_acquire_recovery_marker(&ctx).unwrap();
        assert!(
            result.is_none(),
            "a live pid marker must never be reclaimed, regardless of age"
        );
        assert!(
            marker_path.exists(),
            "the original marker must survive the attempt"
        );
    }

    #[cfg(unix)]
    #[test]
    fn recovery_marker_dead_pid_is_reclaimed() {
        // P2-5: a dead holder's marker must be reclaimed so recovery can proceed.
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(transactions_dir(&ctx)).unwrap();
        let marker_path = recovery_marker_path(&ctx);
        // Use a pid that is almost certainly dead (pid 1 is init, but we pick
        // a very high pid that is unlikely to exist).
        let dead_marker = RecoveryMarker {
            pid: 4_000_000,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        std::fs::write(&marker_path, serde_json::to_vec(&dead_marker).unwrap()).unwrap();

        let result = try_acquire_recovery_marker(&ctx).unwrap();
        assert!(
            result.is_some(),
            "a dead pid marker must be reclaimed so recovery can proceed"
        );
        // Clean up.
        drop(result);
    }

    // ----------------: legacy reset admission ----------------

    fn seed_migrated_placeholder(ctx: &InstanceContext) -> TunJournal {
        std::fs::create_dir_all(active_dir(ctx)).unwrap();
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        let evidence = OldRuntimeEvidence {
            core_running: false,
            core_identity: LEGACY_MIGRATION_PLACEHOLDER_IDENTITY.to_string(),
            core_pid: 0,
            launched_revision: String::new(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: String::new(),
            recorded_at_secs: None,
        };
        let bytes = serde_json::to_vec_pretty(&evidence).unwrap();
        std::fs::write(active_old_runtime_path(ctx), &bytes).unwrap();
        let journal = TunJournal {
            transaction_id: "tun-migrated-placeholder".to_string(),
            generation: 11,
            phase: JournalPhase::RecoveryRequired,
            target_runtime_tun: false,
            candidate_revision: "candidate-sha256".to_string(),
            old_runtime_digest: sha256_revision(&bytes),
            legacy_source: true,
            rollback_evidence_complete: false,
            ..managed_test_journal()
        };
        write_active_journal(ctx, &journal).unwrap();
        journal
    }

    #[test]
    fn legacy_placeholder_shape_accepts_only_the_migration_form() {
        let placeholder = OldRuntimeEvidence {
            core_running: false,
            core_identity: LEGACY_MIGRATION_PLACEHOLDER_IDENTITY.to_string(),
            core_pid: 0,
            launched_revision: String::new(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: String::new(),
            recorded_at_secs: None,
        };
        assert!(is_legacy_migration_placeholder(&placeholder));

        let deviations = [
            OldRuntimeEvidence {
                core_running: true,
                ..placeholder.clone()
            },
            OldRuntimeEvidence {
                core_identity: "/usr/local/lib/mihomo/mihomo".to_string(),
                ..placeholder.clone()
            },
            OldRuntimeEvidence {
                core_pid: 42,
                ..placeholder.clone()
            },
            OldRuntimeEvidence {
                launched_revision: "rev".to_string(),
                ..placeholder.clone()
            },
            OldRuntimeEvidence {
                launch_source: LaunchSource::SystemActiveConfig,
                ..placeholder.clone()
            },
            OldRuntimeEvidence {
                runtime_tun: true,
                ..placeholder.clone()
            },
            OldRuntimeEvidence {
                api_endpoint: "unix:///var/run/mihomo/mihomo.sock".to_string(),
                ..placeholder.clone()
            },
            OldRuntimeEvidence {
                recorded_at_secs: Some(1),
                ..placeholder.clone()
            },
        ];
        for (index, evidence) in deviations.iter().enumerate() {
            assert!(
                !is_legacy_migration_placeholder(evidence),
                "placeholder deviation {index} must be refused"
            );
        }
    }

    #[test]
    fn legacy_reset_admission_requires_the_exact_journal_form() {
        let (_tmp, ctx) = test_context();
        let journal = seed_migrated_placeholder(&ctx);
        legacy_reset_evidence_admission(&ctx, &journal).unwrap();

        let not_legacy = TunJournal {
            legacy_source: false,
            ..journal.clone()
        };
        assert!(legacy_reset_evidence_admission(&ctx, &not_legacy).is_err());

        let evidence_complete = TunJournal {
            rollback_evidence_complete: true,
            ..journal.clone()
        };
        assert!(legacy_reset_evidence_admission(&ctx, &evidence_complete).is_err());

        let candidate_identity = TunJournal {
            candidate_core_identity: Some("/usr/local/lib/mihomo/mihomo".to_string()),
            ..journal.clone()
        };
        assert!(legacy_reset_evidence_admission(&ctx, &candidate_identity).is_err());

        let candidate_pid = TunJournal {
            candidate_core_pid: Some(4242),
            ..journal.clone()
        };
        assert!(legacy_reset_evidence_admission(&ctx, &candidate_pid).is_err());

        let other_phase = TunJournal {
            phase: JournalPhase::RollbackPending,
            ..journal.clone()
        };
        assert!(legacy_reset_evidence_admission(&ctx, &other_phase).is_err());
    }

    #[test]
    fn legacy_reset_admission_refuses_broken_or_foreign_evidence() {
        let (_tmp, ctx) = test_context();
        let journal = seed_migrated_placeholder(&ctx);

        // Digest mismatch (evidence replaced after the journal was written).
        std::fs::write(active_old_runtime_path(&ctx), b"{}").unwrap();
        assert!(legacy_reset_evidence_admission(&ctx, &journal).is_err());

        // A partially attested old runtime is not the migration placeholder.
        let pinned = OldRuntimeEvidence {
            core_running: false,
            core_identity: LEGACY_MIGRATION_PLACEHOLDER_IDENTITY.to_string(),
            core_pid: 4321,
            launched_revision: String::new(),
            launch_source: LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: String::new(),
            recorded_at_secs: None,
        };
        let bytes = serde_json::to_vec_pretty(&pinned).unwrap();
        std::fs::write(active_old_runtime_path(&ctx), &bytes).unwrap();
        let rehashed = TunJournal {
            old_runtime_digest: sha256_revision(&bytes),
            ..journal.clone()
        };
        let refusal = legacy_reset_evidence_admission(&ctx, &rehashed).unwrap_err();
        assert!(refusal.contains("placeholder"), "{refusal}");

        // Missing artifact is a refusal, never "unknown = allow".
        std::fs::remove_file(active_old_runtime_path(&ctx)).unwrap();
        assert!(legacy_reset_evidence_admission(&ctx, &journal).is_err());
    }

    #[test]
    fn legacy_migration_output_is_exactly_what_the_admission_accepts() {
        let (_tmp, ctx) = test_context();
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        let intent = b"mixed-port: 7890\n";
        std::fs::write(&ctx.paths.intent_config_file, intent).unwrap();
        std::fs::create_dir_all(transactions_dir(&ctx)).unwrap();
        let candidate = b"mode: rule\n";
        std::fs::write(legacy_candidate_path(&ctx), candidate).unwrap();
        let legacy = serde_json::json!({
            "transaction_id": "stale-prepared",
            "base_revision": legacy_fnv_revision(intent),
            "candidate_revision": legacy_fnv_revision(candidate),
            "state": "Prepared",
            "candidate_path": legacy_candidate_path(&ctx),
            "snapshot_path": ctx.paths.tun_config_file,
        });
        std::fs::write(
            legacy_journal_path(&ctx),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();

        let migrated = check_and_migrate_legacy_journal(&ctx)
            .unwrap()
            .expect("legacy journal must migrate");
        assert!(is_legacy_reset_journal_form(&migrated));
        let evidence = legacy_reset_evidence_admission(&ctx, &migrated)
            .expect("the migrator output must satisfy the reset admission");
        assert_eq!(
            evidence.core_identity,
            LEGACY_MIGRATION_PLACEHOLDER_IDENTITY
        );
        assert_eq!(evidence.core_pid, 0);
        assert!(evidence.recorded_at_secs.is_none());
    }
}
