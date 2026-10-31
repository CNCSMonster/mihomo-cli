//! Selection intent persistence (selections/<subscription_id>.yaml) and the per-instance
//! selection lock that serializes select (kernel PUT + persist) with replay
//! (re-read + PUT). See SPEC §3.8.1.

use crate::utils::AppPaths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionScope {
    pub subscription_id: String,
    pub path: PathBuf,
}

pub fn active_selection_scope(paths: &AppPaths) -> Result<SelectionScope> {
    let id = match crate::config::get_active_id_at(paths)? {
        Some(id) => id,
        None => {
            return Err(anyhow::anyhow!(
                "No active subscription. Run: mihomo-cli config --add <URL> or --import <FILE>"
            ));
        }
    };
    if !id.starts_with("sub-")
        || id.len() != 12
        || !id.as_bytes()[4..].iter().all(u8::is_ascii_hexdigit)
    {
        anyhow::bail!("Active subscription pointer is invalid; run: mihomo-cli config --list");
    }
    let subscriptions = crate::config::load_subscriptions_at(paths)?;
    if crate::config::find_subscription(&subscriptions, &id).is_none()
        || !paths.subscription_file_path(&id).is_file()
    {
        anyhow::bail!("Active subscription {id} is unavailable; run: mihomo-cli config --list");
    }
    migrate_legacy_selection_if_needed(paths, &id)?;
    Ok(SelectionScope {
        subscription_id: id.clone(),
        path: paths.selection_state_path_for_subscription(&id),
    })
}

fn migrate_legacy_selection_if_needed(paths: &AppPaths, id: &str) -> Result<()> {
    let legacy = paths.selection_state_path();
    if !legacy.exists() {
        return Ok(());
    }
    let target = paths.selection_state_path_for_subscription(id);
    if target.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&legacy).with_context(|| {
        format!(
            "Failed to read legacy selection state: {}",
            legacy.display()
        )
    })?;
    let state: SelectionStateFile = serde_yaml::from_str(&content)
        .with_context(|| "Failed to parse legacy selection-state.yaml")?;
    crate::utils::ensure_dir_all_no_follow(&paths.selections_dir())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let selections_dir = paths.selections_dir();
        let parent_has_setgid = selections_dir
            .parent()
            .and_then(|p| std::fs::symlink_metadata(p).ok())
            .map(|meta| crate::utils::mode_has_setgid(meta.permissions().mode()))
            .unwrap_or(false);
        if parent_has_setgid {
            let _ = crate::utils::set_directory_mode_no_follow(&selections_dir, 0o2755);
        }
    }
    let serialized = serde_yaml::to_string(&state)?;
    crate::utils::atomic_write_file_for_original_user(&target.display().to_string(), &serialized)?;
    let migrated = paths.config_dir().join("selection-state.yaml.legacy");
    crate::generation::replace_file_safely(&legacy, &migrated).with_context(|| {
        format!(
            "Failed to archive legacy selection state: {}",
            legacy.display()
        )
    })?;
    Ok(())
}

const SELECTION_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const SELECTION_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SelectionStateFile {
    #[serde(default)]
    pub selections: BTreeMap<String, String>,
}

pub fn load_selection_state_for_scope(scope: &SelectionScope) -> Result<BTreeMap<String, String>> {
    load_selection_state_path(&scope.path)
}

/// Hard cap for selection map entries accepted from IPC pushes (mirror contract).
#[cfg(any(unix, windows))]
const SELECTION_MAP_ENTRY_LIMIT: usize = 512;

/// Parse and validate a selection-state YAML payload (single `group → node`
/// mapping). Used by the daemon to validate `RecordSelectionIntent` pushes
/// before writing the fixed-runtime mirror.
#[cfg(any(unix, windows))]
pub fn parse_selection_map(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let state: SelectionStateFile =
        serde_yaml::from_slice(bytes).with_context(|| "Failed to parse selection state payload")?;
    if state.selections.len() > SELECTION_MAP_ENTRY_LIMIT {
        anyhow::bail!(
            "selection state payload exceeds {} entries",
            SELECTION_MAP_ENTRY_LIMIT
        );
    }
    for (group, node) in &state.selections {
        if group.trim().is_empty() || node.trim().is_empty() {
            anyhow::bail!("selection state payload contains an empty group or node name");
        }
    }
    Ok(state.selections)
}

/// Serialize a selection map back to the canonical YAML payload form.
#[cfg(unix)]
#[allow(dead_code)]
pub fn format_selection_map(map: &BTreeMap<String, String>) -> String {
    serde_yaml::to_string(&SelectionStateFile {
        selections: map.clone(),
    })
    .expect("selection state serialization cannot fail")
}

pub fn load_selection_state_path(path: &std::path::Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read selection state: {}", path.display()))?;
    let state: SelectionStateFile = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse selection state: {}", path.display()))?;
    Ok(state.selections)
}

pub fn save_selection_state_for_scope(
    scope: &SelectionScope,
    selections: &BTreeMap<String, String>,
) -> Result<()> {
    let selections_dir = scope.path.parent().unwrap();
    crate::utils::ensure_dir_all_no_follow(selections_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent_has_setgid = selections_dir
            .parent()
            .and_then(|p| std::fs::symlink_metadata(p).ok())
            .map(|meta| crate::utils::mode_has_setgid(meta.permissions().mode()))
            .unwrap_or(false);
        if parent_has_setgid {
            let _ = crate::utils::set_directory_mode_no_follow(selections_dir, 0o2755);
        }
    }
    let state = SelectionStateFile {
        selections: selections.clone(),
    };
    let content = serde_yaml::to_string(&state)?;
    crate::utils::atomic_write_file_for_original_user(&scope.path.display().to_string(), &content)?;
    Ok(())
}

/// Persist one `group → node` entry, taking the selection lock itself.
///
/// This is the entry point for writers that do not already hold the lock: the
/// load→modify→save runs inside the lock so a concurrent `select` cannot lose
/// the entry (or resurrect a removed one). Production's only writer today is
/// `select`, which holds the lock across Core PUT + persist and therefore must
/// use [`remember_selection_for_scope_locked`] (the lock is not reentrant).
#[cfg_attr(not(test), allow(dead_code))]
pub fn remember_selection_for_scope(scope: &SelectionScope, group: &str, node: &str) -> Result<()> {
    let _guard = acquire_selection_lock_for_scope(scope)?;
    remember_selection_for_scope_locked(scope, group, node)
}

