//! Upgrade generation state model and immutable staging primitives.
//!
//! Provides isolated storage, manifest schemas, checksum validation, and atomic
//! generation state transitions (Active, Pending, Previous) for system install
//! and apply workflows.

#![allow(dead_code)]

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt;
#[cfg(windows)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024; // 1 MB
pub const MAX_STATE_BYTES: u64 = 64 * 1024; // 64 KB
pub const DEFAULT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_PROTOCOL_VERSION: u32 = 1;
pub const STATE_FILE_NAME: &str = "state.json";
pub const MANIFEST_FILE_NAME: &str = "manifest.json";
pub const LOCK_FILE_NAME: &str = ".generation.lock";
/// SPEC C: tombstone suffix for withdrawn pending generations.
pub const WITHDRAWN_SUFFIX: &str = ".withdrawn";
/// SPEC 裁决 A1: proof marker inside a generation/tombstone directory.
pub const ROLLBACK_PROVEN_MARKER: &str = ".rollback-proven";
/// SPEC P1-1: marker written into every generation copied by the legacy
/// migration. Migration confers no trust, so P2 must reject these (fact unknown).
pub const UNTRUSTED_MARKER: &str = ".migrated-untrusted";
/// SPEC A/P2-3: metadata marker that licenses the `missing` exemption.
pub const BASELINE_KIND: &str = "baseline";
/// SHA-256 of the empty file, used for the sentinel entries a baseline manifest
/// declares for artifacts that were absent at capture time.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_RETRY_ATTEMPTS: u32 = 10;
const RETRY_BACKOFF: Duration = Duration::from_millis(50);

/// Error types occurring during generation storage, validation, or state transition.
#[derive(Debug)]
pub enum GenerationError {
    InvalidId(String),
    InvalidPath(String),
    InvalidSha256Format {
        path: String,
        actual: String,
    },
    UnsupportedSchemaVersion {
        version: u32,
    },
    UnsupportedProtocolVersion {
        version: u32,
    },
    DuplicateArtifactPath(String),
    DaemonCorePathOverlap(String),
    MismatchedArtifactKind {
        path: String,
        expected: ArtifactKind,
        actual: ArtifactKind,
    },
    ManifestNotFound(PathBuf),
    ManifestCorrupted(String),
    ManifestSizeExceeded {
        actual: u64,
        max: u64,
    },
    StateSizeExceeded {
        actual: u64,
        max: u64,
    },
    ArtifactMissing {
        path: PathBuf,
        kind: ArtifactKind,
    },
    ArtifactHashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    ArtifactSizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    ArtifactPermissionMismatch {
        path: PathBuf,
        expected: u32,
        actual: u32,
    },
    SymlinkForbidden(PathBuf),
    NotRegularFile(PathBuf),
    NoPendingGeneration,
    NoPreviousGeneration,
    GenerationMismatch {
        expected: GenerationId,
        actual: GenerationId,
    },
    LockError(String),
    FileBusy {
        path: PathBuf,
        attempts: u32,
        details: String,
    },
    /// SPEC C: tombstone already exists for this ID — collision means
    /// the withdrawn semantic is undefined, refuse to publish.
    TombstoneCollision {
        id: GenerationId,
        path: PathBuf,
    },
    Io(io::Error),
    Serialization(String),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId(msg) => write!(f, "Invalid generation ID: {msg}"),
            Self::InvalidPath(msg) => write!(f, "Invalid artifact path: {msg}"),
            Self::InvalidSha256Format { path, actual } => {
                write!(
                    f,
                    "Invalid SHA-256 format for '{path}': expected 64 hex characters, got '{actual}'"
                )
            }
            Self::UnsupportedSchemaVersion { version } => {
                write!(f, "Unsupported manifest schema version: {version}")
            }
            Self::UnsupportedProtocolVersion { version } => {
                write!(f, "Unsupported protocol version: {version}")
            }
            Self::DuplicateArtifactPath(path) => {
                write!(f, "Duplicate artifact path declared in manifest: '{path}'")
            }
            Self::DaemonCorePathOverlap(path) => {
                write!(
                    f,
                    "Daemon and Core artifacts cannot share the same relative path: '{path}'"
                )
            }
            Self::MismatchedArtifactKind {
                path,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "Mismatched artifact kind for '{path}': expected {expected:?}, got {actual:?}"
                )
            }
            Self::ManifestNotFound(path) => write!(f, "Manifest not found at {}", path.display()),
            Self::ManifestCorrupted(msg) => write!(f, "Manifest corrupted: {msg}"),
            Self::ManifestSizeExceeded { actual, max } => {
                write!(
                    f,
                    "Manifest size {actual} bytes exceeds limit of {max} bytes"
                )
            }
            Self::StateSizeExceeded { actual, max } => {
                write!(f, "State size {actual} bytes exceeds limit of {max} bytes")
            }
            Self::ArtifactMissing { path, kind } => {
                write!(f, "{kind:?} artifact missing at {}", path.display())
            }
            Self::ArtifactHashMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "Hash mismatch for {}: expected {expected}, actual {actual}",
                path.display()
            ),
            Self::ArtifactSizeMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "Size mismatch for {}: expected {expected} bytes, actual {actual} bytes",
                path.display()
            ),
            Self::ArtifactPermissionMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "Permission mismatch for {}: expected {expected:#o}, actual {actual:#o}",
                path.display()
            ),
            Self::SymlinkForbidden(path) => {
                write!(
                    f,
                    "Symlink forbidden in generation artifact path: {}",
                    path.display()
                )
            }
            Self::NotRegularFile(path) => {
                write!(
                    f,
                    "Expected regular file for generation artifact at {}",
                    path.display()
                )
            }
            Self::NoPendingGeneration => write!(f, "No pending generation to commit"),
            Self::NoPreviousGeneration => write!(f, "No previous generation to rollback to"),
            Self::GenerationMismatch { expected, actual } => write!(
                f,
                "Generation ID mismatch: manifest claims {actual}, expected {expected}"
            ),
            Self::LockError(msg) => write!(f, "Generation lock error: {msg}"),
            Self::FileBusy {
                path,
                attempts,
                details,
            } => {
                write!(
                    f,
                    "File busy at {} after {attempts} attempts: {details}",
                    path.display()
                )
            }
            Self::TombstoneCollision { id, path } => {
                write!(
                    f,
                    "Tombstone collision for generation {id}: {} already exists",
                    path.display()
                )
            }
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Serialization(msg) => write!(f, "Serialization error: {msg}"),
        }
    }
}

impl std::error::Error for GenerationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for GenerationError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// A validated, path-safe identifier for an upgrade generation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct GenerationId(String);

impl<'de> Deserialize<'de> for GenerationId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Self::new(id).map_err(serde::de::Error::custom)
    }
}

impl GenerationId {
    /// Validates and constructs a path-safe GenerationId.
    ///
    /// Accepts characters [a-zA-Z0-9._-], length 1..=64,
    /// rejecting ".", "..", and any character containing path separators.
    pub fn new(id: impl Into<String>) -> Result<Self, GenerationError> {
        let s = id.into();
        Self::validate_str(&s)?;
        Ok(Self(s))
    }

    /// Generates a new unique GenerationId with timestamp and random suffix.
    pub fn generate() -> Self {
        let ts = Utc::now().format("%Y%m%d%H%M%S").to_string();
        let rand_val: u32 = rand::random();
        let id_str = format!("gen-{ts}-{rand_val:08x}");
        Self(id_str)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate_str(s: &str) -> Result<(), GenerationError> {
        if s.is_empty() {
            return Err(GenerationError::InvalidId("ID cannot be empty".to_string()));
        }
        if s.len() > 64 {
            return Err(GenerationError::InvalidId(format!(
                "ID exceeds 64 characters (len={})",
                s.len()
            )));
        }
        if s == "." || s == ".." {
            return Err(GenerationError::InvalidId(
                r#"ID cannot be "." or "..""#.to_string(),
            ));
        }
        // SPEC C: `.withdrawn` is a tombstone marker, never an ID — accepting
        // it would let a tombstone be mistaken for a live generation slot.
        if s.ends_with(WITHDRAWN_SUFFIX) {
            return Err(GenerationError::InvalidId(format!(
                "ID cannot carry the tombstone suffix '{WITHDRAWN_SUFFIX}': {s}"
            )));
        }
        for c in s.chars() {
            let valid = c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-';
            if !valid {
                return Err(GenerationError::InvalidId(format!(
                    "Invalid character '{c}' in generation ID: {s}"
                )));
            }
        }
        Ok(())
    }
}

impl fmt::Display for GenerationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for GenerationId {
    type Err = GenerationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// The kind of binary or data asset in a generation bundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Daemon,
    Core,
    Geoip,
    Geosite,
    Other(String),
}

/// An entry describing a single file in the generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactManifestEntry {
    /// Relative path inside the generation directory (e.g. "mihomo-cli", "mihomo", "Country.mmdb").
    pub relative_path: String,
    pub kind: ArtifactKind,
    /// Lowercase hex encoded SHA-256 hash.
    pub sha256: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_mode: Option<u32>,
}

impl ArtifactManifestEntry {
    pub fn new(
        relative_path: impl Into<String>,
        kind: ArtifactKind,
        sha256: impl Into<String>,
        size_bytes: u64,
        unix_mode: Option<u32>,
    ) -> Result<Self, GenerationError> {
        let rel = relative_path.into();
        validate_artifact_path(&rel)?;
        let hash = sha256.into().to_ascii_lowercase();
        validate_sha256_format(&rel, &hash)?;
        Ok(Self {
            relative_path: rel,
            kind,
            sha256: hash,
            size_bytes,
            unix_mode,
        })
    }
}

/// Metadata and artifact manifest for a specific generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub generation_id: GenerationId,
    pub created_at: String,
    #[serde(default = "default_protocol_version")]
    pub protocol_version: u32,
    pub daemon: ArtifactManifestEntry,
    pub core: ArtifactManifestEntry,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_artifacts: Vec<ArtifactManifestEntry>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

fn default_schema_version() -> u32 {
    DEFAULT_SCHEMA_VERSION
}

fn default_protocol_version() -> u32 {
    DEFAULT_PROTOCOL_VERSION
}

