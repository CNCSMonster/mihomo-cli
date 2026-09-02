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

#[allow(dead_code)]
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
    #[cfg(unix)]
    file: File,
    #[cfg(not(unix))]
    _path: PathBuf,
}

impl CoordinatorLock {
    pub fn acquire(ctx: &InstanceContext) -> anyhow::Result<Self> {
        let lock_path = coordinator_lock_path(ctx);
        if let Some(parent) = lock_path.parent() {
            ensure_system_dir(parent, 0o750)?;
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
        #[cfg(not(unix))]
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

#[allow(dead_code)]
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
    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != fence.expected_phase
    {
        anyhow::bail!(
            "stale transaction CAS update: expected {:?}/{}/phase={:?}, found {:?}/{}/phase={:?}",
            fence.transaction_id,
            fence.generation,
            fence.expected_phase,
            journal.transaction_id,
            journal.generation,
            journal.phase
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
    write_active_journal(ctx, &journal)?;
    Ok(journal)
}

// ---------------- Prepare and Publish Active Transaction ----------------

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

    write_file_synced(snapshot_path, &candidate_bytes, 0o640)?;
    Ok(())
}

pub fn restore_snapshot(ctx: &InstanceContext, journal: &TunJournal) -> anyhow::Result<()> {
    let snapshot_path = &ctx.paths.tun_config_file;
    if journal.old_snapshot_revision.is_some() {
        let old_snap_path = active_old_snapshot_path(ctx);
        let old_snap_bytes =
            utils::read_file_no_follow_limited(&old_snap_path, MAX_TRANSACTION_ARTIFACT_BYTES)?;
        write_file_synced(snapshot_path, &old_snap_bytes, 0o640)?;
    } else {
        // Old snapshot did not exist, remove snapshot file if exists
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
    let _lock = crate::lock::ConfigLock::acquire(&ctx.paths.config_dir)?;
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

    if let Some(ref base_rev) = journal.base_intent_revision {
        if current_rev != *base_rev {
            return Ok(IntentCommitResult::Conflict);
        }
    } else if !journal.legacy_source {
        return Ok(IntentCommitResult::Conflict);
    }

    let candidate_path = active_candidate_path(ctx);
    let candidate_bytes =
        utils::read_file_no_follow_limited(&candidate_path, MAX_TRANSACTION_ARTIFACT_BYTES)?;
    if sha256_revision(&candidate_bytes) != journal.candidate_revision {
        anyhow::bail!("candidate revision mismatch during intent commit");
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

#[allow(dead_code)]
pub fn check_user_gate_for_mutation(config_dir: &Path) -> anyhow::Result<()> {
    if let Some(gate) = read_user_gate(config_dir)? {
        anyhow::bail!(
            "A system TUN transaction is in progress (transaction: {}, gen: {}).\n\
             Inspect diagnostics:\n  \
             mihomo-cli status",
            gate.transaction_id,
            gate.generation
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
        JournalPhase::RollbackPending => {
            match snapshot_cls {
                SnapshotClassification::Candidate => RecoveryAction::ContinueRollback,
                // The planner does not have authenticated old-runtime evidence.
                // Always execute the idempotent rollback protocol; only the
                // daemon, after digest validation, may attest the old runtime.
                SnapshotClassification::Old | SnapshotClassification::Missing => {
                    let _ = observation;
                    RecoveryAction::ContinueRollback
                }
                SnapshotClassification::Other | SnapshotClassification::Unreadable => {
                    RecoveryAction::RefuseNeedsEvidence(
                        "Snapshot is an unrecoverable or unexpected revision".to_string(),
                    )
                }
            }
        }
        JournalPhase::RecoveryRequired => {
            // Check matrix in SPEC Section 12.10
            if journal.legacy_source && !journal.rollback_evidence_complete {
                match direction {
                    RecoveryDirection::Resume => match intent_cls {
                        IntentClassification::Base => RecoveryAction::RetryApply,
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
                                    RecoveryAction::RetryApply
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
                                    RecoveryAction::RetryApply
                                }
                            }
                            RecoveryDirection::Abort => RecoveryAction::BeginRollback,
                        }
                    }
                    (SnapshotClassification::Old | SnapshotClassification::Missing, IntentClassification::Base) => {
                        match direction {
                            RecoveryDirection::Resume => RecoveryAction::RetryApply,
                            RecoveryDirection::Abort => RecoveryAction::ContinueRollback,
                        }
                    }
                    (SnapshotClassification::Old | SnapshotClassification::Missing, IntentClassification::Candidate) => {
                        match direction {
                            RecoveryDirection::Resume => RecoveryAction::RetryApply,
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
    let _coordinator_lock = CoordinatorLock::acquire(ctx)?;
    let leg_j_path = legacy_journal_path(ctx);
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
            &obs,
            RecoveryDirection::Resume,
        );
        assert_eq!(plan_resume, RecoveryAction::FinalizeCommittedIntent);

        let plan_abort = plan_recovery(
            &journal,
            SnapshotClassification::Candidate,
            IntentClassification::Candidate,
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
        };
        assert!(read_and_validate_old_runtime(&ctx, &journal).is_err());
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
}
