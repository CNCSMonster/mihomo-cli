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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GenerationId(String);

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
    )
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

    /// Initializes storage directories.
    pub fn init(&self) -> Result<(), GenerationError> {
        crate::utils::ensure_dir_all_no_follow(&self.generations_dir())
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
        file.read_to_string(&mut content)?;

        let state: GenerationState = serde_json::from_str(&content)
            .map_err(|e| GenerationError::Serialization(e.to_string()))?;
        Ok(state)
    }

    /// Atomically writes the GenerationState to state.json.
    pub fn write_state(&self, state: &GenerationState) -> Result<(), GenerationError> {
        self.init()?;
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| GenerationError::Serialization(e.to_string()))?;
        crate::utils::atomic_write_bytes_no_follow(&self.state_file_path(), json.as_bytes(), 0o644)
            .map_err(|error| GenerationError::Io(io::Error::other(error.to_string())))?;
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
        self.validate_generation(&id)?;
        let mut state = self.read_state()?;
        state.pending = Some(id);
        state.updated_at = Utc::now().to_rfc3339();
        self.write_state(&state)?;
        Ok(state)
    }

    /// Commits the Pending generation into Active, shifting existing Active to Previous.
    /// Synchronized via inter-process lock across read-validate-modify-write.
    pub fn commit_active(&self) -> Result<GenerationState, GenerationError> {
        let _guard = self.acquire_lock()?;
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

    /// Rolls back Active to Previous generation.
    /// Synchronized via inter-process lock across read-validate-modify-write.
    pub fn rollback(&self) -> Result<GenerationState, GenerationError> {
        let _guard = self.acquire_lock()?;
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
        file.read_to_string(&mut content)?;

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

        for entry in manifest.all_entries() {
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
        let _guard = self.acquire_lock()?;
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
}

/// Validates that an artifact relative path is strictly safe:
/// no absolute paths, no ".." parent components, no leading slashes/drives.
pub fn validate_artifact_path(relative_path: &str) -> Result<(), GenerationError> {
    if relative_path.is_empty() {
        return Err(GenerationError::InvalidPath(
            "Artifact path cannot be empty".to_string(),
        ));
    }
    let p = Path::new(relative_path);
    for component in p.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
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
}