impl GenerationManifest {
    pub fn new(
        generation_id: GenerationId,
        protocol_version: u32,
        daemon: ArtifactManifestEntry,
        core: ArtifactManifestEntry,
    ) -> Self {
        Self {
            schema_version: DEFAULT_SCHEMA_VERSION,
            generation_id,
            created_at: Utc::now().to_rfc3339(),
            protocol_version,
            daemon,
            core,
            extra_artifacts: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_extra_artifact(mut self, entry: ArtifactManifestEntry) -> Self {
        self.extra_artifacts.push(entry);
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// SPEC A/P2-3: baseline manifests are the only ones allowed to declare
    /// artifacts as absent at capture time.
    pub fn is_baseline(&self) -> bool {
        self.metadata
            .get("kind")
            .is_some_and(|kind| kind == BASELINE_KIND)
    }

    /// Relative paths this manifest declares as not existing at capture time.
    pub fn missing_paths(&self) -> HashSet<String> {
        self.metadata
            .get("missing")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|path| !path.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn all_entries(&self) -> Vec<&ArtifactManifestEntry> {
        let mut entries = Vec::with_capacity(2 + self.extra_artifacts.len());
        entries.push(&self.daemon);
        entries.push(&self.core);
        entries.extend(self.extra_artifacts.iter());
        entries
    }

    /// Performs full unified semantic validation of the manifest schema, versions,
    /// hashes, and guarantees path non-overlapping across entries.
    pub fn validate(&self) -> Result<(), GenerationError> {
        if self.schema_version != DEFAULT_SCHEMA_VERSION {
            return Err(GenerationError::UnsupportedSchemaVersion {
                version: self.schema_version,
            });
        }
        if self.protocol_version == 0 {
            return Err(GenerationError::UnsupportedProtocolVersion {
                version: self.protocol_version,
            });
        }

        // Validate daemon entry kind and path
        if self.daemon.kind != ArtifactKind::Daemon {
            return Err(GenerationError::MismatchedArtifactKind {
                path: self.daemon.relative_path.clone(),
                expected: ArtifactKind::Daemon,
                actual: self.daemon.kind.clone(),
            });
        }
        validate_artifact_path(&self.daemon.relative_path)?;
        validate_sha256_format(&self.daemon.relative_path, &self.daemon.sha256)?;

        // Validate core entry kind and path
        if self.core.kind != ArtifactKind::Core {
            return Err(GenerationError::MismatchedArtifactKind {
                path: self.core.relative_path.clone(),
                expected: ArtifactKind::Core,
                actual: self.core.kind.clone(),
            });
        }
        validate_artifact_path(&self.core.relative_path)?;
        validate_sha256_format(&self.core.relative_path, &self.core.sha256)?;

        // Ensure daemon and core relative paths do not overlap
        if self.daemon.relative_path == self.core.relative_path {
            return Err(GenerationError::DaemonCorePathOverlap(
                self.daemon.relative_path.clone(),
            ));
        }

        let mut seen_paths = HashSet::new();
        seen_paths.insert(self.daemon.relative_path.clone());
        seen_paths.insert(self.core.relative_path.clone());

        for extra in &self.extra_artifacts {
            validate_artifact_path(&extra.relative_path)?;
            validate_sha256_format(&extra.relative_path, &extra.sha256)?;

            if !seen_paths.insert(extra.relative_path.clone()) {
                return Err(GenerationError::DuplicateArtifactPath(
                    extra.relative_path.clone(),
                ));
            }
        }

        Ok(())
    }
}

/// State tracking the Active, Pending, and Previous generations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationState {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub active: Option<GenerationId>,
    pub pending: Option<GenerationId>,
    pub previous: Option<GenerationId>,
    pub updated_at: String,
}

impl Default for GenerationState {
    fn default() -> Self {
        Self {
            schema_version: DEFAULT_SCHEMA_VERSION,
            active: None,
            pending: None,
            previous: None,
            updated_at: Utc::now().to_rfc3339(),
        }
    }
}

/// RAII cross-process lock guard ensuring mutually exclusive generation operations.
pub struct GenerationLockGuard {
    _file: File,
    _path: PathBuf,
}

impl Drop for GenerationLockGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// Cross-platform inter-process file lock for generation state mutations.
pub struct GenerationLock;

impl GenerationLock {
    pub fn acquire(lock_path: &Path) -> Result<GenerationLockGuard, GenerationError> {
        if let Some(parent) = lock_path.parent() {
            crate::utils::ensure_dir_all_no_follow(parent).map_err(|error| {
                GenerationError::LockError(format!(
                    "Cannot create lock directory {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let deadline = Instant::now() + LOCK_TIMEOUT;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let file = crate::utils::open_file_create_no_follow(lock_path).map_err(|error| {
                GenerationError::LockError(format!(
                    "Cannot open lock file {}: {error}",
                    lock_path.display()
                ))
            })?;

            let fd = file.as_raw_fd();
            loop {
                let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                if ret == 0 {
                    return Ok(GenerationLockGuard {
                        _file: file,
                        _path: lock_path.to_path_buf(),
                    });
                }
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::WouldBlock {
                    return Err(GenerationError::LockError(format!(
                        "flock failed on {}: {}",
                        lock_path.display(),
                        err
                    )));
                }
                if Instant::now() >= deadline {
                    return Err(GenerationError::LockError(format!(
                        "Generation lock timeout after {}s on {}",
                        LOCK_TIMEOUT.as_secs(),
                        lock_path.display()
                    )));
                }
                std::thread::sleep(LOCK_POLL_INTERVAL);
            }
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            loop {
                match OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    // share_mode(0) denies read/write/delete sharing while this
                    // handle is alive.
                    .share_mode(0)
                    .open(lock_path)
                {
                    Ok(file) => {
                        return Ok(GenerationLockGuard {
                            _file: file,
                            _path: lock_path.to_path_buf(),
                        });
                    }
                    Err(err) if is_lock_contention(&err) => {
                        if Instant::now() >= deadline {
                            return Err(GenerationError::LockError(format!(
                                "Generation lock timeout after {}s on {}",
                                LOCK_TIMEOUT.as_secs(),
                                lock_path.display()
                            )));
                        }
                        std::thread::sleep(LOCK_POLL_INTERVAL);
                    }
                    Err(err) => {
                        return Err(GenerationError::LockError(format!(
                            "Cannot open lock file {}: {}",
                            lock_path.display(),
                            err
                        )));
                    }
                }
            }
        }

        #[cfg(all(not(unix), not(windows)))]
        {
            loop {
                match OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(lock_path)
                {
                    Ok(file) => {
                        return Ok(GenerationLockGuard {
                            _file: file,
                            _path: lock_path.to_path_buf(),
                        });
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        if Instant::now() >= deadline {
                            return Err(GenerationError::LockError(format!(
                                "Generation lock timeout after {}s on {}",
                                LOCK_TIMEOUT.as_secs(),
                                lock_path.display()
                            )));
                        }
                        std::thread::sleep(LOCK_POLL_INTERVAL);
                    }
                    Err(err) => {
                        return Err(GenerationError::LockError(format!(
                            "Cannot create lock file {}: {}",
                            lock_path.display(),
                            err
                        )));
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
fn is_lock_contention(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::AlreadyExists | io::ErrorKind::WouldBlock
    ) || matches!(err.raw_os_error(), Some(32 | 33))
}

/// 三态 pending 观测——供 `doctor`/`status` 等只读诊断面消费。
/// 代际信任根（`root:root 0750`）以当前身份读不动（EACCES）时是「未知」，
/// 绝不等同「无 pending」。
#[derive(Debug)]
pub enum PendingObservation {
    /// 状态可读且确认无 pending。
    Applied,
    /// 状态可读且存在已暂存的 pending 代际。
    HasPending(GenerationId),
    /// 状态不可读（如身份不足的 EACCES），pending 事实无法判定。
    Undeterminable(GenerationError),
}

/// Storage manager for upgrade generations and state tracking under a root directory.
#[derive(Debug, Clone)]
pub struct GenerationStore {
    root_dir: PathBuf,
}

impl GenerationStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
        }
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn generations_dir(&self) -> PathBuf {
        self.root_dir.join("generations")
    }

    pub fn state_file_path(&self) -> PathBuf {
        self.root_dir.join(STATE_FILE_NAME)
    }

    pub fn lock_file_path(&self) -> PathBuf {
        self.root_dir.join(LOCK_FILE_NAME)
    }

    pub fn generation_dir(&self, id: &GenerationId) -> PathBuf {
        self.generations_dir().join(id.as_str())
    }

    pub fn manifest_path(&self, id: &GenerationId) -> PathBuf {
        self.generation_dir(id).join(MANIFEST_FILE_NAME)
    }

    /// Acquires the cross-process generation lock.
    pub fn acquire_lock(&self) -> Result<GenerationLockGuard, GenerationError> {
        GenerationLock::acquire(&self.lock_file_path())
    }

    /// Initializes storage directories under the trust-root contract
    /// (root:root 0750 or the platform equivalent — never umask-dependent).
    pub fn init(&self) -> Result<(), GenerationError> {
        crate::utils::ensure_generation_trust_root(&self.root_dir)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        crate::utils::ensure_dir_all_no_follow(&self.generations_dir())
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        #[cfg(unix)]
        crate::utils::set_directory_mode_no_follow(&self.generations_dir(), 0o750)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        Ok(())
    }

    /// Reads the current GenerationState without acquiring the lock.
    /// If state.json does not exist, returns Default.
    pub fn read_state(&self) -> Result<GenerationState, GenerationError> {
        let path = self.state_file_path();
        let mut file = match crate::utils::open_regular_file_no_follow(&path) {
            Ok(file) => file,
            Err(error) if crate::utils::is_not_found_error(&error) => {
                return Ok(GenerationState::default());
            }
            Err(error) => {
                if let Some(io_error) = error.root_cause().downcast_ref::<io::Error>() {
                    return Err(GenerationError::Io(io::Error::new(
                        io_error.kind(),
                        io_error.to_string(),
                    )));
                }
                return Err(GenerationError::Serialization(error.to_string()));
            }
        };

        let metadata = file.metadata()?;
        if metadata.len() > MAX_STATE_BYTES {
            return Err(GenerationError::StateSizeExceeded {
                actual: metadata.len(),
                max: MAX_STATE_BYTES,
            });
        }

        let mut content = String::new();
        (&mut file)
            .take(MAX_STATE_BYTES + 1)
            .read_to_string(&mut content)?;
        if content.len() as u64 > MAX_STATE_BYTES {
            return Err(GenerationError::StateSizeExceeded {
                actual: content.len() as u64,
                max: MAX_STATE_BYTES,
            });
        }

        let state: GenerationState = serde_json::from_str(&content)
            .map_err(|e| GenerationError::Serialization(e.to_string()))?;
        if state.schema_version != DEFAULT_SCHEMA_VERSION {
            return Err(GenerationError::UnsupportedSchemaVersion {
                version: state.schema_version,
            });
        }
        Ok(state)
    }

    /// 只读 pending 观测。与 `if let Ok(...)` 调用点不同，
    /// 读取失败保留为 `Undeterminable`，不折叠成「无 pending」。
    pub fn observe_pending(&self) -> PendingObservation {
        match self.read_state() {
            Ok(state) => match state.pending {
                Some(id) => PendingObservation::HasPending(id),
                None => PendingObservation::Applied,
            },
            Err(error) => PendingObservation::Undeterminable(error),
        }
    }

    /// Atomically writes the GenerationState to state.json.
    pub fn write_state(&self, state: &GenerationState) -> Result<(), GenerationError> {
        self.init()?;
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| GenerationError::Serialization(e.to_string()))?;
        crate::utils::atomic_write_bytes_no_follow(&self.state_file_path(), json.as_bytes(), 0o644)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        #[cfg(unix)]
        crate::utils::sync_directory_no_follow(&self.root_dir).map_err(|error| {
            GenerationError::Io(io::Error::other(format!(
                "提交结果不确定，重新只读检查/重入: {error}"
            )))
        })?;
        Ok(())
    }

    /// Stages a new generation ID as Pending after validating its manifest and artifacts.
    /// Synchronized via inter-process lock across read-validate-modify-write.
    pub fn stage_pending(&self, id: GenerationId) -> Result<GenerationState, GenerationError> {
        let lock = self.acquire_lock()?;
        self.stage_pending_with_lock(&lock, id)
    }

    /// Stages a generation while the caller holds this store's generation lock.
    pub fn stage_pending_with_lock(
        &self,
        _lock: &GenerationLockGuard,
        id: GenerationId,
    ) -> Result<GenerationState, GenerationError> {
        self.stage_pending_with_baseline_with_lock(_lock, id, None)
    }

    /// SPEC A/P2-2: stages the pending generation **and** binds the rollback
    /// baseline in one state write, so a crash can never leave a pending
    /// generation without `previous = baseline` (the RMW window is gone).
    pub fn stage_pending_with_baseline_with_lock(
        &self,
        _lock: &GenerationLockGuard,
        id: GenerationId,
        baseline: Option<&GenerationId>,
    ) -> Result<GenerationState, GenerationError> {
        self.validate_generation(&id)?;
        let mut state = self.read_state()?;
        state.pending = Some(id);
        if let Some(baseline) = baseline {
            state.previous = Some(baseline.clone());
        }
        state.updated_at = Utc::now().to_rfc3339();
        self.write_state(&state)?;
        Ok(state)
    }

    /// Commits the Pending generation into Active, shifting existing Active to Previous.
    /// Synchronized via inter-process lock across read-validate-modify-write.
    pub fn commit_active(&self) -> Result<GenerationState, GenerationError> {
        let _guard = self.acquire_lock()?;
        self.commit_active_with_lock(&_guard)
    }

    /// SPEC D: commit variant that accepts an already-held lock guard,
    /// eliminating the double-lock deadlock (R5 anti-pattern).
    pub fn commit_active_with_lock(
        &self,
        _lock: &GenerationLockGuard,
    ) -> Result<GenerationState, GenerationError> {
        let mut state = self.read_state()?;
        let pending = state
            .pending
            .take()
            .ok_or(GenerationError::NoPendingGeneration)?;

        // Re-validate pending generation artifacts before commit
        self.validate_generation(&pending)?;

        state.previous = state.active.take();
        state.active = Some(pending);
        state.updated_at = Utc::now().to_rfc3339();
        self.write_state(&state)?;
        Ok(state)
    }

    /// SPEC D: CAS commit — compares expected pending ID (snapshot at P1)
    /// against current state, refusing to commit if another writer changed it.
    ///
    /// SPEC P6: `previous` is the rollback baseline bound to this upgrade
    /// (not the historical active generation) — it is supplied by the caller
    /// from the verified `manifest.metadata.baseline`.
    pub fn commit_active_cas(
        &self,
        _lock: &GenerationLockGuard,
        expected_pending: &GenerationId,
        previous: Option<&GenerationId>,
    ) -> Result<GenerationState, GenerationError> {
        let mut state = self.read_state()?;
        let pending = state
            .pending
            .as_ref()
            .ok_or(GenerationError::NoPendingGeneration)?;
        if pending != expected_pending {
            return Err(GenerationError::GenerationMismatch {
                expected: expected_pending.clone(),
                actual: pending.clone(),
            });
        }
        let pending = state.pending.take().unwrap();
        self.validate_generation(&pending)?;
        state.previous = previous.cloned();
        state.active = Some(pending);
        state.updated_at = Utc::now().to_rfc3339();
        self.write_state(&state)?;
        Ok(state)
    }

    /// SPEC C: consumes the slots after a compensating rollback:
    /// `active = baseline`, `pending = None`, `previous = None`.
    pub fn compensate_with_lock(
        &self,
        _lock: &GenerationLockGuard,
        baseline: Option<&GenerationId>,
    ) -> Result<GenerationState, GenerationError> {
        let mut state = self.read_state()?;
        state.active = baseline.cloned();
        state.pending = None;
        state.previous = None;
        state.updated_at = Utc::now().to_rfc3339();
        self.write_state(&state)?;
        Ok(state)
    }

    /// Rolls back Active to Previous generation.
    /// Synchronized via inter-process lock across read-validate-modify-write.
    pub fn rollback(&self) -> Result<GenerationState, GenerationError> {
        let _guard = self.acquire_lock()?;
        self.rollback_with_lock(&_guard)
    }

    /// SPEC D: rollback variant that accepts an already-held lock guard.
    pub fn rollback_with_lock(
        &self,
        _lock: &GenerationLockGuard,
    ) -> Result<GenerationState, GenerationError> {
        let mut state = self.read_state()?;
        let previous = state
            .previous
            .take()
            .ok_or(GenerationError::NoPreviousGeneration)?;

        // Re-validate previous generation artifacts before rollback
        self.validate_generation(&previous)?;

        state.active = Some(previous);
        state.pending = None;
        state.updated_at = Utc::now().to_rfc3339();
        self.write_state(&state)?;
        Ok(state)
    }

    /// Clears the Pending generation pointer without affecting files.
    /// Synchronized via inter-process lock.
    pub fn clear_pending(&self) -> Result<GenerationState, GenerationError> {
        let _guard = self.acquire_lock()?;
        self.clear_pending_with_lock(&_guard)
    }

    /// SPEC D: clear_pending variant that accepts an already-held lock guard.
    pub fn clear_pending_with_lock(
        &self,
        _lock: &GenerationLockGuard,
    ) -> Result<GenerationState, GenerationError> {
        let mut state = self.read_state()?;
        state.pending = None;
        state.updated_at = Utc::now().to_rfc3339();
        self.write_state(&state)?;
        Ok(state)
    }

    /// Writes a manifest into the corresponding generation directory atomically after validation.
    pub fn write_manifest(&self, manifest: &GenerationManifest) -> Result<(), GenerationError> {
        manifest.validate()?;
        let gen_dir = self.generation_dir(&manifest.generation_id);
        crate::utils::ensure_dir_all_no_follow(&gen_dir)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;

        let json = serde_json::to_string_pretty(manifest)
            .map_err(|e| GenerationError::Serialization(e.to_string()))?;
        let manifest_file = self.manifest_path(&manifest.generation_id);
        crate::utils::atomic_write_bytes_no_follow(&manifest_file, json.as_bytes(), 0o644)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        Ok(())
    }

    /// Reads, deserializes, and validates a GenerationManifest from a generation directory.
    pub fn read_manifest(&self, id: &GenerationId) -> Result<GenerationManifest, GenerationError> {
        let path = self.manifest_path(id);
        self.read_manifest_from(id, path)
    }

    /// SPEC C: reads the manifest of a withdrawn generation (tombstone
    /// directory) — used by the retry gate and tombstone retirement.
    pub fn read_withdrawn_manifest(
        &self,
        id: &GenerationId,
    ) -> Result<GenerationManifest, GenerationError> {
        let path = self.withdrawn_path(id).join(MANIFEST_FILE_NAME);
        self.read_manifest_from(id, path)
    }

    fn read_manifest_from(
        &self,
        id: &GenerationId,
        path: PathBuf,
    ) -> Result<GenerationManifest, GenerationError> {
        if !path.exists() {
            return Err(GenerationError::ManifestNotFound(path));
        }

        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(GenerationError::SymlinkForbidden(path));
        }
        if !metadata.file_type().is_file() {
            return Err(GenerationError::NotRegularFile(path));
        }

        if metadata.len() > MAX_MANIFEST_BYTES {
            return Err(GenerationError::ManifestSizeExceeded {
                actual: metadata.len(),
                max: MAX_MANIFEST_BYTES,
            });
        }

        let mut file = crate::utils::open_regular_file_no_follow(&path)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        let mut content = String::new();
        (&mut file)
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_string(&mut content)?;
        if content.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(GenerationError::ManifestSizeExceeded {
                actual: content.len() as u64,
                max: MAX_MANIFEST_BYTES,
            });
        }

        let manifest: GenerationManifest = serde_json::from_str(&content)
            .map_err(|e| GenerationError::ManifestCorrupted(e.to_string()))?;

        if &manifest.generation_id != id {
            return Err(GenerationError::GenerationMismatch {
                expected: id.clone(),
                actual: manifest.generation_id,
            });
        }

        manifest.validate()?;
        Ok(manifest)
    }

    /// Fully validates a generation:
    /// 1. Ensures manifest exists, matches ID, and passes semantic validation.
    /// 2. Ensures generation directory is not a symlink.
    /// 3. Ensures all intermediate directories and artifact paths are NOT symlinks, and are regular files.
    /// 4. Ensures artifact file sizes and SHA-256 checksums match manifest.
    /// 5. On Unix: ensures unix_mode permissions match when specified.
    pub fn validate_generation(
        &self,
        id: &GenerationId,
    ) -> Result<GenerationManifest, GenerationError> {
        let gen_dir = self.generation_dir(id);
        if !gen_dir.exists() {
            return Err(GenerationError::ManifestNotFound(self.manifest_path(id)));
        }

        let gen_dir_meta = fs::symlink_metadata(&gen_dir)?;
        if gen_dir_meta.file_type().is_symlink() {
            return Err(GenerationError::SymlinkForbidden(gen_dir));
        }
        if !gen_dir_meta.is_dir() {
            return Err(GenerationError::NotRegularFile(gen_dir));
        }

        let manifest = self.read_manifest(id)?;

        // SPEC A/P2-3: only a baseline manifest may declare artifacts that
        // did not exist at capture time (metadata `missing`). The exemption is
        // keyed on the baseline marker, so a pending/install manifest cannot use
        // it to skip byte verification.
        let missing = if manifest.is_baseline() {
            manifest.missing_paths()
        } else {
            HashSet::new()
        };

        for entry in manifest.all_entries() {
            if missing.contains(entry.relative_path.as_str()) {
                // A declared-missing slot must be genuinely absent: no phantom
                // bytes, and the "did not exist" state stays verifiable.
                let absent_path = gen_dir.join(&entry.relative_path);
                match fs::symlink_metadata(&absent_path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(GenerationError::Io(error)),
                    Ok(_) => {
                        return Err(GenerationError::ManifestCorrupted(format!(
                            "baseline {} declares '{}' as missing but the file exists",
                            id, entry.relative_path
                        )))
                    }
                }
                continue;
            }
            validate_artifact_path(&entry.relative_path)?;

            // Verify that every component along the path inside gen_dir contains no symlinks
            let mut curr = gen_dir.clone();
            let rel_path = Path::new(&entry.relative_path);
            let components: Vec<_> = rel_path.components().collect();

            for (idx, comp) in components.iter().enumerate() {
                match comp {
                    Component::Normal(c) => {
                        curr.push(c);
                        let comp_meta = fs::symlink_metadata(&curr).map_err(|e| {
                            if e.kind() == io::ErrorKind::NotFound {
                                GenerationError::ArtifactMissing {
                                    path: gen_dir.join(&entry.relative_path),
                                    kind: entry.kind.clone(),
                                }
                            } else {
                                GenerationError::Io(e)
                            }
                        })?;

                        if comp_meta.file_type().is_symlink() {
                            return Err(GenerationError::SymlinkForbidden(curr));
                        }

                        let is_last = idx == components.len() - 1;
                        if is_last {
                            if !comp_meta.file_type().is_file() {
                                return Err(GenerationError::NotRegularFile(curr));
                            }
                        } else if !comp_meta.file_type().is_dir() {
                            return Err(GenerationError::NotRegularFile(curr));
                        }
                    }
                    Component::CurDir => {}
                    _ => {
                        return Err(GenerationError::InvalidPath(format!(
                            "Unsafe path component in '{}'",
                            entry.relative_path
                        )));
                    }
                }
            }

            let artifact_path = gen_dir.join(&entry.relative_path);
            let meta = fs::symlink_metadata(&artifact_path)?;

            if meta.len() != entry.size_bytes {
                return Err(GenerationError::ArtifactSizeMismatch {
                    path: artifact_path,
                    expected: entry.size_bytes,
                    actual: meta.len(),
                });
            }

            let calculated_hash = calculate_sha256(&artifact_path)?;
            if !calculated_hash.eq_ignore_ascii_case(&entry.sha256) {
                return Err(GenerationError::ArtifactHashMismatch {
                    path: artifact_path,
                    expected: entry.sha256.clone(),
                    actual: calculated_hash,
                });
            }

            #[cfg(unix)]
            if let Some(expected_mode) = entry.unix_mode {
                use std::os::unix::fs::PermissionsExt;
                let actual_mode = meta.permissions().mode() & 0o7777;
                let expected_masked = expected_mode & 0o7777;
                if actual_mode != expected_masked {
                    return Err(GenerationError::ArtifactPermissionMismatch {
                        path: artifact_path,
                        expected: expected_masked,
                        actual: actual_mode,
                    });
                }
            }
        }

        Ok(manifest)
    }

    /// Cleans up orphan generation directories that are not in active, pending, or previous slots,
    /// keeping up to `keep_limit` extra non-referenced generation directories (ordered by creation/name).
    /// Synchronized via generation lock.
    pub fn cleanup_old_generations(
        &self,
        keep_limit: usize,
    ) -> Result<Vec<GenerationId>, GenerationError> {
        let guard = self.acquire_lock()?;
        self.cleanup_old_generations_with_lock(&guard, keep_limit)
    }

    /// SPEC D: GC variant for callers already holding the generation lock
    /// (the apply pipeline) — no second acquisition, no self-deadlock.
    pub fn cleanup_old_generations_with_lock(
        &self,
        _lock: &GenerationLockGuard,
        keep_limit: usize,
    ) -> Result<Vec<GenerationId>, GenerationError> {
        let state = self.read_state()?;
        let mut referenced = HashSet::new();
        if let Some(a) = &state.active {
            referenced.insert(a.clone());
        }
        if let Some(p) = &state.pending {
            referenced.insert(p.clone());
        }
        if let Some(pr) = &state.previous {
            referenced.insert(pr.clone());
        }

        let gen_parent = self.generations_dir();
        if !gen_parent.exists() {
            return Ok(Vec::new());
        }

        let mut unreferenced = Vec::new();
        for entry in fs::read_dir(&gen_parent)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                // SPEC N3: tombstones are excluded from orphan GC
                if name.ends_with(WITHDRAWN_SUFFIX) || entry.path().join(UNTRUSTED_MARKER).exists()
                {
                    continue;
                }
                if let Ok(id) = GenerationId::new(&name) {
                    if !referenced.contains(&id) {
                        unreferenced.push(id);
                    }
                }
            }
        }

        // Sort ascending by ID (timestamp-prefixed)
        unreferenced.sort();

        let to_remove_count = unreferenced.len().saturating_sub(keep_limit);
        let mut removed = Vec::new();

        for id in unreferenced.into_iter().take(to_remove_count) {
            let dir = self.generation_dir(&id);
            if dir.exists() {
                crate::utils::remove_path_no_follow(&dir)
                    .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
                removed.push(id);
            }
        }

        Ok(removed)
    }

    /// SPEC C: returns the tombstone path for a given generation ID.
    pub fn withdrawn_path(&self, id: &GenerationId) -> PathBuf {
        self.generations_dir()
            .join(format!("{}{WITHDRAWN_SUFFIX}", id.as_str()))
    }

    /// SPEC C: checks whether a tombstone exists for the given ID.
    pub fn has_withdrawn(&self, id: &GenerationId) -> bool {
        self.withdrawn_path(id).exists()
    }

    /// SPEC C: renames the generation directory to `<id>.withdrawn`
    /// using a no-replace semantic. If a tombstone already exists, returns
    /// `TombstoneCollision` (collision = refuse, ID reuse semantic undefined).
    pub fn withdraw_generation(&self, id: &GenerationId) -> Result<PathBuf, GenerationError> {
        let gen_dir = self.generation_dir(id);
        let tombstone = self.withdrawn_path(id);
        if !gen_dir.exists() {
            return Err(GenerationError::ManifestNotFound(self.manifest_path(id)));
        }
        // SPEC C/P2-6: publish with the platform no-replace primitive — a
        // pre-flight `exists()` check alone is a TOCTOU window that could clobber
        // an existing tombstone. Collision is classified from the primitive's
        // failure (destination present) and always refuses.
        #[cfg(unix)]
        let published = match crate::utils::rename_no_replace(&gen_dir, &tombstone) {
            Ok(()) => Ok(()),
            Err(_error) if tombstone.exists() => {
                return Err(GenerationError::TombstoneCollision {
                    id: id.clone(),
                    path: tombstone,
                });
            }
            Err(error) => Err(error.to_string()),
        };
        // Non-unix has no no-replace primitive; `fs::rename` cannot replace an
        // existing directory there, so the semantic is preserved.
        #[cfg(not(unix))]
        let published: Result<(), String> = if tombstone.exists() {
            Err("tombstone already exists".to_string())
        } else {
            fs::rename(&gen_dir, &tombstone).map_err(|error| error.to_string())
        };
        published.map_err(|error| {
            GenerationError::Io(io::Error::other(format!(
                "failed to rename {} to {}: {error}",
                gen_dir.display(),
                tombstone.display()
            )))
        })?;
        Ok(tombstone)
    }

    /// SPEC 裁决 A1: marker written into the pending directory only after a
    /// compensating rollback has been PROVEN. Its absence means the tombstone was
    /// published by an explicit discard (metadata-only revocation).
    pub fn mark_rollback_proven(&self, id: &GenerationId) -> Result<(), GenerationError> {
        let path = self.generation_dir(id).join(ROLLBACK_PROVEN_MARKER);
        crate::utils::atomic_write_bytes_no_follow(&path, b"proven\n", 0o644)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        Ok(())
    }

    /// True when the tombstone (or the still-present generation dir) carries the
    /// rollback proof marker.
    pub fn has_rollback_proof(&self, id: &GenerationId) -> bool {
        self.generation_dir(id)
            .join(ROLLBACK_PROVEN_MARKER)
            .exists()
            || self
                .withdrawn_path(id)
                .join(ROLLBACK_PROVEN_MARKER)
                .exists()
    }

    /// SPEC 裁决 A1: metadata-only revocation terminal state —
    /// `active = None, pending = None, previous = baseline` (the authenticated
    /// baseline is retained so the next candidate can still bind to it).
    pub fn consume_revoked_with_lock(
        &self,
        _lock: &GenerationLockGuard,
        baseline: Option<&GenerationId>,
    ) -> Result<GenerationState, GenerationError> {
        let mut state = self.read_state()?;
        state.active = None;
        state.pending = None;
        state.previous = baseline.cloned();
        state.updated_at = Utc::now().to_rfc3339();
        self.write_state(&state)?;
        Ok(state)
    }

    /// SPEC C: retires (removes) a tombstone after a successful retry.
    pub fn retire_withdrawn(&self, id: &GenerationId) -> Result<(), GenerationError> {
        let tombstone = self.withdrawn_path(id);
        if tombstone.exists() {
            crate::utils::remove_path_no_follow(&tombstone)
                .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        }
        Ok(())
    }

    /// SPEC P1-1: marks a generation as migrated (untrusted) evidence.
    pub fn mark_generation_untrusted(&self, id: &GenerationId) -> Result<(), GenerationError> {
        let path = self.generation_dir(id).join(UNTRUSTED_MARKER);
        crate::utils::atomic_write_bytes_no_follow(&path, b"migrated\n", 0o644)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        Ok(())
    }

    /// SPEC P1-1: a migrated generation can never authenticate a binding.
    pub fn is_generation_untrusted(&self, id: &GenerationId) -> bool {
        self.generation_dir(id).join(UNTRUSTED_MARKER).exists()
    }

    /// SPEC C/N3: scans for all withdrawn tombstone IDs.
    pub fn list_withdrawn(&self) -> Result<Vec<GenerationId>, GenerationError> {
        let gen_parent = self.generations_dir();
        if !gen_parent.exists() {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for entry in fs::read_dir(&gen_parent)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(id_str) = name.strip_suffix(WITHDRAWN_SUFFIX) {
                if let Ok(id) = GenerationId::new(id_str) {
                    result.push(id);
                }
            }
        }
        result.sort();
        Ok(result)
    }
}

/// SPEC A: describes a single artifact to be captured as part of a baseline.
#[derive(Debug, Clone)]
pub struct BaselineArtifact {
    pub relative_path: String,
    pub kind: ArtifactKind,
    pub source_path: PathBuf,
    pub unix_mode: Option<u32>,
}

/// SPEC A/P2-3: the single sentinel-entry constructor for artifacts a
/// baseline declares as absent (the schema still requires daemon/core slots).
fn baseline_sentinel(relative_path: &str, kind: &ArtifactKind) -> ArtifactManifestEntry {
    ArtifactManifestEntry::new(
        relative_path.to_string(),
        kind.clone(),
        EMPTY_SHA256.to_string(),
        0,
        None,
    )
    .expect("sentinel entry is valid")
}

/// SPEC A: captures the current install as a baseline generation.
///
/// Reads each artifact from its installed location, computes hash and size,
/// writes them into a new generation directory under the store, and returns
/// the generated baseline ID. Missing artifacts are recorded in the manifest
/// metadata as `missing=<comma-separated relative paths>`.
///
/// The caller must hold the generation lock to ensure atomicity with the
/// subsequent `stage_pending_with_lock` call.
pub fn capture_baseline(
    store: &GenerationStore,
    _lock: &GenerationLockGuard,
    artifacts: &[BaselineArtifact],
) -> Result<GenerationId, GenerationError> {
    let baseline_id = GenerationId::generate();
    let gen_dir = store.generation_dir(&baseline_id);
    crate::utils::ensure_dir_all_no_follow(&gen_dir)
        .map_err(|e| GenerationError::Io(io::Error::other(e.to_string())))?;

    let mut daemon_entry: Option<ArtifactManifestEntry> = None;
    let mut core_entry: Option<ArtifactManifestEntry> = None;
    let mut extra_entries: Vec<ArtifactManifestEntry> = Vec::new();
    let mut missing_paths: Vec<String> = Vec::new();

    // Declared relative paths, used as sentinels for artifacts that are absent
    // (a baseline manifest must still name the slot it recorded as missing).
    let declared_daemon_path = artifacts
        .iter()
        .find(|artifact| artifact.kind == ArtifactKind::Daemon)
        .map(|artifact| artifact.relative_path.clone());
    let declared_core_path = artifacts
        .iter()
        .find(|artifact| artifact.kind == ArtifactKind::Core)
        .map(|artifact| artifact.relative_path.clone());

    for artifact in artifacts {
        validate_artifact_path(&artifact.relative_path)?;
        let bytes = crate::utils::read_file_no_follow_limited_optional(
            &artifact.source_path,
            512 * 1024 * 1024,
        )
        .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        let Some(bytes) = bytes else {
            missing_paths.push(artifact.relative_path.clone());
            let entry = baseline_sentinel(&artifact.relative_path, &artifact.kind);
            match artifact.kind {
                ArtifactKind::Daemon => daemon_entry = Some(entry),
                ArtifactKind::Core => core_entry = Some(entry),
                _ => extra_entries.push(entry),
            }
            continue;
        };
        let sha256 = calculate_sha256_bytes(&bytes);
        let size = bytes.len() as u64;

        let dest = gen_dir.join(&artifact.relative_path);
        if let Some(parent) = dest.parent() {
            crate::utils::ensure_dir_all_no_follow(parent)
                .map_err(|e| GenerationError::Io(io::Error::other(e.to_string())))?;
        }
        atomic_write_file_safely(&dest, &bytes)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = artifact.unix_mode {
                let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(mode));
            }
        }

        let entry = ArtifactManifestEntry::new(
            artifact.relative_path.clone(),
            artifact.kind.clone(),
            sha256,
            size,
            artifact.unix_mode,
        )?;

        match artifact.kind {
            ArtifactKind::Daemon => daemon_entry = Some(entry),
            ArtifactKind::Core => core_entry = Some(entry),
            _ => extra_entries.push(entry),
        }
    }

    // A baseline may legitimately lack ANY artifact, including the daemon and core
    // (first install / broken install): every absent slot is named in `missing`
    // and carries the sentinel entry the manifest schema requires.
    let daemon = match daemon_entry {
        Some(entry) => entry,
        None => baseline_sentinel(
            declared_daemon_path.as_deref().unwrap_or("mihomo-cli"),
            &ArtifactKind::Daemon,
        ),
    };
    let core = match core_entry {
        Some(entry) => entry,
        None => baseline_sentinel(
            declared_core_path.as_deref().unwrap_or("mihomo"),
            &ArtifactKind::Core,
        ),
    };

    let mut manifest =
        GenerationManifest::new(baseline_id.clone(), DEFAULT_PROTOCOL_VERSION, daemon, core);
    for extra in extra_entries {
        manifest = manifest.with_extra_artifact(extra);
    }
    if !missing_paths.is_empty() {
        manifest = manifest.with_metadata("missing", missing_paths.join(","));
    }
    // SPEC A/P2-3: the baseline marker is what licenses the `missing`
    // exemption in validate_generation — it must not be inferable by others.
    manifest = manifest.with_metadata("kind", "baseline");

    store.write_manifest(&manifest)?;
    Ok(baseline_id)
}

/// SPEC P1-1: result of a legacy migration attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// No legacy data found — first install, nothing to migrate.
    FirstInstall,
    /// Migration completed successfully.
    Migrated,
    /// New root already exists and is authoritative — legacy data ignored.
    NewRootAuthoritative,
}