/// Lock-held variant of [`remember_selection_for_scope`].
///
/// Contract: the caller already holds this scope's selection lock
/// (`acquire_selection_lock_at` on its config dir). Calling it unlocked
/// silently re-opens the lost-update window against a concurrent `select`
/// (the race fixed), so debug builds assert the contract. The `select`
/// path uses this variant to keep exactly one lock acquisition across Core
/// PUT + persist (SPEC §3.8.1); the lock is not reentrant.
pub fn remember_selection_for_scope_locked(
    scope: &SelectionScope,
    group: &str,
    node: &str,
) -> Result<()> {
    // `debug_assert!` keeps its argument in the AST for every profile (it is only
    // guarded by `cfg!(debug_assertions)`), so the probe must be gated here as
    // well — otherwise release builds fail to compile.
    #[cfg(debug_assertions)]
    debug_assert!(
        selection_lock_is_held_for_scope(scope),
        "remember_selection_for_scope_locked called without the selection lock; \
         use remember_selection_for_scope instead"
    );
    let mut selections = load_selection_state_for_scope(scope)?;
    selections.insert(group.to_string(), node.to_string());
    save_selection_state_for_scope(scope, &selections)
}

/// Debug-only probe for the `_locked` contract: an exclusive flock/LockFileEx
/// taken on a second descriptor of the same file fails while the process holds
/// the lock through another descriptor.
#[cfg(debug_assertions)]
fn selection_lock_is_held_for_scope(scope: &SelectionScope) -> bool {
    let Some(config_dir) = scope.path.parent().and_then(|p| p.parent()) else {
        return false;
    };
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .open(config_dir.join(".selection-state.lock"))
    else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        // Unobservable (unsupported filesystem, permission): stay permissive,
        // the assertion is a development aid, not a production gate.
        Err(_) => true,
    }
}

/// Selection lock of the config directory that owns `scope`'s intent file.
/// Not reentrant: callers of the unpin paths must not already hold it.
fn acquire_selection_lock_for_scope(scope: &SelectionScope) -> Result<SelectionLockGuard> {
    let config_dir = scope
        .path
        .parent()
        .and_then(|p| p.parent())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Selection state path has no config directory: {}",
                scope.path.display()
            )
        })?;
    acquire_selection_lock_at(&AppPaths::new(config_dir))
}

/// Remove the pinned selection for `group`.
///
/// The load→modify→save runs inside the selection lock: an
/// unlocked save would let a concurrent `select`, which holds the lock across
/// its own load→modify→save, either lose the record it just persisted or
/// resurrect the record removed here. The lock is taken by this function rather
/// than by its callers because the delete-group cleanup runs after `ConfigLock`
/// is released — both locks are never held together, so the project-wide order
/// (`config transaction lock → selection lock`) is unaffected.
pub fn unpin_selection_for_scope(scope: &SelectionScope, group: &str) -> Result<bool> {
    let _guard = acquire_selection_lock_for_scope(scope)?;
    let mut selections = load_selection_state_for_scope(scope)?;
    let removed = selections.remove(group).is_some();
    if removed {
        save_selection_state_for_scope(scope, &selections)?;
    }
    Ok(removed)
}

pub fn unpin_all_selections_for_scope(scope: &SelectionScope) -> Result<usize> {
    let _guard = acquire_selection_lock_for_scope(scope)?;
    match load_selection_state_for_scope(scope) {
        Ok(selections) if selections.is_empty() => Ok(0),
        Ok(selections) => {
            let removed = selections.len();
            save_selection_state_for_scope(scope, &BTreeMap::new())?;
            Ok(removed)
        }
        Err(_) => {
            save_selection_state_for_scope(scope, &BTreeMap::new())?;
            Ok(0)
        }
    }
}

/// Guard holding the per-instance selection lock (`<config_dir>/.selection-state.lock`).
/// The lock is released when the guard drops (the underlying file handle closes).
pub struct SelectionLockGuard {
    file: std::fs::File,
}

impl Drop for SelectionLockGuard {
    fn drop(&mut self) {
        // Closing the descriptor alone is not enough: a child forked while the
        // guard was alive inherits the descriptor (same open file description)
        // and keeps the flock alive past this process's close. Unlock the open
        // file description explicitly, as `ConfigLockGuard` does. Since the
        // descriptor is also opened with O_CLOEXEC, so only a fork without exec
        // can still carry it; the explicit unlock remains the guarantee, CLOEXEC
        // narrows the inheritance window.
        let _ = self.file.unlock();
    }
}

/// Acquire the per-instance selection lock with the default timeout (10s).
///
/// Serializes select (kernel PUT + intent persist) against replay (re-read + PUT)
/// so a concurrent pair cannot leave runtime state and intent diverged (D8).
/// std `File::try_lock` maps to flock(2) on unix and LockFileEx on Windows, so
/// one code path covers all targets without cfg branches. The lock file content
/// is empty; all semantics live in the file lock itself. Not reentrant by design:
/// the select/replay paths acquire exactly once with no nested locking.
pub fn acquire_selection_lock_at(paths: &AppPaths) -> Result<SelectionLockGuard> {
    acquire_selection_lock_with_timeout(paths, SELECTION_LOCK_TIMEOUT)
}

fn acquire_selection_lock_with_timeout(
    paths: &AppPaths,
    timeout: Duration,
) -> Result<SelectionLockGuard> {
    crate::utils::ensure_dir_all_no_follow(paths.config_dir())?;
    let lock_path = paths.config_dir().join(".selection-state.lock");
    let file = crate::utils::open_file_create_no_follow(&lock_path)
        .with_context(|| format!("Cannot open lock file {}", lock_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o666));
    }
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(SelectionLockGuard { file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    anyhow::bail!(
                        "Another mihomo-cli instance is updating proxy selection (timed out after {}s).\n  \
                         Please retry in a moment.",
                        timeout.as_secs()
                    );
                }
                std::thread::sleep(SELECTION_LOCK_POLL_INTERVAL);
            }
            Err(err) => {
                anyhow::bail!("Failed to lock {}: {}", lock_path.display(), err);
            }
        }
    }
}

pub fn selection_drift_warnings_for_scope(scope: &SelectionScope) -> Result<Vec<String>> {
    let selections = load_selection_state_for_scope(scope)?;
    if selections.is_empty() {
        return Ok(Vec::new());
    }
    let config_path = scope
        .path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("config.yaml");
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
    selection_drift_warnings_in_config(&content, &selections)
}

