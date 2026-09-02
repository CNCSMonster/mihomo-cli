//! Selection intent persistence (selection-state.yaml) and the per-instance
//! selection lock that serializes select (kernel PUT + persist) with replay
//! (re-read + PUT). See SPEC.md §3.8.

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
            #[cfg(test)]
            {
                return Ok(SelectionScope {
                    subscription_id: "test-legacy".to_string(),
                    path: paths.selection_state_path(),
                });
            }
            #[cfg(not(test))]
            {
                return Err(anyhow::anyhow!(
                    "No active subscription. Run: mihomo-cli config --add <URL> or --import <FILE>"
                ));
            }
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
    let serialized = serde_yaml::to_string(&state)?;
    crate::utils::atomic_write_file_for_original_user(&target.display().to_string(), &serialized)?;
    let migrated = paths.config_dir().join("selection-state.yaml.legacy");
    std::fs::rename(&legacy, &migrated).with_context(|| {
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

fn load_selection_state_path(path: &std::path::Path) -> Result<BTreeMap<String, String>> {
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
    crate::utils::ensure_dir_all_no_follow(scope.path.parent().unwrap())?;
    let state = SelectionStateFile {
        selections: selections.clone(),
    };
    let content = serde_yaml::to_string(&state)?;
    crate::utils::atomic_write_file_for_original_user(&scope.path.display().to_string(), &content)?;
    Ok(())
}

pub fn remember_selection_for_scope(scope: &SelectionScope, group: &str, node: &str) -> Result<()> {
    let mut selections = load_selection_state_for_scope(scope)?;
    selections.insert(group.to_string(), node.to_string());
    save_selection_state_for_scope(scope, &selections)
}

#[cfg(test)]
pub fn load_selection_state_at(paths: &AppPaths) -> Result<BTreeMap<String, String>> {
    load_selection_state_path(&paths.selection_state_path())
}

#[cfg(test)]
pub fn save_selection_state_at(
    paths: &AppPaths,
    selections: &BTreeMap<String, String>,
) -> Result<()> {
    crate::utils::ensure_dir_all_no_follow(paths.config_dir())?;
    let state = SelectionStateFile {
        selections: selections.clone(),
    };
    let content = serde_yaml::to_string(&state)?;
    crate::utils::atomic_write_file_for_original_user(
        &paths.selection_state_path().display().to_string(),
        &format!("# Last selections made by mihomo-cli; used only for drift warnings.\n{content}"),
    )?;
    Ok(())
}

#[cfg(test)]
pub fn remember_selection_at(paths: &AppPaths, group: &str, node: &str) -> Result<()> {
    let mut selections = load_selection_state_at(paths)?;
    selections.insert(group.to_string(), node.to_string());
    save_selection_state_at(paths, &selections)
}

pub fn unpin_selection_for_scope(scope: &SelectionScope, group: &str) -> Result<bool> {
    let mut selections = load_selection_state_for_scope(scope)?;
    let removed = selections.remove(group).is_some();
    if removed {
        save_selection_state_for_scope(scope, &selections)?;
    }
    Ok(removed)
}

pub fn unpin_all_selections_for_scope(scope: &SelectionScope) -> Result<usize> {
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

/// Remove one group's persisted selection; returns true if a record existed.
/// Never touches runtime state (SPEC §4-5).
#[cfg(test)]
pub fn unpin_selection_at(paths: &AppPaths, group: &str) -> Result<bool> {
    let _guard = acquire_selection_lock_at(paths)?;
    let mut selections = load_selection_state_at(paths)?;
    let removed = selections.remove(group).is_some();
    if removed {
        save_selection_state_at(paths, &selections)?;
    }
    Ok(removed)
}

/// Remove all persisted selections; returns how many were removed.
/// A corrupt intent file is reset to empty: this is the documented repair
/// path for parse failures (SPEC §3.3-5), so it must not fail on them.
#[cfg(test)]
pub fn unpin_all_selections_at(paths: &AppPaths) -> Result<usize> {
    let _guard = acquire_selection_lock_at(paths)?;
    match load_selection_state_at(paths) {
        Ok(selections) if selections.is_empty() => Ok(0),
        Ok(selections) => {
            let removed = selections.len();
            save_selection_state_at(paths, &BTreeMap::new())?;
            Ok(removed)
        }
        Err(_) => {
            save_selection_state_at(paths, &BTreeMap::new())?;
            Ok(0)
        }
    }
}

/// Guard holding the per-instance selection lock (`<config_dir>/.selection-state.lock`).
/// The lock is released when the guard drops (the underlying file handle closes).
pub struct SelectionLockGuard {
    _file: std::fs::File,
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
            Ok(()) => return Ok(SelectionLockGuard { _file: file }),
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
    SkippedNodeMissing { current: Option<String> },
    Failed { error: String },
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
}

impl ReplayReport {
    pub fn format_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(err) = &self.intent_error {
            lines.push(format!("⚠ Selection intent not replayed: {err}"));
            lines.push(
                "  Run: mihomo-cli select --unpin --all  (to reset stored selections)".to_string(),
            );
            return lines;
        }
        let applied: Vec<String> = self
            .results
            .iter()
            .filter(|r| r.outcome == ReplayOutcome::Applied)
            .map(|r| format!("{} → {}", r.group, r.node))
            .collect();
        if !applied.is_empty() {
            lines.push(format!("✓ Selections restored: {}", applied.join(", ")));
        }
        for r in &self.results {
            let reason = match &r.outcome {
                ReplayOutcome::Applied => continue,
                ReplayOutcome::SkippedGroupMissing => "group no longer exists".to_string(),
                ReplayOutcome::SkippedNodeMissing { current } => match current {
                    Some(current) => {
                        format!("node no longer in group (current: {current})")
                    }
                    None => "node no longer in group".to_string(),
                },
                ReplayOutcome::Failed { error } => format!("apply failed: {error}"),
                ReplayOutcome::BudgetExceeded => "time budget exceeded".to_string(),
            };
            lines.push(format!(
                "⚠ Selection {} → {} not applied: {reason}",
                r.group, r.node
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

#[cfg(unix)]
pub async fn replay_scope_until(
    scope: &SelectionScope,
    client: &impl crate::mihomo_api::MihomoApiClient,
    deadline: Instant,
) -> Result<ReplayReport> {
    replay_scope_with_deadline(scope, client, deadline, REPLAY_PER_GROUP_TIMEOUT).await
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
            });
        }
    };
    if selections.is_empty() {
        return Ok(ReplayReport::default());
    }
    let mut results = Vec::new();
    for (group, node) in selections {
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
    Ok(ReplayReport {
        results,
        intent_error: None,
    })
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

    #[test]
    fn remember_selection_round_trips_under_lock() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let _guard = acquire_selection_lock_at(&paths).unwrap();
        remember_selection_at(&paths, "OpenAI", "US-01").unwrap();
        let selections = load_selection_state_at(&paths).unwrap();
        assert_eq!(selections.get("OpenAI"), Some(&"US-01".to_string()));
    }

    struct ReplayFakeClient {
        groups: BTreeMap<String, serde_json::Value>,
        put_failures: std::collections::BTreeSet<String>,
        puts: std::sync::Mutex<Vec<(String, String)>>,
        get_calls: std::sync::Mutex<usize>,
    }

    impl ReplayFakeClient {
        fn new() -> Self {
            Self {
                groups: BTreeMap::new(),
                put_failures: std::collections::BTreeSet::new(),
                puts: std::sync::Mutex::new(Vec::new()),
                get_calls: std::sync::Mutex::new(0),
            }
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
            match self.group_for_path(path) {
                Some(group) => Ok(self.groups[group].clone()),
                None => anyhow::bail!("mihomo API returned: HTTP/1.0 404 Not Found"),
            }
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
        let client = ReplayFakeClient::new();
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert_eq!(report, ReplayReport::default());
        assert!(report.format_lines().is_empty());
    }

    #[tokio::test]
    async fn replay_applies_all_groups() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        remember_selection_at(&paths, "A", "a1").unwrap();
        remember_selection_at(&paths, "B", "b1").unwrap();
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
    async fn replay_skips_missing_node_and_reports_current() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        remember_selection_at(&paths, "A", "gone").unwrap();
        let client = ReplayFakeClient::new().with_group("A", "a2", &["a1", "a2"]);
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert_eq!(
            report.results[0].outcome,
            ReplayOutcome::SkippedNodeMissing {
                current: Some("a2".to_string())
            }
        );
        assert!(client.puts().is_empty());
        // Intent is preserved so a returning node replays later.
        assert_eq!(
            load_selection_state_at(&paths).unwrap().get("A"),
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
        remember_selection_at(&paths, "Ghost", "g1").unwrap();
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
        remember_selection_at(&paths, "A", "a1").unwrap();
        remember_selection_at(&paths, "B", "b1").unwrap();
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
        crate::utils::ensure_dir_all_no_follow(paths.config_dir()).unwrap();
        std::fs::write(paths.selection_state_path(), "selections: [not: a map").unwrap();
        let client = ReplayFakeClient::new();
        let report = replay_selections_at(&paths, &client).await.unwrap();
        assert!(report.intent_error.is_some());
        assert_eq!(*client.get_calls.lock().unwrap(), 0);
        let lines = report.format_lines();
        assert!(lines[0].starts_with("⚠ Selection intent not replayed:"));
        assert!(lines[1].contains("select --unpin --all"));
        // Corrupt file is preserved for inspection.
        assert!(paths.selection_state_path().exists());
    }

    #[tokio::test]
    async fn replay_applies_after_node_returns() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        remember_selection_at(&paths, "A", "a1").unwrap();
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

    #[tokio::test]
    async fn replay_zero_budget_marks_all_budget_exceeded() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        remember_selection_at(&paths, "A", "a1").unwrap();
        let client = ReplayFakeClient::new().with_group("A", "a1", &["a1"]);
        let report = replay_scope_until(
            &SelectionScope {
                subscription_id: "test-legacy".to_string(),
                path: paths.selection_state_path(),
            },
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
        remember_selection_at(&paths, "A", "a1").unwrap();
        let client = ReplayFakeClient::new().with_group("A", "a1", &["a1"]);
        let report = replay_selections_until(&paths, &client, Instant::now())
            .await
            .unwrap();
        assert_eq!(report.results[0].outcome, ReplayOutcome::BudgetExceeded);
        assert_eq!(*client.get_calls.lock().unwrap(), 0);
    }

    #[test]
    fn unpin_removes_only_the_named_group() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        remember_selection_at(&paths, "A", "a1").unwrap();
        remember_selection_at(&paths, "B", "b1").unwrap();
        assert!(unpin_selection_at(&paths, "A").unwrap());
        assert!(!unpin_selection_at(&paths, "A").unwrap());
        let selections = load_selection_state_at(&paths).unwrap();
        assert!(!selections.contains_key("A"));
        assert_eq!(selections.get("B"), Some(&"b1".to_string()));
    }

    #[test]
    fn unpin_all_clears_and_resets_corrupt_intent() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        remember_selection_at(&paths, "A", "a1").unwrap();
        remember_selection_at(&paths, "B", "b1").unwrap();
        assert_eq!(unpin_all_selections_at(&paths).unwrap(), 2);
        assert!(load_selection_state_at(&paths).unwrap().is_empty());

        crate::utils::ensure_dir_all_no_follow(paths.config_dir()).unwrap();
        std::fs::write(paths.selection_state_path(), "selections: [not: a map").unwrap();
        assert_eq!(unpin_all_selections_at(&paths).unwrap(), 0);
        assert!(load_selection_state_at(&paths).unwrap().is_empty());
    }
}