/// SPEC P1-1: migrates generation store from legacy runtime root to the
/// new independent trust root. Whitelist: only `state.json` and `generations/`.
///
/// 4-state reentry matrix:
/// - `no old, no new` = first install → `FirstInstall`
/// - `old exists, no new` = trigger migration → `Migrated`
/// - `new exists (with or without state)` = new root authoritative → `NewRootAuthoritative`
/// - `new exists but missing state + old has data` = error (must not misidentify as first install)
///
/// Atomicity: the new root is **built as a staging tree, fully self-validated,
/// fsynced and then published with a single no-replace rename**. Any failure
/// before that rename removes the staging tree and leaves the new root absent,
/// so a crashed migration can only re-enter as "old exists, no new".
///
/// Old originals are left in place as inert evidence (zero renaming).
/// Migration does NOT confer trust: migrated generations are marked before the
/// publish, so a partially migrated store can never authenticate a binding.
pub fn migrate_legacy_store(
    legacy_root: &Path,
    new_store: &GenerationStore,
) -> Result<MigrationOutcome, GenerationError> {
    let new_root = new_store.root_dir();
    let new_state = new_store.state_file_path();
    let new_generations = new_store.generations_dir();

    let legacy_state = legacy_root.join(STATE_FILE_NAME);
    let legacy_generations = legacy_root.join("generations");

    #[cfg(unix)]
    if legacy_root != new_root && fs::symlink_metadata(legacy_root).is_ok() {
        crate::utils::open_directory_no_follow(legacy_root)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
    }

    // SPEC P1-1: when the legacy store root *is* the new root (pre-
    // layout, store root = runtime root) the legacy paths alias our own files.
    // Copying them would self-copy and mark the live store untrusted.
    let has_legacy_state = fs::symlink_metadata(&legacy_state).is_ok() && legacy_state != new_state;
    let has_legacy_generations =
        fs::symlink_metadata(&legacy_generations).is_ok() && legacy_generations != new_generations;
    let has_legacy = has_legacy_state || has_legacy_generations;

    let has_new_state = new_state.exists();
    let mut has_new_root = new_root.exists();

    // an install path may have pre-created the trust root before
    // migration (old install plans did this with `install -d`). A completely
    // empty root has no authority and cannot be distinguished from "absent";
    // remove it atomically and continue. A non-empty root without state is still
    // a partial/corrupt scene and stays fail-closed.
    if has_legacy
        && has_new_root
        && !has_new_state
        && crate::utils::remove_empty_dir_no_follow(new_root).is_ok()
    {
        has_new_root = false;
    }

    // 4-state matrix
    match (has_legacy, has_new_root) {
        (false, false) => return Ok(MigrationOutcome::FirstInstall),
        (false, true) => return Ok(MigrationOutcome::NewRootAuthoritative),
        (true, true) => {
            if !has_new_state {
                // New root exists but missing state + old has data = error
                return Err(GenerationError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "new generation root {} exists but state.json is missing while legacy data exists at {}; \
                         refuse to misidentify as first install — manual repair required",
                        new_root.display(),
                        legacy_root.display()
                    ),
                )));
            }
            return Ok(MigrationOutcome::NewRootAuthoritative);
        }
        (true, false) => {}
    }

    // (true, false): build staging → validate → fsync → one no-replace publish.
    let parent = new_root.parent().ok_or_else(|| {
        GenerationError::InvalidPath(format!(
            "generation root {} has no parent directory",
            new_root.display()
        ))
    })?;
    crate::utils::ensure_dir_all_no_follow(parent)
        .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;

    let staging = staging_root_path(new_root);
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let dir = crate::utils::open_directory_no_follow(parent)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        let name = std::ffi::CString::new(staging.file_name().unwrap().as_bytes())
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o750) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    #[cfg(not(unix))]
    fs::create_dir(&staging)?;
    let built = crate::utils::ensure_generation_trust_root(&staging)
        .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))
        .and_then(|()| {
            build_migration_staging(
                &legacy_state,
                &legacy_generations,
                &staging,
                has_legacy_state,
                has_legacy_generations,
            )
        })
        .and_then(|()| validate_migration_staging(&staging))
        .and_then(|()| sync_directory_tree(&staging));
    if let Err(error) = built {
        let _ = crate::utils::remove_path_no_follow(&staging);
        return Err(error);
    }

    if let Err(error) = crate::utils::rename_no_replace(&staging, new_root) {
        let _ = crate::utils::remove_path_no_follow(&staging);
        // A concurrent migration may have published a complete root first.
        if new_state.exists() {
            return Ok(MigrationOutcome::NewRootAuthoritative);
        }
        return Err(GenerationError::Io(io::Error::other(format!(
            "failed to publish migrated generation root {}: {error}",
            new_root.display()
        ))));
    }

    crate::utils::sync_directory_no_follow(parent).map_err(|error| {
        GenerationError::Io(io::Error::other(format!(
            "迁移发布状态可能不确定，重新只读检查/重入: {error}"
        )))
    })?;
    // Old originals left in place — zero renaming, zero deletion.
    Ok(MigrationOutcome::Migrated)
}