pub fn selection_drift_warnings_in_config(
    config_content: &str,
    selections: &BTreeMap<String, String>,
) -> Result<Vec<String>> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(config_content).with_context(|| "Failed to parse config.yaml")?;
    let groups = yaml
        .get("proxy-groups")
        .and_then(|v| v.as_sequence())
        .ok_or_else(|| anyhow::anyhow!("config.yaml does not contain proxy-groups"))?;
    let mut members =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for group in groups {
        let Some(name) = group.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let set = group
            .get("proxies")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(ToString::to_string))
                    .collect::<std::collections::BTreeSet<_>>()
            })
            .unwrap_or_default();
        members.insert(name.to_string(), set);
    }
    let mut warnings = Vec::new();
    for (group, selected) in selections {
        match members.get(group) {
            None => warnings.push(format!(
                "Warning: selected group `{group}` is not available in current proxy groups."
            )),
            Some(nodes) if !nodes.contains(selected) => warnings.push(format!(
                "Warning: selected node `{selected}` is not available in group `{group}`."
            )),
            Some(_) => {}
        }
    }
    Ok(warnings)
}

// D6: replay adds bounded latency to start/restart. Select groups are
// single-digit in practice, so 2s per group inside a 5s total budget keeps
// the worst case inside the confirmed budget while tolerating a slow Core.
pub const REPLAY_TOTAL_BUDGET: Duration = Duration::from_secs(5);
pub const REPLAY_PER_GROUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayOutcome {
    Applied,
    SkippedGroupMissing,
    SkippedNodeMissing {
        current: Option<String>,
        /// Group members observed by the replay query itself: reused by the
        /// remedy matrix (no secondary fetch).
        members: Vec<String>,
    },
    Failed {
        error: String,
    },
    BudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]

pub struct ReplayGroupResult {
    pub group: String,
    pub node: String,
    pub outcome: ReplayOutcome,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]

pub struct ReplayReport {
    pub results: Vec<ReplayGroupResult>,
    /// Intent file exists but could not be read/parsed; no group was attempted.
    pub intent_error: Option<String>,
    /// Selectable groups fetched once (within the replay budget) when a group
    /// turned out to be missing; reused by the remedy matrix.
    pub selectable_groups: Option<Vec<String>>,
}

impl ReplayReport {
    pub fn format_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(err) = &self.intent_error {
            lines.push(format!(
                "⚠ Selection intent not replayed: {}",
                sanitize_for_display(err)
            ));
            lines.push(
                "  Run: mihomo-cli select --unpin --all  (to reset stored selections)".to_string(),
            );
            return lines;
        }
        let applied: Vec<String> = self
            .results
            .iter()
            .filter(|r| r.outcome == ReplayOutcome::Applied)
            .map(|r| {
                format!(
                    "{} → {}",
                    sanitize_for_display(&r.group),
                    sanitize_for_display(&r.node)
                )
            })
            .collect();
        if !applied.is_empty() {
            lines.push(format!("✓ Selections restored: {}", applied.join(", ")));
        }
        for r in &self.results {
            let reason = match &r.outcome {
                ReplayOutcome::Applied => continue,
                ReplayOutcome::SkippedGroupMissing => "group no longer exists".to_string(),
                ReplayOutcome::SkippedNodeMissing { current, .. } => match current {
                    Some(current) => {
                        format!(
                            "node no longer in group (current: {})",
                            sanitize_for_display(current)
                        )
                    }
                    None => "node no longer in group".to_string(),
                },
                ReplayOutcome::Failed { error } => {
                    format!("apply failed: {}", sanitize_for_display(error))
                }
                ReplayOutcome::BudgetExceeded => "time budget exceeded".to_string(),
            };
            lines.push(format!(
                "⚠ Selection {} → {} not applied: {reason}",
                sanitize_for_display(&r.group),
                sanitize_for_display(&r.node)
            ));
        }
        lines
    }
}

/// Replay persisted selection intent against a ready Core.
/// The caller guarantees API readiness; waiting is the caller's job.
#[allow(dead_code)]
pub async fn replay_selections_at(
    paths: &AppPaths,
    client: &impl crate::mihomo_api::MihomoApiClient,
) -> Result<ReplayReport> {
    replay_selections_until(paths, client, Instant::now() + REPLAY_TOTAL_BUDGET).await
}

async fn replay_scope_with_deadline(
    scope: &SelectionScope,
    client: &impl crate::mihomo_api::MihomoApiClient,
    deadline: Instant,
    per_group_timeout: Duration,
) -> Result<ReplayReport> {
    let intent_path = &scope.path;
    if !intent_path.exists() {
        return Ok(ReplayReport::default());
    }
    let selections = match load_selection_state_for_scope(scope) {
        Ok(selections) => selections,
        Err(err) => {
            return Ok(ReplayReport {
                results: Vec::new(),
                intent_error: Some(format!("{err:#}")),
                selectable_groups: None,
            });
        }
    };
    replay_map_with_deadline(&selections, client, deadline, per_group_timeout).await
}

/// Replay an in-memory selection map (fixed-runtime mirror path). The caller
/// guarantees API readiness; an empty map yields an empty report.
#[cfg(any(unix, windows))]
pub async fn replay_map_until(
    selections: &BTreeMap<String, String>,
    client: &impl crate::mihomo_api::MihomoApiClient,
    deadline: Instant,
) -> Result<ReplayReport> {
    replay_map_with_deadline(selections, client, deadline, REPLAY_PER_GROUP_TIMEOUT).await
}