/// Unique sibling staging path for the migration publish (same filesystem, so the
/// final publish is a single rename).
fn staging_root_path(new_root: &Path) -> PathBuf {
    let name = new_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("generations");
    let suffix: u64 = rand::random();
    new_root.with_file_name(format!(".{name}.migrating-{suffix:016x}"))
}

/// Copies the legacy whitelist into the staging root and marks every migrated
/// generation untrusted **before** anyone can publish it.
fn build_migration_staging(
    legacy_state: &Path,
    legacy_generations: &Path,
    staging: &Path,
    has_legacy_state: bool,
    has_legacy_generations: bool,
) -> Result<(), GenerationError> {
    if has_legacy_state {
        let content = crate::utils::read_file_no_follow_limited(legacy_state, MAX_STATE_BYTES)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
        let state: GenerationState = serde_json::from_slice(&content).map_err(|error| {
            GenerationError::Serialization(format!(
                "legacy state.json at {} is not valid: {error}",
                legacy_state.display()
            ))
        })?;
        if state.schema_version != DEFAULT_SCHEMA_VERSION {
            return Err(GenerationError::UnsupportedSchemaVersion {
                version: state.schema_version,
            });
        }
        atomic_write_file_safely(&staging.join(STATE_FILE_NAME), &content)?;
    }

    let staging_generations = staging.join("generations");
    crate::utils::ensure_dir_all_no_follow(&staging_generations)
        .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;

    if !has_legacy_generations {
        return Ok(());
    }
    copy_legacy_tree(legacy_generations, &staging_generations)?;
    Ok(())
}

/// Full self-consistency check of the staging tree: state parses and every slot
/// it references exists with a manifest that matches its directory.
fn validate_migration_staging(staging: &Path) -> Result<(), GenerationError> {
    let state_path = staging.join(STATE_FILE_NAME);
    if !state_path.exists() {
        return Ok(());
    }
    let content = crate::utils::read_file_no_follow_limited(&state_path, MAX_STATE_BYTES)
        .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
    let state: GenerationState = serde_json::from_slice(&content).map_err(|error| {
        GenerationError::Serialization(format!(
            "staged state.json at {} is not valid: {error}",
            state_path.display()
        ))
    })?;
    for id in [state.active, state.pending, state.previous]
        .into_iter()
        .flatten()
    {
        let dir = staging.join("generations").join(id.as_str());
        if !dir.is_dir() {
            return Err(GenerationError::Serialization(format!(
                "staged migration is incomplete: state references generation {id} but its directory is missing"
            )));
        }
        let manifest_path = dir.join(MANIFEST_FILE_NAME);
        let manifest_bytes = crate::utils::read_file_no_follow_limited(&manifest_path, 1024 * 1024)
            .map_err(|error| {
                GenerationError::Serialization(format!(
                    "staged migration is incomplete: {id} manifest unreadable ({error})"
                ))
            })?;
        let manifest: GenerationManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|error| {
                GenerationError::Serialization(format!(
                    "staged manifest for {id} is invalid: {error}"
                ))
            })?;
        if manifest.generation_id != id {
            return Err(GenerationError::GenerationMismatch {
                expected: id,
                actual: manifest.generation_id,
            });
        }
    }
    Ok(())
}

/// fsync every directory in the staging tree so the single rename cannot expose
/// unflushed directory entries after a crash.
fn sync_directory_tree(path: &Path) -> Result<(), GenerationError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_directory_tree(&entry.path())?;
        }
    }
    crate::utils::sync_directory_no_follow(path)
        .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
    Ok(())
}

#[cfg(unix)]
fn copy_legacy_tree(src: &Path, dst: &Path) -> Result<(), GenerationError> {
    fn copy(dir: &File, dst: &Path, top: bool) -> Result<(), GenerationError> {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        let dup = unsafe { libc::dup(dir.as_raw_fd()) };
        if dup < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let stream = unsafe { libc::fdopendir(dup) };
        if stream.is_null() {
            unsafe {
                libc::close(dup);
            }
            return Err(io::Error::last_os_error().into());
        }
        struct Dir(*mut libc::DIR);
        impl Drop for Dir {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Dir(stream);
        loop {
            // readdir errors are checked via errno, not treated as end-of-directory.
            #[cfg(any(target_os = "linux", target_os = "android"))]
            unsafe {
                *libc::__errno_location() = 0;
            }
            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            unsafe {
                *libc::__error() = 0;
            }
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                let err = io::Error::last_os_error();
                if err.raw_os_error().unwrap_or(0) != 0 {
                    return Err(err.into());
                }
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error().into());
            }
            let mut file = unsafe { File::from_raw_fd(fd) };
            let metadata = file.metadata()?;
            let name_os = std::ffi::OsStr::from_bytes(name.to_bytes());
            let target = dst.join(name_os);
            if metadata.is_dir() {
                crate::utils::ensure_dir_all_no_follow(&target)
                    .map_err(|e| GenerationError::Io(io::Error::other(e.to_string())))?;
                copy(&file, &target, false)?;
                let name = name_os.to_string_lossy();
                let id = name.strip_suffix(WITHDRAWN_SUFFIX).unwrap_or(&name);
                if top && GenerationId::new(id).is_ok() {
                    atomic_write_file_safely(&target.join(UNTRUSTED_MARKER), b"migrated\n")?;
                }
            } else if metadata.is_file() {
                use std::os::unix::fs::MetadataExt;
                let limit = if name.to_bytes() == MANIFEST_FILE_NAME.as_bytes() {
                    1024 * 1024
                } else {
                    512 * 1024 * 1024
                };
                if metadata.nlink() != 1 || metadata.len() > limit {
                    return Err(io::Error::other("unsafe or oversized legacy file").into());
                }
                let mut bytes = Vec::new();
                (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
                if bytes.len() as u64 > limit {
                    return Err(io::Error::other("legacy file exceeds limit").into());
                }
                if name.to_bytes() == MANIFEST_FILE_NAME.as_bytes() {
                    serde_json::from_slice::<GenerationManifest>(&bytes).map_err(|e| {
                        GenerationError::Serialization(format!("invalid legacy manifest: {e}"))
                    })?;
                }
                atomic_write_file_safely(&target, &bytes)?;
            } else {
                return Err(io::Error::other("unsafe legacy file type").into());
            }
        }
        Ok(())
    }
    let fd = crate::utils::open_directory_no_follow(src)
        .map_err(|e| GenerationError::Io(io::Error::other(e.to_string())))?;
    let file = File::from(fd);
    copy(&file, dst, true)
}

#[cfg(not(unix))]
fn copy_legacy_tree(_src: &Path, _dst: &Path) -> Result<(), GenerationError> {
    Err(io::Error::other("safe legacy directory migration is unsupported on this platform").into())
}

/// Validates that an artifact relative path is strictly safe:
/// no absolute paths, no ".." parent components, no leading slashes/drives.
pub fn validate_artifact_path(relative_path: &str) -> Result<(), GenerationError> {
    if relative_path.is_empty() {
        return Err(GenerationError::InvalidPath(
            "Artifact path cannot be empty".to_string(),
        ));
    }
    if relative_path
        .split('/')
        .any(|part| part.is_empty() || part == ".")
        || relative_path.contains('\\')
    {
        return Err(GenerationError::InvalidPath(format!(
            "Non-canonical artifact path: {relative_path}"
        )));
    }
    let p = Path::new(relative_path);
    for component in p.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(GenerationError::InvalidPath(format!(
                    "Unsafe path component in '{relative_path}': forbidden traversal or absolute component"
                )));
            }
        }
    }
    Ok(())
}

/// Validates that a string is a 64-character lowercase hex encoded SHA-256 digest.
pub fn validate_sha256_format(path: &str, sha256: &str) -> Result<(), GenerationError> {
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(GenerationError::InvalidSha256Format {
            path: path.to_string(),
            actual: sha256.to_string(),
        });
    }
    Ok(())
}

/// Calculates the SHA-256 hex string of a file.
pub fn calculate_sha256(path: &Path) -> Result<String, io::Error> {
    let mut file = crate::utils::open_regular_file_no_follow(path)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

/// Calculates the SHA-256 hex string of in-memory bytes.
pub fn calculate_sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Atomically writes content to the given file path using a temporary sibling file and safe replace.
/// Never removes the target beforehand on any platform. If replacement fails, the target remains intact.
pub fn atomic_write_file_safely(target_path: &Path, content: &[u8]) -> Result<(), io::Error> {
    let parent = target_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing parent directory"))?;
    fs::create_dir_all(parent)?;

    let rand_suffix: u32 = rand::random();
    let file_name = target_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp_name = format!(".{file_name}.tmp.{rand_suffix:08x}");
    let tmp_path = parent.join(tmp_name);

    {
        let mut tmp_file = File::create(&tmp_path)?;
        tmp_file.write_all(content)?;
        tmp_file.sync_all()?;
    }

    replace_file_safely(&tmp_path, target_path)
}

/// Safely replaces target_path with source_path without deleting target_path beforehand.
/// Handles transient file-busy errors with bounded retry. Cleans up source on success or failure.
pub fn replace_file_safely(source_path: &Path, target_path: &Path) -> Result<(), io::Error> {
    #[cfg(unix)]
    {
        let parent = target_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Missing parent directory")
        })?;

        let res = fs::rename(source_path, target_path);
        if res.is_ok() {
            if let Ok(dir_file) = File::open(parent) {
                let _ = dir_file.sync_all();
            }
            return Ok(());
        }

        // Clean up source on failure
        let _ = fs::remove_file(source_path);
        res
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        let to_wide = |p: &Path| -> Vec<u16> {
            p.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        };

        if !target_path.exists() {
            // Target does not exist: simple rename with file-busy retry
            for attempt in 1..=MAX_RETRY_ATTEMPTS {
                match fs::rename(source_path, target_path) {
                    Ok(()) => return Ok(()),
                    Err(err) if is_file_busy_os_error(&err) && attempt < MAX_RETRY_ATTEMPTS => {
                        std::thread::sleep(RETRY_BACKOFF * attempt);
                    }
                    Err(err) => {
                        let _ = fs::remove_file(source_path);
                        return Err(err);
                    }
                }
            }
            let _ = fs::remove_file(source_path);
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!(
                    "File busy renaming to non-existent target {}",
                    target_path.display()
                ),
            ));
        }

        // Target exists: replace atomically using ReplaceFileW without deleting target first
        let target_wide = to_wide(target_path);
        let source_wide = to_wide(source_path);

        for attempt in 1..=MAX_RETRY_ATTEMPTS {
            let success = unsafe {
                windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
                    target_wide.as_ptr(),
                    source_wide.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };

            if success != 0 {
                return Ok(());
            }

            let err_code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            // ERROR_SHARING_VIOLATION = 32, ERROR_LOCK_VIOLATION = 33, ERROR_ACCESS_DENIED = 5
            if (err_code == 32 || err_code == 33 || err_code == 5) && attempt < MAX_RETRY_ATTEMPTS {
                std::thread::sleep(RETRY_BACKOFF * attempt);
                continue;
            }

            let _ = fs::remove_file(source_path);
            return Err(io::Error::from_raw_os_error(err_code as i32));
        }

        let _ = fs::remove_file(source_path);
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!(
                "File busy replacing target {} after {MAX_RETRY_ATTEMPTS} attempts",
                target_path.display()
            ),
        ))
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        let res = fs::rename(source_path, target_path);
        if res.is_err() {
            let _ = fs::remove_file(source_path);
        }
        res
    }
}