async fn replay_map_with_deadline(
    selections: &BTreeMap<String, String>,
    client: &impl crate::mihomo_api::MihomoApiClient,
    deadline: Instant,
    per_group_timeout: Duration,
) -> Result<ReplayReport> {
    if selections.is_empty() {
        return Ok(ReplayReport::default());
    }
    let mut results = Vec::new();
    for (group, node) in selections.iter() {
        let (group, node) = (group.clone(), node.clone());
        if Instant::now() >= deadline {
            results.push(ReplayGroupResult {
                group,
                node,
                outcome: ReplayOutcome::BudgetExceeded,
            });
            continue;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let outcome =
            replay_one_group(client, &group, &node, remaining.min(per_group_timeout)).await;
        results.push(ReplayGroupResult {
            group,
            node,
            outcome,
        });
    }
    // A missing group needs the current group list for the remedy matrix:
    // fetch it once, within the remaining replay budget, and carry it in the
    // report so downstream printing never queries the Core again.
    let needs_groups = results
        .iter()
        .any(|r| matches!(r.outcome, ReplayOutcome::SkippedGroupMissing));
    let selectable_groups = if needs_groups {
        match remedy_query_budget(deadline) {
            Some(remaining) => tokio::time::timeout(remaining, list_selectable_groups(client))
                .await
                .ok()
                .and_then(|r| r.ok()),
            None => None,
        }
    } else {
        None
    };
    Ok(ReplayReport {
        results,
        intent_error: None,
        selectable_groups,
    })
}

/// Selectable group names (entries with an `all` array) from a Core
/// `/proxies` payload; `GLOBAL`/`COMPATIBLE` are synthetic and excluded.
pub fn selectable_groups_from_proxies(data: &serde_json::Value) -> Vec<String> {
    let Some(proxies) = data["proxies"].as_object() else {
        return Vec::new();
    };
    let mut groups: Vec<String> = proxies
        .iter()
        .filter(|(name, p)| {
            name.as_str() != "GLOBAL"
                && name.as_str() != "COMPATIBLE"
                && p.get("all").and_then(|v| v.as_array()).is_some()
        })
        .map(|(name, _)| name.clone())
        .collect();
    groups.sort();
    groups
}

/// Remaining shared replay budget for a remediation query (`GET /proxies`).
/// `None` once the replay deadline is reached: stop querying and render the
/// "list unavailable" guidance instead of guessing members.
fn remedy_query_budget(deadline: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then_some(remaining)
}

pub async fn list_selectable_groups(
    client: &impl crate::mihomo_api::MihomoApiClient,
) -> anyhow::Result<Vec<String>> {
    let data = client.get("/proxies").await?;
    if data["proxies"].as_object().is_none() {
        anyhow::bail!("no proxies");
    }
    Ok(selectable_groups_from_proxies(&data))
}

pub async fn replay_selections_until(
    paths: &AppPaths,
    client: &impl crate::mihomo_api::MihomoApiClient,
    deadline: Instant,
) -> Result<ReplayReport> {
    let _guard = acquire_selection_lock_at(paths)?;
    let scope = active_selection_scope(paths)?;
    replay_scope_with_deadline(&scope, client, deadline, REPLAY_PER_GROUP_TIMEOUT).await
}

async fn replay_one_group(
    client: &impl crate::mihomo_api::MihomoApiClient,
    group: &str,
    node: &str,
    per_group_timeout: Duration,
) -> ReplayOutcome {
    use crate::mihomo_api::{proxy_group_path, select_proxy_with_client};
    let path = proxy_group_path(group);
    let group_state = match tokio::time::timeout(per_group_timeout, client.get(&path)).await {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            let msg = format!("{err:#}");
            // The socket client surfaces the HTTP status line; 404 means the
            // group is gone (subscription changed), anything else is a real failure.
            if msg.contains("404") {
                return ReplayOutcome::SkippedGroupMissing;
            }
            return ReplayOutcome::Failed { error: msg };
        }
        Err(_) => {
            return ReplayOutcome::Failed {
                error: format!(
                    "group query timed out after {}s",
                    per_group_timeout.as_secs()
                ),
            };
        }
    };
    let members: Vec<&str> = group_state["all"]
        .as_array()
        .map(|all| all.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    if members.is_empty() {
        return ReplayOutcome::SkippedGroupMissing;
    }
    if !members.contains(&node) {
        return ReplayOutcome::SkippedNodeMissing {
            current: group_state["now"].as_str().map(ToString::to_string),
            members: members.iter().map(|m| (*m).to_string()).collect(),
        };
    }
    match tokio::time::timeout(
        per_group_timeout,
        select_proxy_with_client(client, group, node),
    )
    .await
    {
        Ok(Ok(())) => ReplayOutcome::Applied,
        Ok(Err(err)) => ReplayOutcome::Failed {
            error: format!("{err:#}"),
        },
        Err(_) => ReplayOutcome::Failed {
            error: format!("select timed out after {}s", per_group_timeout.as_secs()),
        },
    }
}

/// POSIX shell single-quote escaping for remediation command arguments.
pub fn shell_single_quote(s: &str) -> String {
    // Replace every `'` with `'\''` (close quote, escaped literal quote, reopen).
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Sanitize untrusted strings (group/node names, error messages) to prevent
/// CR/LF/ESC injection into control lines.
fn sanitize_for_display(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Build a `mihomo-cli select -g <group> --node <node>` command with POSIX
/// single-quote escaping. The `--node=<quoted>` form binds the parameter
/// explicitly (no ambiguity with positional args).
pub fn select_fix_command(group: &str, node: &str) -> String {
    format!(
        "mihomo-cli select -g {} --node={}",
        shell_single_quote(&sanitize_for_display(group)),
        shell_single_quote(&sanitize_for_display(node))
    )
}

/// Per-outcome remediation lines (A3' outcome matrix).
/// Shared by CLI and daemon: both render the same remedy text from the same
/// ReplayReport structure.
pub fn selection_remedy_lines(
    outcome: &ReplayOutcome,
    group: &str,
    members: Option<&[String]>,
    groups: Option<&[String]>,
) -> Vec<String> {
    match outcome {
        ReplayOutcome::Applied => Vec::new(),
        ReplayOutcome::SkippedNodeMissing { .. } => {
            let mut lines = Vec::new();
            if let Some(members) = members {
                lines.push(format!(
                    "  Current members of {}: {}",
                    sanitize_for_display(group),
                    members
                        .iter()
                        .take(10)
                        .map(|m| sanitize_for_display(m))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            lines.push(format!(
                "  Pick interactively: mihomo-cli select -g {}  (interactive terminal only)",
                shell_single_quote(&sanitize_for_display(group))
            ));
            lines.push(
                "  Non-interactive: run one of the member commands above, e.g.".to_string(),
            );
            if let Some(first) = members.and_then(|m| m.first()) {
                lines.push(format!("    {}", select_fix_command(group, first)));
            }
            lines
        }
        ReplayOutcome::SkippedGroupMissing => {
            let mut lines = Vec::new();
            match groups {
                Some(groups) => {
                    lines.push(format!(
                        "  Current groups: {}",
                        groups
                            .iter()
                            .take(10)
                            .map(|g| sanitize_for_display(g))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                None => {
                    // (b′) 共享预算超尽：replay 内的 /proxies 查询未在 deadline 内返回，
                    // 不猜测成员，给显式重试指引。
                    lines.push(
                        "  Group list unavailable (budget exhausted or query failed). Retry: mihomo-cli select --replay or mihomo-cli restart --system"
                            .to_string(),
                    );
                }
            }
            lines.push(
                "  Pick interactively: mihomo-cli select  (interactive terminal only)".to_string(),
            );
            lines
        }
        ReplayOutcome::Failed { error } => vec![format!(
            "  Intent kept; apply failed: {}. Retry explicitly: mihomo-cli select --replay or mihomo-cli restart --system",
            sanitize_for_display(error)
        )],
        ReplayOutcome::BudgetExceeded => vec![
            "  Intent kept; replay time budget exceeded.".to_string(),
            "  Retry explicitly: mihomo-cli select --replay or mihomo-cli restart --system"
                .to_string(),
        ],
    }
}

/// Remedy lines for non-applied replay outcomes, reusing the data the replay
/// already fetched (node-missing members come from the replay's own group
/// query; the group list is carried on the report). Network-free:
/// defect 7 (no unbounded/duplicate secondary fetches), usable from both the
/// CLI and the daemon exits.
pub fn selection_replay_remedy_lines(report: &ReplayReport) -> Vec<String> {
    let mut lines = Vec::new();
    for r in &report.results {
        let members = match &r.outcome {
            ReplayOutcome::SkippedNodeMissing { members, .. } => Some(members.as_slice()),
            _ => None,
        };
        lines.extend(selection_remedy_lines(
            &r.outcome,
            &r.group,
            members,
            report.selectable_groups.as_deref(),
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn selection_drift_reports_missing_group_and_node() {
        let mut selections = BTreeMap::new();
        selections.insert("OpenAI".to_string(), "US-01".to_string());
        selections.insert("Netflix".to_string(), "JP-01".to_string());
        let config = r#"
proxy-groups:
  - name: OpenAI
    type: select
    proxies:
      - US-02
"#;
        let warnings = selection_drift_warnings_in_config(config, &selections).unwrap();
        assert!(warnings.contains(
            &"Warning: selected node `US-01` is not available in group `OpenAI`.".to_string()
        ));
        assert!(warnings.contains(
            &"Warning: selected group `Netflix` is not available in current proxy groups."
                .to_string()
        ));
    }

    #[test]
    fn selection_lock_times_out_while_held_by_another_thread() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let guard =
            acquire_selection_lock_with_timeout(&paths, Duration::from_millis(500)).unwrap();

        let blocked_paths = paths.clone();
        let handle = std::thread::spawn(move || {
            acquire_selection_lock_with_timeout(&blocked_paths, Duration::from_millis(300))
        });
        let result = handle.join().unwrap();
        assert!(
            result.is_err(),
            "second acquirer must time out while the first holds the lock"
        );
        drop(guard);
    }

    #[test]
    fn selection_lock_reacquirable_after_release() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let guard = acquire_selection_lock_at(&paths).unwrap();
        assert!(paths.config_dir().join(".selection-state.lock").exists());
        drop(guard);
        let _guard2 = acquire_selection_lock_at(&paths).unwrap();
    }

    /// 锁文件不以 O_CLOEXEC 打开：guard 存活期间 fork 的子进程会继承该描述符。
    /// 释放因此不能依赖「最后一份描述符关闭」，否则锁会活到子进程退出，之后每次
    /// select/清理都要空等 10s 超时（全量测试暴露的确定性复现）。
    #[cfg(unix)]
    #[test]
    fn selection_lock_release_survives_a_duplicated_descriptor() {
        use std::os::unix::io::AsRawFd;

        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let guard = acquire_selection_lock_at(&paths).unwrap();
        let inherited = unsafe { libc::dup(guard.file.as_raw_fd()) };
        assert!(inherited >= 0, "dup failed");

        drop(guard);
        let reacquired =
            acquire_selection_lock_with_timeout(&paths, Duration::from_millis(500)).is_ok();
        unsafe { libc::close(inherited) };

        assert!(
            reacquired,
            "guard 释放必须显式解锁，不得等到最后一份描述符关闭"
        );
    }

    /// 锁描述符不得跨 exec 继承。父进程持锁期间 spawn 子进程，子进程 exec
    /// 后不得再持有锁文件描述符（`O_CLOEXEC`）；父进程显式 unlock 后，另一执行体
    /// 必须立即取到锁，不等待任何子进程退出。修前（无 `O_CLOEXEC`）子进程能
    /// `readlink` 到锁文件，一旦 guard 只 close 不解锁，锁会活到子进程退出。
    #[cfg(target_os = "linux")]
    #[test]
    fn selection_lock_fd_is_not_inherited_across_exec() {
        use std::os::unix::io::AsRawFd;

        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let guard = acquire_selection_lock_at(&paths).unwrap();
        let fd = guard.file.as_raw_fd();

        // 子进程在 exec 之后自报 fd 指向哪里：O_CLOEXEC 生效时该 fd 已关闭。
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("readlink /proc/self/fd/{fd} 2>/dev/null || true"))
            .output()
            .expect("spawn probe shell");
        let seen = String::from_utf8_lossy(&output.stdout);
        assert!(
            !seen.contains(".selection-state.lock"),
            "锁描述符被 exec 出的子进程继承（O_CLOEXEC 缺失）：{seen:?}"
        );

        drop(guard);
        assert!(
            acquire_selection_lock_with_timeout(&paths, Duration::from_millis(500)).is_ok(),
            "父进程显式解锁后必须立即可再取锁，不得等待任何子进程"
        );
    }

    fn setup_test_active_subscription(paths: &AppPaths, id: &str) -> SelectionScope {
        let subs_dir = paths.subscriptions_dir();
        std::fs::create_dir_all(&subs_dir).unwrap();
        std::fs::write(paths.active_file_path(), id).unwrap();
        std::fs::write(
            paths.subscriptions_meta_path(),
            format!("- id: {id}\n  url: file://{id}.yaml\n  updated: 2026-09-01T00:00:00Z\n"),
        )
        .unwrap();
        std::fs::write(
            paths.subscription_file_path(id),
            "proxies: []\nproxy-groups: []\nrules: []\n",
        )
        .unwrap();
        SelectionScope {
            subscription_id: id.to_string(),
            path: paths.selection_state_path_for_subscription(id),
        }
    }

    #[test]
    fn active_selection_scope_fails_fast_when_no_active_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let err = active_selection_scope(&paths).unwrap_err();
        assert!(err.to_string().contains("No active subscription"));
    }

    #[test]
    fn active_selection_scope_resolves_correct_scope() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let sub_id = "sub-11111111";
        setup_test_active_subscription(&paths, sub_id);

        let scope = active_selection_scope(&paths).unwrap();
        assert_eq!(scope.subscription_id, sub_id);
        assert_eq!(
            scope.path,
            paths.selection_state_path_for_subscription(sub_id)
        );
    }

    #[test]
    fn active_selection_scope_migrates_legacy_file_and_archives() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let sub_id = "sub-aaaaaaaa";
        setup_test_active_subscription(&paths, sub_id);

        let legacy_file = paths.selection_state_path();
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(&legacy_file, "selections:\n  Proxy: NodeA\n").unwrap();

        let scope = active_selection_scope(&paths).unwrap();
        assert_eq!(scope.subscription_id, sub_id);

        // Legacy file archived
        assert!(!legacy_file.exists());
        assert!(paths
            .config_dir()
            .join("selection-state.yaml.legacy")
            .exists());

        // Target selections file contains migrated data
        let selections = load_selection_state_for_scope(&scope).unwrap();
        assert_eq!(selections.get("Proxy"), Some(&"NodeA".to_string()));
    }

    #[test]
    fn remember_locked_variant_round_trips_under_lock() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-12345678");
        let _guard = acquire_selection_lock_at(&paths).unwrap();
        remember_selection_for_scope_locked(&scope, "OpenAI", "US-01").unwrap();
        let selections = load_selection_state_for_scope(&scope).unwrap();
        assert_eq!(selections.get("OpenAI"), Some(&"US-01".to_string()));
    }

    /// 自持锁变体在另一执行体真实持锁期间不得完成落盘——证明它真的取锁，
    /// 而不是回到旧实现的裸读改写。
    #[test]
    fn remember_self_locking_variant_waits_for_the_selection_lock() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-12345678");
        let guard = acquire_selection_lock_at(&paths).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let writer_scope = scope.clone();
        let handle = std::thread::spawn(move || {
            tx.send(remember_selection_for_scope(&writer_scope, "A", "a1").is_ok())
                .unwrap();
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "remember 在锁被占用时完成了落盘：仍是锁外读改写"
        );
        drop(guard);
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "锁释放后写入必须成功"
        );
        handle.join().unwrap();
        assert_eq!(
            load_selection_state_for_scope(&scope).unwrap().get("A"),
            Some(&"a1".to_string())
        );
    }

    /// 误用负控：`_locked` 变体要求调用方真实持锁，无锁调用在 debug
    /// 构建必须被断言拦下（release 构建不做运行时检查）。
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "without the selection lock")]
    fn remember_locked_variant_rejects_callers_without_the_lock() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-12345678");
        drop(acquire_selection_lock_at(&paths).unwrap());
        let _ = remember_selection_for_scope_locked(&scope, "A", "a1");
    }

    struct ReplayFakeClient {
        groups: BTreeMap<String, serde_json::Value>,
        put_failures: std::collections::BTreeSet<String>,
        puts: std::sync::Mutex<Vec<(String, String)>>,
        get_calls: std::sync::Mutex<usize>,
        get_paths: std::sync::Mutex<Vec<String>>,
        slow_groups: BTreeMap<String, Duration>,
        proxies: Option<serde_json::Value>,
    }

    impl ReplayFakeClient {
        fn new() -> Self {
            Self {
                groups: BTreeMap::new(),
                put_failures: std::collections::BTreeSet::new(),
                puts: std::sync::Mutex::new(Vec::new()),
                get_calls: std::sync::Mutex::new(0),
                get_paths: std::sync::Mutex::new(Vec::new()),
                slow_groups: BTreeMap::new(),
                proxies: None,
            }
        }

        /// Make a group query hang for this long (budget exhaustion tests).
        fn with_slow_group(mut self, group: &str, delay: Duration) -> Self {
            self.slow_groups.insert(group.to_string(), delay);
            self
        }

        /// Serve this payload for `/proxies` (remedy group list).
        fn with_proxies(mut self, data: serde_json::Value) -> Self {
            self.proxies = Some(data);
            self
        }

        fn get_paths(&self) -> Vec<String> {
            self.get_paths.lock().unwrap().clone()
        }

        fn with_group(mut self, group: &str, now: &str, members: &[&str]) -> Self {
            self.groups.insert(
                group.to_string(),
                serde_json::json!({"now": now, "all": members}),
            );
            self
        }

        fn with_put_failure(mut self, group: &str) -> Self {
            self.put_failures.insert(group.to_string());
            self
        }

        fn group_for_path(&self, path: &str) -> Option<&str> {
            self.groups
                .keys()
                .map(String::as_str)
                .find(|g| crate::mihomo_api::proxy_group_path(g) == path)
        }

        fn puts(&self) -> Vec<(String, String)> {
            self.puts.lock().unwrap().clone()
        }
    }

    impl crate::mihomo_api::MihomoApiClient for ReplayFakeClient {
        async fn get(&self, path: &str) -> Result<serde_json::Value> {
            *self.get_calls.lock().unwrap() += 1;
            self.get_paths.lock().unwrap().push(path.to_string());
            if path == "/proxies" {
                if let Some(data) = &self.proxies {
                    return Ok(data.clone());
                }
            }
            if let Some(delay) = self
                .group_for_path(path)
                .and_then(|g| self.slow_groups.get(g))
            {
                tokio::time::sleep(*delay).await;
            }
            match self.group_for_path(path) {
                Some(group) => Ok(self.groups[group].clone()),
                None => anyhow::bail!("mihomo API returned: HTTP/1.0 404 Not Found"),
            }
        }

        async fn get_bounded(&self, path: &str, _byte_limit: usize) -> Result<serde_json::Value> {
            self.get(path).await
        }

        async fn put(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
            let group = self
                .group_for_path(path)
                .map(ToString::to_string)
                .unwrap_or_default();
            let node = body["name"].as_str().unwrap_or_default().to_string();
            self.puts.lock().unwrap().push((group.clone(), node));
            if self.put_failures.contains(&group) {
                anyhow::bail!("mihomo API returned: HTTP/1.0 500 Internal Server Error");
            }
            Ok(serde_json::Value::Null)
        }

        async fn patch(&self, _path: &str, _body: serde_json::Value) -> Result<serde_json::Value> {
            unimplemented!("replay never patches")
        }

        async fn delete(&self, _path: &str) -> Result<serde_json::Value> {
            unimplemented!("replay never deletes")
        }
    }

    #[tokio::test]
    async fn replay_without_intent_file_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        setup_test_active_subscription(&paths, "sub-11111111");
        let client = ReplayFakeClient::new();
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert_eq!(report, ReplayReport::default());
        assert!(report.format_lines().is_empty());
    }

    #[tokio::test]
    async fn replay_applies_all_groups() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "A", "a1").unwrap();
        remember_selection_for_scope(&scope, "B", "b1").unwrap();
        let client = ReplayFakeClient::new()
            .with_group("A", "a2", &["a1", "a2"])
            .with_group("B", "b1", &["b1"]);
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert_eq!(report.results.len(), 2);
        assert!(report
            .results
            .iter()
            .all(|r| r.outcome == ReplayOutcome::Applied));
        assert_eq!(
            client.puts(),
            vec![
                ("A".to_string(), "a1".to_string()),
                ("B".to_string(), "b1".to_string())
            ]
        );
        assert_eq!(
            report.format_lines(),
            vec!["✓ Selections restored: A → a1, B → b1".to_string()]
        );
    }

    #[tokio::test]
    async fn sub_a_and_sub_b_selections_are_completely_isolated() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope_a = setup_test_active_subscription(&paths, "sub-aaaaaaaa");
        remember_selection_for_scope(&scope_a, "Proxy", "Node-A").unwrap();

        let scope_b = setup_test_active_subscription(&paths, "sub-bbbbbbbb");
        remember_selection_for_scope(&scope_b, "Proxy", "Node-B").unwrap();

        // sub-a selections preserved
        let sel_a = load_selection_state_for_scope(&scope_a).unwrap();
        assert_eq!(sel_a.get("Proxy"), Some(&"Node-A".to_string()));

        // sub-b selections preserved
        let sel_b = load_selection_state_for_scope(&scope_b).unwrap();
        assert_eq!(sel_b.get("Proxy"), Some(&"Node-B".to_string()));

        // unpin on sub-a does not touch sub-b
        unpin_selection_for_scope(&scope_a, "Proxy").unwrap();
        assert!(load_selection_state_for_scope(&scope_a).unwrap().is_empty());
        assert_eq!(
            load_selection_state_for_scope(&scope_b)
                .unwrap()
                .get("Proxy"),
            Some(&"Node-B".to_string())
        );
    }

    #[tokio::test]
    async fn replay_skips_missing_node_and_reports_current() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "A", "gone").unwrap();
        let client = ReplayFakeClient::new().with_group("A", "a2", &["a1", "a2"]);
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert_eq!(
            report.results[0].outcome,
            ReplayOutcome::SkippedNodeMissing {
                current: Some("a2".to_string()),
                members: vec!["a1".to_string(), "a2".to_string()],
            }
        );
        assert!(client.puts().is_empty());
        // Intent is preserved so a returning node replays later.
        assert_eq!(
            load_selection_state_for_scope(&scope).unwrap().get("A"),
            Some(&"gone".to_string())
        );
        assert_eq!(
            report.format_lines(),
            vec![
                "⚠ Selection A → gone not applied: node no longer in group (current: a2)"
                    .to_string()
            ]
        );
    }

    #[tokio::test]
    async fn replay_skips_missing_group() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "Ghost", "g1").unwrap();
        let client = ReplayFakeClient::new();
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert_eq!(
            report.results[0].outcome,
            ReplayOutcome::SkippedGroupMissing
        );
        assert!(client.puts().is_empty());
    }

    #[tokio::test]
    async fn replay_continues_after_single_group_put_failure() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "A", "a1").unwrap();
        remember_selection_for_scope(&scope, "B", "b1").unwrap();
        let client = ReplayFakeClient::new()
            .with_group("A", "a1", &["a1"])
            .with_group("B", "b1", &["b1"])
            .with_put_failure("A");
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert!(matches!(
            report.results[0].outcome,
            ReplayOutcome::Failed { .. }
        ));
        assert_eq!(report.results[1].outcome, ReplayOutcome::Applied);
    }

    #[tokio::test]
    async fn replay_corrupt_intent_degrades_without_api_calls() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        crate::utils::ensure_dir_all_no_follow(&paths.selections_dir()).unwrap();
        std::fs::write(&scope.path, "selections: [not: a map").unwrap();
        let client = ReplayFakeClient::new();
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert!(report.intent_error.is_some());
        assert_eq!(*client.get_calls.lock().unwrap(), 0);
        let lines = report.format_lines();
        assert!(lines[0].starts_with("⚠ Selection intent not replayed:"));
        assert!(lines[1].contains("select --unpin --all"));
        // Corrupt file is preserved for inspection.
        assert!(scope.path.exists());
    }

    #[tokio::test]
    async fn replay_applies_after_node_returns() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "A", "a1").unwrap();
        let missing = ReplayFakeClient::new().with_group("A", "a2", &["a2"]);
        let first = replay_selections_at(&paths, &missing).await.unwrap();
        assert!(matches!(
            first.results[0].outcome,
            ReplayOutcome::SkippedNodeMissing { .. }
        ));
        let returned = ReplayFakeClient::new().with_group("A", "a2", &["a1", "a2"]);
        let second = replay_selections_at(&paths, &returned).await.unwrap();
        assert_eq!(second.results[0].outcome, ReplayOutcome::Applied);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn replay_zero_budget_marks_all_budget_exceeded() {
        let client = ReplayFakeClient::new().with_group("A", "a1", &["a1"]);
        let report = replay_map_until(
            &std::collections::BTreeMap::from([("A".to_string(), "a1".to_string())]),
            &client,
            Instant::now() + Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(report.results[0].outcome, ReplayOutcome::BudgetExceeded);
        assert_eq!(*client.get_calls.lock().unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn selection_lock_rejects_symlink_target() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        crate::utils::ensure_dir_all_no_follow(paths.config_dir()).unwrap();
        let target = tmp.path().join("outside");
        std::fs::write(&target, b"outside").unwrap();
        std::os::unix::fs::symlink(&target, paths.config_dir().join(".selection-state.lock"))
            .unwrap();
        assert!(acquire_selection_lock_at(&paths).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"outside");
    }

    #[tokio::test]
    async fn replay_deadline_is_shared_by_all_requests() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "A", "a1").unwrap();
        let client = ReplayFakeClient::new().with_group("A", "a1", &["a1"]);
        let report = replay_selections_until(&paths, &client, Instant::now())
            .await
            .unwrap();
        assert_eq!(report.results[0].outcome, ReplayOutcome::BudgetExceeded);
        assert_eq!(*client.get_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn replay_group_list_fetched_once_within_shared_budget() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "Gone-A", "a1").unwrap();
        remember_selection_for_scope(&scope, "Gone-B", "b1").unwrap();
        let client = ReplayFakeClient::new().with_proxies(serde_json::json!({
            "proxies": {
                "GLOBAL": {"all": ["X"]},
                "COMPATIBLE": {"all": ["X"]},
                "X 组": {"all": ["X"]},
                "Y": {"all": ["Y"]},
                "node-only": {"type": "Direct"}
            }
        }));
        let report = replay_selections_until(&paths, &client, Instant::now() + REPLAY_TOTAL_BUDGET)
            .await
            .unwrap();
        assert!(report
            .results
            .iter()
            .all(|r| r.outcome == ReplayOutcome::SkippedGroupMissing));
        // One shared /proxies query for both missing groups (no per-group 5s).
        assert_eq!(
            client
                .get_paths()
                .iter()
                .filter(|p| p.as_str() == "/proxies")
                .count(),
            1
        );
        assert_eq!(
            report.selectable_groups,
            Some(vec!["X 组".to_string(), "Y".to_string()])
        );
        let lines = selection_replay_remedy_lines(&report).join("\n");
        assert!(lines.contains("Current groups: X 组, Y"), "{lines}");
    }

    #[test]
    fn remedy_query_budget_stops_at_deadline() {
        // 超尽即停止补救查询：deadline 已到（或已过）时不发 /proxies。
        assert_eq!(remedy_query_budget(Instant::now()), None);
        assert_eq!(
            remedy_query_budget(Instant::now() - Duration::from_secs(1)),
            None
        );
        assert!(remedy_query_budget(Instant::now() + Duration::from_secs(1)).is_some());
    }

    #[tokio::test]
    async fn replay_group_list_query_skipped_when_budget_exhausted() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "Gone", "a1").unwrap();
        remember_selection_for_scope(&scope, "Slow", "b1").unwrap();
        // "Gone" 404s immediately (group list needed); "Slow" burns the shared
        // budget, so remediation must not fabricate a group list.
        let client = ReplayFakeClient::new().with_slow_group("Slow", Duration::from_secs(5));
        let report =
            replay_selections_until(&paths, &client, Instant::now() + Duration::from_millis(300))
                .await
                .unwrap();
        assert_eq!(
            report.results[0].outcome,
            ReplayOutcome::SkippedGroupMissing
        );
        assert_eq!(report.selectable_groups, None);
        let lines = selection_replay_remedy_lines(&report).join("\n");
        assert!(
            lines.contains("Group list unavailable (budget exhausted or query failed)"),
            "{lines}"
        );
    }

    #[test]
    fn replay_format_lines_sanitizes_untrusted_names() {
        // Group/node names and error text come from Core and from persisted
        // files; CR/LF/ESC must not forge extra control lines.
        let report = ReplayReport {
            results: vec![ReplayGroupResult {
                group: "G\n[selection-mirror-missing]".to_string(),
                node: "n\r\n✓ Selections restored: forged".to_string(),
                outcome: ReplayOutcome::Failed {
                    error: "boom\x1b[31m\n[selection-identity-mismatch] x".to_string(),
                },
            }],
            intent_error: None,
            selectable_groups: None,
        };
        let lines = report.format_lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        for bad in ['\n', '\r', '\x1b'] {
            assert!(!lines[0].contains(bad), "control char {bad:?}: {lines:?}");
        }
        assert!(
            !lines[0].starts_with("[selection-mirror-missing]"),
            "{lines:?}"
        );
        assert!(
            !lines[0].starts_with("[selection-identity-mismatch]"),
            "{lines:?}"
        );
    }

    #[test]
    fn unpin_removes_only_the_named_group() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "A", "a1").unwrap();
        remember_selection_for_scope(&scope, "B", "b1").unwrap();
        assert!(unpin_selection_for_scope(&scope, "A").unwrap());
        assert!(!unpin_selection_for_scope(&scope, "A").unwrap());
        let selections = load_selection_state_for_scope(&scope).unwrap();
        assert!(!selections.contains_key("A"));
        assert_eq!(selections.get("B"), Some(&"b1".to_string()));
    }

    /// 交错注入方式：让 `select` 的临界区（持锁、已读入映射、尚未落盘）真实
    /// 存在于另一个线程/时间点上，清理只可能在锁内或锁外二选一，不靠 sleep
    /// 抢占去赌调度顺序。
    fn assert_unpin_serializes_with_select(
        unpin: fn(&SelectionScope) -> Result<usize>,
        expect_empty: bool,
    ) {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "Gone", "g1").unwrap();

        let guard = acquire_selection_lock_at(&paths).unwrap();
        let mut select_view = load_selection_state_for_scope(&scope).unwrap();
        select_view.insert("B".to_string(), "b1".to_string());

        let (tx, rx) = std::sync::mpsc::channel();
        let unpin_scope = scope.clone();
        let handle = std::thread::spawn(move || {
            tx.send(unpin(&unpin_scope).is_ok()).unwrap();
        });

        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "selection 清理在 select 临界区内完成了落盘：丢失更新窗口仍在"
        );

        save_selection_state_for_scope(&scope, &select_view).unwrap();
        drop(guard);

        assert!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "锁释放后清理必须成功"
        );
        handle.join().unwrap();

        let selections = load_selection_state_for_scope(&scope).unwrap();
        assert!(
            !selections.contains_key("Gone"),
            "被删组的记录必须清除：{selections:?}"
        );
        if expect_empty {
            assert!(
                selections.is_empty(),
                "清理全部必须在 select 落盘之后执行（不得留下复活记录）：{selections:?}"
            );
        } else {
            assert_eq!(
                selections.get("B"),
                Some(&"b1".to_string()),
                "select 在临界区内落盘的记录不得被清理回写覆盖/复活：{selections:?}"
            );
        }
    }

    #[test]
    fn unpin_waits_for_the_selection_lock_and_keeps_a_concurrent_select() {
        assert_unpin_serializes_with_select(
            |scope| unpin_selection_for_scope(scope, "Gone").map(usize::from),
            false,
        );
    }

    #[test]
    fn unpin_all_waits_for_the_selection_lock() {
        assert_unpin_serializes_with_select(unpin_all_selections_for_scope, true);
    }

    #[test]
    fn unpin_all_clears_and_resets_corrupt_intent() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let scope = setup_test_active_subscription(&paths, "sub-11111111");
        remember_selection_for_scope(&scope, "A", "a1").unwrap();
        remember_selection_for_scope(&scope, "B", "b1").unwrap();
        assert_eq!(unpin_all_selections_for_scope(&scope).unwrap(), 2);
        assert!(load_selection_state_for_scope(&scope).unwrap().is_empty());

        crate::utils::ensure_dir_all_no_follow(&paths.selections_dir()).unwrap();
        std::fs::write(&scope.path, "selections: [not: a map").unwrap();
        assert_eq!(unpin_all_selections_for_scope(&scope).unwrap(), 0);
        assert!(load_selection_state_for_scope(&scope).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn save_selection_inherits_parent_setgid() {
        use std::os::unix::fs::MetadataExt;

        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let _ = crate::utils::set_directory_mode_no_follow(paths.config_dir(), 0o2755);
        let parent_meta = std::fs::metadata(paths.config_dir()).unwrap();
        if crate::utils::mode_has_setgid(parent_meta.mode()) {
            let scope = SelectionScope {
                subscription_id: "sub-setgid-test".to_string(),
                path: paths.selections_dir().join("sub-setgid-test.yaml"),
            };
            save_selection_state_for_scope(&scope, &BTreeMap::new()).unwrap();

            let sel_dir = paths.selections_dir();
            let sel_meta = std::fs::metadata(&sel_dir).unwrap();
            assert!(crate::utils::mode_has_setgid(sel_meta.mode()));
        }
    }
}