#[cfg(windows)]
fn is_file_busy_os_error(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::WouldBlock | io::ErrorKind::AlreadyExists
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generation_id_generation_and_validation() {
        let generated = GenerationId::generate();
        assert!(GenerationId::new(generated.as_str()).is_ok());

        assert!(GenerationId::new("gen-20260827-abcdef01").is_ok());
        assert!(GenerationId::new("v1.2.3_patch-1").is_ok());

        // Invalid IDs
        assert!(GenerationId::new("").is_err());
        assert!(GenerationId::new(".").is_err());
        assert!(GenerationId::new("..").is_err());
        assert!(GenerationId::new("a/b").is_err());
        assert!(GenerationId::new("a\\b").is_err());
        assert!(GenerationId::new("gen with space").is_err());
        assert!(GenerationId::new("../escaped").is_err());
        assert!(GenerationId::new("a".repeat(65)).is_err());
        // SPEC C: the tombstone suffix is a marker, never an ID.
        assert!(GenerationId::new(format!("gen-1{WITHDRAWN_SUFFIX}")).is_err());
        for id in ["../escaped", "..", "gen-1.withdrawn", "a/b"] {
            assert!(
                serde_json::from_str::<GenerationId>(&serde_json::json!(id).to_string()).is_err()
            );
        }
        assert!(serde_json::from_str::<GenerationId>("\"gen-valid\"").is_ok());
    }

    #[test]
    fn test_artifact_path_validation() {
        assert!(validate_artifact_path("mihomo-cli").is_ok());
        assert!(validate_artifact_path("bin/mihomo").is_ok());
        assert!(validate_artifact_path("data/geoip.dat").is_ok());

        // Unsafe paths
        assert!(validate_artifact_path("").is_err());
        assert!(validate_artifact_path("/etc/passwd").is_err());
        assert!(validate_artifact_path("../outside").is_err());
        assert!(validate_artifact_path("a/../../b").is_err());
        for path in ["./mihomo", "a/./b", "a//b", "a/", "a\\b"] {
            assert!(validate_artifact_path(path).is_err(), "{path}");
        }
    }

    #[test]
    fn migration_marks_withdrawn_evidence_untrusted() {
        let temp = TempDir::new().unwrap();
        let legacy = temp.path().join("legacy");
        let staging = temp.path().join("staging");
        fs::create_dir_all(legacy.join("gen-old.withdrawn")).unwrap();
        fs::create_dir_all(&staging).unwrap();
        build_migration_staging(&legacy.join("state.json"), &legacy, &staging, false, true)
            .unwrap();
        assert!(staging
            .join("generations/gen-old.withdrawn")
            .join(UNTRUSTED_MARKER)
            .is_file());
    }

    #[test]
    fn test_sha256_format_validation() {
        let valid_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(validate_sha256_format("file", valid_hash).is_ok());

        // Invalid lengths or characters
        assert!(validate_sha256_format("file", "short").is_err());
        assert!(validate_sha256_format("file", &"a".repeat(63)).is_err());
        assert!(validate_sha256_format("file", &"a".repeat(65)).is_err());
        assert!(validate_sha256_format(
            "file",
            "g3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        )
        .is_err());
    }

    #[test]
    fn test_manifest_semantic_validation_rejects_duplicate_paths() {
        let id = GenerationId::new("gen-dup").unwrap();
        let valid_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        let daemon = ArtifactManifestEntry::new(
            "mihomo-cli",
            ArtifactKind::Daemon,
            valid_hash,
            100,
            Some(0o755),
        )
        .unwrap();

        let core =
            ArtifactManifestEntry::new("mihomo", ArtifactKind::Core, valid_hash, 200, Some(0o755))
                .unwrap();

        let extra_dup = ArtifactManifestEntry::new(
            "mihomo", // duplicate of core
            ArtifactKind::Geoip,
            valid_hash,
            50,
            None,
        )
        .unwrap();

        let manifest = GenerationManifest::new(id, 1, daemon, core).with_extra_artifact(extra_dup);

        let err = manifest.validate().unwrap_err();
        match err {
            GenerationError::DuplicateArtifactPath(p) => assert_eq!(p, "mihomo"),
            other => panic!("Expected DuplicateArtifactPath, got {other:?}"),
        }
    }

    #[test]
    fn test_manifest_semantic_validation_rejects_daemon_core_overlap() {
        let id = GenerationId::new("gen-overlap").unwrap();
        let valid_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        let daemon = ArtifactManifestEntry::new(
            "same-binary",
            ArtifactKind::Daemon,
            valid_hash,
            100,
            Some(0o755),
        )
        .unwrap();

        let core = ArtifactManifestEntry::new(
            "same-binary",
            ArtifactKind::Core,
            valid_hash,
            100,
            Some(0o755),
        )
        .unwrap();

        let manifest = GenerationManifest::new(id, 1, daemon, core);
        let err = manifest.validate().unwrap_err();
        match err {
            GenerationError::DaemonCorePathOverlap(p) => assert_eq!(p, "same-binary"),
            other => panic!("Expected DaemonCorePathOverlap, got {other:?}"),
        }
    }

    #[test]
    fn test_manifest_semantic_validation_rejects_invalid_schema_or_protocol() {
        let id = GenerationId::new("gen-ver").unwrap();
        let valid_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

        let daemon =
            ArtifactManifestEntry::new("mihomo-cli", ArtifactKind::Daemon, valid_hash, 100, None)
                .unwrap();

        let core = ArtifactManifestEntry::new("mihomo", ArtifactKind::Core, valid_hash, 100, None)
            .unwrap();

        let mut manifest = GenerationManifest::new(id, 0, daemon, core);
        assert!(matches!(
            manifest.validate().unwrap_err(),
            GenerationError::UnsupportedProtocolVersion { .. }
        ));

        manifest.protocol_version = 1;
        manifest.schema_version = 999;
        assert!(matches!(
            manifest.validate().unwrap_err(),
            GenerationError::UnsupportedSchemaVersion { .. }
        ));
    }

    #[test]
    fn test_generation_lifecycle_and_validation() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();

        let id = GenerationId::new("gen-test-01").unwrap();
        let gen_dir = store.generation_dir(&id);
        fs::create_dir_all(&gen_dir).unwrap();

        let daemon_content = b"fake-daemon-binary-content-v1";
        let core_content = b"fake-core-binary-content-v1";
        let geo_content = b"fake-geoip-data-v1";

        let daemon_path = gen_dir.join("mihomo-cli");
        let core_path = gen_dir.join("mihomo");
        let geo_path = gen_dir.join("Country.mmdb");

        fs::write(&daemon_path, daemon_content).unwrap();
        fs::write(&core_path, core_content).unwrap();
        fs::write(&geo_path, geo_content).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&daemon_path, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&core_path, fs::Permissions::from_mode(0o755)).unwrap();
            fs::set_permissions(&geo_path, fs::Permissions::from_mode(0o644)).unwrap();
        }

        let daemon_entry = ArtifactManifestEntry::new(
            "mihomo-cli",
            ArtifactKind::Daemon,
            calculate_sha256_bytes(daemon_content),
            daemon_content.len() as u64,
            Some(0o755),
        )
        .unwrap();

        let core_entry = ArtifactManifestEntry::new(
            "mihomo",
            ArtifactKind::Core,
            calculate_sha256_bytes(core_content),
            core_content.len() as u64,
            Some(0o755),
        )
        .unwrap();

        let geo_entry = ArtifactManifestEntry::new(
            "Country.mmdb",
            ArtifactKind::Geoip,
            calculate_sha256_bytes(geo_content),
            geo_content.len() as u64,
            Some(0o644),
        )
        .unwrap();

        let manifest = GenerationManifest::new(id.clone(), 1, daemon_entry, core_entry)
            .with_extra_artifact(geo_entry)
            .with_metadata("target_os", "linux");

        store.write_manifest(&manifest).unwrap();

        // Validation succeeds
        let validated = store.validate_generation(&id).unwrap();
        assert_eq!(validated.generation_id, id);
        assert_eq!(validated.extra_artifacts.len(), 1);

        // Stage Pending
        let state = store.stage_pending(id.clone()).unwrap();
        assert_eq!(state.pending, Some(id.clone()));
        assert_eq!(state.active, None);

        // Commit Active
        let state = store.commit_active().unwrap();
        assert_eq!(state.active, Some(id.clone()));
        assert_eq!(state.pending, None);
        assert_eq!(state.previous, None);
    }

    #[cfg(unix)]
    #[test]
    fn test_unix_mode_permission_validation() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());

        let id = GenerationId::new("gen-perm-test").unwrap();
        let gen_dir = store.generation_dir(&id);
        fs::create_dir_all(&gen_dir).unwrap();

        let daemon_path = gen_dir.join("mihomo-cli");
        let core_path = gen_dir.join("mihomo");

        fs::write(&daemon_path, b"daemon").unwrap();
        fs::write(&core_path, b"core").unwrap();

        // Set actual mode to 0o644
        fs::set_permissions(&daemon_path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&core_path, fs::Permissions::from_mode(0o755)).unwrap();

        let daemon_entry = ArtifactManifestEntry::new(
            "mihomo-cli",
            ArtifactKind::Daemon,
            calculate_sha256_bytes(b"daemon"),
            6,
            Some(0o755), // Expected mode is 0o755
        )
        .unwrap();

        let core_entry = ArtifactManifestEntry::new(
            "mihomo",
            ArtifactKind::Core,
            calculate_sha256_bytes(b"core"),
            4,
            Some(0o755),
        )
        .unwrap();

        let manifest = GenerationManifest::new(id.clone(), 1, daemon_entry, core_entry);
        store.write_manifest(&manifest).unwrap();

        let err = store.validate_generation(&id).unwrap_err();
        match err {
            GenerationError::ArtifactPermissionMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, 0o755);
                assert_eq!(actual, 0o644);
            }
            other => panic!("Expected ArtifactPermissionMismatch, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_artifact_rejected() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());

        let id = GenerationId::new("gen-sym-test").unwrap();
        let gen_dir = store.generation_dir(&id);
        fs::create_dir_all(&gen_dir).unwrap();

        let outside_dir = temp.path().join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        let outside_daemon = outside_dir.join("daemon-real");
        fs::write(&outside_daemon, b"secret-binary").unwrap();

        // Create symlink inside generation pointing outside
        let sym_daemon = gen_dir.join("mihomo-cli");
        std::os::unix::fs::symlink(&outside_daemon, &sym_daemon).unwrap();

        let core_path = gen_dir.join("mihomo");
        fs::write(&core_path, b"core").unwrap();

        let daemon_entry = ArtifactManifestEntry::new(
            "mihomo-cli",
            ArtifactKind::Daemon,
            calculate_sha256_bytes(b"secret-binary"),
            13,
            None,
        )
        .unwrap();

        let core_entry = ArtifactManifestEntry::new(
            "mihomo",
            ArtifactKind::Core,
            calculate_sha256_bytes(b"core"),
            4,
            None,
        )
        .unwrap();

        let manifest = GenerationManifest::new(id.clone(), 1, daemon_entry, core_entry);
        store.write_manifest(&manifest).unwrap();

        let err = store.validate_generation(&id).unwrap_err();
        match err {
            GenerationError::SymlinkForbidden(p) => assert_eq!(p, sym_daemon),
            other => panic!("Expected SymlinkForbidden, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_intermediate_symlink_directory_rejected() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());

        let id = GenerationId::new("gen-symdir-test").unwrap();
        let gen_dir = store.generation_dir(&id);
        fs::create_dir_all(&gen_dir).unwrap();

        let outside_dir = temp.path().join("outside_bin");
        fs::create_dir_all(&outside_dir).unwrap();
        let outside_core = outside_dir.join("mihomo");
        fs::write(&outside_core, b"core-payload").unwrap();

        // Create symlink for "bin" subdirectory
        let bin_link = gen_dir.join("bin");
        std::os::unix::fs::symlink(&outside_dir, &bin_link).unwrap();

        let daemon_path = gen_dir.join("mihomo-cli");
        fs::write(&daemon_path, b"daemon").unwrap();

        let daemon_entry = ArtifactManifestEntry::new(
            "mihomo-cli",
            ArtifactKind::Daemon,
            calculate_sha256_bytes(b"daemon"),
            6,
            None,
        )
        .unwrap();

        let core_entry = ArtifactManifestEntry::new(
            "bin/mihomo",
            ArtifactKind::Core,
            calculate_sha256_bytes(b"core-payload"),
            12,
            None,
        )
        .unwrap();

        let manifest = GenerationManifest::new(id.clone(), 1, daemon_entry, core_entry);
        store.write_manifest(&manifest).unwrap();

        let err = store.validate_generation(&id).unwrap_err();
        match err {
            GenerationError::SymlinkForbidden(p) => assert_eq!(p, bin_link),
            other => panic!("Expected SymlinkForbidden on bin dir, got {other:?}"),
        }
    }

    #[test]
    fn test_corrupted_artifact_fails_validation() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());

        let id = GenerationId::new("gen-test-corrupt").unwrap();
        let gen_dir = store.generation_dir(&id);
        fs::create_dir_all(&gen_dir).unwrap();

        let daemon_content = b"daemon-content";
        let core_content = b"core-content";

        fs::write(gen_dir.join("mihomo-cli"), daemon_content).unwrap();
        fs::write(gen_dir.join("mihomo"), core_content).unwrap();

        let daemon_entry = ArtifactManifestEntry::new(
            "mihomo-cli",
            ArtifactKind::Daemon,
            calculate_sha256_bytes(daemon_content),
            daemon_content.len() as u64,
            None,
        )
        .unwrap();

        let core_entry = ArtifactManifestEntry::new(
            "mihomo",
            ArtifactKind::Core,
            calculate_sha256_bytes(core_content),
            core_content.len() as u64,
            None,
        )
        .unwrap();

        let manifest = GenerationManifest::new(id.clone(), 1, daemon_entry, core_entry);
        store.write_manifest(&manifest).unwrap();

        // Tamper with core file content
        fs::write(gen_dir.join("mihomo"), b"tampered-content").unwrap();

        let err = store.validate_generation(&id).unwrap_err();
        match err {
            GenerationError::ArtifactSizeMismatch { .. }
            | GenerationError::ArtifactHashMismatch { .. } => {}
            other => panic!("Expected hash or size mismatch error, got {other:?}"),
        }
    }

    #[test]
    fn test_stage_commit_rollback_state_transitions() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());

        // Setup Gen 1
        let id1 = GenerationId::new("gen-001").unwrap();
        let dir1 = store.generation_dir(&id1);
        fs::create_dir_all(&dir1).unwrap();
        fs::write(dir1.join("mihomo-cli"), b"d1").unwrap();
        fs::write(dir1.join("mihomo"), b"c1").unwrap();
        let m1 = GenerationManifest::new(
            id1.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"d1"),
                2,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"c1"),
                2,
                None,
            )
            .unwrap(),
        );
        store.write_manifest(&m1).unwrap();

        // Setup Gen 2
        let id2 = GenerationId::new("gen-002").unwrap();
        let dir2 = store.generation_dir(&id2);
        fs::create_dir_all(&dir2).unwrap();
        fs::write(dir2.join("mihomo-cli"), b"d2").unwrap();
        fs::write(dir2.join("mihomo"), b"c2").unwrap();
        let m2 = GenerationManifest::new(
            id2.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"d2"),
                2,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"c2"),
                2,
                None,
            )
            .unwrap(),
        );
        store.write_manifest(&m2).unwrap();

        // 1. Stage and commit Gen 1
        store.stage_pending(id1.clone()).unwrap();
        let state = store.commit_active().unwrap();
        assert_eq!(state.active, Some(id1.clone()));
        assert_eq!(state.pending, None);
        assert_eq!(state.previous, None);

        // 2. Stage and commit Gen 2
        store.stage_pending(id2.clone()).unwrap();
        let state = store.commit_active().unwrap();
        assert_eq!(state.active, Some(id2.clone()));
        assert_eq!(state.previous, Some(id1.clone()));
        assert_eq!(state.pending, None);

        // 3. Clear pending if another is staged
        let id3 = GenerationId::new("gen-003").unwrap();
        let dir3 = store.generation_dir(&id3);
        fs::create_dir_all(&dir3).unwrap();
        fs::write(dir3.join("mihomo-cli"), b"d3").unwrap();
        fs::write(dir3.join("mihomo"), b"c3").unwrap();
        let m3 = GenerationManifest::new(
            id3.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"d3"),
                2,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"c3"),
                2,
                None,
            )
            .unwrap(),
        );
        store.write_manifest(&m3).unwrap();

        store.stage_pending(id3.clone()).unwrap();
        assert_eq!(store.read_state().unwrap().pending, Some(id3));
        store.clear_pending().unwrap();
        assert_eq!(store.read_state().unwrap().pending, None);

        // 4. Rollback to Gen 1
        let state = store.rollback().unwrap();
        assert_eq!(state.active, Some(id1.clone()));
        assert_eq!(state.previous, None);
        assert_eq!(state.pending, None);

        // 5. Rollback when no previous exists fails cleanly
        assert!(store.rollback().is_err());
    }

    #[test]
    fn test_interprocess_state_lock_serialization() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());

        // Setup Gen 1
        let id1 = GenerationId::new("gen-lock-01").unwrap();
        let dir1 = store.generation_dir(&id1);
        fs::create_dir_all(&dir1).unwrap();
        fs::write(dir1.join("mihomo-cli"), b"d1").unwrap();
        fs::write(dir1.join("mihomo"), b"c1").unwrap();
        let m1 = GenerationManifest::new(
            id1.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"d1"),
                2,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"c1"),
                2,
                None,
            )
            .unwrap(),
        );
        store.write_manifest(&m1).unwrap();

        // Acquire lock explicitly
        let guard = store.acquire_lock().unwrap();

        // Attempting to stage from another thread while lock is held will wait / serialize
        let store_clone = store.clone();
        let id1_clone = id1.clone();
        let handle = std::thread::spawn(move || store_clone.stage_pending(id1_clone));

        std::thread::sleep(Duration::from_millis(150));
        assert!(!handle.is_finished());

        // Release lock
        drop(guard);

        let result = handle.join().unwrap();
        assert!(result.is_ok());
        assert_eq!(store.read_state().unwrap().pending, Some(id1));
    }

    #[test]
    fn test_atomic_write_file_safely_preserves_target_on_error() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("active-target.txt");
        fs::write(&target, b"original-content").unwrap();

        // Writing new content replaces it safely
        atomic_write_file_safely(&target, b"new-content").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new-content");
    }

    #[test]
    fn test_cleanup_keeps_active_pending_previous() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());

        let id1 = GenerationId::new("gen-101").unwrap();
        let id2 = GenerationId::new("gen-102").unwrap();
        let id3 = GenerationId::new("gen-103").unwrap();
        let id4 = GenerationId::new("gen-104").unwrap();

        for id in [&id1, &id2, &id3, &id4] {
            fs::create_dir_all(store.generation_dir(id)).unwrap();
        }

        // Active = id3, Previous = id2, Pending = id4 (id1 is orphan)
        let state = GenerationState {
            schema_version: 1,
            active: Some(id3.clone()),
            pending: Some(id4.clone()),
            previous: Some(id2.clone()),
            updated_at: Utc::now().to_rfc3339(),
        };
        store.write_state(&state).unwrap();

        // Cleanup with keep_limit = 0 should remove id1, but preserve referenced ones
        let removed = store.cleanup_old_generations(0).unwrap();
        assert_eq!(removed, vec![id1.clone()]);
        assert!(!store.generation_dir(&id1).exists());
        assert!(store.generation_dir(&id2).exists());
        assert!(store.generation_dir(&id3).exists());
        assert!(store.generation_dir(&id4).exists());
    }

    #[test]
    fn test_manifest_generation_id_mismatch() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());

        let real_id = GenerationId::new("gen-real").unwrap();
        let fake_id = GenerationId::new("gen-fake").unwrap();

        let dir = store.generation_dir(&real_id);
        fs::create_dir_all(&dir).unwrap();

        let m = GenerationManifest::new(
            fake_id,
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"d"),
                1,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"c"),
                1,
                None,
            )
            .unwrap(),
        );
        // Write fake manifest into real_id folder
        let json = serde_json::to_string(&m).unwrap();
        fs::write(store.manifest_path(&real_id), json).unwrap();

        let err = store.read_manifest(&real_id).unwrap_err();
        match err {
            GenerationError::GenerationMismatch { .. } => {}
            other => panic!("Expected GenerationMismatch, got {other:?}"),
        }
    }

    #[test]
    fn test_tombstone_withdraw_and_collision() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();

        let id = GenerationId::new("gen-tomb-01").unwrap();
        let gen_dir = store.generation_dir(&id);
        fs::create_dir_all(&gen_dir).unwrap();
        fs::write(gen_dir.join("mihomo-cli"), b"daemon").unwrap();
        fs::write(gen_dir.join("mihomo"), b"core").unwrap();

        let manifest = GenerationManifest::new(
            id.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"daemon"),
                6,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"core"),
                4,
                None,
            )
            .unwrap(),
        );
        store.write_manifest(&manifest).unwrap();

        // Withdraw: renames dir to <id>.withdrawn
        let tombstone = store.withdraw_generation(&id).unwrap();
        assert!(tombstone.exists());
        assert!(!gen_dir.exists());
        assert!(store.has_withdrawn(&id));

        // Collision: second withdraw fails
        // Re-create the dir to test collision
        fs::create_dir_all(&gen_dir).unwrap();
        let err = store.withdraw_generation(&id).unwrap_err();
        match err {
            GenerationError::TombstoneCollision { .. } => {}
            other => panic!("Expected TombstoneCollision, got {other:?}"),
        }

        // List withdrawn
        let withdrawn = store.list_withdrawn().unwrap();
        assert_eq!(withdrawn, vec![id.clone()]);

        // Retire
        store.retire_withdrawn(&id).unwrap();
        assert!(!store.has_withdrawn(&id));
    }

    #[test]
    fn test_cleanup_excludes_tombstones() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();

        let id1 = GenerationId::new("gen-gc-01").unwrap();
        let id2 = GenerationId::new("gen-gc-02").unwrap();
        fs::create_dir_all(store.generation_dir(&id1)).unwrap();
        fs::create_dir_all(store.generation_dir(&id2)).unwrap();

        // Set id1 as active
        let state = GenerationState {
            schema_version: 1,
            active: Some(id1.clone()),
            pending: None,
            previous: None,
            updated_at: Utc::now().to_rfc3339(),
        };
        store.write_state(&state).unwrap();

        // Create a tombstone for id2
        store.withdraw_generation(&id2).unwrap();

        // Cleanup should not remove the tombstone
        let removed = store.cleanup_old_generations(0).unwrap();
        assert!(removed.is_empty(), "tombstone must not be collected by GC");
        assert!(store.has_withdrawn(&id2));
    }

    #[test]
    fn test_locked_variants_no_double_lock() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();

        let id = GenerationId::new("gen-lock-var-01").unwrap();
        let gen_dir = store.generation_dir(&id);
        fs::create_dir_all(&gen_dir).unwrap();
        fs::write(gen_dir.join("mihomo-cli"), b"d").unwrap();
        fs::write(gen_dir.join("mihomo"), b"c").unwrap();
        let manifest = GenerationManifest::new(
            id.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"d"),
                1,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"c"),
                1,
                None,
            )
            .unwrap(),
        );
        store.write_manifest(&manifest).unwrap();

        // Acquire lock externally
        let lock = store.acquire_lock().unwrap();

        // Stage with lock
        store.stage_pending_with_lock(&lock, id.clone()).unwrap();
        let state = store.read_state().unwrap();
        assert_eq!(state.pending, Some(id.clone()));

        // Commit with lock (no double-lock)
        let state = store.commit_active_with_lock(&lock).unwrap();
        assert_eq!(state.active, Some(id.clone()));
        assert_eq!(state.pending, None);

        // Clear pending with lock (no-op since pending is None)
        let state = store.clear_pending_with_lock(&lock).unwrap();
        assert_eq!(state.pending, None);

        drop(lock);
    }

    #[test]
    fn test_cas_commit_rejects_mismatched_expected_id() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();

        let id1 = GenerationId::new("gen-cas-01").unwrap();
        let id2 = GenerationId::new("gen-cas-02").unwrap();
        for id in [&id1, &id2] {
            let gen_dir = store.generation_dir(id);
            fs::create_dir_all(&gen_dir).unwrap();
            fs::write(gen_dir.join("mihomo-cli"), b"d").unwrap();
            fs::write(gen_dir.join("mihomo"), b"c").unwrap();
            let manifest = GenerationManifest::new(
                id.clone(),
                1,
                ArtifactManifestEntry::new(
                    "mihomo-cli",
                    ArtifactKind::Daemon,
                    calculate_sha256_bytes(b"d"),
                    1,
                    None,
                )
                .unwrap(),
                ArtifactManifestEntry::new(
                    "mihomo",
                    ArtifactKind::Core,
                    calculate_sha256_bytes(b"c"),
                    1,
                    None,
                )
                .unwrap(),
            );
            store.write_manifest(&manifest).unwrap();
        }

        let lock = store.acquire_lock().unwrap();
        store.stage_pending_with_lock(&lock, id1.clone()).unwrap();

        // CAS with wrong expected ID
        let err = store.commit_active_cas(&lock, &id2, None).unwrap_err();
        match err {
            GenerationError::GenerationMismatch { .. } => {}
            other => panic!("Expected GenerationMismatch, got {other:?}"),
        }

        // CAS with correct expected ID
        let state = store.commit_active_cas(&lock, &id1, None).unwrap();
        assert_eq!(state.active, Some(id1));
    }

    #[test]
    fn test_capture_baseline_creates_generation_with_metadata() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();

        // Create fake installed artifacts
        let install_dir = temp.path().join("install");
        fs::create_dir_all(&install_dir).unwrap();
        let daemon_path = install_dir.join("mihomo-cli");
        let core_path = install_dir.join("mihomo");
        fs::write(&daemon_path, b"installed-daemon").unwrap();
        fs::write(&core_path, b"installed-core").unwrap();

        let artifacts = vec![
            BaselineArtifact {
                relative_path: "mihomo-cli".to_string(),
                kind: ArtifactKind::Daemon,
                source_path: daemon_path,
                unix_mode: None,
            },
            BaselineArtifact {
                relative_path: "mihomo".to_string(),
                kind: ArtifactKind::Core,
                source_path: core_path,
                unix_mode: None,
            },
        ];

        let lock = store.acquire_lock().unwrap();
        let baseline_id = capture_baseline(&store, &lock, &artifacts).unwrap();

        // Verify baseline manifest exists and has baseline=true metadata
        let manifest = store.read_manifest(&baseline_id).unwrap();
        assert!(manifest.is_baseline());
        assert_eq!(manifest.metadata.get("kind").unwrap(), BASELINE_KIND);
        assert_eq!(
            manifest.daemon.sha256,
            calculate_sha256_bytes(b"installed-daemon")
        );
    }

    #[test]
    fn test_capture_baseline_records_missing_artifacts() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();

        let install_dir = temp.path().join("install");
        fs::create_dir_all(&install_dir).unwrap();
        let daemon_path = install_dir.join("mihomo-cli");
        fs::write(&daemon_path, b"installed-daemon").unwrap();
        // Core is missing

        let artifacts = vec![
            BaselineArtifact {
                relative_path: "mihomo-cli".to_string(),
                kind: ArtifactKind::Daemon,
                source_path: daemon_path,
                unix_mode: None,
            },
            BaselineArtifact {
                relative_path: "mihomo".to_string(),
                kind: ArtifactKind::Core,
                source_path: install_dir.join("mihomo"),
                unix_mode: None,
            },
        ];

        let lock = store.acquire_lock().unwrap();
        let baseline_id = capture_baseline(&store, &lock, &artifacts).unwrap();

        let manifest = store.read_manifest(&baseline_id).unwrap();
        assert!(manifest.metadata.contains_key("missing"));
        assert!(manifest.metadata["missing"].contains("mihomo"));
    }

    #[test]
    fn test_migration_four_state_matrix() {
        let temp = TempDir::new().unwrap();
        let legacy_root = temp.path().join("legacy");
        let new_root = temp.path().join("new-generations");

        // State 00: no old, no new = first install
        let new_store = GenerationStore::new(&new_root);
        let result = migrate_legacy_store(&legacy_root, &new_store).unwrap();
        assert_eq!(result, MigrationOutcome::FirstInstall);

        // State 10: old exists, no new = trigger migration
        fs::create_dir_all(&legacy_root).unwrap();
        let legacy_state = GenerationState::default();
        fs::write(
            legacy_root.join(STATE_FILE_NAME),
            serde_json::to_string_pretty(&legacy_state).unwrap(),
        )
        .unwrap();
        let result = migrate_legacy_store(&legacy_root, &new_store).unwrap();
        assert_eq!(result, MigrationOutcome::Migrated);
        assert!(new_root.join(STATE_FILE_NAME).exists());
        // Old originals left in place
        assert!(legacy_root.join(STATE_FILE_NAME).exists());

        // State 01/11: new exists = authoritative
        let result = migrate_legacy_store(&legacy_root, &new_store).unwrap();
        assert_eq!(result, MigrationOutcome::NewRootAuthoritative);
    }

    /// SPEC P1-1/P0-2: migration publishes the built staging tree with one
    /// no-replace rename and leaves no staging residue behind.
    #[test]
    fn test_migration_publishes_atomically_without_staging_residue() {
        let temp = TempDir::new().unwrap();
        let legacy_root = temp.path().join("legacy");
        let new_root = temp.path().join("new-generations");
        let id = GenerationId::new("gen-atomic-01").unwrap();

        fs::create_dir_all(legacy_root.join("generations").join(id.as_str())).unwrap();
        fs::write(
            legacy_root.join(STATE_FILE_NAME),
            serde_json::to_string_pretty(&GenerationState {
                schema_version: 1,
                active: Some(id.clone()),
                pending: None,
                previous: None,
                updated_at: Utc::now().to_rfc3339(),
            })
            .unwrap(),
        )
        .unwrap();
        let manifest = GenerationManifest::new(
            id.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"legacy-daemon"),
                13,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"legacy-core"),
                11,
                None,
            )
            .unwrap(),
        );
        fs::write(
            legacy_root
                .join("generations")
                .join(id.as_str())
                .join(MANIFEST_FILE_NAME),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let withdrawn = format!("{}{}", id.as_str(), WITHDRAWN_SUFFIX);
        let withdrawn_dir = legacy_root.join("generations").join(&withdrawn);
        fs::create_dir(&withdrawn_dir).unwrap();
        fs::write(
            withdrawn_dir.join(MANIFEST_FILE_NAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let new_store = GenerationStore::new(&new_root);
        assert_eq!(
            migrate_legacy_store(&legacy_root, &new_store).unwrap(),
            MigrationOutcome::Migrated
        );
        assert!(new_root.join(STATE_FILE_NAME).exists());
        assert!(new_store.is_generation_untrusted(&id));
        assert!(new_root
            .join("generations")
            .join(withdrawn)
            .join(UNTRUSTED_MARKER)
            .is_file());
        crate::utils::verify_generation_trust_root(&new_root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&new_root).unwrap().permissions().mode() & 0o777,
                0o750
            );
        }
        // Single publish, no staging residue next to the new root.
        let residue: Vec<String> = fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".migrating-"))
            .collect();
        assert!(residue.is_empty(), "staging residue: {residue:?}");
    }

    #[cfg(unix)]
    #[test]
    fn test_migration_rejects_unsafe_legacy_entries_and_cleans_staging() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;
        for kind in ["fifo", "symlink", "directory-symlink", "bad-manifest"] {
            let temp = TempDir::new().unwrap();
            let legacy = temp.path().join("legacy");
            let generations = legacy.join("generations");
            let generation = generations.join("gen-safe");
            fs::create_dir_all(&generation).unwrap();
            let item = generation.join("artifact");
            match kind {
                "fifo" => {
                    let path = std::ffi::CString::new(item.as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                }
                "symlink" => symlink(temp.path().join("outside"), &item).unwrap(),
                "directory-symlink" => {
                    fs::remove_dir(&generation).unwrap();
                    symlink(temp.path(), &generation).unwrap();
                }
                _ => fs::write(generation.join(MANIFEST_FILE_NAME), b"broken").unwrap(),
            }
            let new_root = temp.path().join("new");
            assert!(
                migrate_legacy_store(&legacy, &GenerationStore::new(&new_root)).is_err(),
                "{kind}"
            );
            assert!(!new_root.exists());
            assert!(fs::read_dir(temp.path()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".migrating-")));
            assert!(fs::symlink_metadata(&generation).is_ok());
        }
    }

    /// SPEC P1-1/P0-2: a failed build/publish leaves the new root **absent**
    /// (no half-migrated root that could be mistaken for authoritative).
    #[test]
    fn test_migration_failure_leaves_no_new_root() {
        let temp = TempDir::new().unwrap();
        let legacy_root = temp.path().join("legacy");
        let new_root = temp.path().join("new-generations");
        fs::create_dir_all(&legacy_root).unwrap();
        // Corrupt legacy state → the build must fail before publishing.
        fs::write(legacy_root.join(STATE_FILE_NAME), b"{not-json").unwrap();

        let new_store = GenerationStore::new(&new_root);
        assert!(migrate_legacy_store(&legacy_root, &new_store).is_err());
        assert!(
            !new_root.exists(),
            "half-built root must never be published"
        );
        assert!(legacy_root.join(STATE_FILE_NAME).exists());
        let residue: Vec<String> = fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.contains(".migrating-"))
            .collect();
        assert!(residue.is_empty(), "staging residue: {residue:?}");
    }

    /// SPEC P1-1: when the legacy paths alias the new root itself (pre-
    /// layout: store root == runtime root) migration must not self-copy or mark
    /// the live store untrusted.
    #[test]
    fn test_migration_excludes_aliased_self_paths() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("store");
        let id = GenerationId::new("gen-alias-01").unwrap();
        fs::create_dir_all(root.join("generations").join(id.as_str())).unwrap();
        let store = GenerationStore::new(&root);

        // 1) state present → new root authoritative, nothing marked.
        fs::write(
            root.join(STATE_FILE_NAME),
            serde_json::to_string_pretty(&GenerationState::default()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            migrate_legacy_store(&root, &store).unwrap(),
            MigrationOutcome::NewRootAuthoritative
        );
        assert!(!store.is_generation_untrusted(&id));

        // 2) state missing but only our own (aliased) generations dir exists →
        // still no legacy data, so no error and no marking.
        fs::remove_file(root.join(STATE_FILE_NAME)).unwrap();
        assert_eq!(
            migrate_legacy_store(&root, &store).unwrap(),
            MigrationOutcome::NewRootAuthoritative
        );
        assert!(!store.is_generation_untrusted(&id));
    }

    /// SPEC A/P2-3: the `missing` exemption is licensed by the baseline
    /// marker only — a pending manifest cannot use it to skip byte verification.
    #[test]
    fn test_missing_exemption_is_baseline_only() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();
        let id = GenerationId::new("gen-not-baseline").unwrap();
        let gen_dir = store.generation_dir(&id);
        fs::create_dir_all(&gen_dir).unwrap();
        fs::write(gen_dir.join("mihomo-cli"), b"daemon").unwrap();

        let mut manifest = GenerationManifest::new(
            id.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"daemon"),
                6,
                None,
            )
            .unwrap(),
            baseline_sentinel("mihomo", &ArtifactKind::Core),
        );
        manifest = manifest.with_metadata("missing", "mihomo");
        store.write_manifest(&manifest).unwrap();

        // Not a baseline manifest → the absent core is a validation failure.
        assert!(store.validate_generation(&id).is_err());

        // Marking it as a baseline licenses the exemption (core absent on disk).
        manifest = manifest.with_metadata("kind", BASELINE_KIND);
        store.write_manifest(&manifest).unwrap();
        assert!(store.validate_generation(&id).is_ok());

        // A declared-missing slot that does exist is a corruption, not a pass.
        fs::write(gen_dir.join("mihomo"), b"phantom").unwrap();
        assert!(store.validate_generation(&id).is_err());
    }

    #[test]
    fn test_migration_marks_migrated_generations_untrusted() {
        let temp = TempDir::new().unwrap();
        let legacy_root = temp.path().join("legacy");
        let new_root = temp.path().join("new-generations");
        let id = GenerationId::new("gen-legacy-01").unwrap();

        // Legacy generation directory with a manifest.
        let legacy_gen_dir = legacy_root.join("generations").join(id.as_str());
        fs::create_dir_all(&legacy_gen_dir).unwrap();
        fs::write(legacy_gen_dir.join("mihomo-cli"), b"legacy-daemon").unwrap();
        fs::write(legacy_gen_dir.join("mihomo"), b"legacy-core").unwrap();
        let manifest = GenerationManifest::new(
            id.clone(),
            1,
            ArtifactManifestEntry::new(
                "mihomo-cli",
                ArtifactKind::Daemon,
                calculate_sha256_bytes(b"legacy-daemon"),
                13,
                None,
            )
            .unwrap(),
            ArtifactManifestEntry::new(
                "mihomo",
                ArtifactKind::Core,
                calculate_sha256_bytes(b"legacy-core"),
                11,
                None,
            )
            .unwrap(),
        );
        fs::write(
            legacy_gen_dir.join(MANIFEST_FILE_NAME),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(
            legacy_root.join(STATE_FILE_NAME),
            serde_json::to_string_pretty(&GenerationState {
                schema_version: 1,
                active: None,
                pending: Some(id.clone()),
                previous: None,
                updated_at: Utc::now().to_rfc3339(),
            })
            .unwrap(),
        )
        .unwrap();

        let new_store = GenerationStore::new(&new_root);
        assert_eq!(
            migrate_legacy_store(&legacy_root, &new_store).unwrap(),
            MigrationOutcome::Migrated
        );
        // Copied generation is marked: migration confers no trust.
        assert!(new_store.is_generation_untrusted(&id));
        // Old originals stay in place (zero renaming).
        assert!(legacy_gen_dir.join(MANIFEST_FILE_NAME).exists());
    }

    #[test]
    fn test_baseline_validation_skips_artifacts_recorded_as_missing() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();

        let install_dir = temp.path().join("install");
        fs::create_dir_all(&install_dir).unwrap();
        let daemon_path = install_dir.join("mihomo-cli");
        fs::write(&daemon_path, b"installed-daemon").unwrap();

        let artifacts = vec![
            BaselineArtifact {
                relative_path: "mihomo-cli".to_string(),
                kind: ArtifactKind::Daemon,
                source_path: daemon_path,
                unix_mode: None,
            },
            BaselineArtifact {
                relative_path: "mihomo".to_string(),
                kind: ArtifactKind::Core,
                source_path: install_dir.join("mihomo"),
                unix_mode: None,
            },
            BaselineArtifact {
                relative_path: "GeoSite.dat".to_string(),
                kind: ArtifactKind::Geosite,
                source_path: install_dir.join("GeoSite.dat"),
                unix_mode: None,
            },
        ];

        let lock = store.acquire_lock().unwrap();
        let baseline_id = capture_baseline(&store, &lock, &artifacts).unwrap();
        // The missing slots are named in the pre-image list and still valid.
        let manifest = store.validate_generation(&baseline_id).unwrap();
        let missing = manifest.metadata.get("missing").cloned().unwrap();
        assert!(missing.contains("mihomo"));
        assert!(missing.contains("GeoSite.dat"));
        assert_eq!(manifest.extra_artifacts.len(), 1);
        assert_eq!(manifest.extra_artifacts[0].relative_path, "GeoSite.dat");
        assert_eq!(manifest.extra_artifacts[0].size_bytes, 0);
    }

    #[test]
    fn test_compensate_with_lock_consumes_slots() {
        let temp = TempDir::new().unwrap();
        let store = GenerationStore::new(temp.path());
        store.init().unwrap();
        let baseline = GenerationId::new("gen-baseline").unwrap();
        let pending = GenerationId::new("gen-pending").unwrap();

        store
            .write_state(&GenerationState {
                schema_version: 1,
                active: Some(pending.clone()),
                pending: Some(pending),
                previous: Some(baseline.clone()),
                updated_at: Utc::now().to_rfc3339(),
            })
            .unwrap();

        let lock = store.acquire_lock().unwrap();
        let state = store.compensate_with_lock(&lock, Some(&baseline)).unwrap();
        assert_eq!(state.active, Some(baseline));
        assert_eq!(state.pending, None);
        assert_eq!(state.previous, None);
    }

    #[test]
    fn test_migration_cleans_precreated_empty_root() {
        let temp = TempDir::new().unwrap();
        let legacy_root = temp.path().join("legacy");
        let new_root = temp.path().join("new-generations");

        fs::create_dir_all(&legacy_root).unwrap();
        fs::write(
            legacy_root.join(STATE_FILE_NAME),
            serde_json::to_string_pretty(&GenerationState::default()).unwrap(),
        )
        .unwrap();
        // The old install plan created an empty trust root before migration.
        fs::create_dir_all(&new_root).unwrap();

        let new_store = GenerationStore::new(&new_root);
        let result = migrate_legacy_store(&legacy_root, &new_store).unwrap();
        assert_eq!(result, MigrationOutcome::Migrated);
        assert!(new_root.join(STATE_FILE_NAME).exists());
        assert!(legacy_root.join(STATE_FILE_NAME).exists());
    }

    #[test]
    fn test_migration_non_empty_root_missing_state_with_legacy_errors() {
        let temp = TempDir::new().unwrap();
        let legacy_root = temp.path().join("legacy");
        let new_root = temp.path().join("new-generations");

        // Create legacy data
        fs::create_dir_all(&legacy_root).unwrap();
        fs::write(
            legacy_root.join(STATE_FILE_NAME),
            serde_json::to_string_pretty(&GenerationState::default()).unwrap(),
        )
        .unwrap();

        // A non-empty new root without state is a partial/corrupt scene: it must
        // stay fail-closed rather than being cleaned as a pre-created empty root.
        fs::create_dir_all(&new_root).unwrap();
        fs::write(new_root.join("stray"), b"evidence").unwrap();

        let new_store = GenerationStore::new(&new_root);
        let err = migrate_legacy_store(&legacy_root, &new_store).unwrap_err();
        match err {
            GenerationError::Io(e) => {
                assert!(
                    e.to_string().contains("refuse to misidentify"),
                    "unexpected error: {e}"
                );
            }
            other => panic!("Expected Io error, got {other:?}"),
        }
        assert_eq!(fs::read(new_root.join("stray")).unwrap(), b"evidence");
    }
}
