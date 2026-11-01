use crate::{
    instance::ApiEndpoint,
    utils::{self, AppPaths},
};
use anyhow::Context;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Multi-subscription data structures ──

/// Subscription metadata stored in subscriptions.yaml
///
/// (SPEC-selection-mirror-resilience A1'): `config --remove` marks
/// the entry `retired` instead of deleting it, so a byte-identical URL re-add
/// revives the same subscription id and keeps its selection/overlay slots
/// reachable. Entries without the field (older files) read as alive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionMeta {
    pub id: String,
    pub url: String,
    pub updated: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent_mode: Option<UserAgentMode>,
    #[serde(default, skip_serializing_if = "retired_flag_is_false")]
    pub retired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<DateTime<Utc>>,
}

fn retired_flag_is_false(value: &bool) -> bool {
    !*value
}

impl SubscriptionMeta {
    pub fn is_alive(&self) -> bool {
        !self.retired
    }
}

/// Tombstone FIFO cap: at most this many retired entries are kept in
/// subscriptions.yaml; older ones are evicted with a single log line.
pub const RETIRED_TOMBSTONE_LIMIT: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UserAgentMode {
    Auto,
    Fixed,
}

/// Generate a new subscription ID: `sub-` + 8 hex chars
pub fn generate_subscription_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let hex: String = (0..4).map(|_| format!("{:02x}", rng.gen::<u8>())).collect();
    format!("sub-{hex}")
}

fn validate_subscription_id(id: &str) -> anyhow::Result<()> {
    if id.len() != 12
        || !id.starts_with("sub-")
        || !id.as_bytes()[4..].iter().all(u8::is_ascii_hexdigit)
    {
        anyhow::bail!("invalid subscription ID: {id}");
    }
    Ok(())
}

/// Load subscription metadata from subscriptions.yaml
pub fn load_subscriptions_at(paths: &AppPaths) -> anyhow::Result<Vec<SubscriptionMeta>> {
    let path = paths.subscriptions_meta_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;
    let subs: Vec<SubscriptionMeta> =
        serde_yaml::from_str(&content).with_context(|| "Failed to parse subscriptions.yaml")?;
    Ok(subs)
}

/// Save subscription metadata to subscriptions.yaml (atomic write)
pub fn save_subscriptions_at(paths: &AppPaths, subs: &[SubscriptionMeta]) -> anyhow::Result<()> {
    let path = paths.subscriptions_meta_path();
    ensure_config_directory(paths.config_dir())?;
    let content = serde_yaml::to_string(subs)?;
    utils::atomic_write_file_for_original_user(&path.display().to_string(), &content)?;
    Ok(())
}

fn ensure_config_directory(path: &std::path::Path) -> anyhow::Result<()> {
    utils::ensure_dir_all_no_follow(path)?;
    if let Some(parent) = path.parent() {
        utils::restore_original_user_config_ownership(parent)?;
    }
    utils::restore_original_user_config_ownership(path)
}

pub(crate) fn ensure_subscriptions_directory(paths: &AppPaths) -> anyhow::Result<()> {
    let path = paths.subscriptions_dir();
    utils::ensure_dir_all_no_follow(&path)?;
    utils::restore_original_user_config_ownership(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent_has_setgid = path
            .parent()
            .and_then(|parent| std::fs::symlink_metadata(parent).ok())
            .map(|meta| utils::mode_has_setgid(meta.permissions().mode()))
            .unwrap_or(false);
        if parent_has_setgid {
            utils::set_directory_mode_no_follow(&path, 0o2755)?;
        }
    }
    Ok(())
}

/// Get the active subscription ID from subscriptions/active
pub fn get_active_id_at(paths: &AppPaths) -> anyhow::Result<Option<String>> {
    let path = paths.active_file_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read: {}", path.display()))?;
    let id = content.trim().to_string();
    if id.is_empty() {
        Ok(None)
    } else {
        Ok(Some(id))
    }
}

/// Set the active subscription ID in subscriptions/active
pub fn set_active_id_at(paths: &AppPaths, id: &str) -> anyhow::Result<()> {
    let path = paths.active_file_path();
    ensure_subscriptions_directory(paths)?;
    utils::atomic_write_file_for_original_user(&path.display().to_string(), id)?;
    Ok(())
}

/// Find a subscription by ID
pub fn find_subscription<'a>(
    subs: &'a [SubscriptionMeta],
    id: &str,
) -> Option<&'a SubscriptionMeta> {
    subs.iter().find(|s| s.id == id)
}

// ── Phase 4: Subscription CRUD ──

/// Downloaded, converted and validated subscription content awaiting commit.
#[derive(Debug, Clone)]
pub struct PreparedSubscription {
    pub yaml_content: String,
}

/// Download + convert + validate a subscription **without touching disk**.
///
/// (A1' concurrency contract): the network fetch stays outside the
/// config write lock; the URL identity match and the commit happen together
/// inside one critical section (`commit_subscription_matched_at`).
pub async fn prepare_subscription_with_user_agent(
    url: &str,
    user_agent: Option<String>,
) -> anyhow::Result<PreparedSubscription> {
    let result = download_sub_with_user_agent(url, user_agent.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("Cannot reach subscription URL.\n  {e}"))?;
    let content = result.content;
    let is_yaml = result.is_clash_yaml;

    let yaml_content = if is_yaml {
        content
    } else {
        warn_raw_subscription_conversion();
        crate::log!("Converting subscription format (vmess/base64 → Clash YAML)...");
        convert_vmess_to_clash(&content)?
    };

    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).context("Subscription content is not valid YAML")?;
    validate_subscription_yaml(&yaml)?;
    Ok(PreparedSubscription { yaml_content })
}

/// Outcome of a URL-matched subscription commit (A1').
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionCommitKind {
    /// No entry holds this URL: the usual new-identity flow.
    New,
    /// A living entry already holds this byte-identical URL: reuse its id
    /// (equivalent to refresh — snapshot/rollback semantics, never the
    /// delete-style new-subscription rollback).
    Reuse,
    /// The most recent retired entry held this URL: revive it with its id,
    /// selections and overlay intact.
    Revive {
        retired_at: Option<DateTime<Utc>>,
        /// Same-URL tombstones that stay retired (single-line hint).
        other_retired: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct SubscriptionCommit {
    pub id: String,
    pub kind: SubscriptionCommitKind,
}

/// True pre-images of one commit transaction, captured inside the config
/// write lock immediately before the commit writes (A1' defect 1/2):
/// rollback restores exactly what THIS transaction overwrote — never a
/// pre-download snapshot that could clobber a concurrent successful commit.
#[derive(Debug)]
pub(crate) struct SubscriptionPreImages {
    cache: Option<String>,
    metadata: Option<String>,
    active: Option<String>,
    /// Whether this transaction modified the active pointer at all; the
    /// active pre-image is only restored when it did. Set to `true` by a
    /// caller critical section that activates outside `commit` (e.g. the
    /// interactive activate prompt) so its rollback also restores `active`.
    pub(crate) activated: bool,
}

impl SubscriptionPreImages {
    /// Restore cache + metadata to this transaction's old state; restore the
    /// active pointer only if this transaction changed it.
    pub(crate) fn restore(self, paths: &AppPaths, id: &str) -> anyhow::Result<()> {
        let mut errors = Vec::new();
        if let Err(error) = restore_optional_file(&paths.subscription_file_path(id), self.cache) {
            errors.push(format!("subscription cache: {error}"));
        }
        if let Err(error) = restore_optional_file(&paths.subscriptions_meta_path(), self.metadata) {
            errors.push(format!("subscriptions metadata: {error}"));
        }
        if self.activated {
            if let Err(error) = restore_optional_file(&paths.active_file_path(), self.active) {
                errors.push(format!("active subscription: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(errors.join("; "))
        }
    }
}

/// URL-matched commit: reload metadata inside the config write lock, resolve
/// the identity (living > newest retired > new), then commit cache + metadata
/// + active pointer as one rollback-protected transaction.
pub fn commit_subscription_matched_at(
    paths: &AppPaths,
    url: &str,
    yaml_content: &str,
    user_agent: Option<String>,
    activate: Option<bool>,
) -> anyhow::Result<SubscriptionCommit> {
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let mut subs = load_subscriptions_at(paths)?;
    let (kind, id) = resolve_subscription_identity(&subs, url)?;
    commit_subscription_entry_at(
        paths,
        &id,
        kind,
        yaml_content,
        &mut subs,
        url,
        user_agent,
        activate,
    )
    .map(|(commit, _pre_images)| commit)
}

/// Lock-internal commit transaction with a caller-supplied critical section
/// (A1' defect 1): identity match, pre-images, commit, `after_commit`
/// (e.g. activate prompt + config merge validation) and failure rollback all
/// happen inside ONE config write lock hold. `after_commit` failures roll the
/// commit back to this transaction's true pre-images, so a command that fails
/// after downloading can never overwrite another command's successful commit.
pub(crate) fn commit_subscription_matched_checked_at(
    paths: &AppPaths,
    url: &str,
    yaml_content: &str,
    user_agent: Option<String>,
    activate: Option<bool>,
    after_commit: impl FnOnce(&SubscriptionCommit, &mut SubscriptionPreImages) -> anyhow::Result<()>,
) -> anyhow::Result<SubscriptionCommit> {
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let mut subs = load_subscriptions_at(paths)?;
    let (kind, id) = resolve_subscription_identity(&subs, url)?;
    let (commit, mut pre_images) = commit_subscription_entry_at(
        paths,
        &id,
        kind,
        yaml_content,
        &mut subs,
        url,
        user_agent,
        activate,
    )?;
    if let Err(error) = after_commit(&commit, &mut pre_images) {
        if let Err(rollback_error) = pre_images.restore(paths, &commit.id) {
            anyhow::bail!(
                "Subscription change failed and rollback incomplete: {rollback_error}.\n  Original error: {error}"
            );
        }
        anyhow::bail!("Subscription change failed; last-known-good state restored.\n  {error}");
    }
    Ok(commit)
}

/// Resolve the byte-exact URL identity inside the caller's critical section:
/// living match (fail-fast when ambiguous) > newest tombstone > brand new.
fn resolve_subscription_identity(
    subs: &[SubscriptionMeta],
    url: &str,
) -> anyhow::Result<(SubscriptionCommitKind, String)> {
    let alive_matches: Vec<usize> = subs
        .iter()
        .enumerate()
        .filter(|(_, sub)| sub.is_alive() && sub.url == url)
        .map(|(index, _)| index)
        .collect();
    if alive_matches.len() > 1 {
        anyhow::bail!(ambiguous_alive_subscription_message(subs, &alive_matches));
    }

    let retired_matches: Vec<usize> = if alive_matches.is_empty() {
        subs.iter()
            .enumerate()
            .filter(|(_, sub)| !sub.is_alive() && sub.url == url)
            .map(|(index, _)| index)
            .collect()
    } else {
        Vec::new()
    };
    // Newest tombstone wins; entries without retired_at sort oldest.
    let newest_retired = retired_matches.iter().copied().max_by_key(|index| {
        subs[*index]
            .retired_at
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
    });

    if let Some(&index) = alive_matches.first() {
        Ok((SubscriptionCommitKind::Reuse, subs[index].id.clone()))
    } else if let Some(index) = newest_retired {
        let other_retired = retired_matches
            .iter()
            .filter(|candidate| **candidate != index)
            .map(|candidate| subs[*candidate].id.clone())
            .collect();
        let kind = SubscriptionCommitKind::Revive {
            retired_at: subs[index].retired_at,
            other_retired,
        };
        Ok((kind, subs[index].id.clone()))
    } else {
        Ok((SubscriptionCommitKind::New, generate_subscription_id()))
    }
}

fn ambiguous_alive_subscription_message(subs: &[SubscriptionMeta], alive: &[usize]) -> String {
    let mut message = String::from(
        "Subscription URL is already registered under multiple living subscriptions; converge to one identity first:",
    );
    for index in alive {
        message.push_str(&format!(
            "\n  {} (updated {})",
            subs[*index].id,
            subs[*index].updated.format("%Y-%m-%d %H:%M:%S UTC")
        ));
    }
    let keeper = &subs[alive[0]].id;
    message.push_str(&format!(
        "\n  Keep {keeper}: mihomo-cli config --switch {keeper}"
    ));
    for index in alive.iter().skip(1) {
        message.push_str(&format!(
            "\n  Then remove the other: mihomo-cli config --remove {}",
            subs[*index].id
        ));
    }
    message.push_str(&format!(
        "\n  (one switch + {} remove(s); removing does not delete its identity, no re-add required)",
        alive.len().saturating_sub(1)
    ));
    message
}

/// Add a new subscription: download → save → update metadata → set active if first
pub async fn add_subscription_at(paths: &AppPaths, url: &str) -> anyhow::Result<String> {
    Ok(add_subscription_at_with_user_agent(paths, url, None, None)
        .await?
        .id)
}

/// Add a subscription with optional user-agent and explicit activate control.
/// `activate`: Some(true) = force activate, Some(false) = force skip, None = auto (first only).
pub async fn add_subscription_at_with_user_agent(
    paths: &AppPaths,
    url: &str,
    user_agent: Option<String>,
    activate: Option<bool>,
) -> anyhow::Result<SubscriptionCommit> {
    let prepared = prepare_subscription_with_user_agent(url, user_agent.clone()).await?;
    let commit =
        commit_subscription_matched_at(paths, url, &prepared.yaml_content, user_agent, activate)?;

    crate::log!(
        "Added subscription {} with {} lines",
        commit.id,
        prepared.yaml_content.lines().count()
    );
    Ok(commit)
}

fn snapshot_optional_file(path: &std::path::Path) -> anyhow::Result<Option<String>> {
    if path.exists() {
        Ok(Some(std::fs::read_to_string(path)?))
    } else {
        Ok(None)
    }
}

fn restore_optional_file(path: &std::path::Path, snapshot: Option<String>) -> anyhow::Result<()> {
    match snapshot {
        Some(content) => {
            utils::atomic_write_file_for_original_user(&path.display().to_string(), &content)?
        }
        None => utils::remove_file_if_exists(path)?,
    }
    Ok(())
}

#[cfg(test)]
fn commit_new_subscription_at(
    paths: &AppPaths,
    id: &str,
    yaml_content: &str,
    subs: &mut Vec<SubscriptionMeta>,
    url: &str,
    user_agent: Option<String>,
    activate: Option<bool>,
) -> anyhow::Result<()> {
    commit_subscription_entry_at(
        paths,
        id,
        SubscriptionCommitKind::New,
        yaml_content,
        subs,
        url,
        user_agent,
        activate,
    )?;
    Ok(())
}

/// Commit cache + metadata + active pointer for one resolved identity.
///
/// Rollback is restore-only: a pre-existing cache/metadata pair is always
/// restored byte-for-byte, never deleted — reuse/revive paths must not hit
/// the delete-style rollback that would erase last-known-good state
/// (A1' rollback contract).
#[allow(clippy::too_many_arguments)]
fn commit_subscription_entry_at(
    paths: &AppPaths,
    id: &str,
    kind: SubscriptionCommitKind,
    yaml_content: &str,
    subs: &mut Vec<SubscriptionMeta>,
    url: &str,
    user_agent: Option<String>,
    activate: Option<bool>,
) -> anyhow::Result<(SubscriptionCommit, SubscriptionPreImages)> {
    let cache_path = paths.subscription_file_path(id);
    let metadata_path = paths.subscriptions_meta_path();
    let active_path = paths.active_file_path();
    let cache_snapshot = snapshot_optional_file(&cache_path)?;
    let metadata_snapshot = snapshot_optional_file(&metadata_path)?;
    let active_snapshot = snapshot_optional_file(&active_path)?;
    let fixed_user_agent = user_agent.is_some();

    // Resolve the identity before touching disk: a missing Reuse/Revive entry
    // must fail without having rewritten the cache.
    match &kind {
        SubscriptionCommitKind::New => subs.push(SubscriptionMeta {
            id: id.to_string(),
            url: url.to_string(),
            updated: Utc::now(),
            user_agent,
            user_agent_mode: Some(if fixed_user_agent {
                UserAgentMode::Fixed
            } else {
                UserAgentMode::Auto
            }),
            retired: false,
            retired_at: None,
        }),
        SubscriptionCommitKind::Reuse | SubscriptionCommitKind::Revive { .. } => {
            let entry = subs
                .iter_mut()
                .find(|sub| sub.id == id)
                .ok_or_else(|| anyhow::anyhow!("Subscription not found: {id}"))?;
            entry.updated = Utc::now();
            if let Some(ua) = user_agent {
                entry.user_agent = Some(ua);
                entry.user_agent_mode = Some(UserAgentMode::Fixed);
            }
            // Revive: identity, selections slot and overlay continue; only the
            // tombstone markers are cleared (A1').
            entry.retired = false;
            entry.retired_at = None;
        }
    }
    let should_activate = match activate {
        Some(value) => value,
        // "First subscription" counts living entries only, so a pile of
        // tombstones does not suppress first-add activation.
        None => subs.iter().filter(|sub| sub.is_alive()).count() == 1,
    };

    ensure_subscriptions_directory(paths)?;
    utils::atomic_write_file_for_original_user(&cache_path.display().to_string(), yaml_content)?;

    let result = (|| {
        save_subscriptions_at(paths, subs)?;
        if should_activate {
            set_active_id_at(paths, id)?;
        }
        Ok::<(), anyhow::Error>(())
    })();

    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        if let Err(rollback_error) = restore_optional_file(&cache_path, cache_snapshot.clone()) {
            rollback_errors.push(format!("subscription cache: {rollback_error}"));
        }
        if let Err(rollback_error) =
            restore_optional_file(&metadata_path, metadata_snapshot.clone())
        {
            rollback_errors.push(format!("subscriptions metadata: {rollback_error}"));
        }
        if let Err(rollback_error) = restore_optional_file(&active_path, active_snapshot.clone()) {
            rollback_errors.push(format!("active subscription: {rollback_error}"));
        }
        if rollback_errors.is_empty() {
            return Err(error);
        }
        anyhow::bail!(
            "subscription add rollback incomplete: {}. Original error: {}",
            rollback_errors.join("; "),
            error
        );
    }
    Ok((
        SubscriptionCommit {
            id: id.to_string(),
            kind,
        },
        SubscriptionPreImages {
            cache: cache_snapshot,
            metadata: metadata_snapshot,
            active: active_snapshot,
            activated: should_activate,
        },
    ))
}

/// Retire a subscription by ID (A1' tombstone semantics).
///
/// The metadata entry is kept as `retired: true` so a byte-identical URL
/// re-add revives the same subscription id with its selections/overlay
/// slots intact; only the downloaded cache is deleted. Tombstones are capped
/// at [`RETIRED_TOMBSTONE_LIMIT`] (FIFO by `retired_at`), one log line per
/// eviction.
pub fn remove_subscription_at(paths: &AppPaths, id: &str) -> anyhow::Result<()> {
    validate_subscription_id(id)?;
    // Defect 3: every subscriptions.yaml writer holds the config write lock
    // and re-reads inside it, so a lock-external download window can never
    // write back a stale list that resurrects a concurrently retired entry.
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let mut subs = load_subscriptions_at(paths)?;
    let active = get_active_id_at(paths)?;

    if active.as_deref() == Some(id) {
        anyhow::bail!(
            "cannot remove the active subscription directly; switch to another verified subscription first"
        );
    }

    let idx = subs
        .iter()
        .position(|s| s.id == id)
        .ok_or_else(|| anyhow::anyhow!("Subscription not found: {}", id))?;
    if subs[idx].retired {
        crate::log!("Subscription {} is already retired", id);
        return Ok(());
    }
    let cache_path = paths.subscription_file_path(id);
    let metadata_path = paths.subscriptions_meta_path();
    let cache_snapshot = snapshot_optional_file(&cache_path)?;
    let metadata_snapshot = snapshot_optional_file(&metadata_path)?;
    subs[idx].retired = true;
    subs[idx].retired_at = Some(Utc::now());

    // Tombstone FIFO: drop the oldest retired entries beyond the cap.
    let mut retired: Vec<(DateTime<Utc>, usize)> = subs
        .iter()
        .enumerate()
        .filter(|(_, sub)| sub.retired)
        .map(|(index, sub)| (sub.retired_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH), index))
        .collect();
    retired.sort();
    let mut evicted: Vec<String> = Vec::new();
    if retired.len() > RETIRED_TOMBSTONE_LIMIT {
        let excess = retired.len() - RETIRED_TOMBSTONE_LIMIT;
        let mut doomed: Vec<usize> = retired
            .into_iter()
            .take(excess)
            .map(|(_, index)| index)
            .collect();
        doomed.sort_unstable_by(|a, b| b.cmp(a));
        for index in doomed {
            evicted.push(subs[index].id.clone());
            subs.remove(index);
        }
    }

    let result = (|| {
        save_subscriptions_at(paths, &subs)?;
        utils::remove_file_if_exists(&cache_path)?;
        Ok::<(), anyhow::Error>(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        if let Err(rollback_error) = restore_optional_file(&cache_path, cache_snapshot) {
            rollback_errors.push(format!("subscription cache: {rollback_error}"));
        }
        if let Err(rollback_error) = restore_optional_file(&metadata_path, metadata_snapshot) {
            rollback_errors.push(format!("subscriptions metadata: {rollback_error}"));
        }
        if rollback_errors.is_empty() {
            return Err(error);
        }
        anyhow::bail!(
            "subscription removal rollback incomplete: {}. Original error: {}",
            rollback_errors.join("; "),
            error
        );
    }

    for id in &evicted {
        crate::log!(
            "Evicted retired subscription {} (tombstone FIFO limit {})",
            id,
            RETIRED_TOMBSTONE_LIMIT
        );
    }
    crate::log!(
        "Retired subscription {} (tombstone kept; re-adding the same URL revives it)",
        id
    );
    Ok(())
}

/// Refresh a subscription by ID: re-download and update file.
/// Test-only convenience wrapper over prepare + lock-internal commit.
#[cfg(test)]
pub async fn refresh_subscription_at(paths: &AppPaths, id: &str) -> anyhow::Result<()> {
    let prepared = prepare_subscription_refresh_at(paths, id, None).await?;
    commit_refreshes_checked_at(paths, vec![(id.to_string(), prepared)], || Ok(()))?;
    Ok(())
}

/// Phase 1 (outside the config write lock): resolve URL/UA from metadata and
/// download + convert + validate. No disk write happens here — the identity
/// re-read, pre-images and writes all happen in the lock-internal commit
/// (`commit_refreshes_checked_at`), so this download window cannot resurrect a
/// concurrently retired entry (defect 1/3).
pub async fn prepare_subscription_refresh_at(
    paths: &AppPaths,
    id: &str,
    override_user_agent: Option<&str>,
) -> anyhow::Result<PreparedSubscription> {
    let subs = load_subscriptions_at(paths)?;
    let sub = find_subscription(&subs, id)
        .ok_or_else(|| anyhow::anyhow!("Subscription not found: {}", id))?;
    if !sub.is_alive() {
        anyhow::bail!(
            "Subscription {} is retired (tombstone). Re-add its URL to revive it, or remove it to discard.",
            id
        );
    }
    let url = sub.url.clone();
    let fixed_user_agent =
        override_user_agent
            .map(str::to_string)
            .or_else(|| match sub.user_agent_mode {
                Some(UserAgentMode::Fixed) => sub.user_agent.clone(),
                _ => None,
            });

    // Download. Fixed UA uses exactly that UA; auto mode uses bounded Clash-UA negotiation.
    let result = download_sub_with_user_agent(&url, fixed_user_agent.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("Cannot reach subscription URL.\n  {e}"))?;
    let content = result.content;
    let is_yaml = result.is_clash_yaml;

    // Convert if needed
    let yaml_content = if is_yaml {
        content
    } else {
        warn_raw_subscription_conversion();
        convert_vmess_to_clash(&content)?
    };

    // Validate
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).context("Downloaded content is not valid YAML")?;
    validate_subscription_yaml(&yaml)?;

    Ok(PreparedSubscription { yaml_content })
}

/// Phase 2 (lock-internal): commit prepared refresh content for one or more
/// identities and run `after_commit` (config merge validation) inside the SAME
/// config write lock hold. Metadata is re-read inside the lock; per-identity
/// cache pre-images and the metadata pre-image are taken inside the lock, so a
/// merge failure restores exactly this transaction's state and can never
/// overwrite another command's commit made during the download window
/// (defect 1/3).
/// Pre-images captured inside the config write lock for one refresh commit;
/// used to restore exactly this transaction's state if the critical section
/// (including the caller's config merge validation) fails.
struct RefreshCommitPreImages {
    caches: Vec<(std::path::PathBuf, Option<String>)>,
    /// `None` = do not touch the metadata file at all (this transaction never
    /// reached the metadata save); `Some(pre)` = restore it to `pre`.
    metadata: Option<Option<String>>,
}

impl RefreshCommitPreImages {
    fn restore(self, paths: &AppPaths) -> anyhow::Result<()> {
        let mut errors = Vec::new();
        for (path, pre) in self.caches {
            if let Err(error) = restore_optional_file(&path, pre) {
                errors.push(format!("{}: {error}", path.display()));
            }
        }
        if let Some(pre) = self.metadata {
            if let Err(error) = restore_optional_file(&paths.subscriptions_meta_path(), pre) {
                errors.push(format!("metadata: {error}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("{}", errors.join("; "))
        }
    }
}

pub fn commit_refreshes_checked_at(
    paths: &AppPaths,
    items: Vec<(String, PreparedSubscription)>,
    after_commit: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<RefreshAllReport> {
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let mut report = RefreshAllReport {
        refreshed: Vec::new(),
        failed: Vec::new(),
    };
    let mut subs = load_subscriptions_at(paths)?;
    let meta_path = paths.subscriptions_meta_path();
    let previous_meta = snapshot_file(&meta_path)?;
    let mut previous_caches: Vec<(std::path::PathBuf, Option<String>)> = Vec::new();
    let mut metadata_dirty = false;

    for (id, prepared) in items {
        let alive = match subs.iter().find(|s| s.id == id) {
            Some(entry) => entry.is_alive(),
            None => {
                report
                    .failed
                    .push((id, "Subscription not found during refresh.".to_string()));
                continue;
            }
        };
        if !alive {
            report.failed.push((
                id,
                "Subscription was retired during refresh. Re-add its URL to revive it.".to_string(),
            ));
            continue;
        }
        let entry = subs
            .iter_mut()
            .find(|s| s.id == id)
            .expect("entry checked alive above");
        let cache_path = paths.subscription_file_path(&id);
        let previous_cache = snapshot_file(&cache_path)?;
        if let Err(error) = utils::atomic_write_file_for_original_user(
            &cache_path.display().to_string(),
            &prepared.yaml_content,
        ) {
            // Metadata is not saved until after this loop, so only the caches
            // written earlier in this transaction need restoring.
            let rollback = RefreshCommitPreImages {
                caches: std::mem::take(&mut previous_caches),
                metadata: None,
            };
            let _ = rollback.restore(paths);
            return Err(error).context(format!("Subscription refresh cache write failed for {id}"));
        }
        previous_caches.push((cache_path, previous_cache));
        entry.updated = Utc::now();
        metadata_dirty = true;
        report.refreshed.push(id);
    }

    if metadata_dirty {
        if let Err(error) = save_subscriptions_at(paths, &subs) {
            let rollback = RefreshCommitPreImages {
                caches: previous_caches,
                metadata: Some(previous_meta),
            }
            .restore(paths);
            match rollback {
                Ok(()) => anyhow::bail!(
                    "Subscription refresh metadata write failed; restored last-known-good cache.\n  {error}"
                ),
                Err(rollback_error) => anyhow::bail!(
                    "Subscription refresh metadata write failed and rollback is incomplete: {rollback_error}.\n  Original error: {error}"
                ),
            };
        }
    }

    let pre_images = RefreshCommitPreImages {
        caches: previous_caches,
        metadata: Some(previous_meta),
    };
    if let Err(error) = after_commit() {
        match pre_images.restore(paths) {
            Ok(()) => anyhow::bail!(
                "Subscription refresh produced an invalid config; restored all last-known-good caches.\n  {error}"
            ),
            Err(rollback_error) => anyhow::bail!(
                "Subscription refresh produced an invalid config and rollback is incomplete: {rollback_error}.\n  Original error: {error}"
            ),
        };
    }
    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshAllReport {
    pub refreshed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl RefreshAllReport {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }

    pub fn failure_summary(&self) -> String {
        self.failed
            .iter()
            .map(|(id, error)| format!("{id}: {error}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Refresh all subscriptions and report every individual outcome.
///
/// Downloads happen OUTSIDE the config write lock; the commit + merge +
/// failure recovery happen together in `commit_refreshes_checked_at`
/// (defect 1/3). Download failures are collected per subscription instead of
/// aborting the batch.
pub async fn prepare_subscription_refresh_all_at(
    paths: &AppPaths,
    override_user_agent: Option<&str>,
) -> anyhow::Result<(Vec<(String, PreparedSubscription)>, Vec<(String, String)>)> {
    let subs = load_subscriptions_at(paths)?;
    let mut prepared = Vec::new();
    let mut failed = Vec::new();
    for sub in &subs {
        if !sub.is_alive() {
            crate::log!("Skipping retired subscription {} (tombstone)", sub.id);
            continue;
        }
        crate::log!("Refreshing subscription {}...", sub.id);
        match prepare_subscription_refresh_at(paths, &sub.id, override_user_agent).await {
            Ok(item) => prepared.push((sub.id.clone(), item)),
            Err(error) => {
                crate::log!("Failed to refresh {}: {}", sub.id, error);
                failed.push((sub.id.clone(), error.to_string()));
            }
        }
    }
    Ok((prepared, failed))
}

/// Switch to a subscription by ID by updating subscriptions/active.
///
/// The cached subscription is validated before the active pointer changes, so
/// a missing or malformed cache cannot create an invalid active state.
pub fn switch_subscription_at(paths: &AppPaths, id: &str) -> anyhow::Result<()> {
    let subs = load_subscriptions_at(paths)?;
    match find_subscription(&subs, id) {
        None => anyhow::bail!("Subscription not found: {}", id),
        Some(sub) if !sub.is_alive() => anyhow::bail!(
            "Subscription {} is retired (tombstone) and cannot be activated. Re-add its URL to revive it.",
            id
        ),
        Some(_) => {}
    }

    let cache_path = paths.subscription_file_path(id);
    let content = std::fs::read_to_string(&cache_path)
        .with_context(|| format!("Subscription cache not found: {}", cache_path.display()))?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&content).with_context(|| {
        format!(
            "Subscription cache is not valid YAML: {}",
            cache_path.display()
        )
    })?;
    validate_subscription_yaml(&yaml)?;

    set_active_id_at(paths, id)?;

    crate::log!("Switched to subscription {}", id);
    Ok(())
}

pub fn set_subscription_user_agent_at(
    paths: &AppPaths,
    id: &str,
    user_agent: Option<String>,
) -> anyhow::Result<()> {
    // Defect 3: metadata writes hold the config write lock and re-read inside
    // it, so a concurrent retire/commit cannot be clobbered by a stale list.
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let mut subs = load_subscriptions_at(paths)?;
    let sub = subs
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or_else(|| anyhow::anyhow!("Subscription not found: {}", id))?;
    sub.user_agent = user_agent;
    sub.user_agent_mode = Some(if sub.user_agent.is_some() {
        UserAgentMode::Fixed
    } else {
        UserAgentMode::Auto
    });
    save_subscriptions_at(paths, &subs)
}

const SUBSCRIPTION_UA_CANDIDATES: &[(&str, &str)] = &[
    ("clash-verge", "clash-verge/v2.0.4"),
    ("clash-meta", "clash-meta/v1.19.0"),
    ("clash", "clash/v1.0.0"),
];

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub content: String,
    pub is_clash_yaml: bool,
}

#[derive(Debug, Clone)]
pub struct SubscriptionProbeResult {
    pub label: String,
    pub user_agent: Option<String>,
    pub format: String,
    pub http_status: Option<u16>,
    pub proxy_count: usize,
    pub proxy_group_count: usize,
    pub rule_count: usize,
    pub proxy_provider_count: usize,
    pub rule_provider_count: usize,
    pub bytes: usize,
    pub score: i32,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct HttpTextResponse {
    status: u16,
    body: String,
}

trait SubscriptionFetcher {
    fn get_text<'a>(
        &'a self,
        url: String,
        user_agent: Option<String>,
    ) -> futures::future::BoxFuture<'a, Result<HttpTextResponse, String>>;
}

struct ReqwestSubscriptionFetcher {
    client: reqwest::Client,
}

impl SubscriptionFetcher for ReqwestSubscriptionFetcher {
    fn get_text<'a>(
        &'a self,
        url: String,
        user_agent: Option<String>,
    ) -> futures::future::BoxFuture<'a, Result<HttpTextResponse, String>> {
        Box::pin(async move {
            let mut request = self.client.get(&url);
            if let Some(ua) = user_agent {
                request = request.header("User-Agent", ua);
            }
            let resp = request.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.map_err(|e| e.to_string())?;
            Ok(HttpTextResponse { status, body })
        })
    }
}

/// Backward-compatible helper. Auto mode tries a bounded Clash-UA set and stops at first YAML.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchSubscriptionReport {
    pub output_path: std::path::PathBuf,
    pub is_clash_yaml: bool,
    pub proxy_count: usize,
    pub proxy_group_count: usize,
    pub rule_count: usize,
}

pub async fn fetch_subscription_to_file(
    url: &str,
    output_path: &std::path::Path,
    user_agent: Option<&str>,
) -> anyhow::Result<FetchSubscriptionReport> {
    let result = download_sub_with_user_agent(url, user_agent)
        .await
        .map_err(|e| anyhow::anyhow!("Cannot reach subscription URL.\n  {e}"))?;
    fetch_subscription_content_to_file(result, output_path)
}

fn fetch_subscription_content_to_file(
    result: DownloadResult,
    output_path: &std::path::Path,
) -> anyhow::Result<FetchSubscriptionReport> {
    let yaml_content = if result.is_clash_yaml {
        result.content
    } else {
        crate::log!("Converting subscription format (vmess/base64/raw → Clash YAML)...");
        convert_vmess_to_clash(&result.content)?
    };

    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&yaml_content).context("Subscription content is not valid YAML")?;
    validate_subscription_yaml(&yaml)?;

    if let Some(parent) = output_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        utils::ensure_dir_all_no_follow(parent)?;
    }
    utils::atomic_write_file_for_original_user(&output_path.display().to_string(), &yaml_content)?;

    Ok(FetchSubscriptionReport {
        output_path: output_path.to_path_buf(),
        is_clash_yaml: result.is_clash_yaml,
        proxy_count: count_sequence_field(&yaml, "proxies"),
        proxy_group_count: count_sequence_field(&yaml, "proxy-groups"),
        rule_count: count_sequence_field(&yaml, "rules"),
    })
}

fn count_sequence_field(yaml: &serde_yaml::Value, key: &str) -> usize {
    yaml.get(key)
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.len())
        .unwrap_or(0)
}

pub async fn download_sub_smart(url: &str) -> anyhow::Result<(String, bool)> {
    let result = download_sub_with_user_agent(url, None).await?;
    Ok((result.content, result.is_clash_yaml))
}

pub async fn download_sub_with_user_agent(
    url: &str,
    user_agent: Option<&str>,
) -> anyhow::Result<DownloadResult> {
    let client = crate::utils::http_client_builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let fetcher = ReqwestSubscriptionFetcher { client };
    download_sub_with_user_agent_and_fetcher(url, user_agent, &fetcher).await
}

async fn download_sub_with_user_agent_and_fetcher(
    url: &str,
    user_agent: Option<&str>,
    fetcher: &impl SubscriptionFetcher,
) -> anyhow::Result<DownloadResult> {
    if let Some(ua) = user_agent {
        let clash_url = append_query_param(url, "flag=clashmeta");
        let resp = fetcher
            .get_text(clash_url, Some(ua.to_string()))
            .await
            .map_err(|e| subscription_network_error(url, e, None))?;
        let is_clash_yaml = is_clash_yaml(&resp.body);
        return Ok(DownloadResult {
            content: resp.body,
            is_clash_yaml,
        });
    }

    let clash_url = append_query_param(url, "flag=clashmeta");
    let mut first_error: Option<String> = None;

    // Auto mode is intentionally bounded and stops as soon as Clash YAML is found.
    // This avoids unnecessary requests against providers that rate-limit subscription URLs.
    for (label, ua) in SUBSCRIPTION_UA_CANDIDATES {
        match fetcher
            .get_text(clash_url.clone(), Some((*ua).to_string()))
            .await
        {
            Ok(resp) if (200..=299).contains(&resp.status) => {
                if is_clash_yaml(&resp.body) {
                    crate::log!(
                        "got Clash YAML via UA={label}, flag=clashmeta ({} lines)",
                        resp.body.lines().count()
                    );
                    return Ok(DownloadResult {
                        content: resp.body,
                        is_clash_yaml: true,
                    });
                }
                crate::log!(
                    "UA={label}, flag=clashmeta returned non-YAML format ({} bytes)",
                    resp.body.len()
                );
            }
            Ok(resp) => {
                crate::log!(
                    "UA={label}, flag=clashmeta request failed (HTTP {})",
                    resp.status
                );
            }
            Err(e) => {
                crate::log!("UA={label}, flag=clashmeta network error: {e}");
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }

    match fetcher.get_text(url.to_string(), None).await {
        Ok(resp) => {
            let is_clash_yaml = is_clash_yaml(&resp.body);
            if is_clash_yaml {
                crate::log!("got Clash YAML via bare subscription URL");
            } else {
                crate::log!(
                    "bare subscription URL returned non-YAML format ({} bytes)",
                    resp.body.len()
                );
            }
            Ok(DownloadResult {
                content: resp.body,
                is_clash_yaml,
            })
        }
        Err(e) => Err(subscription_network_error(url, e, first_error)),
    }
}

pub async fn probe_subscription_url(url: &str) -> anyhow::Result<Vec<SubscriptionProbeResult>> {
    let client = crate::utils::http_client_builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let fetcher = ReqwestSubscriptionFetcher { client };
    probe_subscription_url_with_fetcher(url, &fetcher, true).await
}

async fn probe_subscription_url_with_fetcher(
    url: &str,
    fetcher: &impl SubscriptionFetcher,
    rate_limit_delay: bool,
) -> anyhow::Result<Vec<SubscriptionProbeResult>> {
    let mut results = Vec::new();
    for (idx, (label, ua)) in SUBSCRIPTION_UA_CANDIDATES.iter().enumerate() {
        if rate_limit_delay && idx > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
        results.push(probe_once(url, label, Some(*ua), fetcher).await);
    }
    if rate_limit_delay {
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
    }
    results.push(probe_once(url, "bare", None, fetcher).await);
    results.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.cmp(&b.label)));
    Ok(results)
}

async fn probe_once(
    url: &str,
    label: &str,
    user_agent: Option<&str>,
    fetcher: &impl SubscriptionFetcher,
) -> SubscriptionProbeResult {
    let request_url = if user_agent.is_some() {
        append_query_param(url, "flag=clashmeta")
    } else {
        url.to_string()
    };
    match fetcher
        .get_text(request_url, user_agent.map(str::to_string))
        .await
    {
        Ok(resp) => analyze_subscription_response(
            label.to_string(),
            user_agent.map(str::to_string),
            Some(resp.status),
            resp.body,
            None,
        ),
        Err(e) => SubscriptionProbeResult::error(label, user_agent, None, e),
    }
}

impl SubscriptionProbeResult {
    fn error(label: &str, user_agent: Option<&str>, status: Option<u16>, error: String) -> Self {
        Self {
            label: label.to_string(),
            user_agent: user_agent.map(str::to_string),
            format: "error".to_string(),
            http_status: status,
            proxy_count: 0,
            proxy_group_count: 0,
            rule_count: 0,
            proxy_provider_count: 0,
            rule_provider_count: 0,
            bytes: 0,
            score: -2000,
            error: Some(error),
        }
    }
}

fn analyze_subscription_response(
    label: String,
    user_agent: Option<String>,
    http_status: Option<u16>,
    content: String,
    error: Option<String>,
) -> SubscriptionProbeResult {
    let bytes = content.len();
    let mut result = SubscriptionProbeResult {
        label,
        user_agent,
        format: "unknown".to_string(),
        http_status,
        proxy_count: 0,
        proxy_group_count: 0,
        rule_count: 0,
        proxy_provider_count: 0,
        rule_provider_count: 0,
        bytes,
        score: 0,
        error,
    };
    if !matches!(http_status, Some(200..=299)) {
        result.score = -1000;
        result.error = Some(format!("HTTP {}", http_status.unwrap_or_default()));
        return result;
    }
    if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
        if is_clash_yaml(&content) {
            result.format = "Clash YAML".to_string();
            result.proxy_count = yaml["proxies"].as_sequence().map_or(0, |v| v.len());
            result.proxy_group_count = yaml["proxy-groups"].as_sequence().map_or(0, |v| v.len());
            result.rule_count = yaml["rules"].as_sequence().map_or(0, |v| v.len());
            result.proxy_provider_count =
                yaml["proxy-providers"].as_mapping().map_or(0, |v| v.len());
            result.rule_provider_count = yaml["rule-providers"].as_mapping().map_or(0, |v| v.len());
            result.score = 1000
                + result.proxy_count.min(300) as i32
                + (result.proxy_group_count.min(30) as i32 * 10)
                + result.rule_count.min(500) as i32
                + ((result.proxy_provider_count + result.rule_provider_count).min(20) as i32 * 20);
            return result;
        }
    }
    let raw_count = parse_lines(&content)
        .into_iter()
        .filter(|l| {
            l.starts_with("vmess://") || l.starts_with("trojan://") || l.starts_with("ss://")
        })
        .count();
    if raw_count > 0 {
        result.format = "Raw links".to_string();
        result.proxy_count = raw_count;
        result.score = 100 + raw_count.min(300) as i32;
    }
    result
}

fn subscription_network_error(
    url: &str,
    error: String,
    first_error: Option<String>,
) -> anyhow::Error {
    let detail = first_error
        .map(|first| format!("first UA attempt: {first}; final retry: {error}"))
        .unwrap_or(error);
    crate::log!("Network error: {detail}");
    crate::log!(
        "Is the URL reachable? Try: curl -I '{}'",
        utils::sanitize_url(url)
    );
    let sanitized = utils::sanitize_url(url);
    anyhow::anyhow!(
        "Network error: {detail}\n\n\
         Possible causes:\n  \
         - DNS resolution failed (DNS may be polluted)\n  \
         - Connection refused or timed out\n  \
         - TLS certificate error\n  \
         - The URL requires a proxy to reach\n  \
           → If mihomo is already running: eval \"$(mihomo-cli proxy on)\"\n  \
           → Or manually: export http_proxy=http://127.0.0.1:PORT\n  \
         - TUN mode is intercepting traffic (try: mihomo-cli tun off)\n\n\
         Verify the URL is reachable:\n  \
         curl -I '{sanitized}'\n\n\
         Workaround (if DNS is polluted):\n  \
         1. Download config on another machine:\n  \
            curl --doh-url https://dns.alidns.com/dns-query -o config.yaml '{sanitized}'\n  \
         2. Transfer to this machine and import:\n  \
            mihomo-cli config --import config.yaml"
    )
}

fn append_query_param(url: &str, param: &str) -> String {
    let separator = if url.contains('?') { "&" } else { "?" };
    format!("{url}{separator}{param}")
}

pub fn warn_raw_subscription_conversion() {
    eprintln!(
        "  ⚠ Subscription server did not return Clash YAML; provider preset rules, custom proxy-groups, and service-specific routing may be lost."
    );
    eprintln!(
        "  Hint: verify the subscription supports Clash format, or import a Clash YAML export directly."
    );
    eprintln!("  You can add local rules with: mihomo-cli rule add <rule>");
}

pub fn is_clash_yaml(content: &str) -> bool {
    content.contains("proxies:") || content.contains("mixed-port:") || content.contains("mode:")
}

pub fn convert_vmess_to_clash(content: &str) -> anyhow::Result<String> {
    let lines = parse_lines(content);

    let mut proxies: Vec<serde_yaml::Value> = Vec::new();

    for line in &lines {
        if line.starts_with("vmess://") {
            if let Some(proxy) = parse_vmess(line) {
                proxies.push(proxy);
            }
        } else if line.starts_with("trojan://") {
            if let Some(proxy) = parse_trojan(line) {
                proxies.push(proxy);
            }
        }
    }

    if proxies.is_empty() {
        anyhow::bail!("No proxies found in subscription");
    }

    let names: Vec<String> = proxies
        .iter()
        .filter_map(|p| p["name"].as_str().map(String::from))
        .collect();
    // let names_json = serde_json::to_string(&names)?;

    let controller_line = current_api_endpoint().controller_line();

    // Indent the serialized output for proper YAML nesting
    let proxies_yaml = indent(&serde_yaml::to_string(&proxies)?, 2);
    let names_yaml = indent(&serde_yaml::to_string(&names)?, 6);

    let config = format!(
        r#"# Generated by mihomo-cli
mode: rule
mixed-port: 7897
allow-lan: false
log-level: info
ipv6: true
{controller_line}
tun:
  enable: false
  stack: system
  auto-route: true
  auto-detect-interface: true
  strict-route: true
  dns-hijack:
    - any:53
    - tcp://any:53
dns:
  enable: true
  listen: 127.0.0.1:1053
  default-nameserver:
    - 223.5.5.5
  enhanced-mode: fake-ip
  fake-ip-range: 198.18.0.1/16
  fake-ip-filter:
    - '+.lan'
    - '+.local'
profile:
  store-selected: true

proxies:
{proxies_yaml}

proxy-groups:
  - name: 节点选择
    type: select
    proxies:
{names_yaml}
  - name: 自动选择
    type: url-test
    proxies:
{names_yaml}
    url: http://www.gstatic.com/generate_204
    interval: 300
    tolerance: 50

rules:
  - MATCH,节点选择
"#,
        proxies_yaml = proxies_yaml,
        names_yaml = names_yaml,
    );

    Ok(config)
}

fn parse_lines(content: &str) -> Vec<String> {
    let raw = content.trim();
    // Check if it starts with protocol prefixes
    for prefix in &["vmess://", "ss://", "trojan://"] {
        if raw.starts_with(prefix) || raw.lines().any(|l| l.starts_with(prefix)) {
            return raw
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
        }
    }
    // Try base64 decode
    for pad in &["", "=", "==", "==="] {
        let b64 = format!("{raw}{pad}");
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&b64) {
            if let Ok(decoded_str) = String::from_utf8(decoded) {
                return decoded_str
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
            }
        }
    }
    raw.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn parse_vmess(line: &str) -> Option<serde_yaml::Value> {
    let b64 = line.strip_prefix("vmess://")?;
    let b64_padded = format!("{}{}", b64, "=".repeat((4 - b64.len() % 4) % 4));
    let json = base64::engine::general_purpose::STANDARD
        .decode(&b64_padded)
        .ok()?;
    let d: serde_json::Value = serde_json::from_slice(&json).ok()?;

    let name = d["ps"].as_str().unwrap_or("vmess");
    let server = d["add"].as_str().unwrap_or("");
    let port: u64 = d["port"].as_str()?.parse().ok()?;
    let uuid = d["id"].as_str().unwrap_or("");
    let aid: u64 = d["aid"].as_str()?.parse().unwrap_or(0);
    let net = d["net"].as_str().unwrap_or("tcp");
    let tls = d["tls"].as_str().unwrap_or("");

    let mut proxy = serde_yaml::Mapping::new();
    macro_rules! insert {
        ($m:expr, $k:expr, $v:expr) => {
            $m.insert(
                serde_yaml::Value::String($k.to_string()),
                serde_yaml::Value::String($v.to_string()),
            );
        };
    }

    insert!(proxy, "name", name);
    insert!(proxy, "type", "vmess");
    insert!(proxy, "server", server);
    insert!(proxy, "port", &port.to_string());
    insert!(proxy, "uuid", uuid);
    insert!(proxy, "alterId", &aid.to_string());
    insert!(proxy, "cipher", "auto");

    if tls == "tls" {
        proxy.insert(
            serde_yaml::Value::String("tls".into()),
            serde_yaml::Value::Bool(true),
        );
    }

    if net == "ws" || net == "h2" {
        insert!(proxy, "network", net);
        let mut opts = serde_yaml::Mapping::new();
        if let Some(path) = d["path"].as_str() {
            if !path.is_empty() {
                insert!(opts, "path", path);
            }
        }
        if let Some(host) = d["host"].as_str() {
            if !host.is_empty() {
                let mut headers = serde_yaml::Mapping::new();
                insert!(headers, "Host", host);
                opts.insert(
                    serde_yaml::Value::String("headers".into()),
                    serde_yaml::Value::Mapping(headers),
                );
            }
        }
        if !opts.is_empty() {
            proxy.insert(
                serde_yaml::Value::String(format!("{net}-opts")),
                serde_yaml::Value::Mapping(opts),
            );
        }
    } else if net == "grpc" {
        insert!(proxy, "network", "grpc");
        let mut opts = serde_yaml::Mapping::new();
        let svc = d["path"].as_str().unwrap_or("").trim_start_matches('/');
        if !svc.is_empty() {
            insert!(opts, "grpc-service-name", svc);
            proxy.insert(
                serde_yaml::Value::String("grpc-opts".into()),
                serde_yaml::Value::Mapping(opts),
            );
        }
    } else if !net.is_empty() {
        insert!(proxy, "network", net);
    }

    Some(serde_yaml::Value::Mapping(proxy))
}

fn parse_trojan(line: &str) -> Option<serde_yaml::Value> {
    let rest = line.strip_prefix("trojan://")?;
    let at_idx = rest.find('@')?;
    let password = &rest[..at_idx];
    let after_at = &rest[at_idx + 1..];
    let qm = after_at.find('?').unwrap_or(after_at.len());
    let host_port = &after_at[..qm];
    let (host, port_str) = host_port.rsplit_once(':')?;
    let port: u64 = port_str.parse().ok()?;

    let _query = if qm < after_at.len() {
        &after_at[qm + 1..]
    } else {
        ""
    };

    let mut proxy = serde_yaml::Mapping::new();
    macro_rules! insert {
        ($m:expr, $k:expr, $v:expr) => {
            $m.insert(
                serde_yaml::Value::String($k.to_string()),
                serde_yaml::Value::String($v.to_string()),
            );
        };
    }

    insert!(proxy, "name", "trojan");
    insert!(proxy, "type", "trojan");
    insert!(proxy, "server", host);
    insert!(proxy, "port", &port.to_string());
    insert!(proxy, "password", password);

    Some(serde_yaml::Value::Mapping(proxy))
}

#[allow(dead_code)]
pub fn save_config(content: &str) -> anyhow::Result<()> {
    let paths = AppPaths::from_system();
    let mihomo = std::path::PathBuf::from(utils::mihomo_path());
    save_config_at(&paths, content, Some(&mihomo))?;
    println!("Config saved to {}", paths.config_path().display());
    Ok(())
}

/// Save config.yaml with post-write validation and rollback.
///
/// The content is parsed, normalized, patched with the controller setting,
/// written atomically, then validated. If validation fails, the previous
/// config.yaml is restored (or the newly-created file is removed).
#[allow(dead_code)]
pub fn save_config_at(
    paths: &AppPaths,
    content: &str,
    mihomo_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    save_config_at_endpoint(paths, content, mihomo_path, &current_api_endpoint())
}

fn validation_rollback_error(
    action: &str,
    validation_error: &anyhow::Error,
    rollback: anyhow::Result<()>,
) -> anyhow::Result<()> {
    if let Err(rollback_error) = rollback {
        anyhow::bail!(
            "Config validation failed after {action}; rollback incomplete: {}.\n  Original error: {}",
            rollback_error,
            validation_error
        );
    }
    anyhow::bail!(
        "Config validation failed after {action}; rolled back config.yaml.\n  {}",
        validation_error
    )
}

pub fn save_config_at_endpoint(
    paths: &AppPaths,
    content: &str,
    mihomo_path: Option<&std::path::Path>,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<()> {
    ensure_config_directory(paths.config_dir())?;
    let path = paths.config_path();
    let previous = snapshot_file(&path)?;

    let yaml: serde_yaml::Value =
        serde_yaml::from_str(content).context("Subscription content is not valid YAML")?;

    if yaml.get("proxies").is_none() && yaml.get("proxy-providers").is_none() {
        anyhow::bail!("Subscription does not contain `proxies` or `proxy-providers`");
    }

    let normalized = serde_yaml::to_string(&yaml)?;
    let content = ensure_controller_for_endpoint(&normalized, endpoint)?;

    utils::atomic_write_file_for_original_user(&path.display().to_string(), &content)?;

    if let Err(validation_error) = validate_config_at(paths, mihomo_path) {
        return validation_rollback_error(
            "save",
            &validation_error,
            restore_file_snapshot(&path, previous),
        );
    }

    Ok(())
}

/// Fix the existing config file: ensure it has external-controller-unix/pipe.
/// Reads current config, applies ensure_controller_for_endpoint(), writes back.
/// Returns true if the file was modified.
#[allow(dead_code)]
pub fn fix_existing_config() -> bool {
    let paths = AppPaths::from_system();
    let mihomo = std::path::PathBuf::from(utils::mihomo_path());
    match fix_existing_config_at(&paths, Some(&mihomo)) {
        Ok(fixed) => fixed,
        Err(e) => {
            eprintln!("  Warning: Failed to fix config: {}", e);
            false
        }
    }
}

#[allow(dead_code)]
pub fn fix_existing_config_at(
    paths: &AppPaths,
    mihomo_path: Option<&std::path::Path>,
) -> anyhow::Result<bool> {
    let path = paths.config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    let fixed = ensure_controller_for_endpoint(&content, &current_api_endpoint())?;
    if fixed == content {
        return Ok(false);
    }

    let previous = Some(content);
    utils::atomic_write_file_for_original_user(&path.display().to_string(), &fixed)?;

    if let Err(validation_error) = validate_config_at(paths, mihomo_path) {
        validation_rollback_error(
            "fix",
            &validation_error,
            restore_file_snapshot(&path, previous),
        )?;
    }

    Ok(true)
}

pub fn fix_existing_config_at_endpoint(
    paths: &AppPaths,
    mihomo_path: Option<&std::path::Path>,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<bool> {
    let path = paths.config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    let fixed = ensure_controller_for_endpoint(&content, endpoint)?;
    if fixed == content {
        return Ok(false);
    }

    let previous = Some(content);
    utils::atomic_write_file_for_original_user(&path.display().to_string(), &fixed)?;

    if let Err(validation_error) = validate_config_at(paths, mihomo_path) {
        validation_rollback_error(
            "fix",
            &validation_error,
            restore_file_snapshot(&path, previous),
        )?;
    }

    Ok(true)
}

#[allow(dead_code)]
fn snapshot_file(path: &std::path::Path) -> anyhow::Result<Option<String>> {
    if path.exists() {
        Ok(Some(std::fs::read_to_string(path)?))
    } else {
        Ok(None)
    }
}

fn restore_file_snapshot(path: &std::path::Path, snapshot: Option<String>) -> anyhow::Result<()> {
    match snapshot {
        Some(content) => {
            if let Some(parent) = path.parent() {
                utils::ensure_dir_all_no_follow(parent)?;
            }
            utils::atomic_write_file_for_original_user(&path.display().to_string(), &content)?;
        }
        None => utils::remove_file_if_exists(path)?,
    }
    Ok(())
}

/// Add external-controller-unix/pipe if missing from the config
pub(crate) fn current_api_endpoint() -> ApiEndpoint {
    if cfg!(target_os = "windows") {
        ApiEndpoint::WindowsNamedPipe(windows_user_core_pipe_name(
            std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string()),
        ))
    } else {
        #[cfg(unix)]
        {
            unix_api_endpoint_from_socket_dir(crate::utils::socket_dir())
        }
        #[cfg(not(unix))]
        {
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("mihomo.sock"))
        }
    }
}

#[cfg(unix)]
fn unix_api_endpoint_from_socket_dir(socket_dir: impl AsRef<std::path::Path>) -> ApiEndpoint {
    ApiEndpoint::UnixSocket(socket_dir.as_ref().join("mihomo.sock"))
}

fn windows_user_core_pipe_name(username_or_sid: impl AsRef<str>) -> String {
    format!(r"\\.\pipe\mihomo-{}", username_or_sid.as_ref())
}

/// Add or repair external-controller-unix/pipe for an explicit instance endpoint.
pub(crate) fn ensure_controller_for_endpoint(
    yaml: &str,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<String> {
    let controller_line = endpoint.controller_line();

    let mut editor = crate::yaml_editor::YamlEditor::parse(yaml)
        .map_err(|e| anyhow::anyhow!("Failed to parse config.yaml: {}", e))?;

    editor
        .ensure_controller(&controller_line)
        .map_err(|e| anyhow::anyhow!("Failed to ensure controller in config.yaml: {}", e))?;

    Ok(editor.into_source())
}

#[allow(dead_code)]
pub fn check_config_exists() -> bool {
    std::path::Path::new(&utils::config_path()).exists()
}

fn indent(s: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    s.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{prefix}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Phase 3: Core generation logic ──

/// Validate that subscription YAML contains required fields
fn validate_subscription_yaml(yaml: &serde_yaml::Value) -> anyhow::Result<()> {
    if yaml.get("proxies").is_none() && yaml.get("proxy-providers").is_none() {
        anyhow::bail!(
            "Subscription does not contain `proxies` or `proxy-providers`.\n  \
             The downloaded content may not be a valid Clash subscription."
        );
    }
    Ok(())
}

/// Generate the initial direct-only user-effective configuration used before the first subscription.
pub fn generate_direct_only_config_yaml_for_endpoint(
    endpoint: &ApiEndpoint,
    user_rules: &[String],
    dns_policies: &[crate::dns::DnsPolicy],
    fake_ip_filters: &[String],
    rule_position: crate::rules::RulePosition,
) -> anyhow::Result<String> {
    let base = serde_yaml::from_str::<serde_yaml::Value>(
        "mode: rule\nmixed-port: 7897\ntun:\n  enable: false\nproxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n",
    )?;
    generate_config_yaml_with_fake_ip_filters_for_endpoint(
        &base,
        user_rules,
        dns_policies,
        fake_ip_filters,
        rule_position,
        endpoint,
    )
}

/// Generate final config.yaml from subscription + user rules + DNS policies.
///
/// This is the core function that replaces older incremental editing paths.
/// It performs a full generation from source files.
#[allow(dead_code)]
pub fn generate_config_yaml(
    sub_yaml: &serde_yaml::Value,
    user_rules: &[String],
    dns_policies: &[crate::dns::DnsPolicy],
    rule_position: crate::rules::RulePosition,
) -> anyhow::Result<String> {
    generate_config_yaml_with_fake_ip_filters_for_endpoint(
        sub_yaml,
        user_rules,
        dns_policies,
        &[],
        rule_position,
        &current_api_endpoint(),
    )
}

pub fn generate_config_yaml_with_fake_ip_filters_for_endpoint(
    sub_yaml: &serde_yaml::Value,
    user_rules: &[String],
    dns_policies: &[crate::dns::DnsPolicy],
    fake_ip_filters: &[String],
    rule_position: crate::rules::RulePosition,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<String> {
    let mut config = sub_yaml.clone();
    let config_map = config
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Subscription is not a valid YAML mapping"))?;

    // 0. 收敛旧版模板遗留的 DNS 默认值（A3/A4），只动我们自己写过的取值。
    normalize_dns_legacy_defaults(config_map);

    // 1. Merge rules
    let sub_rules = config_map
        .get("rules")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let merged_rules = match rule_position {
        crate::rules::RulePosition::Front => {
            let mut rules = user_rules.to_vec();
            rules.extend(sub_rules);
            rules
        }
        crate::rules::RulePosition::Back => {
            let mut rules = sub_rules;
            rules.extend(user_rules.iter().cloned());
            rules
        }
    };

    config_map.insert(
        serde_yaml::Value::String("rules".to_string()),
        serde_yaml::Value::Sequence(
            merged_rules
                .into_iter()
                .map(serde_yaml::Value::String)
                .collect(),
        ),
    );

    // 2. Merge dns.nameserver-policy
    if !dns_policies.is_empty() {
        let dns = config_map
            .entry(serde_yaml::Value::String("dns".to_string()))
            .or_insert(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

        let dns_map = dns
            .as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("dns is not a mapping"))?;

        let mut ns_policy = dns_map
            .get("nameserver-policy")
            .and_then(|v| v.as_mapping())
            .cloned()
            .unwrap_or_default();

        for policy in dns_policies {
            let key = if policy.match_pattern.starts_with("+.") {
                policy.match_pattern.clone()
            } else {
                format!("+{}", policy.match_pattern)
            };

            let targets: Vec<&str> = policy.target.split(',').map(|s| s.trim()).collect();
            let value = if targets.len() == 1 {
                serde_yaml::Value::String(targets[0].to_string())
            } else {
                serde_yaml::Value::Sequence(
                    targets
                        .into_iter()
                        .map(|s| serde_yaml::Value::String(s.to_string()))
                        .collect(),
                )
            };

            ns_policy.insert(serde_yaml::Value::String(key), value);
        }

        dns_map.insert(
            serde_yaml::Value::String("nameserver-policy".to_string()),
            serde_yaml::Value::Mapping(ns_policy),
        );
    }

    // 2.1. Merge dns.fake-ip-filter
    if !fake_ip_filters.is_empty() {
        let dns = config_map
            .entry(serde_yaml::Value::String("dns".to_string()))
            .or_insert(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

        let dns_map = dns
            .as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("dns is not a mapping"))?;

        let mut merged = dns_map
            .get("fake-ip-filter")
            .and_then(|v| v.as_sequence())
            .cloned()
            .unwrap_or_default();
        for filter in fake_ip_filters {
            let value = serde_yaml::Value::String(filter.clone());
            if !merged.contains(&value) {
                merged.push(value);
            }
        }
        dns_map.insert(
            serde_yaml::Value::String("fake-ip-filter".to_string()),
            serde_yaml::Value::Sequence(merged),
        );
    }

    // 2.5. Inject mixed-port if no port is configured
    let has_any_port = ["mixed-port", "port", "socks-port"]
        .iter()
        .any(|k| config_map.contains_key(serde_yaml::Value::String(k.to_string())));
    if !has_any_port {
        config_map.insert(
            serde_yaml::Value::String("mixed-port".to_string()),
            serde_yaml::Value::Number(serde_yaml::Number::from(7897)),
        );
    }

    // 3. Inject runtime-owned API controller fields last.
    inject_runtime_controller_for_endpoint(config_map, endpoint);

    // 4. Serialize
    let output = serde_yaml::to_string(&config)?;
    Ok(output)
}

#[allow(dead_code)]
fn inject_runtime_controller(config_map: &mut serde_yaml::Mapping) {
    inject_runtime_controller_for_endpoint(config_map, &current_api_endpoint());
}

fn inject_runtime_controller_for_endpoint(
    config_map: &mut serde_yaml::Mapping,
    endpoint: &ApiEndpoint,
) {
    config_map.remove(serde_yaml::Value::String("external-controller".to_string()));
    config_map.remove(serde_yaml::Value::String(
        "external-controller-unix".to_string(),
    ));
    config_map.remove(serde_yaml::Value::String(
        "external-controller-pipe".to_string(),
    ));
    config_map.remove(serde_yaml::Value::String("external-ui".to_string()));

    let (key, value) = endpoint.controller_key_and_value();
    config_map.insert(
        serde_yaml::Value::String(key.to_string()),
        serde_yaml::Value::String(value),
    );
}

fn deep_merge_yaml(base: &mut serde_yaml::Value, override_value: serde_yaml::Value) {
    match (base, override_value) {
        (serde_yaml::Value::Mapping(base_map), serde_yaml::Value::Mapping(override_map)) => {
            for (key, value) in override_map {
                match base_map.get_mut(&key) {
                    Some(existing) => deep_merge_yaml(existing, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (base_slot, override_value) => {
            *base_slot = override_value;
        }
    }
}

/// 结构等价判断：映射按**键集合**比较（忽略键序），序列按顺序逐项比较。
///
/// 用于判断“override 派生键是否缺失”，避免因序列化键序变化误写用户配置。
fn yaml_structurally_equal(a: &serde_yaml::Value, b: &serde_yaml::Value) -> bool {
    use serde_yaml::Value;
    match (a, b) {
        (Value::Mapping(left), Value::Mapping(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|other| yaml_structurally_equal(value, other))
                })
        }
        (Value::Sequence(left), Value::Sequence(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right.iter())
                    .all(|(l, r)| yaml_structurally_equal(l, r))
        }
        (Value::Mapping(_), _) | (_, Value::Mapping(_)) => {
            // 一侧是映射另一侧不是 → 不等；但 Null vs 空映射等退化情形交给 `==`。
            a == b
        }
        _ => a == b,
    }
}

#[allow(dead_code)]
fn apply_override_at(paths: &AppPaths, config: &mut serde_yaml::Value) -> anyhow::Result<()> {
    apply_override_at_endpoint(paths, config, &current_api_endpoint())
}

fn apply_override_at_endpoint(
    paths: &AppPaths,
    config: &mut serde_yaml::Value,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<()> {
    let override_path = paths.override_path();
    if override_path.exists() {
        let override_content = std::fs::read_to_string(&override_path).with_context(|| {
            format!("Failed to read override.yaml: {}", override_path.display())
        })?;
        let mut override_yaml: serde_yaml::Value = serde_yaml::from_str(&override_content)
            .with_context(|| {
                format!("Failed to parse override.yaml: {}", override_path.display())
            })?;
        if !override_yaml.is_mapping() {
            anyhow::bail!("override.yaml must be a YAML mapping");
        }
        // 缺陷 2：类型绕过（tun: null/标量/数组）必须在合并前拒绝。
        validate_override_tun_shape(&override_yaml)?;
        validate_override_proxies_append_shape(&override_yaml)?;
        merge_proxies_append(config, &mut override_yaml);
        let override_map = override_yaml
            .as_mapping_mut()
            .expect("mapping checked above");
        // §4.7：TUN 管辖字段与受管 runtime 端点不得静默从 override 透入配置。
        warn_rejected_transaction_owned_override_keys(override_map);
        deep_merge_yaml(config, override_yaml);
    }

    let config_map = config
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Merged config is not a YAML mapping after override"))?;
    inject_runtime_controller_for_endpoint(config_map, endpoint);
    Ok(())
}

/// 缺陷 2（裁决定稿）：override 的 `tun` 必须是映射。
///
/// `tun: null` / 标量 / 数组属于**类型绕过**：递归合并会把已有的 tun 映射整块顶掉
/// （等于关掉 TUN 并丢掉全部控制键）。必须在写入前拒绝，`tun: {}` 不算删除（空映射
/// 合并不改变任何键）。TUN 控制键另由 `TUN_TRANSACTION_CONTROLLED_KEYS` 独占。
pub(crate) fn validate_override_tun_shape(override_yaml: &serde_yaml::Value) -> anyhow::Result<()> {
    let Some(tun) = override_yaml.get("tun") else {
        return Ok(());
    };
    if tun.is_mapping() {
        return Ok(());
    }
    let shape = match tun {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "boolean",
        serde_yaml::Value::Number(_) => "number",
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "sequence",
        _ => "non-mapping",
    };
    anyhow::bail!(
        "override.yaml: `tun` must be a YAML mapping (found {shape}); a non-mapping `tun` would replace the whole TUN block.\n  Write only the keys you want to change, e.g.:\n    tun:\n      route-exclude-address:\n      - 224.0.0.0/4\n  TUN 的开关与控制键由 `tun on/off` 独占（override 不能开关 TUN）；nothing was written"
    )
}

pub(crate) fn validate_override_proxies_append_shape(
    override_yaml: &serde_yaml::Value,
) -> anyhow::Result<()> {
    let Some(append) = override_yaml.get("proxies-append") else {
        return Ok(());
    };
    let Some(list) = append.as_sequence() else {
        anyhow::bail!("override.yaml: `proxies-append` must be a YAML sequence of proxy mappings");
    };
    let mut names = std::collections::HashSet::new();
    for (idx, item) in list.iter().enumerate() {
        let Some(map) = item.as_mapping() else {
            anyhow::bail!("override.yaml: `proxies-append[{idx}]` must be a YAML mapping");
        };
        let name = map
            .get("name")
            .and_then(serde_yaml::Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "override.yaml: `proxies-append[{idx}]` must have a non-empty `name`"
                )
            })?;
        if name.trim().is_empty() {
            anyhow::bail!("override.yaml: `proxies-append[{idx}].name` cannot be empty");
        }
        let ptype = map
            .get("type")
            .and_then(serde_yaml::Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "override.yaml: `proxies-append[{idx}]` ({name}) must have a non-empty `type`"
                )
            })?;
        if ptype.trim().is_empty() {
            anyhow::bail!("override.yaml: `proxies-append[{idx}]` ({name}) `type` cannot be empty");
        }
        if !names.insert(name) {
            anyhow::bail!("override.yaml: duplicate proxy name `{name}` in `proxies-append`");
        }
    }
    Ok(())
}

pub(crate) fn merge_proxies_append(
    config: &mut serde_yaml::Value,
    override_yaml: &mut serde_yaml::Value,
) {
    let append_val = match override_yaml.as_mapping_mut() {
        Some(map) => map.remove("proxies-append"),
        None => None,
    };
    if let Some(serde_yaml::Value::Sequence(appended_proxies)) = append_val {
        let config_map = match config.as_mapping_mut() {
            Some(m) => m,
            None => return,
        };
        let proxies_key = serde_yaml::Value::String("proxies".to_string());
        let current_proxies = match config_map.entry(proxies_key) {
            serde_yaml::mapping::Entry::Occupied(entry) => match entry.into_mut() {
                serde_yaml::Value::Sequence(seq) => seq,
                other => {
                    *other = serde_yaml::Value::Sequence(Vec::new());
                    match other {
                        serde_yaml::Value::Sequence(seq) => seq,
                        _ => unreachable!(),
                    }
                }
            },
            serde_yaml::mapping::Entry::Vacant(entry) => {
                match entry.insert(serde_yaml::Value::Sequence(Vec::new())) {
                    serde_yaml::Value::Sequence(seq) => seq,
                    _ => unreachable!(),
                }
            }
        };

        for proxy in appended_proxies {
            let proxy_name = proxy
                .as_mapping()
                .and_then(|m| m.get("name"))
                .and_then(serde_yaml::Value::as_str);
            if let Some(name) = proxy_name {
                if let Some(idx) = current_proxies.iter().position(|p| {
                    p.as_mapping()
                        .and_then(|m| m.get("name"))
                        .and_then(serde_yaml::Value::as_str)
                        == Some(name)
                }) {
                    current_proxies[idx] = proxy;
                } else {
                    current_proxies.push(proxy);
                }
            } else {
                current_proxies.push(proxy);
            }
        }
    }
    if let Some(map) = config.as_mapping_mut() {
        map.remove("proxies-append");
    }
}

pub fn extract_override_proxies(override_val: &serde_yaml::Value) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(map) = override_val.as_mapping() {
        if let Some(list) = map.get("proxies-append").and_then(|v| v.as_sequence()) {
            for item in list {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    names.push(name.to_string());
                }
            }
        }
        if let Some(list) = map.get("proxies").and_then(|v| v.as_sequence()) {
            for item in list {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

pub fn extract_override_providers(override_val: &serde_yaml::Value) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(map) = override_val.as_mapping() {
        if let Some(providers) = map.get("proxy-providers").and_then(|v| v.as_mapping()) {
            for key in providers.keys() {
                if let Some(name) = key.as_str() {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

pub fn extract_inline_provider_payload_nodes(
    val: &serde_yaml::Value,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some(root) = val.as_mapping() {
        if let Some(providers) = root.get("proxy-providers").and_then(|v| v.as_mapping()) {
            for (pname_val, pcfg) in providers {
                let Some(pname) = pname_val.as_str() else {
                    continue;
                };
                if let Some(payload) = pcfg.get("payload").and_then(|v| v.as_sequence()) {
                    for node in payload {
                        if let Some(nname) = node.get("name").and_then(|v| v.as_str()) {
                            map.insert(nname.to_string(), pname.to_string());
                        }
                    }
                }
            }
        }
    }
    map
}

/// 有界、不跟随 symlink 的配置读取（对齐 的加固读法，review MINOR-e）。
fn read_config_text_bounded(path: &std::path::Path) -> anyhow::Result<String> {
    let bytes = crate::utils::read_file_no_follow_limited(
        path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    )
    .map_err(|e| anyhow::anyhow!("Failed to read {}: {e}", path.display()))?;
    String::from_utf8(bytes)
        .map_err(|e| anyhow::anyhow!("{} is not valid UTF-8: {e}", path.display()))
}

/// 控制键常量归 TUN 事务模块所有；这里只做再导出，保持 config 内
/// 既有引用与 `config::TUN_TRANSACTION_CONTROLLED_KEYS` 调用点不变。
pub(crate) use crate::tun_transaction::TUN_TRANSACTION_CONTROLLED_KEYS;

/// 从 override 中**剔除**事务/受管端点独占的键，返回被剔除的键名。
///
/// - `tun` 中的控制键（见 [`TUN_TRANSACTION_CONTROLLED_KEYS`]）：归 TUN 事务管辖，
///   override 若能静默改变 `tun.enable`，会绕过事务把意图翻回去（review MAJOR-1）；
///   非控制键原样保留（review 回归-3）；
/// - `external-controller*` / `external-ui*`：由 `inject_runtime_controller_for_endpoint`
///   独占，override 写入会把 API 端点改到任意地址（review BLOCK-1、MINOR-a）。
fn reject_transaction_owned_override_keys(override_map: &mut serde_yaml::Mapping) -> Vec<String> {
    let mut rejected = Vec::new();
    if let Some(serde_yaml::Value::Mapping(tun)) =
        override_map.get_mut(serde_yaml::Value::String("tun".to_string()))
    {
        for key in TUN_TRANSACTION_CONTROLLED_KEYS {
            if tun
                .remove(serde_yaml::Value::String((*key).to_string()))
                .is_some()
            {
                rejected.push(format!("tun.{key}"));
            }
        }
        if tun.is_empty() {
            override_map.remove(serde_yaml::Value::String("tun".to_string()));
        }
    }
    for key in [
        "external-controller",
        "external-controller-unix",
        "external-controller-pipe",
        "external-controller-tls",
        "external-ui",
        "external-ui-url",
        "external-ui-name",
    ] {
        if override_map
            .remove(serde_yaml::Value::String(key.to_string()))
            .is_some()
        {
            rejected.push(key.to_string());
        }
    }
    rejected
}

/// 生成路径专用：剔除并输出可观测告警（§4.6：不采用用户可见配置必须可观察）。
fn warn_rejected_transaction_owned_override_keys(override_map: &mut serde_yaml::Mapping) {
    let rejected = reject_transaction_owned_override_keys(override_map);
    if !rejected.is_empty() {
        crate::log!(
            "  ⚠ override 中的 {} 由 TUN 事务/受管端点管辖，合并时拒绝（§4.7）；TUN 请用 tun on/off 调整",
            rejected.join(", ")
        );
    }
}

#[allow(dead_code)]
fn apply_override_to_config_text(paths: &AppPaths, config_content: &str) -> anyhow::Result<String> {
    apply_override_to_config_text_at_endpoint(paths, config_content, &current_api_endpoint())
}

fn apply_override_to_config_text_at_endpoint(
    paths: &AppPaths,
    config_content: &str,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<String> {
    let mut config: serde_yaml::Value = serde_yaml::from_str(config_content)
        .map_err(|e| anyhow::anyhow!("Generated config is invalid YAML before override: {}", e))?;
    apply_override_at_endpoint(paths, &mut config, endpoint)?;
    Ok(serde_yaml::to_string(&config)?)
}

/// reconcile 的锁内返回值（review MAJOR-3）。
///
/// 调用方必须使用这里返回的 `content` / `revision`，**不得在锁外重读 intent 文件**，
/// 否则“基线比对 → 合并 → 写回”之间的并发写入仍会漏检。
pub(crate) struct ReconciledIntent {
    pub(crate) changed: bool,
    pub(crate) content: String,
    pub(crate) revision: String,
}

/// B：把 `override.yaml` **幂等地**合并回 intent `config.yaml`（锁内版本）。
///
/// `override import` 本身会合并，但配置的其它写入路径可能产出不含 override 派生键的
/// intent；随后 `restart` 会把这个残缺的 intent 当作启动 payload，静默丢掉用户的 override。
/// 这里在“以 intent 为源的启动/应用入口”先做一次收敛，保证 override 参与合并（ADR-22）。
///
/// - 语义等价时（override 已合并）什么都不写，不改变文件字节 → 不干扰基线 revision；
/// - **不合并** TUN 控制键与受管 runtime 端点键（§4.7 / review BLOCK-1、MAJOR-1、回归-3）；
/// - 基线比对与合并写回都在 `ConfigLock` 内完成（review MAJOR-3）；
/// - 复用生成流程的悬空规则兜底，保证与生成路径幂等（review MAJOR-2）。
pub(crate) fn reconcile_override_into_intent_checked(
    paths: &AppPaths,
    base_revision: Option<&str>,
) -> anyhow::Result<ReconciledIntent> {
    let config_path = paths.config_path();
    if !config_path.exists() {
        anyhow::bail!("intent config {} does not exist", config_path.display());
    }
    // 线程内可重入的配置锁：与 `tun on` 的 intent 提交互斥，
    // 避免“读完 intent → 并发提交 → 用旧内容覆盖写回”丢掉 TUN 意图（review MAJOR-3）。
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let current = read_config_text_bounded(&config_path)?;

    // 基线比对必须在**合并之前**、且在锁内完成：reconcile 主动改写 intent 不能成为
    // 跳过并发检查的理由（原实现 `if !reconciled` 会整体跳过，见 review MAJOR-3）。
    if let Some(expected_base) = base_revision {
        let pre_revision = crate::tun_transaction::content_revision(current.as_bytes());
        if pre_revision != expected_base {
            anyhow::bail!(
                "config.yaml was modified concurrently (expected revision {}, found {})",
                &expected_base[..expected_base.len().min(12)],
                &pre_revision[..pre_revision.len().min(12)],
            );
        }
    }

    if !paths.override_path().exists() {
        return Ok(ReconciledIntent {
            changed: false,
            revision: crate::tun_transaction::content_revision(current.as_bytes()),
            content: current,
        });
    }
    let override_content = read_config_text_bounded(&paths.override_path())?;
    let mut override_value: serde_yaml::Value = serde_yaml::from_str(&override_content)
        .map_err(|e| anyhow::anyhow!("override.yaml is not valid YAML: {e}"))?;
    // 缺陷 2：类型绕过（tun: null/标量/数组）在合并前拒绝，`tun: {}` 放行。
    validate_override_tun_shape(&override_value)?;
    validate_override_proxies_append_shape(&override_value)?;
    let current_value: serde_yaml::Value = serde_yaml::from_str(&current)
        .map_err(|e| anyhow::anyhow!("config.yaml is not valid YAML: {e}"))?;
    let mut merged = current_value.clone();
    merge_proxies_append(&mut merged, &mut override_value);
    let override_map = override_value
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("override.yaml must be a YAML mapping"))?;
    // BLOCK-1：runtime 端点字段与 TUN 控制键必须在合并前剥离；
    // 生成流程靠 `inject_runtime_controller_for_endpoint` 收尾，reconcile 不做端点注入，
    // 不剥离就会把 override 里的 external-controller/external-ui 写进 intent（可开放 0.0.0.0 API）。
    // 这里**静默**剔除（MINOR-c）：用户已在 `override import` 与生成路径收到过告警，
    // 每次 reload/restart 重复打印会淹没正常输出。
    let _rejected = reject_transaction_owned_override_keys(override_map);
    deep_merge_yaml(&mut merged, override_value);
    // 与生成流程同序（override → 悬空规则兜底），否则 override 中指向已删除组的旧规则
    // 会把已兜底成 DIRECT 的取值改回去，reload 时配置校验失败且每次都不收敛（review MAJOR-2）。
    // MINOR-b：override 可能残留旧 DNS 默认值（28.0.0.1/8、198.51.100.53），
    // 与 `tun on` 的归一化来回拉锯；在同一条合并路径里收敛到同一取值，双方即刻收敛。
    if let Some(root) = merged.as_mapping_mut() {
        normalize_dns_legacy_defaults(root);
    }
    let merged_text = serde_yaml::to_string(&merged)?;
    let merged_text = apply_dangling_rule_fallback_to_config_text(&merged_text)?;
    let merged_value: serde_yaml::Value = serde_yaml::from_str(&merged_text)
        .map_err(|e| anyhow::anyhow!("merged config is not valid YAML: {e}"))?;
    // 忽略映射键序比较：键序变化不代表 override 派生键缺失。
    if yaml_structurally_equal(&current_value, &merged_value) {
        return Ok(ReconciledIntent {
            changed: false,
            revision: crate::tun_transaction::content_revision(current.as_bytes()),
            content: current,
        });
    }
    utils::atomic_write_file_for_original_user(&config_path.display().to_string(), &merged_text)?;
    Ok(ReconciledIntent {
        changed: true,
        revision: crate::tun_transaction::content_revision(merged_text.as_bytes()),
        content: merged_text,
    })
}

/// Warning produced by the enterprise intranet guard linter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntranetGuardWarning {
    pub message: String,
    pub recommendation: String,
}

/// RFC1918 IPv4 ranges the intranet guard expects to be covered by DIRECT or route exclusion.
const PRIVATE_V4_RANGES: &[&str] = &["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"];

/// Parse an IPv4 CIDR into an inclusive integer interval. Invalid or IPv6 values are ignored.
fn parse_ipv4_cidr(cidr: &str) -> Option<(u32, u32)> {
    let (addr, prefix) = cidr.trim().split_once('/')?;
    let ip: std::net::Ipv4Addr = addr.parse().ok()?;
    let prefix: u32 = prefix.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let start = u32::from(ip) & mask;
    Some((start, start | !mask))
}

/// Whether the union of `cidrs` fully covers `target` (an IPv4 CIDR).
fn cidrs_cover(cidrs: &[String], target: &str) -> bool {
    let Some((target_start, target_end)) = parse_ipv4_cidr(target) else {
        return false;
    };
    let mut intervals: Vec<(u32, u32)> = cidrs
        .iter()
        .filter_map(|cidr| parse_ipv4_cidr(cidr))
        .filter_map(|(start, end)| {
            let start = start.max(target_start);
            let end = end.min(target_end);
            (start <= end).then_some((start, end))
        })
        .collect();
    intervals.sort_unstable();

    let mut covered_until = target_start;
    for (start, end) in intervals {
        if start > covered_until {
            return false;
        }
        if end >= target_end {
            return true;
        }
        if end >= covered_until {
            covered_until = end.saturating_add(1);
        }
    }
    false
}

fn covers_private_ranges(cidrs: &[String]) -> bool {
    PRIVATE_V4_RANGES
        .iter()
        .all(|target| cidrs_cover(cidrs, target))
}

/// SPEC: Lint configuration for intranet asset protection.
pub fn lint_intranet_guard(config: &serde_yaml::Value) -> Vec<IntranetGuardWarning> {
    let mut warnings = Vec::new();

    // 1. Check if sniffer override-destination is enabled
    let sniffer_override = config
        .get("sniffer")
        .and_then(|s| s.as_mapping())
        .is_some_and(|m| {
            let enabled = m
                .get(serde_yaml::Value::String("enable".into()))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let override_dest = m
                .get(serde_yaml::Value::String("override-destination".into()))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            enabled && override_dest
        });

    if !sniffer_override {
        return warnings;
    }

    // The guard describes a TUN topology risk (sniffer override + proxy fallback +
    // private ranges captured by TUN). Without TUN enabled the warning would be a
    // false positive for system-proxy/per-user users.
    let tun_enabled = config
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(|enabled| enabled.as_bool())
        .unwrap_or(false);
    if !tun_enabled {
        return warnings;
    }

    // 2. Check if the fallback / MATCH policy routes to proxy.
    let mut fallback_is_proxy = false;
    let mut direct_cidrs: Vec<String> = Vec::new();
    let mut geoip_private_direct = false;

    if let Some(rules) = config.get("rules").and_then(|v| v.as_sequence()) {
        for r in rules {
            if let Some(s) = r.as_str() {
                let s_trim = s.trim();
                let upper = s_trim.to_ascii_uppercase();
                if upper.starts_with("MATCH,") {
                    let policy = s_trim.split(',').nth(1).unwrap_or("").trim();
                    if !policy.eq_ignore_ascii_case("DIRECT")
                        && !policy.eq_ignore_ascii_case("REJECT")
                        && !policy.eq_ignore_ascii_case("REJECT-DROP")
                        && !policy.eq_ignore_ascii_case("PASS")
                    {
                        fallback_is_proxy = true;
                    }
                } else if upper.starts_with("IP-CIDR,") || upper.starts_with("IP-CIDR6,") {
                    let parts: Vec<&str> = s_trim.split(',').map(str::trim).collect();
                    if parts.len() >= 3 && parts[2].eq_ignore_ascii_case("DIRECT") {
                        direct_cidrs.push(parts[1].to_string());
                    }
                } else if upper.starts_with("GEOIP,") {
                    let parts: Vec<&str> = s_trim.split(',').map(str::trim).collect();
                    if parts.len() >= 3 && parts[2].eq_ignore_ascii_case("DIRECT") {
                        let country = parts[1].to_ascii_uppercase();
                        if country == "PRIVATE" || country == "LAN" {
                            geoip_private_direct = true;
                        }
                    }
                }
            }
        }
    }

    let direct_rules_cover_private = geoip_private_direct || covers_private_ranges(&direct_cidrs);

    // 3. Check if tun.route-exclude-address excludes RFC1918
    let mut exclude_cidrs: Vec<String> = Vec::new();

    if let Some(tun) = config.get("tun").and_then(|v| v.as_mapping()) {
        if let Some(excludes) = tun
            .get(serde_yaml::Value::String("route-exclude-address".into()))
            .and_then(|v| v.as_sequence())
        {
            for item in excludes {
                if let Some(cidr) = item.as_str() {
                    exclude_cidrs.push(cidr.trim().to_string());
                }
            }
        }
    }

    let exclude_covers_private = covers_private_ranges(&exclude_cidrs);

    // If sniffer overrides destination and fallback is proxy, but private ranges are unprotected:
    if fallback_is_proxy && !direct_rules_cover_private && !exclude_covers_private {
        warnings.push(IntranetGuardWarning {
            message: "检测到内网安全隐患拓扑：已启用 Sniffer 目标覆写（override-destination: true）且兜底规则为代理，但未完整配置 RFC1918 私有网段直连保护或路由排除。".to_string(),
            recommendation: "内网流量被嗅探后若无精确直连规则，可能滑落至代理节点导致连接被强拆。建议在 override.yaml 的 tun.route-exclude-address 中排除内网网段（如 [10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16]），或在 rules.yaml 中添加内网 IP-CIDR/DOMAIN 直连规则。".to_string(),
        });
    }

    warnings
}

/// Rewrite rules whose target policy is absent from the merged config (groups + proxies +
/// built-ins) to target `DIRECT`, emitting a warning for each rewritten rule.
///
/// This operates purely on the **in-memory** `config_content` string; it never touches
/// `rules.yaml` on disk.
fn apply_dangling_rule_fallback_to_config_text(config_content: &str) -> anyhow::Result<String> {
    use crate::groups::BUILTIN_POLICIES;

    let mut config: serde_yaml::Value = serde_yaml::from_str(config_content).map_err(|e| {
        anyhow::anyhow!(
            "Generated config is invalid YAML before dangling rule fallback: {}",
            e
        )
    })?;

    // Collect the full set of valid policy names from the merged config.
    let mut valid_policies: std::collections::HashSet<String> =
        BUILTIN_POLICIES.iter().map(|s| s.to_string()).collect();

    // Proxy groups
    if let Some(groups) = config
        .get("proxy-groups")
        .and_then(serde_yaml::Value::as_sequence)
    {
        for g in groups {
            if let Some(name) = g.get("name").and_then(serde_yaml::Value::as_str) {
                valid_policies.insert(name.to_string());
            }
        }
    }

    // Proxy nodes
    if let Some(proxies) = config
        .get("proxies")
        .and_then(serde_yaml::Value::as_sequence)
    {
        for proxy in proxies {
            if let Some(name) = proxy.get("name").and_then(serde_yaml::Value::as_str) {
                valid_policies.insert(name.to_string());
            }
        }
    }

    // Proxy providers
    if let Some(providers) = config
        .get("proxy-providers")
        .and_then(serde_yaml::Value::as_mapping)
    {
        for key in providers.keys() {
            if let Some(name) = key.as_str() {
                valid_policies.insert(name.to_string());
            }
        }
    }

    // Rewrite dangling rules
    let Some(rules_val) = config.get_mut("rules") else {
        return Ok(serde_yaml::to_string(&config)?);
    };
    let Some(rules_seq) = rules_val.as_sequence_mut() else {
        return Ok(serde_yaml::to_string(&config)?);
    };

    for rule_val in rules_seq.iter_mut() {
        let Some(rule) = rule_val.as_str() else {
            continue;
        };
        let Some(policy) = crate::rules::simple_rule_policy(rule) else {
            continue;
        };
        if valid_policies.contains(policy) || crate::groups::is_builtin_policy(policy) {
            continue;
        }
        // Policy is dangling – rewrite to DIRECT
        let rewritten = rewrite_rule_policy(rule, "DIRECT");
        eprintln!(
            "  ⚠ [WARN] Rule [{rule}] target group `{policy}` missing in the effective config; downgraded to DIRECT for the effective config"
        );
        *rule_val = serde_yaml::Value::String(rewritten);
    }

    Ok(serde_yaml::to_string(&config)?)
}

/// carry the user's TUN intent across config regeneration.
///
/// `tun on` commits a `tun` block into the intent `config.yaml`
/// (`compare_and_commit_user_intent`). The regeneration pipeline rebuilds
/// config.yaml from the subscription and would silently drop that block,
/// making `configured_tun` unobservable and breaking runtime attestation
/// (`launched == active == intent`). Preserve the previous `tun` block when the
/// freshly generated content does not define one. The block is appended last,
/// matching the daemon's `merge_snapshot_tun_block_into_config_yaml` insertion
/// point, so the promoted runtime snapshot and the intent file stay
/// byte-identical.
fn carry_over_tun_block_from_previous_config(
    previous_content: Option<&str>,
    new_content: &str,
) -> anyhow::Result<String> {
    let Some(previous) = previous_content else {
        return Ok(new_content.to_string());
    };
    // （P2-4，deliberate）：上一份 intent 不可解析或根节点不是
    // 映射时，这里**有意**静默降级为“没有可携带的 TUN 意图”。I/O 不可读已在
    // 调用方 fail-closed（EACCES/读失败直接拒绝重生成）；只有内容已损坏的历史
    // 文件会走到这里，此时再失败只会让 refresh/import 永久不可用，而新生成的
    // 配置随后仍需通过完整 YAML 校验。
    let Ok(previous_yaml) = serde_yaml::from_str::<serde_yaml::Value>(previous) else {
        return Ok(new_content.to_string());
    };
    let Some(tun_block) = previous_yaml
        .as_mapping()
        .and_then(|map| map.get(serde_yaml::Value::String("tun".to_string())))
    else {
        return Ok(new_content.to_string());
    };

    let mut config: serde_yaml::Value = serde_yaml::from_str(new_content).map_err(|e| {
        anyhow::anyhow!(
            "Generated config is invalid YAML before TUN carry-over: {}",
            e
        )
    })?;
    // 新生成内容不是映射时同样直接返回原文：carry-over 不制造第二种错误，
    // 后续 validate 会以既有文案拒绝该配置。
    let Some(map) = config.as_mapping_mut() else {
        return Ok(new_content.to_string());
    };
    if map
        .get(serde_yaml::Value::String("tun".to_string()))
        .is_none()
    {
        map.insert(
            serde_yaml::Value::String("tun".to_string()),
            tun_block.clone(),
        );
        return Ok(serde_yaml::to_string(&config)?);
    }
    // B：新配置里已经有一个 `tun` 块（订阅/模板自带 `enable: false`，
    // 或 override 的非控制键）时，不能直接跳过——否则上一次 `tun on` 写下的
    // 完整 TUN 意图（enable/stack/dns-hijack…）会被这个块静默顶掉，`tun.enable` 直接消失。
    //
    // 按**键分类**的优先级（review MAJOR-4 + 回归-3）：
    //   - 控制键（TUN_TRANSACTION_CONTROLLED_KEYS）：历史 TUN 意图优先（归 `tun on/off` 事务），
    //     因此模板的 `enable: false` 翻不掉 `tun on` 的 true；
    //   - 非控制键（mtu / device / route-exclude-address…）：新块优先——override 是 ADR-22
    //     最后合并的用户层，这些键必须能通过 override 更新；新块没有时才由历史意图补齐。
    let new_tun = map
        .get(serde_yaml::Value::String("tun".to_string()))
        .cloned()
        .unwrap_or(serde_yaml::Value::Null);
    let mut merged_tun = new_tun.clone();
    if let (serde_yaml::Value::Mapping(previous_tun), serde_yaml::Value::Mapping(merged_map)) =
        (&tun_block, &mut merged_tun)
    {
        // fill=true：新生成内容是订阅模板，缺失非控制键只表示模板没提，
        // 必须由历史 intent 补齐；该语义与 promotion 的 payload-strict 不同。
        crate::tun_transaction::merge_tun_key_policy(previous_tun, merged_map, true);
    }
    map.insert(serde_yaml::Value::String("tun".to_string()), merged_tun);
    Ok(serde_yaml::to_string(&config)?)
}

/// 参考实现（mihomo 官方 wiki `config/inbound/tun` 与 Clash Verge Rev 默认配置）
/// 为 TUN 声明的 DNS 劫持目标。
///
/// 只劫持 UDP 53 无法覆盖走 TCP 的 DNS 查询，对齐参考实现需要
/// 同时包含 `any:53` 与 `tcp://any:53`。
pub(crate) const DEFAULT_DNS_HIJACK_SPEC: &str = "any:53,tcp://any:53";

/// 旧版 CLI 写入的单项默认值；升级时需收敛到 [`DEFAULT_DNS_HIJACK_SPEC`]。
const LEGACY_DNS_HIJACK_ENTRIES: &[&str] = &["any:53", "0.0.0.0:53"];

/// `--dns-hijack` 接受逗号分隔的多个目标，返回去空白后的条目列表。
pub(crate) fn dns_hijack_entries(spec: &str) -> Vec<String> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 参考实现的默认 DNS 劫持条目。
pub(crate) fn default_dns_hijack_entries() -> Vec<String> {
    dns_hijack_entries(DEFAULT_DNS_HIJACK_SPEC)
}

fn dns_hijack_entries_of(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::String(s) => dns_hijack_entries(s),
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn dns_hijack_needs_alignment(value: Option<&serde_yaml::Value>) -> bool {
    match value {
        None => true,
        Some(value) => {
            let entries = dns_hijack_entries_of(value);
            entries.is_empty()
                || (entries.len() == 1 && LEGACY_DNS_HIJACK_ENTRIES.contains(&entries[0].as_str()))
        }
    }
}

fn yaml_string_sequence(entries: Vec<String>) -> serde_yaml::Value {
    serde_yaml::Value::Sequence(entries.into_iter().map(serde_yaml::Value::String).collect())
}

/// `tun on` 归一化 TUN 块，使其与参考实现一致（对齐清单）：
///
/// - `dns-hijack`：显式 `--dns-hijack` 优先；未指定时，仅当现有值缺失、为空，或仍是
///   旧版单项默认（`any:53` / `0.0.0.0:53`）才收敛为 [`DEFAULT_DNS_HIJACK_SPEC`]，
///   用户自定义目标一律保留；
/// - `strict-route`：仅在缺失时补 `true`（参考实现用它强制所有连接进入 TUN、防止 DNS
///   泄漏），用户显式写下的 `false` 不被覆盖。
///
/// 两个字段都写进 intent `tun` 块，因此仍由既有 candidate/revision 事务、snapshot
/// promotion 与 `launched == active == intent` attestation 覆盖，不引入第二个配置来源。
pub(crate) fn normalize_tun_block_for_enable(
    tun: &mut serde_yaml::Mapping,
    dns_hijack: Option<&str>,
) {
    if let Some(spec) = dns_hijack {
        tun.insert(
            serde_yaml::Value::String("dns-hijack".to_string()),
            yaml_string_sequence(dns_hijack_entries(spec)),
        );
    } else if dns_hijack_needs_alignment(tun.get("dns-hijack")) {
        tun.insert(
            serde_yaml::Value::String("dns-hijack".to_string()),
            yaml_string_sequence(default_dns_hijack_entries()),
        );
    }

    if !tun.contains_key("strict-route") {
        tun.insert(
            serde_yaml::Value::String("strict-route".to_string()),
            serde_yaml::Value::Bool(true),
        );
    }
}

/// （A3/A4）：收敛旧版模板遗留的 DNS 默认值，使其与参考实现一致。
///
/// - `dns.fake-ip-range` 曾默认 `28.0.0.1/8`（真实公网地址段，且 TUN 接口地址由该段派生），
///   收敛为参考实现的 `198.18.0.1/16`；
/// - `dns.default-nameserver` 曾写入 `198.51.100.53`（文档保留段 TEST-NET-2，无可用主机），
///   直接剔除；剔除后为空则补参考实现的 `223.5.5.5`。
///
/// **只在取值完全等于我们自己写过的旧默认值时才改写**，订阅方/用户自定义取值原样保留；
/// 没有 `dns` 块时直接返回（旧的 direct-only 基础配置就是这种）。
///
/// 两个调用点：`config` 生成漏斗（`generate_config_yaml_with_fake_ip_filters_for_endpoint`）
/// 与 `tun on`（`build_tun_candidate`），后者让存量安装不必额外跑一次 `config -u`。
pub(crate) fn normalize_dns_legacy_defaults(config: &mut serde_yaml::Mapping) {
    const LEGACY_FAKE_IP_RANGE: &str = "28.0.0.1/8";
    const REFERENCE_FAKE_IP_RANGE: &str = "198.18.0.1/16";
    const LEGACY_TEST_NET_NAMESERVER: &str = "198.51.100.53";
    const REFERENCE_FALLBACK_NAMESERVER: &str = "223.5.5.5";

    let Some(dns) = config.get_mut("dns").and_then(|v| v.as_mapping_mut()) else {
        return;
    };

    if dns
        .get("fake-ip-range")
        .is_some_and(|v| v.as_str() == Some(LEGACY_FAKE_IP_RANGE))
    {
        dns.insert(
            serde_yaml::Value::String("fake-ip-range".to_string()),
            serde_yaml::Value::String(REFERENCE_FAKE_IP_RANGE.to_string()),
        );
    }

    let needs_fallback_nameserver =
        if let Some(serde_yaml::Value::Sequence(list)) = dns.get_mut("default-nameserver") {
            let before = list.len();
            list.retain(|v| v.as_str() != Some(LEGACY_TEST_NET_NAMESERVER));
            list.len() != before && list.is_empty()
        } else {
            false
        };
    if needs_fallback_nameserver {
        dns.insert(
            serde_yaml::Value::String("default-nameserver".to_string()),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String(
                REFERENCE_FALLBACK_NAMESERVER.to_string(),
            )]),
        );
    }
}

/// Replace the last significant comma-separated token in a rule string with `new_policy`.
/// For `MATCH,<policy>` (2 tokens) the second token is replaced; for all other types
/// (3-4 tokens) the third token is replaced, preserving any trailing option (e.g. `no-resolve`).
fn rewrite_rule_policy(rule: &str, new_policy: &str) -> String {
    let parts: Vec<&str> = rule.splitn(4, ',').collect();
    match parts.as_slice() {
        // MATCH,<policy>
        [rule_type, _policy] if rule_type.trim().eq_ignore_ascii_case("MATCH") => {
            format!("{},{}", rule_type, new_policy)
        }
        // TYPE,PARAM,<policy>
        [rule_type, param, _policy] => {
            format!("{},{},{}", rule_type, param, new_policy)
        }
        // TYPE,PARAM,<policy>,option  (e.g. no-resolve)
        [rule_type, param, _policy, option] => {
            format!("{},{},{},{}", rule_type, param, new_policy, option)
        }
        _ => rule.to_string(),
    }
}

#[allow(dead_code)]
fn apply_groups_override_to_config_text(
    paths: &AppPaths,
    subscription_id: &str,
    config_content: &str,
) -> anyhow::Result<String> {
    apply_groups_override_to_config_text_with_mode(
        paths,
        subscription_id,
        config_content,
        crate::groups::MergeMode::ActiveStrict,
    )
}

fn apply_groups_override_to_config_text_with_mode(
    paths: &AppPaths,
    subscription_id: &str,
    config_content: &str,
    mode: crate::groups::MergeMode,
) -> anyhow::Result<String> {
    let overlay_path = paths.groups_override_path_for_subscription(subscription_id);
    let overlay = crate::groups::GroupsOverlay::load(&overlay_path)?;
    if overlay.prepend.is_empty()
        && overlay.append.is_empty()
        && overlay.delete.is_empty()
        && overlay.patches.is_empty()
    {
        return Ok(config_content.to_string());
    }

    let global_override_path = paths.override_path();
    let mut override_proxies = Vec::new();
    let mut override_providers = Vec::new();
    if global_override_path.exists() {
        let global_content = std::fs::read_to_string(&global_override_path).with_context(|| {
            format!(
                "Failed to read override.yaml: {}",
                global_override_path.display()
            )
        })?;
        let global: serde_yaml::Value =
            serde_yaml::from_str(&global_content).with_context(|| {
                format!(
                    "Failed to parse override.yaml: {}",
                    global_override_path.display()
                )
            })?;
        if global
            .as_mapping()
            .is_some_and(|map| map.contains_key(serde_yaml::Value::String("proxy-groups".into())))
        {
            anyhow::bail!(
                "groups overlay cannot be applied while override.yaml defines proxy-groups; remove that section or edit it through its owning configuration"
            );
        }
        override_proxies = extract_override_proxies(&global);
        override_providers = extract_override_providers(&global);
    }

    let mut config: serde_yaml::Value = serde_yaml::from_str(config_content).map_err(|e| {
        anyhow::anyhow!(
            "Generated config is invalid YAML before groups overlay: {}",
            e
        )
    })?;
    let config_map = config
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Generated config must be a YAML mapping"))?;
    let original_groups = match config_map.remove(serde_yaml::Value::String("proxy-groups".into()))
    {
        Some(serde_yaml::Value::Sequence(groups)) => groups,
        Some(value) => {
            config_map.insert(serde_yaml::Value::String("proxy-groups".into()), value);
            anyhow::bail!("Generated config proxy-groups must be a list")
        }
        None => Vec::new(),
    };
    let mut known_proxies: std::collections::HashSet<String> = config_map
        .get(serde_yaml::Value::String("proxies".into()))
        .and_then(serde_yaml::Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(|proxy| match proxy {
            serde_yaml::Value::Mapping(map) => map
                .get(serde_yaml::Value::String("name".into()))
                .and_then(serde_yaml::Value::as_str),
            serde_yaml::Value::String(name) => Some(name.as_str()),
            _ => None,
        })
        .map(str::to_owned)
        .collect();
    for p in override_proxies {
        known_proxies.insert(p);
    }

    let mut known_providers: std::collections::HashSet<String> = config_map
        .get(serde_yaml::Value::String("proxy-providers".into()))
        .and_then(serde_yaml::Value::as_mapping)
        .into_iter()
        .flatten()
        .filter_map(|(name, _)| name.as_str().map(str::to_owned))
        .collect();
    for p in override_providers {
        known_providers.insert(p);
    }
    let (merged, report) = overlay.merged_groups_with_mode(
        &original_groups,
        &known_proxies,
        &known_providers,
        mode,
    )?;
    if mode == crate::groups::MergeMode::PassiveTolerant {
        for warning in &report.warnings {
            eprintln!("  ⚠ [WARN] {warning}");
        }
    }
    config_map.insert(
        serde_yaml::Value::String("proxy-groups".into()),
        serde_yaml::Value::Sequence(merged),
    );
    Ok(serde_yaml::to_string(&config)?)
}

/// Merge user-defined rules and DNS policies into config.yaml.
///
/// New implementation: reads from subscriptions/<active-id>.yaml and generates config.yaml.
/// Falls back to old behavior if no active subscription (for backward compatibility during transition).
pub fn merge_user_config_at(paths: &AppPaths) -> anyhow::Result<()> {
    merge_user_config_at_endpoint(paths, &current_api_endpoint())
}

pub fn merge_user_config_at_endpoint(
    paths: &AppPaths,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<()> {
    use crate::rules::{self, RulePosition};

    // Try new multi-subscription flow first
    let active_id = get_active_id_at(paths)?;

    if let Some(id) = active_id {
        // New flow: read from subscription file
        let sub_path = paths.subscription_file_path(&id);
        if !sub_path.exists() {
            anyhow::bail!(
                "Subscription file not found: {}\n  \
                 Run: mihomo-cli config --refresh  to re-download",
                sub_path.display()
            );
        }

        let sub_content = std::fs::read_to_string(&sub_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to read subscription file {}: {}\n  \
                 Run: mihomo-cli config --refresh  to re-download",
                sub_path.display(),
                e
            )
        })?;

        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(&sub_content).map_err(|e| {
            anyhow::anyhow!(
                "Subscription file {} is not valid YAML: {}\n  \
                 The file may have been manually edited with a syntax error.",
                sub_path.display(),
                e
            )
        })?;

        validate_subscription_yaml(&sub_yaml)?;

        let user_rules = if paths.rules_path().exists() {
            rules::load_rules_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };

        let dns_policies = if paths.dns_policy_path().exists() {
            crate::dns::load_policies_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };

        let fake_ip_filters = if paths.dns_fake_ip_filter_path().exists() {
            crate::dns::load_fake_ip_filters_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };

        let position = rules::get_position_at(paths).unwrap_or(RulePosition::Front);

        let config_content = generate_config_yaml_with_fake_ip_filters_for_endpoint(
            &sub_yaml,
            &user_rules,
            &dns_policies,
            &fake_ip_filters,
            position,
            endpoint,
        )?;
        let config_content = apply_groups_override_to_config_text_with_mode(
            paths,
            &id,
            &config_content,
            crate::groups::MergeMode::PassiveTolerant,
        )?;
        // the dangling-rule fallback must see the FINAL merged view,
        // so groups defined only in override.yaml count as valid targets.
        let config_content =
            apply_override_to_config_text_at_endpoint(paths, &config_content, endpoint)?;
        let config_content = apply_dangling_rule_fallback_to_config_text(&config_content)?;
        // regeneration must not drop the TUN intent committed by `tun on`.
        // 读不到上一份 intent 时**不能**退化成"没有 TUN 意图"继续写——
        // 那会把用户的 tun 控制键静默删除（override import / config -u 都会走到这里）。
        let previous_config = match std::fs::read_to_string(paths.config_path()) {
            Ok(content) => Some(content),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                anyhow::bail!(
                    "cannot read the existing config.yaml for TUN intent carry-over: {err}\n  config.yaml was NOT modified."
                );
            }
        };
        let config_content =
            carry_over_tun_block_from_previous_config(previous_config.as_deref(), &config_content)?;

        // Validate the generated YAML
        serde_yaml::from_str::<serde_yaml::Value>(&config_content)
            .map_err(|e| anyhow::anyhow!("Generated config is invalid YAML: {}", e))?;

        utils::atomic_write_file_for_original_user(
            &paths.config_path().display().to_string(),
            &config_content,
        )?;

        crate::log!(
            "Generated config from subscription {}: {} rules, {} DNS policies",
            id,
            user_rules.len(),
            dns_policies.len()
        );
    } else if paths.config_path().exists() {
        // Preserve a manually imported legacy config until a subscription is activated.
        let config_path = paths.config_path();
        let config_content = std::fs::read_to_string(&config_path)
            .map_err(|e| anyhow::anyhow!("Failed to read config.yaml: {e}"))?;
        let user_rules = if paths.rules_path().exists() {
            rules::load_rules_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };
        let dns_policies = if paths.dns_policy_path().exists() {
            crate::dns::load_policies_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };
        let position = rules::get_position_at(paths).unwrap_or(RulePosition::Front);
        use crate::yaml_editor::YamlEditor;
        let mut editor = YamlEditor::parse(&config_content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config.yaml: {e}"))?;
        editor
            .merge_rules(&user_rules, matches!(position, RulePosition::Front))
            .map_err(|e| anyhow::anyhow!("Failed to merge user rules: {e}"))?;
        if !dns_policies.is_empty() {
            editor
                .merge_dns_policies(&dns_policies)
                .map_err(|e| anyhow::anyhow!("Failed to merge user DNS policies: {e}"))?;
        }
        let result =
            apply_override_to_config_text_at_endpoint(paths, &editor.into_source(), endpoint)?;
        let result = apply_dangling_rule_fallback_to_config_text(&result)?;
        serde_yaml::from_str::<serde_yaml::Value>(&result)
            .map_err(|e| anyhow::anyhow!("Merged config is invalid YAML: {e}"))?;
        if result != config_content {
            utils::atomic_write_file_for_original_user(
                &config_path.display().to_string(),
                &result,
            )?;
        }
        crate::log!("Preserved legacy config without an active subscription");
    } else {
        let user_rules = if paths.rules_path().exists() {
            rules::load_rules_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };
        let dns_policies = if paths.dns_policy_path().exists() {
            crate::dns::load_policies_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };
        let fake_ip_filters = if paths.dns_fake_ip_filter_path().exists() {
            crate::dns::load_fake_ip_filters_at(paths).unwrap_or_default()
        } else {
            Vec::new()
        };
        let position = rules::get_position_at(paths).unwrap_or(RulePosition::Front);
        let config_content = generate_direct_only_config_yaml_for_endpoint(
            endpoint,
            &user_rules,
            &dns_policies,
            &fake_ip_filters,
            position,
        )?;
        let config_content =
            apply_override_to_config_text_at_endpoint(paths, &config_content, endpoint)?;
        let config_content = apply_dangling_rule_fallback_to_config_text(&config_content)?;
        serde_yaml::from_str::<serde_yaml::Value>(&config_content)
            .map_err(|e| anyhow::anyhow!("Generated direct-only config is invalid YAML: {e}"))?;
        utils::atomic_write_file_for_original_user(
            &paths.config_path().display().to_string(),
            &config_content,
        )?;
        crate::log!("Generated direct-only config without an active subscription");
    }

    Ok(())
}

#[allow(dead_code)]
pub fn merge_user_config() -> anyhow::Result<()> {
    merge_user_config_at(&AppPaths::from_system())
}

/// Merge user configuration into config.yaml, then validate the result.
/// If validation fails, config.yaml is restored to its previous contents.
pub fn merge_user_config_checked_at(
    paths: &AppPaths,
    mihomo_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    merge_user_config_checked_at_endpoint(paths, mihomo_path, &current_api_endpoint())
}

pub fn merge_user_config_checked_at_endpoint(
    paths: &AppPaths,
    mihomo_path: Option<&std::path::Path>,
    endpoint: &ApiEndpoint,
) -> anyhow::Result<()> {
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let config_path = paths.config_path();
    let previous =
        if config_path.exists() {
            Some(std::fs::read_to_string(&config_path).map_err(|e| {
                anyhow::anyhow!("Failed to read existing config before merge: {}", e)
            })?)
        } else {
            None
        };

    merge_user_config_at_endpoint(paths, endpoint)?;

    if let Err(validation_error) = validate_config_at(paths, mihomo_path) {
        let rollback = match previous {
            Some(content) => utils::atomic_write_file_for_original_user(
                &config_path.display().to_string(),
                &content,
            ),
            None => utils::remove_file_if_exists(&config_path),
        };
        if let Err(rollback_error) = rollback {
            anyhow::bail!(
                "Config validation failed after write; rollback incomplete: {}.\n  Original error: {}",
                rollback_error,
                validation_error
            );
        }
        anyhow::bail!(
            "Config validation failed after write; rolled back config.yaml.\n  {}",
            validation_error
        );
    }

    Ok(())
}

#[allow(dead_code)]
pub fn merge_user_config_checked() -> anyhow::Result<()> {
    let mihomo = std::path::PathBuf::from(crate::utils::mihomo_path());
    merge_user_config_checked_at(&AppPaths::from_system(), Some(&mihomo))
}

// ── Change classification & fail-open orchestration (R2 / ADR-02) ──────────

/// Change classification for configuration updates (R2 / ADR-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    /// Effective config changes (proxies, rules, DNS, mode, mixed-port, etc.)
    /// that do not alter listener endpoints or TUN device, and can be hot-reloaded
    /// via Core API PUT /configs without restarting Core.
    HotReload,
    /// Changes affecting TUN device (enable/stack/device) or controller listen
    /// endpoints/secret that necessitate a process restart / promotion.
    Promote,
}

fn norm_yaml_field<'a>(val: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    match val.get(key) {
        None | Some(serde_yaml::Value::Null) => None,
        Some(v) => Some(v),
    }
}

impl ChangeKind {
    /// Classifies the difference between `before` YAML and `after` YAML content.
    /// Falls back to `ChangeKind::Promote` if either cannot be parsed.
    pub fn classify_diff(before: &str, after: &str) -> Self {
        let b = serde_yaml::from_str::<serde_yaml::Value>(before).ok();
        let a = serde_yaml::from_str::<serde_yaml::Value>(after).ok();
        match (b, a) {
            (Some(b), Some(a)) => Self::classify_values(&b, &a),
            _ => ChangeKind::Promote,
        }
    }

    /// Classifies differences between two parsed YAML root values.
    pub fn classify_values(before: &serde_yaml::Value, after: &serde_yaml::Value) -> Self {
        // 1. Controller endpoint and authentication fields
        if norm_yaml_field(before, "external-controller")
            != norm_yaml_field(after, "external-controller")
            || norm_yaml_field(before, "external-controller-unix")
                != norm_yaml_field(after, "external-controller-unix")
            || norm_yaml_field(before, "secret") != norm_yaml_field(after, "secret")
        {
            return ChangeKind::Promote;
        }

        // 2. TUN configuration:
        let tun_enabled = |val: &serde_yaml::Value| -> bool {
            val.get("tun")
                .and_then(|t| t.get("enable"))
                .and_then(|e| e.as_bool())
                .unwrap_or(false)
        };
        let before_tun_enabled = tun_enabled(before);
        let after_tun_enabled = tun_enabled(after);
        if before_tun_enabled != after_tun_enabled {
            return ChangeKind::Promote;
        }
        // If TUN is active in either, or if TUN block changes while enabled:
        if (before_tun_enabled || after_tun_enabled) && before.get("tun") != after.get("tun") {
            return ChangeKind::Promote;
        }

        // All other fields (rules, proxies, proxy-groups, dns, mode, mixed-port, etc.)
        // can be reloaded safely via Core API.
        ChangeKind::HotReload
    }

    #[allow(dead_code)]
    pub fn requires_promotion(self) -> bool {
        matches!(self, ChangeKind::Promote)
    }
}

/// Indicates which path actually succeeded in applying the configuration change.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppliedVia {
    HotReload,
    Promotion,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub applied_via: AppliedVia,
}

/// Orchestration that attempts hot-reload first when ChangeKind is HotReload,
/// and fails open to promotion if hot-reload returns an error (R2.3).
#[allow(dead_code)]
pub async fn apply_with_fail_open<R, P, FutR, FutP>(
    change_kind: ChangeKind,
    do_reload: R,
    do_promote: P,
) -> anyhow::Result<ApplyOutcome>
where
    R: FnOnce() -> FutR,
    FutR: std::future::Future<Output = anyhow::Result<()>>,
    P: FnOnce() -> FutP,
    FutP: std::future::Future<Output = anyhow::Result<()>>,
{
    match change_kind {
        ChangeKind::HotReload => match do_reload().await {
            Ok(()) => Ok(ApplyOutcome {
                applied_via: AppliedVia::HotReload,
            }),
            Err(reload_err) => {
                crate::log!("hot-reload failed ({reload_err:#}); failing open to promotion");
                do_promote().await.map(|()| ApplyOutcome {
                    applied_via: AppliedVia::Promotion,
                })
            }
        },
        ChangeKind::Promote => do_promote().await.map(|()| ApplyOutcome {
            applied_via: AppliedVia::Promotion,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{self, RulePosition};
    use std::collections::VecDeque;
    use tempfile::TempDir;

    fn setup_config(content: &str) -> (TempDir, AppPaths) {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), content).unwrap();
        (tmp, paths)
    }

    #[test]
    fn subscription_ua_candidates_cover_common_clash_clients() {
        let labels: Vec<&str> = SUBSCRIPTION_UA_CANDIDATES
            .iter()
            .map(|(label, _)| *label)
            .collect();
        assert_eq!(labels, vec!["clash-verge", "clash-meta", "clash"]);
        assert!(SUBSCRIPTION_UA_CANDIDATES
            .iter()
            .all(|(_, ua)| ua.contains("clash")));
    }

    #[derive(Default)]
    struct FakeSubscriptionFetcher {
        responses: std::sync::Mutex<VecDeque<Result<HttpTextResponse, String>>>,
        requests: std::sync::Mutex<Vec<(String, Option<String>)>>,
    }

    impl FakeSubscriptionFetcher {
        fn new(responses: Vec<Result<HttpTextResponse, String>>) -> Self {
            Self {
                responses: std::sync::Mutex::new(responses.into()),
                requests: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<(String, Option<String>)> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl SubscriptionFetcher for FakeSubscriptionFetcher {
        fn get_text<'a>(
            &'a self,
            url: String,
            user_agent: Option<String>,
        ) -> futures::future::BoxFuture<'a, Result<HttpTextResponse, String>> {
            Box::pin(async move {
                self.requests.lock().unwrap().push((url, user_agent));
                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| Err("no fake response".to_string()))
            })
        }
    }

    fn yaml_response(body: &str) -> Result<HttpTextResponse, String> {
        Ok(HttpTextResponse {
            status: 200,
            body: body.to_string(),
        })
    }

    #[tokio::test]
    async fn fetch_subscription_to_file_writes_only_requested_output() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("nested/config.yaml");
        let report = fetch_subscription_content_to_file(
            DownloadResult {
                content: "proxies:\n  - name: direct\n    type: direct\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n".to_string(),
                is_clash_yaml: true,
            },
            &out,
        )
        .unwrap();

        assert!(out.exists());
        assert_eq!(report.proxy_count, 1);
        assert_eq!(report.proxy_group_count, 0);
        assert_eq!(report.rule_count, 1);
        assert!(!tmp.path().join("subscriptions").exists());
        assert!(!tmp.path().join("active").exists());
        assert!(!tmp.path().join("rules.yaml").exists());
    }

    #[tokio::test]
    async fn auto_ua_negotiation_stops_after_first_clash_yaml() {
        let fetcher = FakeSubscriptionFetcher::new(vec![yaml_response(
            "proxies:\n  - name: direct\n    type: direct\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n",
        )]);

        let result =
            download_sub_with_user_agent_and_fetcher("https://example.test/sub", None, &fetcher)
                .await
                .unwrap();

        assert!(result.is_clash_yaml);
        let requests = fetcher.requests();
        assert_eq!(requests.len(), 1, "auto mode should stop after first YAML");
        assert_eq!(requests[0].1.as_deref(), Some("clash-verge/v2.0.4"));
        assert!(requests[0].0.contains("flag=clashmeta"));
    }

    #[tokio::test]
    async fn fixed_ua_uses_exactly_one_request() {
        let fetcher = FakeSubscriptionFetcher::new(vec![yaml_response(
            "proxies:\n  - name: direct\n    type: direct\nrules:\n  - MATCH,DIRECT\n",
        )]);

        let result = download_sub_with_user_agent_and_fetcher(
            "https://example.test/sub",
            Some("custom-clash/v1"),
            &fetcher,
        )
        .await
        .unwrap();

        assert!(result.is_clash_yaml);
        let requests = fetcher.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].1.as_deref(), Some("custom-clash/v1"));
    }

    #[tokio::test]
    async fn probe_scores_clash_yaml_above_raw_links() {
        let fetcher = FakeSubscriptionFetcher::new(vec![
            yaml_response("vmess://abc\nvmess://def\n"),
            yaml_response(
                "proxies:\n  - name: direct\n    type: direct\nproxy-groups:\n  - name: Proxy\n    type: select\n    proxies:\n      - direct\nrules:\n  - DOMAIN-SUFFIX,example.com,Proxy\n  - MATCH,DIRECT\n",
            ),
            yaml_response("trojan://password@example.com:443\n"),
            yaml_response("vmess://bare\n"),
        ]);

        let results =
            probe_subscription_url_with_fetcher("https://example.test/sub", &fetcher, false)
                .await
                .unwrap();

        assert_eq!(results[0].label, "clash-meta");
        assert_eq!(results[0].format, "Clash YAML");
        assert_eq!(results[0].rule_count, 2);
        assert!(results[0].score > results[1].score);
        assert_eq!(fetcher.requests().len(), 4);
    }

    #[test]
    fn append_query_param_preserves_existing_query() {
        assert_eq!(
            append_query_param("https://example.com/sub", "flag=clashmeta"),
            "https://example.com/sub?flag=clashmeta"
        );
        assert_eq!(
            append_query_param("https://example.com/sub?token=x", "flag=clashmeta"),
            "https://example.com/sub?token=x&flag=clashmeta"
        );
    }

    #[test]
    fn test_merge_user_rules_front_is_valid_yaml() {
        let (_tmp, paths) = setup_config(
            "mixed-port: 7890\nproxy-groups:\n  - name: Proxy\n    type: select\n    proxies: [DIRECT]\nrules:\n  - DOMAIN-SUFFIX,google.com,Proxy\n  - DOMAIN-SUFFIX,github.com,DIRECT\ndns:\n  enable: true\n  enhanced-mode: fake-ip\n",
        );
        rules::save_rules_at(
            &paths,
            &[
                "DOMAIN-SUFFIX,company.com,DIRECT".to_string(),
                "IP-CIDR,10.0.0.0/8,DIRECT".to_string(),
            ],
        )
        .unwrap();

        merge_user_config_at(&paths).unwrap();

        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let rule_values = parsed["rules"].as_sequence().unwrap();
        let rules: Vec<&str> = rule_values.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(
            rules,
            vec![
                "DOMAIN-SUFFIX,company.com,DIRECT",
                "IP-CIDR,10.0.0.0/8,DIRECT",
                "DOMAIN-SUFFIX,google.com,Proxy",
                "DOMAIN-SUFFIX,github.com,DIRECT",
            ]
        );
    }

    #[test]
    fn test_merge_user_rules_back_before_next_top_level_key() {
        let (_tmp, paths) = setup_config(
            "mixed-port: 7890\nproxy-groups:\n  - name: Proxy\n    type: select\n    proxies: [DIRECT]\nrules:\n  - DOMAIN-SUFFIX,google.com,Proxy\n  - DOMAIN-SUFFIX,github.com,DIRECT\ndns:\n  enable: true\n  enhanced-mode: fake-ip\n",
        );
        rules::set_position_at(&paths, RulePosition::Back).unwrap();
        rules::save_rules_at(&paths, &["DOMAIN-SUFFIX,back.com,DIRECT".to_string()]).unwrap();

        merge_user_config_at(&paths).unwrap();

        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let rule_values = parsed["rules"].as_sequence().unwrap();
        let rules: Vec<&str> = rule_values.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(
            rules,
            vec![
                "DOMAIN-SUFFIX,google.com,Proxy",
                "DOMAIN-SUFFIX,github.com,DIRECT",
                "DOMAIN-SUFFIX,back.com,DIRECT",
            ]
        );
    }

    #[test]
    fn test_merge_empty_rules_removes_marker_block() {
        let (_tmp, paths) = setup_config(
            "mixed-port: 7890\nproxy-groups:\n  - name: Proxy\n    type: select\n    proxies: [DIRECT]\nrules:\n# === USER RULES START ===\n  - DOMAIN-SUFFIX,old.com,DIRECT\n# === USER RULES END ===\n  - DOMAIN-SUFFIX,google.com,Proxy\n",
        );
        rules::clear_rules_at(&paths).unwrap();

        merge_user_config_at(&paths).unwrap();

        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(!text.contains("USER RULES START"), "{text}");
        let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let rule_values = parsed["rules"].as_sequence().unwrap();
        let rules: Vec<&str> = rule_values.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(rules, vec!["DOMAIN-SUFFIX,google.com,Proxy"]);
    }

    // ── Failure path tests ──

    #[test]
    fn test_ensure_controller_inserts_at_top_level() {
        let yaml = "proxies:\n  - name: 节点选择\n    type: selector\n";
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from("/tmp/test.sock"));
        let result = ensure_controller_for_endpoint(yaml, &endpoint).unwrap();
        assert!(result.contains("external-controller-unix"));
        assert!(
            result.find("external-controller-unix").unwrap() < result.find("proxies:").unwrap(),
            "\n---\n{result}\n---"
        );
    }

    #[test]
    fn test_ensure_controller_malformed_input() {
        let yaml = "mixed-port: 7890\n  - invalid indent\nfoo: bar";
        // The editor accepts any serde_yaml-valid input and still repairs the controller.
        // Verify it produces a valid result
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from("/tmp/test.sock"));
        let result = ensure_controller_for_endpoint(yaml, &endpoint).unwrap();
        assert!(result.contains("external-controller-unix"));
    }

    #[test]
    fn test_ensure_controller_empty() {
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from("/tmp/test.sock"));
        let result = ensure_controller_for_endpoint("", &endpoint).unwrap();
        assert!(result.contains("external-controller-unix"));
    }

    #[test]
    #[cfg(unix)] // UnixSocket endpoint semantics; not applicable on Windows
    fn test_ensure_controller_already_has_unix() {
        #[cfg(unix)]
        let input = format!(
            "mixed-port: 7890\nexternal-controller-unix: {}/mihomo.sock\n",
            crate::utils::socket_dir()
        );
        #[cfg(not(unix))]
        let input = "mixed-port: 7890\nexternal-controller-unix: mihomo.sock\n".to_string();
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from(format!(
            "{}/mihomo.sock",
            crate::utils::socket_dir()
        )));
        let result = ensure_controller_for_endpoint(&input, &endpoint).unwrap();
        assert_eq!(result, input, "should not modify already-configured config");
    }

    #[test]
    #[cfg(unix)] // UnixSocket endpoint semantics; not applicable on Windows
    fn test_ensure_controller_replaces_wrong_unix_socket() {
        let input = "mixed-port: 7890\nexternal-controller: ''\nexternal-controller-unix: /tmp/verge/verge-mihomo.sock\n";
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from(format!(
            "{}/mihomo.sock",
            crate::utils::socket_dir()
        )));
        let result = ensure_controller_for_endpoint(input, &endpoint).unwrap();
        #[cfg(unix)]
        let expected = format!(
            "external-controller-unix: {}/mihomo.sock",
            crate::utils::socket_dir()
        );
        #[cfg(not(unix))]
        let expected = "external-controller-unix: mihomo.sock".to_string();

        assert!(result.contains(&expected), "{result}");
        assert!(!result.contains("/tmp/verge/verge-mihomo.sock"), "{result}");
    }

    #[cfg(unix)]
    #[test]
    fn unix_current_api_endpoint_joins_socket_dir_without_duplicate_separator() {
        let endpoint = unix_api_endpoint_from_socket_dir("/run/user/1000/");
        assert_eq!(
            endpoint,
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/run/user/1000/mihomo.sock"))
        );
    }

    #[test]
    fn windows_user_core_pipe_name_follows_v3_per_user_contract() {
        assert_eq!(
            windows_user_core_pipe_name("alice"),
            r"\\.\pipe\mihomo-alice"
        );
    }

    #[test]
    fn test_ensure_controller_for_endpoint_replaces_pipe_and_unix_keys() {
        let endpoint = ApiEndpoint::WindowsNamedPipe(r"\\.\pipe\mihomo-core".to_string());
        let input =
            "mixed-port: 7890\nexternal-controller-unix: /tmp/old.sock\nexternal-controller: ''\n";
        let result = ensure_controller_for_endpoint(input, &endpoint).unwrap();

        assert!(
            result.contains(r"external-controller-pipe: \\.\pipe\mihomo-core"),
            "{result}"
        );
        assert!(!result.contains("external-controller-unix"), "{result}");
        assert!(!result.contains("external-controller: ''"), "{result}");
    }

    #[test]
    fn generate_config_yaml_for_endpoint_uses_resolved_instance_endpoint() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"proxies:
  - name: direct
    type: direct
proxy-groups: []
rules: []
external-controller-unix: /tmp/stale-user.sock
external-controller: 127.0.0.1:9090
"#,
        )
        .unwrap();
        let endpoint =
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock"));

        let generated = generate_config_yaml_with_fake_ip_filters_for_endpoint(
            &sub_yaml,
            &[],
            &[],
            &[],
            crate::rules::RulePosition::Front,
            &endpoint,
        )
        .unwrap();

        assert!(generated.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!generated.contains("/tmp/stale-user.sock"));
        assert!(!generated.contains("external-controller: 127.0.0.1:9090"));
    }

    #[test]
    fn test_inject_runtime_controller_for_endpoint_removes_external_ui_and_old_endpoint() {
        let mut config: serde_yaml::Value = serde_yaml::from_str(
            r#"
mixed-port: 7890
external-controller: ''
external-controller-unix: /tmp/evil.sock
external-ui: ui
proxies: []
"#,
        )
        .unwrap();
        let map = config.as_mapping_mut().unwrap();
        inject_runtime_controller_for_endpoint(
            map,
            &ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock")),
        );

        assert!(map
            .get(serde_yaml::Value::String("external-controller".to_string()))
            .is_none());
        assert!(map
            .get(serde_yaml::Value::String("external-ui".to_string()))
            .is_none());
        assert_eq!(
            map.get(serde_yaml::Value::String(
                "external-controller-unix".to_string()
            ))
            .and_then(|v| v.as_str()),
            Some("/var/run/mihomo/mihomo.sock")
        );
    }

    #[test]
    fn save_config_at_endpoint_uses_resolved_instance_endpoint() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let endpoint =
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock"));

        save_config_at_endpoint(
            &paths,
            r#"proxies:
  - name: direct
    type: direct
external-controller-unix: /tmp/stale-user.sock
"#,
            None,
            &endpoint,
        )
        .unwrap();

        let saved = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(saved.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!saved.contains("/tmp/stale-user.sock"));
    }

    #[test]
    #[cfg(unix)] // UnixSocket endpoint semantics; not applicable on Windows
    fn test_fix_existing_config_no_controller_adds_it() {
        let content = "mixed-port: 7890\nmode: rule\n";
        let endpoint = ApiEndpoint::UnixSocket(std::path::PathBuf::from(format!(
            "{}/mihomo.sock",
            crate::utils::socket_dir()
        )));
        let result = ensure_controller_for_endpoint(content, &endpoint).unwrap();
        #[cfg(unix)]
        let expected = format!("{}/mihomo.sock", crate::utils::socket_dir());
        #[cfg(not(unix))]
        let expected = "mihomo.sock".to_string();
        assert!(result.contains(&format!("external-controller-unix: {}", expected)));
        assert!(result.contains("mixed-port: 7890"));
    }

    #[test]
    fn test_atomic_write_and_interrupt_leaves_target_untouched() {
        use std::io::Write;
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("config.yaml");
        let target_str = target.display().to_string();

        // Write original content
        std::fs::write(&target, "original").unwrap();

        // Simulate interrupt by writing .tmp but NOT renaming
        let tmp_path = format!("{}.tmp", target_str);
        let mut f = std::fs::File::create(&tmp_path).unwrap();
        f.write_all(b"new content that should not appear").unwrap();
        drop(f); // close handle without renaming

        // The current writer uses a random O_EXCL temp name, so an interrupted
        // legacy fixed .tmp cannot redirect or block the transaction.
        crate::utils::atomic_write_file(&target_str, "final content").unwrap();

        // Target should be "final content", not "original" or "new content"
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "final content");
        assert_eq!(
            std::fs::read_to_string(&tmp_path).unwrap(),
            "new content that should not appear"
        );
    }

    // ── Phase 2: Multi-subscription data layer tests ──

    #[test]
    fn test_generate_subscription_id_format() {
        let id = generate_subscription_id();
        assert!(id.starts_with("sub-"));
        assert_eq!(id.len(), 12); // "sub-" + 8 hex chars
                                  // Verify hex chars
        let hex_part = &id[4..];
        assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_subscription_id_unique() {
        let ids: Vec<_> = (0..100).map(|_| generate_subscription_id()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "IDs should be unique");
    }

    #[test]
    fn test_load_subscriptions_empty() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let subs = load_subscriptions_at(&paths).unwrap();
        assert!(subs.is_empty());
    }

    #[test]
    fn test_save_and_load_subscriptions() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let subs = vec![
            SubscriptionMeta {
                id: "sub-abc12345".to_string(),
                url: "https://example.com/sub?token=xxx".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            },
            SubscriptionMeta {
                id: "sub-def67890".to_string(),
                url: "https://other.com/api/v1".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            },
        ];

        save_subscriptions_at(&paths, &subs).unwrap();

        let loaded = load_subscriptions_at(&paths).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "sub-abc12345");
        assert_eq!(loaded[1].id, "sub-def67890");
    }

    #[test]
    fn test_get_active_id_none() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let active = get_active_id_at(&paths).unwrap();
        assert!(active.is_none());
    }

    #[test]
    fn test_set_and_get_active_id() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        set_active_id_at(&paths, "sub-abc12345").unwrap();

        let active = get_active_id_at(&paths).unwrap();
        assert_eq!(active, Some("sub-abc12345".to_string()));
    }

    #[test]
    fn test_find_subscription() {
        let subs = vec![
            SubscriptionMeta {
                id: "sub-abc12345".to_string(),
                url: "https://example.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            },
            SubscriptionMeta {
                id: "sub-def67890".to_string(),
                url: "https://other.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            },
        ];

        assert!(find_subscription(&subs, "sub-abc12345").is_some());
        assert!(find_subscription(&subs, "sub-def67890").is_some());
        assert!(find_subscription(&subs, "sub-notexist").is_none());
    }

    // ── Phase 3: Core generation tests ──

    #[test]
    fn test_validate_subscription_yaml_valid() {
        let yaml: serde_yaml::Value = serde_yaml::from_str("proxies: []").unwrap();
        assert!(validate_subscription_yaml(&yaml).is_ok());
    }

    #[test]
    fn test_validate_subscription_yaml_with_proxy_providers() {
        let yaml: serde_yaml::Value = serde_yaml::from_str("proxy-providers: {}").unwrap();
        assert!(validate_subscription_yaml(&yaml).is_ok());
    }

    #[test]
    fn test_validate_subscription_yaml_invalid() {
        let yaml: serde_yaml::Value = serde_yaml::from_str("mixed-port: 7890").unwrap();
        assert!(validate_subscription_yaml(&yaml).is_err());
    }

    #[test]
    #[cfg(unix)] // platform-specific path semantics
    fn test_generate_config_yaml_basic() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
mixed-port: 7890
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#,
        )
        .unwrap();

        let result =
            generate_config_yaml(&sub_yaml, &[], &[], crate::rules::RulePosition::Front).unwrap();

        // Verify: contains external-controller-unix
        assert!(result.contains("external-controller-unix"));
        // Verify: rules preserved
        assert!(result.contains("DOMAIN-SUFFIX,google.com,Proxy"));
        // Verify: valid YAML
        let _: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
    }

    #[test]
    fn test_generate_config_yaml_with_rules_front() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#,
        )
        .unwrap();

        let user_rules = vec![
            "DOMAIN-SUFFIX,company.com,DIRECT".to_string(),
            "IP-CIDR,10.0.0.0/8,DIRECT".to_string(),
        ];

        let result = generate_config_yaml(
            &sub_yaml,
            &user_rules,
            &[],
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let rules: Vec<&str> = parsed["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        assert_eq!(
            rules,
            vec![
                "DOMAIN-SUFFIX,company.com,DIRECT",
                "IP-CIDR,10.0.0.0/8,DIRECT",
                "DOMAIN-SUFFIX,google.com,Proxy",
            ]
        );
    }

    #[test]
    fn test_generate_config_yaml_with_rules_back() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#,
        )
        .unwrap();

        let user_rules = vec!["DOMAIN-SUFFIX,company.com,DIRECT".to_string()];

        let result = generate_config_yaml(
            &sub_yaml,
            &user_rules,
            &[],
            crate::rules::RulePosition::Back,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let rules: Vec<&str> = parsed["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        assert_eq!(
            rules,
            vec![
                "DOMAIN-SUFFIX,google.com,Proxy",
                "DOMAIN-SUFFIX,company.com,DIRECT",
            ]
        );
    }

    #[test]
    fn test_generate_config_yaml_with_fake_ip_filters() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
dns:
  enhanced-mode: fake-ip
  fake-ip-filter:
    - geosite:private
rules:
  - MATCH,DIRECT
"#,
        )
        .unwrap();
        let result = generate_config_yaml_with_fake_ip_filters_for_endpoint(
            &sub_yaml,
            &[],
            &[],
            &[
                "+.corp.example.com".to_string(),
                "geosite:private".to_string(),
            ],
            crate::rules::RulePosition::Front,
            &crate::instance::ApiEndpoint::UnixSocket(std::path::PathBuf::from("/tmp/test.sock")),
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let filters = parsed["dns"]["fake-ip-filter"].as_sequence().unwrap();
        assert!(filters.contains(&serde_yaml::Value::String("geosite:private".to_string())));
        assert!(filters.contains(&serde_yaml::Value::String("+.corp.example.com".to_string())));
        assert_eq!(
            filters
                .iter()
                .filter(|v| v.as_str() == Some("geosite:private"))
                .count(),
            1
        );
    }

    #[test]
    fn test_generate_config_yaml_with_dns_policies() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
dns:
  enable: true
  nameserver:
    - 223.5.5.5
"#,
        )
        .unwrap();

        let dns_policies = vec![
            crate::dns::DnsPolicy {
                match_pattern: "company.com".to_string(),
                target: "system".to_string(),
            },
            crate::dns::DnsPolicy {
                match_pattern: "internal.corp".to_string(),
                target: "192.0.2.53,198.51.100.53".to_string(),
            },
        ];

        let result = generate_config_yaml(
            &sub_yaml,
            &[],
            &dns_policies,
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let ns_policy = &parsed["dns"]["nameserver-policy"];

        assert_eq!(ns_policy["+company.com"].as_str().unwrap(), "system");
        assert!(ns_policy["+internal.corp"].is_sequence());
    }

    #[test]
    fn test_generate_config_yaml_dns_policy_override() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
dns:
  enable: true
  nameserver-policy:
    "+.company.com": 8.8.8.8
"#,
        )
        .unwrap();

        let dns_policies = vec![crate::dns::DnsPolicy {
            match_pattern: "company.com".to_string(),
            target: "system".to_string(),
        }];

        let result = generate_config_yaml(
            &sub_yaml,
            &[],
            &dns_policies,
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        // User policy overrides subscription policy
        assert_eq!(
            parsed["dns"]["nameserver-policy"]["+company.com"]
                .as_str()
                .unwrap(),
            "system"
        );
    }

    #[test]
    fn test_generate_config_yaml_no_subscription_rules() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
mixed-port: 7890
proxies: []
"#,
        )
        .unwrap();

        let user_rules = vec!["DOMAIN-SUFFIX,company.com,DIRECT".to_string()];

        let result = generate_config_yaml(
            &sub_yaml,
            &user_rules,
            &[],
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        let rules: Vec<&str> = parsed["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        assert_eq!(rules, vec!["DOMAIN-SUFFIX,company.com,DIRECT"]);
    }

    #[test]
    fn test_generate_config_yaml_no_subscription_dns() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
mixed-port: 7890
"#,
        )
        .unwrap();

        let dns_policies = vec![crate::dns::DnsPolicy {
            match_pattern: "company.com".to_string(),
            target: "system".to_string(),
        }];

        let result = generate_config_yaml(
            &sub_yaml,
            &[],
            &dns_policies,
            crate::rules::RulePosition::Front,
        )
        .unwrap();

        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        assert!(parsed["dns"]["nameserver-policy"].is_mapping());
    }

    #[test]
    fn test_generate_config_yaml_injects_mixed_port_when_no_port_present() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#,
        )
        .unwrap();

        let result =
            generate_config_yaml(&sub_yaml, &[], &[], crate::rules::RulePosition::Front).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(parsed["mixed-port"].as_u64(), Some(7897));
    }

    #[test]
    fn test_generate_config_yaml_preserves_existing_port() {
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
port: 7890
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
"#,
        )
        .unwrap();

        let result =
            generate_config_yaml(&sub_yaml, &[], &[], crate::rules::RulePosition::Front).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&result).unwrap();
        assert_eq!(parsed["port"].as_u64(), Some(7890));
        assert!(parsed["mixed-port"].is_null());
    }

    #[test]
    #[cfg(unix)] // platform-specific path semantics
    fn test_merge_user_config_new_flow() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        // Create subscription file
        let sub_content = r#"
mixed-port: 7890
proxies:
  - name: proxy1
    type: http
    server: example.com
    port: 443
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
"#;
        std::fs::write(paths.subscription_file_path("sub-test123"), sub_content).unwrap();
        set_active_id_at(&paths, "sub-test123").unwrap();

        // Run merge
        merge_user_config_at(&paths).unwrap();

        // Verify config.yaml was generated
        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(config.contains("external-controller-unix"));
        // "Proxy" is not a valid proxy-group or proxy name in this subscription,
        // so the dangling rule fallback rewrites it to DIRECT.
        assert!(config.contains("DOMAIN-SUFFIX,google.com,DIRECT"));
    }

    #[test]
    fn merge_user_config_legacy_flow_uses_resolved_endpoint() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.config_path(),
            "mixed-port: 7897\nexternal-controller-unix: /tmp/stale-user.sock\nrules: []\n",
        )
        .unwrap();
        let endpoint =
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock"));

        merge_user_config_at_endpoint(&paths, &endpoint).unwrap();

        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(config.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!config.contains("/tmp/stale-user.sock"));
    }

    #[test]
    fn merge_user_config_legacy_flow_downgrades_dangling_rules_to_direct() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.config_path(),
            "mixed-port: 7897\nproxies: []\nproxy-groups:\n  - name: ValidGroup\n    type: select\n    proxies: [DIRECT]\nrules:\n  - MATCH,DIRECT\n",
        )
        .unwrap();
        std::fs::write(
            paths.rules_path(),
            "rules:\n  - DOMAIN-SUFFIX,example.com,DIRECT\n  - DOMAIN-SUFFIX,openai.com,MissingGroup\n",
        )
        .unwrap();
        let endpoint =
            ApiEndpoint::UnixSocket(std::path::PathBuf::from("/var/run/mihomo/mihomo.sock"));

        merge_user_config_at_endpoint(&paths, &endpoint).unwrap();

        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(
            config.contains("DOMAIN-SUFFIX,openai.com,DIRECT"),
            "Dangling rule with MissingGroup should be downgraded to DIRECT in legacy flow, but was: {config}"
        );
    }

    #[test]
    fn test_merge_user_config_missing_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        set_active_id_at(&paths, "sub-nonexist").unwrap();

        let result = merge_user_config_at(&paths);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Subscription file not found"));
    }

    // ── Phase 4: Subscription CRUD tests ──

    fn setup_test_subscription(paths: &AppPaths, id: &str, content: &str) {
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::write(paths.subscription_file_path(id), content).unwrap();
    }

    #[test]
    fn test_remove_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());

        // Setup: two subscriptions
        let sub1 = "sub-aaaa1111";
        let sub2 = "sub-bbbb2222";
        setup_test_subscription(&paths, sub1, "proxies: []");
        setup_test_subscription(&paths, sub2, "proxies: []");

        let subs = vec![
            SubscriptionMeta {
                id: sub1.to_string(),
                url: "https://a.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            },
            SubscriptionMeta {
                id: sub2.to_string(),
                url: "https://b.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            },
        ];
        save_subscriptions_at(&paths, &subs).unwrap();
        set_active_id_at(&paths, sub1).unwrap();

        // Remove sub2
        remove_subscription_at(&paths, sub2).unwrap();

        // A1' tombstone: metadata entry is retired, not deleted; cache is gone.
        let remaining = load_subscriptions_at(&paths).unwrap();
        assert_eq!(remaining.len(), 2);
        let retired = remaining.iter().find(|s| s.id == sub2).unwrap();
        assert!(!retired.is_alive());
        assert!(retired.retired_at.is_some());
        assert!(remaining.iter().find(|s| s.id == sub1).unwrap().is_alive());
        assert!(!paths.subscription_file_path(sub2).exists());
        // Active should still be sub1
        assert_eq!(get_active_id_at(&paths).unwrap(), Some(sub1.to_string()));

        // Idempotent re-remove of the tombstone succeeds without change.
        remove_subscription_at(&paths, sub2).unwrap();
        assert_eq!(load_subscriptions_at(&paths).unwrap().len(), 2);
    }

    #[test]
    fn test_remove_active_subscription_requires_verified_replacement() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());

        let sub1 = "sub-aaaa1111";
        let content = "proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n";
        setup_test_subscription(&paths, sub1, content);

        let subs = vec![SubscriptionMeta {
            id: sub1.to_string(),
            url: "https://a.com".to_string(),
            updated: Utc::now(),
            user_agent: None,
            user_agent_mode: Some(UserAgentMode::Auto),
            retired: false,
            retired_at: None,
        }];
        save_subscriptions_at(&paths, &subs).unwrap();
        set_active_id_at(&paths, sub1).unwrap();

        let error = remove_subscription_at(&paths, sub1).unwrap_err();

        assert!(error.to_string().contains("active subscription"));
        assert_eq!(get_active_id_at(&paths).unwrap(), Some(sub1.to_string()));
        assert_eq!(load_subscriptions_at(&paths).unwrap().len(), 1);
        assert_eq!(
            std::fs::read_to_string(paths.subscription_file_path(sub1)).unwrap(),
            content
        );
    }

    #[test]
    fn test_remove_nonexistent_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let result = remove_subscription_at(&paths, "sub-nonexist");
        assert!(result.is_err());
    }

    #[test]
    fn test_new_subscription_commit_writes_cache_metadata_and_active_pointer() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let mut subs = Vec::new();
        let id = "sub-aaaa1111";
        let yaml = "proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n";

        commit_new_subscription_at(
            &paths,
            id,
            yaml,
            &mut subs,
            "file:///fixture.yaml",
            None,
            Some(true),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(paths.subscription_file_path(id)).unwrap(),
            yaml
        );
        assert_eq!(get_active_id_at(&paths).unwrap(), Some(id.to_string()));
        let saved = load_subscriptions_at(&paths).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, id);
        assert_eq!(saved[0].url, "file:///fixture.yaml");
    }

    #[test]
    fn test_switch_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());

        let sub1 = "sub-aaaa1111";
        let sub2 = "sub-bbbb2222";
        let content1 = "mixed-port: 7890\nproxies:\n  - name: p1\n    type: http\n    server: a.com\n    port: 443\n";
        let content2 = "mixed-port: 7891\nproxies:\n  - name: p2\n    type: http\n    server: b.com\n    port: 443\n";

        setup_test_subscription(&paths, sub1, content1);
        setup_test_subscription(&paths, sub2, content2);

        let subs = vec![
            SubscriptionMeta {
                id: sub1.to_string(),
                url: "https://a.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            },
            SubscriptionMeta {
                id: sub2.to_string(),
                url: "https://b.com".to_string(),
                updated: Utc::now(),
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            },
        ];
        save_subscriptions_at(&paths, &subs).unwrap();
        set_active_id_at(&paths, sub1).unwrap();

        // Switch to sub2
        switch_subscription_at(&paths, sub2).unwrap();

        // Verify active changed
        assert_eq!(get_active_id_at(&paths).unwrap(), Some(sub2.to_string()));

        // Caller is responsible for checked merge after switching.
        merge_user_config_checked_at(&paths, None).unwrap();
        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(config.contains("mixed-port: 7891"));
    }

    #[test]
    fn test_switch_missing_cache_keeps_previous_active_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let sub1 = "sub-aaaa1111";
        let sub2 = "sub-bbbb2222";
        setup_test_subscription(&paths, sub1, "proxies: []\n");
        save_subscriptions_at(
            &paths,
            &[
                SubscriptionMeta {
                    id: sub1.to_string(),
                    url: "https://a.com".to_string(),
                    updated: Utc::now(),
                    user_agent: None,
                    user_agent_mode: Some(UserAgentMode::Auto),
                    retired: false,
                    retired_at: None,
                },
                SubscriptionMeta {
                    id: sub2.to_string(),
                    url: "https://b.com".to_string(),
                    updated: Utc::now(),
                    user_agent: None,
                    user_agent_mode: Some(UserAgentMode::Auto),
                    retired: false,
                    retired_at: None,
                },
            ],
        )
        .unwrap();
        set_active_id_at(&paths, sub1).unwrap();

        let error = switch_subscription_at(&paths, sub2).unwrap_err();

        assert!(error.to_string().contains("cache not found"));
        assert_eq!(get_active_id_at(&paths).unwrap(), Some(sub1.to_string()));
    }

    #[test]
    fn test_switch_nonexistent_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        let result = switch_subscription_at(&paths, "sub-nonexist");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // ── Phase 2: Groups overlay PassiveTolerant + Dangling Rule Fallback tests ──

    /// Test that PassiveTolerant mode gracefully prunes stale member nodes from a group overlay
    /// and synthesises the config successfully (no error), producing a valid YAML output.
    #[test]
    fn groups_overlay_passive_tolerant_prunes_stale_nodes_and_synthesises_ok() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        let sub_id = "sub-aa110011";
        // Subscription has two nodes; the overlay group references both plus a stale one.
        let sub_content = r#"
proxies:
  - name: NodeA
    type: http
    server: a.example.com
    port: 443
  - name: NodeB
    type: http
    server: b.example.com
    port: 443
proxy-groups:
  - name: Auto
    type: url-test
    proxies:
      - NodeA
      - NodeB
rules:
  - MATCH,DIRECT
"#;
        std::fs::write(paths.subscription_file_path(sub_id), sub_content).unwrap();
        set_active_id_at(&paths, sub_id).unwrap();

        // Write a groups overlay that patches "Auto" to add a stale node that no longer exists.
        let overlay_path = paths.groups_override_path_for_subscription(sub_id);
        std::fs::create_dir_all(overlay_path.parent().unwrap()).unwrap();
        let overlay_content = r#"
patches:
  Auto:
    add_proxies:
      - StaleNode
    remove_proxies: []
"#;
        std::fs::write(&overlay_path, overlay_content).unwrap();

        // Apply with_mode PassiveTolerant – should succeed without error.
        let sub_yaml: serde_yaml::Value = serde_yaml::from_str(sub_content).unwrap();
        let config_text = generate_config_yaml_with_fake_ip_filters_for_endpoint(
            &sub_yaml,
            &[],
            &[],
            &[],
            crate::rules::RulePosition::Front,
            &current_api_endpoint(),
        )
        .unwrap();

        let result = apply_groups_override_to_config_text_with_mode(
            &paths,
            sub_id,
            &config_text,
            crate::groups::MergeMode::PassiveTolerant,
        );

        assert!(
            result.is_ok(),
            "PassiveTolerant should succeed even with stale overlay node, got: {:?}",
            result.err()
        );

        let merged: serde_yaml::Value = serde_yaml::from_str(&result.unwrap()).unwrap();
        let groups = merged["proxy-groups"].as_sequence().unwrap();
        let auto_group = groups
            .iter()
            .find(|g| g["name"].as_str() == Some("Auto"))
            .unwrap();
        let members: Vec<&str> = auto_group["proxies"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        // StaleNode was pruned; NodeA and NodeB remain
        assert!(members.contains(&"NodeA"), "NodeA should still be present");
        assert!(members.contains(&"NodeB"), "NodeB should still be present");
        assert!(
            !members.contains(&"StaleNode"),
            "StaleNode should have been pruned"
        );
    }

    /// Test that a global rule pointing to a non-existent proxy-group is automatically
    /// downgraded to DIRECT in the generated config.yaml, while the on-disk rules.yaml
    /// is not modified.
    #[test]
    fn dangling_rule_target_is_downgraded_to_direct_without_touching_rules_yaml() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        let sub_id = "sub-bb220022";
        let sub_content = r#"
proxies:
  - name: HK01
    type: http
    server: hk.example.com
    port: 443
proxy-groups:
  - name: HK Select
    type: select
    proxies:
      - HK01
rules:
  - MATCH,DIRECT
"#;
        std::fs::write(paths.subscription_file_path(sub_id), sub_content).unwrap();
        set_active_id_at(&paths, sub_id).unwrap();

        // User rule referencing a proxy-group that does NOT exist in the subscription.
        let user_rules = vec![
            "DOMAIN-SUFFIX,google.com,NonExistentGroup".to_string(),
            "DOMAIN-SUFFIX,github.com,HK Select".to_string(),
            "IP-CIDR,192.168.0.0/16,DIRECT,no-resolve".to_string(),
            "MATCH,DIRECT".to_string(),
        ];
        rules::save_rules_at(&paths, &user_rules).unwrap();

        // Run merge
        merge_user_config_at(&paths).unwrap();

        // Verify the generated config.yaml
        let config_text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&config_text).unwrap();
        let rules_in_config: Vec<&str> = config["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        // The dangling rule must be rewritten to DIRECT
        assert!(
            rules_in_config.contains(&"DOMAIN-SUFFIX,google.com,DIRECT"),
            "Dangling rule should have been downgraded to DIRECT, found rules: {:?}",
            rules_in_config
        );
        // The valid rule must be unchanged
        assert!(
            rules_in_config.contains(&"DOMAIN-SUFFIX,github.com,HK Select"),
            "Valid rule targeting 'HK Select' must remain unchanged"
        );
        // Rule with no-resolve option must be preserved
        assert!(
            rules_in_config.contains(&"IP-CIDR,192.168.0.0/16,DIRECT,no-resolve"),
            "CIDR rule with no-resolve option must be preserved"
        );

        // Verify that rules.yaml on disk is NOT modified
        let disk_rules = rules::load_rules_at(&paths).unwrap();
        assert_eq!(
            disk_rules, user_rules,
            "rules.yaml must not be modified by dangling rule fallback"
        );
    }

    /// regression: a rule targeting a group that only exists in
    /// override.yaml must survive config regeneration without being downgraded
    /// to DIRECT.
    #[test]
    fn rule_targeting_override_only_group_is_not_downgraded() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        let sub_id = "sub-override-group";
        let sub_content = r#"
proxies:
  - name: HK01
    type: http
    server: hk.example.com
    port: 443
proxy-groups:
  - name: 节点选择
    type: select
    proxies:
      - HK01
rules:
  - MATCH,DIRECT
"#;
        std::fs::write(paths.subscription_file_path(sub_id), sub_content).unwrap();
        set_active_id_at(&paths, sub_id).unwrap();

        // override.yaml defines a group that does not exist in the subscription.
        std::fs::write(
            paths.override_path(),
            r#"
proxy-groups:
  - name: 节点选择
    type: select
    proxies:
      - HK01
  - name: 苹果香港
    type: select
    proxies:
      - HK01
"#,
        )
        .unwrap();

        let user_rules = vec![
            "DOMAIN-SUFFIX,store.apple.com,苹果香港".to_string(),
            "DOMAIN-SUFFIX,github.com,节点选择".to_string(),
        ];
        rules::save_rules_at(&paths, &user_rules).unwrap();

        merge_user_config_at(&paths).unwrap();

        let config_text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&config_text).unwrap();
        let rules_in_config: Vec<&str> = config["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        assert!(
            rules_in_config.contains(&"DOMAIN-SUFFIX,store.apple.com,苹果香港"),
            "rule targeting an override-only group must not be downgraded, found: {:?}",
            rules_in_config
        );
        assert_eq!(
            rules::load_rules_at(&paths).unwrap(),
            user_rules,
            "rules.yaml must not be modified"
        );
    }

    /// regression: a `tun` block committed into the intent config.yaml
    /// by `tun on` must survive config regeneration (e.g. triggered by `rule add`).
    #[test]
    fn tun_intent_block_survives_config_regeneration() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        let sub_id = "sub-tun-carry";
        let sub_content = r#"
proxies:
  - name: HK01
    type: http
    server: hk.example.com
    port: 443
proxy-groups:
  - name: 节点选择
    type: select
    proxies:
      - HK01
rules:
  - MATCH,DIRECT
"#;
        std::fs::write(paths.subscription_file_path(sub_id), sub_content).unwrap();
        set_active_id_at(&paths, sub_id).unwrap();
        rules::save_rules_at(&paths, &["DOMAIN-SUFFIX,github.com,节点选择".to_string()]).unwrap();
        merge_user_config_at(&paths).unwrap();

        // Simulate the `tun on` intent commit: the candidate (== promoted snapshot)
        // appended the tun block last, mirroring the daemon merge position.
        let promoted = {
            let mut doc: serde_yaml::Value =
                serde_yaml::from_str(&std::fs::read_to_string(paths.config_path()).unwrap())
                    .unwrap();
            doc.as_mapping_mut().unwrap().insert(
                serde_yaml::Value::String("tun".to_string()),
                serde_yaml::from_str(
                    r#"
enable: true
stack: mixed
"dns-hijack":
  - any:53
"#,
                )
                .unwrap(),
            );
            serde_yaml::to_string(&doc).unwrap()
        };
        std::fs::write(paths.config_path(), &promoted).unwrap();

        // Trigger regeneration as `rule add` would.
        rules::save_rules_at(
            &paths,
            &[
                "DOMAIN-SUFFIX,github.com,节点选择".to_string(),
                "DOMAIN-SUFFIX,zz.invalid,DIRECT".to_string(),
            ],
        )
        .unwrap();
        merge_user_config_at(&paths).unwrap();

        let config_text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&config_text).unwrap();
        assert_eq!(
            config["tun"]["enable"].as_bool(),
            Some(true),
            "TUN intent must survive regeneration; config was:\n{config_text}"
        );

        // Byte-identity: the daemon re-serializes the promoted content by inserting
        // the snapshot tun block (same position/value); the result must be a fixed
        // point so intent, snapshot and launched revisions hash equal.
        let reparsed: serde_yaml::Value = serde_yaml::from_str(&config_text).unwrap();
        let round_tripped = serde_yaml::to_string(&reparsed).unwrap();
        assert_eq!(
            round_tripped, config_text,
            "intent config.yaml must be stable under the daemon promotion round-trip"
        );
    }

    /// 对齐清单：`config` 转换产出的 TUN/DNS 默认段必须与参考实现
    /// （mihomo 官方 wiki / Clash Verge Rev 默认配置）一致，且不得默认开启 sniffer 兜底。
    #[test]
    fn generated_template_aligns_with_reference_tun_and_dns_defaults() {
        // `vmess://` 原始订阅经 `convert_vmess_to_clash` 生成含 tun/dns 段的完整配置。
        let vmess = "vmess://eyJwcyI6Im4xIiwiYWRkIjoiYS5leGFtcGxlLmNvbSIsInBvcnQiOiI0NDMiLCJpZCI6IjAwMDAwMDAwLTAwMDAtMDAwMC0wMDAwLTAwMDAwMDAwMDAwMCIsImFpZCI6IjAiLCJuZXQiOiJ0Y3AiLCJ0bHMiOiIifQ==";
        let output = super::convert_vmess_to_clash(vmess).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&output).unwrap();

        let hijack: Vec<String> = config["tun"]["dns-hijack"]
            .as_sequence()
            .expect("tun.dns-hijack must be a sequence")
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert_eq!(hijack, vec!["any:53", "tcp://any:53"]);
        assert_eq!(config["tun"]["strict-route"].as_bool(), Some(true));
        assert_eq!(config["tun"]["auto-route"].as_bool(), Some(true));

        assert_eq!(config["dns"]["enhanced-mode"].as_str(), Some("fake-ip"));
        assert_eq!(
            config["dns"]["fake-ip-range"].as_str(),
            Some("198.18.0.1/16")
        );
        let nameservers: Vec<&str> = config["dns"]["default-nameserver"]
            .as_sequence()
            .expect("dns.default-nameserver must be a sequence")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        // 198.51.100.0/24 是文档保留段（TEST-NET-2），不是可用解析器。
        assert!(!nameservers.contains(&"198.51.100.53"));

        // sniffer 保持参考实现的默认（关闭），不得作为兜底默认开启。
        assert!(config.get("sniffer").is_none());
    }

    #[test]
    fn dns_hijack_entries_parse_comma_separated_targets() {
        assert_eq!(
            super::dns_hijack_entries("any:53,tcp://any:53"),
            vec!["any:53", "tcp://any:53"]
        );
        assert_eq!(super::dns_hijack_entries(" any:53 "), vec!["any:53"]);
        assert!(super::dns_hijack_entries(" , ").is_empty());
    }

    #[test]
    fn normalize_tun_block_aligns_legacy_dns_hijack_and_defaults_strict_route() {
        let mut tun: serde_yaml::Mapping =
            serde_yaml::from_str("enable: true\nstack: system\ndns-hijack:\n  - any:53\n").unwrap();
        super::normalize_tun_block_for_enable(&mut tun, None);

        assert_eq!(
            tun.get("dns-hijack"),
            Some(&serde_yaml::Value::Sequence(vec![
                serde_yaml::Value::String("any:53".to_string()),
                serde_yaml::Value::String("tcp://any:53".to_string()),
            ]))
        );
        assert_eq!(
            tun.get("strict-route"),
            Some(&serde_yaml::Value::Bool(true))
        );
    }

    #[test]
    fn normalize_tun_block_preserves_custom_dns_hijack_and_explicit_strict_route() {
        let mut tun: serde_yaml::Mapping = serde_yaml::from_str(
            "enable: true\ndns-hijack:\n  - 10.0.0.1:53\n  - tcp://any:53\nstrict-route: false\n",
        )
        .unwrap();
        super::normalize_tun_block_for_enable(&mut tun, None);

        assert_eq!(
            tun.get("dns-hijack"),
            Some(&serde_yaml::Value::Sequence(vec![
                serde_yaml::Value::String("10.0.0.1:53".to_string()),
                serde_yaml::Value::String("tcp://any:53".to_string()),
            ]))
        );
        assert_eq!(
            tun.get("strict-route"),
            Some(&serde_yaml::Value::Bool(false))
        );
    }

    #[test]
    fn normalize_tun_block_honors_explicit_dns_hijack_flag() {
        let mut tun: serde_yaml::Mapping =
            serde_yaml::from_str("enable: true\ndns-hijack:\n  - any:53\n").unwrap();
        super::normalize_tun_block_for_enable(&mut tun, Some("custom:53,tcp://any:53"));

        assert_eq!(
            tun.get("dns-hijack"),
            Some(&serde_yaml::Value::Sequence(vec![
                serde_yaml::Value::String("custom:53".to_string()),
                serde_yaml::Value::String("tcp://any:53".to_string()),
            ]))
        );
    }

    #[test]
    fn normalize_dns_legacy_defaults_only_rewrites_our_stale_values() {
        // 旧版模板产出：两个字段都收敛到参考实现值
        let mut config: serde_yaml::Mapping = serde_yaml::from_str(
            "dns:\n  fake-ip-range: 28.0.0.1/8\n  default-nameserver:\n    - 198.51.100.53\n    - 223.5.5.5\n",
        )
        .unwrap();
        super::normalize_dns_legacy_defaults(&mut config);
        let dns = config.get("dns").unwrap().as_mapping().unwrap();
        assert_eq!(
            dns.get("fake-ip-range"),
            Some(&serde_yaml::Value::String("198.18.0.1/16".to_string()))
        );
        assert_eq!(
            dns.get("default-nameserver"),
            Some(&serde_yaml::Value::Sequence(vec![
                serde_yaml::Value::String("223.5.5.5".to_string())
            ]))
        );

        // 订阅方/用户自定义取值原样保留
        let mut custom: serde_yaml::Mapping = serde_yaml::from_str(
            "dns:\n  fake-ip-range: 10.0.0.0/16\n  default-nameserver:\n    - 1.1.1.1\n",
        )
        .unwrap();
        super::normalize_dns_legacy_defaults(&mut custom);
        assert_eq!(
            custom
                .get("dns")
                .unwrap()
                .as_mapping()
                .unwrap()
                .get("fake-ip-range"),
            Some(&serde_yaml::Value::String("10.0.0.0/16".to_string()))
        );

        // 只剩 TEST-NET 名称服务器时补参考实现的兜底值
        let mut only_test_net: serde_yaml::Mapping =
            serde_yaml::from_str("dns:\n  default-nameserver:\n    - 198.51.100.53\n").unwrap();
        super::normalize_dns_legacy_defaults(&mut only_test_net);
        assert_eq!(
            only_test_net
                .get("dns")
                .unwrap()
                .as_mapping()
                .unwrap()
                .get("default-nameserver"),
            Some(&serde_yaml::Value::Sequence(vec![
                serde_yaml::Value::String("223.5.5.5".to_string())
            ]))
        );

        // 没有 dns 块（direct-only 基础配置）时直接返回
        let mut no_dns: serde_yaml::Mapping =
            serde_yaml::from_str("mixed-port: 7890\ntun:\n  enable: false\n").unwrap();
        super::normalize_dns_legacy_defaults(&mut no_dns);
        assert_eq!(
            no_dns.get("mixed-port"),
            Some(&serde_yaml::Value::Number(serde_yaml::Number::from(7890)))
        );
    }

    #[test]
    fn rule_targeting_proxy_provider_is_not_downgraded() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        let sub_id = "sub-provider-test";
        let sub_content = r#"
proxy-providers:
  my-provider:
    type: http
    url: http://example.com
    path: ./my-provider.yaml
rules:
  - MATCH,DIRECT
"#;
        std::fs::write(paths.subscription_file_path(sub_id), sub_content).unwrap();
        set_active_id_at(&paths, sub_id).unwrap();

        let user_rules = vec!["DOMAIN-SUFFIX,provider.com,my-provider".to_string()];
        rules::save_rules_at(&paths, &user_rules).unwrap();

        merge_user_config_at(&paths).unwrap();

        let config_text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&config_text).unwrap();
        let rules_in_config: Vec<&str> = config["rules"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();

        assert!(
            rules_in_config.contains(&"DOMAIN-SUFFIX,provider.com,my-provider"),
            "Rule targeting proxy-provider must remain unchanged, found rules: {:?}",
            rules_in_config
        );
    }

    #[test]
    fn merge_user_config_at_end_to_end_uses_passive_tolerant() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        let sub_id = "sub-e2e-passive";
        let sub_content = r#"
proxies:
  - name: LiveNode
    type: http
    server: live.example.com
    port: 443
proxy-groups:
  - name: Main
    type: select
    proxies:
      - LiveNode
rules:
  - MATCH,DIRECT
"#;
        std::fs::write(paths.subscription_file_path(sub_id), sub_content).unwrap();
        set_active_id_at(&paths, sub_id).unwrap();

        // Overlay has a patch adding a stale node
        let overlay_path = paths.groups_override_path_for_subscription(sub_id);
        std::fs::create_dir_all(overlay_path.parent().unwrap()).unwrap();
        let overlay_content = r#"
patches:
  Main:
    add_proxies:
      - ExpiredNode
    remove_proxies: []
"#;
        std::fs::write(&overlay_path, overlay_content).unwrap();

        // merge_user_config_at calls merge_user_config_at_endpoint - must succeed via PassiveTolerant
        let res = merge_user_config_at(&paths);
        assert!(
            res.is_ok(),
            "merge_user_config_at should succeed via PassiveTolerant: {:?}",
            res.err()
        );

        let config_text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&config_text).unwrap();
        let groups = config["proxy-groups"].as_sequence().unwrap();
        let main_group = groups
            .iter()
            .find(|g| g["name"].as_str() == Some("Main"))
            .unwrap();
        let members: Vec<&str> = main_group["proxies"]
            .as_sequence()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(members, vec!["LiveNode"]);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationReport {
    pub yaml_valid: bool,
    pub mihomo_tested: bool,
}

pub fn validate_config_at(
    paths: &crate::utils::AppPaths,
    mihomo_path: Option<&std::path::Path>,
) -> anyhow::Result<ConfigValidationReport> {
    let path = paths.config_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Cannot read config: {}\n  {}", path.display(), e))?;
    let _: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Config is not valid YAML: {e}"))?;

    let Some(mihomo) = mihomo_path else {
        return Ok(ConfigValidationReport {
            yaml_valid: true,
            mihomo_tested: false,
        });
    };
    if !mihomo.exists() {
        return Ok(ConfigValidationReport {
            yaml_valid: true,
            mihomo_tested: false,
        });
    }

    let out = std::process::Command::new(mihomo)
        .args(["-t", "-d"])
        .arg(paths.config_dir())
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run mihomo -t: {e}"))?;
    if out.status.success() {
        Ok(ConfigValidationReport {
            yaml_valid: true,
            mihomo_tested: true,
        })
    } else {
        let output = crate::utils::combine_output(&out);
        anyhow::bail!(
            "mihomo -t failed:\n{}",
            output.lines().take(20).collect::<Vec<_>>().join("\n")
        )
    }
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validate_config_accepts_valid_yaml_without_mihomo() {
        let tmp = TempDir::new().unwrap();
        let paths = crate::utils::AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), "port: 7890\nproxies: []\n").unwrap();

        let report = validate_config_at(&paths, None).unwrap();
        assert!(report.yaml_valid);
        assert!(!report.mihomo_tested);
    }

    #[test]
    fn validate_config_rejects_invalid_yaml() {
        let tmp = TempDir::new().unwrap();
        let paths = crate::utils::AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), "port: [\n").unwrap();

        let err = validate_config_at(&paths, None).unwrap_err();
        assert!(err.to_string().contains("not valid YAML"));
    }

    #[test]
    fn validate_config_reports_missing_file() {
        let tmp = TempDir::new().unwrap();
        let paths = crate::utils::AppPaths::for_test(tmp.path());

        let err = validate_config_at(&paths, None).unwrap_err();
        assert!(err.to_string().contains("Cannot read config"));
    }
}

#[cfg(test)]
mod checked_merge_tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn failing_mihomo(tmp: &TempDir) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp.path().join("mihomo-fail");
        std::fs::write(
            &path,
            "#!/usr/bin/env sh\necho simulated validation failure >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn setup_active_subscription(paths: &AppPaths) {
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::write(
            paths.subscription_file_path("sub-a"),
            r#"
proxies:
  - name: Proxy
    type: direct
proxy-groups:
  - name: ProxyGroup
    type: select
    proxies:
      - Proxy
rules:
  - MATCH,DIRECT
"#,
        )
        .unwrap();
        set_active_id_at(paths, "sub-a").unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn checked_merge_rolls_back_config_when_runtime_validation_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.config_path(), "port: 7890\n").unwrap();
        setup_active_subscription(&paths);
        crate::rules::save_rules_at(&paths, &["DOMAIN-SUFFIX,example.com,DIRECT".to_string()])
            .unwrap();
        let mihomo = failing_mihomo(&tmp);

        let err = merge_user_config_checked_at(&paths, Some(&mihomo)).unwrap_err();

        assert!(err.to_string().contains("rolled back config.yaml"));
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            "port: 7890\n"
        );
    }

    #[test]
    #[cfg(unix)]
    fn save_config_rolls_back_when_runtime_validation_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.config_path(),
            "port: 7890
proxies: []
",
        )
        .unwrap();
        let mihomo = failing_mihomo(&tmp);

        let err = save_config_at(
            &paths,
            "proxies:
  - name: Proxy
    type: direct
",
            Some(&mihomo),
        )
        .unwrap_err();

        assert!(err.to_string().contains("rolled back config.yaml"));
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            "port: 7890
proxies: []
"
        );
    }

    #[test]
    #[cfg(unix)]
    fn fix_existing_config_rolls_back_when_runtime_validation_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let original = "port: 7890
proxies: []
";
        std::fs::write(paths.config_path(), original).unwrap();
        let mihomo = failing_mihomo(&tmp);

        let err = fix_existing_config_at(&paths, Some(&mihomo)).unwrap_err();

        assert!(err.to_string().contains("rolled back config.yaml"));
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            original
        );
    }

    #[test]
    fn checked_merge_generates_direct_only_config_without_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        merge_user_config_checked_at(&paths, None).unwrap();

        let config: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(paths.config_path()).unwrap()).unwrap();
        assert_eq!(config["mode"].as_str(), Some("rule"));
        assert_eq!(config["proxies"].as_sequence().map(Vec::len), Some(0));
        assert_eq!(config["proxy-groups"].as_sequence().map(Vec::len), Some(0));
        assert_eq!(config["rules"][0].as_str(), Some("MATCH,DIRECT"));
        assert!(config["mixed-port"].as_u64().is_some());
        #[cfg(unix)]
        assert!(config["external-controller-unix"].as_str().is_some());
        #[cfg(windows)]
        assert!(config["external-controller-pipe"].as_str().is_some());
    }

    #[test]
    fn checked_merge_preserves_legacy_config_without_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let original = "mode: rule\nmixed-port: 7890\nproxies: []\nrules:\n  - MATCH,DIRECT\n";
        std::fs::write(paths.config_path(), original).unwrap();

        merge_user_config_checked_at(&paths, None).unwrap();

        let config: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(paths.config_path()).unwrap()).unwrap();
        assert_eq!(config["mixed-port"].as_u64(), Some(7890));
        assert_eq!(config["rules"][0].as_str(), Some("MATCH,DIRECT"));
        #[cfg(unix)]
        assert!(config["external-controller-unix"].as_str().is_some());
        #[cfg(windows)]
        assert!(config["external-controller-pipe"].as_str().is_some());
    }

    #[test]
    fn checked_merge_succeeds_when_yaml_validation_passes_without_mihomo() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        setup_active_subscription(&paths);

        merge_user_config_checked_at(&paths, None).unwrap();

        let config = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(config.contains("external-controller"));
        assert!(config.contains("MATCH,DIRECT") || config.contains("MATCH, DIRECT"));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionInfo {
    pub id: String,
    pub url: String,
    pub updated: DateTime<Utc>,
    pub proxy_count: usize,
    pub expire: Option<String>,
}

pub fn subscription_info_at(paths: &AppPaths, id: &str) -> anyhow::Result<SubscriptionInfo> {
    let subs = load_subscriptions_at(paths)?;
    let sub = find_subscription(&subs, id)
        .ok_or_else(|| anyhow::anyhow!("Subscription not found: {}", id))?;
    let path = paths.subscription_file_path(id);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read subscription file: {}", path.display()))?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse subscription file: {}", path.display()))?;
    let proxy_count = yaml
        .get("proxies")
        .and_then(|v| v.as_sequence())
        .map(|seq| seq.len())
        .unwrap_or(0)
        + yaml
            .get("proxy-providers")
            .and_then(|v| v.as_mapping())
            .map(|m| m.len())
            .unwrap_or(0);
    let expire = yaml
        .get("subscription-userinfo")
        .and_then(|v| v.as_str())
        .and_then(parse_expire_from_userinfo);

    Ok(SubscriptionInfo {
        id: sub.id.clone(),
        url: sub.url.clone(),
        updated: sub.updated,
        proxy_count,
        expire,
    })
}

fn parse_expire_from_userinfo(userinfo: &str) -> Option<String> {
    for part in userinfo.split(';') {
        let part = part.trim();
        let Some(value) = part.strip_prefix("expire=") else {
            continue;
        };
        if let Ok(ts) = value.parse::<i64>() {
            if let Some(dt) = DateTime::<Utc>::from_timestamp(ts, 0) {
                return Some(dt.to_rfc3339());
            }
        }
    }
    None
}

#[cfg(test)]
mod subscription_info_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn subscription_info_counts_proxies_and_parses_expire() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        let updated = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        save_subscriptions_at(
            &paths,
            &[SubscriptionMeta {
                id: "sub-a".to_string(),
                url: "file://sub-a.yaml".to_string(),
                updated,
                user_agent: None,
                user_agent_mode: Some(UserAgentMode::Auto),
                retired: false,
                retired_at: None,
            }],
        )
        .unwrap();
        std::fs::write(
            paths.subscription_file_path("sub-a"),
            r#"
subscription-userinfo: upload=0; download=0; total=100; expire=1700000000
proxies:
  - name: A
    type: direct
  - name: B
    type: direct
proxy-providers:
  p1:
    type: file
"#,
        )
        .unwrap();

        let info = subscription_info_at(&paths, "sub-a").unwrap();
        assert_eq!(info.proxy_count, 3);
        assert_eq!(info.expire, Some("2023-11-14T22:13:20+00:00".to_string()));
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn override_yaml_deep_merges_maps_and_replaces_lists() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(
            paths.override_path(),
            r#"
dns:
  enhanced-mode: redir-host
  nameserver:
    - 1.1.1.1
proxy-groups:
  - name: Custom
    type: select
    proxies:
      - DIRECT
external-controller-unix: /tmp/evil.sock
"#,
        )
        .unwrap();

        let output = apply_override_to_config_text(
            &paths,
            r#"
dns:
  enable: true
  enhanced-mode: fake-ip
  nameserver:
    - 8.8.8.8
proxy-groups:
  - name: Original
    type: select
    proxies:
      - DIRECT
rules:
  - MATCH,DIRECT
"#,
        )
        .unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&output).unwrap();

        assert_eq!(parsed["dns"]["enable"].as_bool(), Some(true));
        assert_eq!(parsed["dns"]["enhanced-mode"].as_str(), Some("redir-host"));
        assert_eq!(parsed["dns"]["nameserver"][0].as_str(), Some("1.1.1.1"));
        assert_eq!(parsed["proxy-groups"][0]["name"].as_str(), Some("Custom"));
        assert_ne!(
            parsed["external-controller-unix"].as_str(),
            Some("/tmp/evil.sock")
        );
    }

    #[test]
    fn override_yaml_must_be_mapping() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.override_path(), "- invalid\n").unwrap();

        let err = apply_override_to_config_text(&paths, "rules:\n  - MATCH,DIRECT\n").unwrap_err();
        assert!(err
            .to_string()
            .contains("override.yaml must be a YAML mapping"));
    }

    #[cfg(unix)]
    #[test]
    fn test_ensure_subscriptions_directory_inherits_parent_setgid() {
        use std::os::unix::fs::MetadataExt;

        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        // 为父目录 (config_dir) 设置 setgid
        let _ = utils::set_directory_mode_no_follow(paths.config_dir(), 0o2755);
        let parent_meta = std::fs::metadata(paths.config_dir()).unwrap();
        if utils::mode_has_setgid(parent_meta.mode()) {
            ensure_subscriptions_directory(&paths).unwrap();
            let sub_dir = paths.subscriptions_dir();
            let sub_meta = std::fs::metadata(&sub_dir).unwrap();
            assert!(utils::mode_has_setgid(sub_meta.mode()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_set_active_id_at_writes_group_readable_when_parent_setgid() {
        use std::os::unix::fs::MetadataExt;

        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();

        // 为父目录 (config_dir) 设置 setgid
        let _ = utils::set_directory_mode_no_follow(paths.config_dir(), 0o2755);
        let parent_meta = std::fs::metadata(paths.config_dir()).unwrap();
        if utils::mode_has_setgid(parent_meta.mode()) {
            crate::config::set_active_id_at(&paths, "sub-real-scene").unwrap();
            let sub_dir = paths.subscriptions_dir();
            let sub_meta = std::fs::metadata(&sub_dir).unwrap();
            assert!(utils::mode_has_setgid(sub_meta.mode()));

            let active_path = paths.active_file_path();
            let active_meta = std::fs::metadata(&active_path).unwrap();
            assert_eq!(active_meta.mode() & 0o777, 0o640);
        }
    }
}

#[cfg(test)]
mod tun_carry_over_tests {
    use super::carry_over_tun_block_from_previous_config;
    use serde_yaml::Value;

    fn parse(yaml: &str) -> Value {
        serde_yaml::from_str(yaml).expect("valid YAML")
    }

    fn seq(items: &[&str]) -> Value {
        Value::Sequence(
            items
                .iter()
                .map(|item| Value::String((*item).to_string()))
                .collect(),
        )
    }

    /// B：override 只定义 `tun.dns-hijack` 时，绝不能把上一次 `tun on`
    /// 写下的完整 TUN 意图整块顶掉（`tun.enable` 必须存活）。
    #[test]
    fn partial_tun_block_must_not_drop_previous_tun_intent() {
        let previous = "\
tun:
  enable: true
  stack: gvisor
  auto-route: true
  strict-route: true
  device-name: Meta
  dns-hijack:
  - any:53
";
        let new = "\
rules:
- MATCH,Proxy
tun:
  dns-hijack:
  - 10.0.0.1:53
";
        let out =
            carry_over_tun_block_from_previous_config(Some(previous), new).expect("carry over");
        let tun = &parse(&out)["tun"];
        assert_eq!(tun["enable"], Value::Bool(true), "tun.enable must survive");
        assert_eq!(tun["stack"], Value::String("gvisor".to_string()));
        assert_eq!(tun["auto-route"], Value::Bool(true));
        assert_eq!(tun["strict-route"], Value::Bool(true));
        assert_eq!(tun["device-name"], Value::String("Meta".to_string()));
        assert_eq!(
            tun["dns-hijack"],
            seq(&["any:53"]),
            "历史 TUN 意图优先于模板/订阅带来的新块（review MAJOR-4）"
        );
    }

    /// review MAJOR-4：订阅/模板自带的 `enable: false`（vmess 模板默认值）绝不能
    /// 把上一次 `tun on` 写下的意图翻回去；新块里历史意图没有的键仍要保留。
    #[test]
    fn carry_over_historical_intent_wins_over_template_defaults() {
        let previous = "tun:\n  enable: true\n  stack: gvisor\n  auto-route: true\n";
        let new = "tun:\n  enable: false\n  stack: system\n  strict-route: true\n";
        let out =
            carry_over_tun_block_from_previous_config(Some(previous), new).expect("carry over");
        let tun = &parse(&out)["tun"];
        assert_eq!(
            tun["enable"],
            Value::Bool(true),
            "模板的 enable:false 不得胜出"
        );
        assert_eq!(tun["stack"], Value::String("gvisor".to_string()));
        assert_eq!(tun["auto-route"], Value::Bool(true));
        assert_eq!(
            tun["strict-route"],
            Value::Bool(true),
            "历史意图没有的新键必须保留"
        );
    }

    #[test]
    fn carry_over_applies_when_new_config_has_no_tun_block() {
        let previous = "tun:\n  enable: true\n  stack: system\n";
        let new = "rules:\n- MATCH,Proxy\n";
        let out =
            carry_over_tun_block_from_previous_config(Some(previous), new).expect("carry over");
        let tun = &parse(&out)["tun"];
        assert_eq!(tun["enable"], Value::Bool(true));
        assert_eq!(tun["stack"], Value::String("system".to_string()));
    }

    /// review 回归-3：非控制键（mtu 等）必须允许 override/新块更新，
    /// 否则这些键一旦写入历史意图就再也改不动。
    #[test]
    fn carry_over_lets_non_controlled_tun_keys_be_updated() {
        let previous = "tun:\n  mtu: 1400\n  device-name: Meta\n  enable: true\n";
        let new = "tun:\n  mtu: 9000\n  enable: false\n";
        let out =
            carry_over_tun_block_from_previous_config(Some(previous), new).expect("carry over");
        let tun = &parse(&out)["tun"];
        assert_eq!(
            tun["mtu"],
            Value::Number(serde_yaml::Number::from(9000_i64)),
            "非控制键以新块为准"
        );
        assert_eq!(
            tun["device-name"],
            Value::String("Meta".to_string()),
            "新块没有的历史键必须保留"
        );
        assert_eq!(
            tun["enable"],
            Value::Bool(true),
            "控制键以历史 TUN 意图为准"
        );
    }

    #[test]
    fn carry_over_is_noop_without_previous_tun_intent() {
        let new = "tun:\n  enable: false\n";
        let out = carry_over_tun_block_from_previous_config(None, new).expect("carry over");
        assert_eq!(parse(&out)["tun"]["enable"], Value::Bool(false));
    }

    #[test]
    fn carry_over_keeps_new_config_untouched_without_previous_tun() {
        let previous = "mode: rule\n";
        let new = "rules:\n- MATCH,Proxy\n";
        let out =
            carry_over_tun_block_from_previous_config(Some(previous), new).expect("carry over");
        let parsed = parse(&out);
        assert!(parsed.get("tun").is_none(), "no tun intent to carry over");
        assert_eq!(parsed["rules"], parse("rules:\n- MATCH,Proxy\n")["rules"]);
    }
}

/// `override import` 对 `tun` 块必须是深合并——只覆盖/新增 override 显式
/// 给出的键，控制键（enable/stack/dns-hijack/strict-route/auto-route）与未提及的
/// 非控制键一律保留。
#[cfg(test)]
mod issue041_tun_merge_tests {
    use super::*;
    use serde_yaml::Value;
    use tempfile::TempDir;

    fn seq(items: &[&str]) -> Value {
        Value::Sequence(
            items
                .iter()
                .map(|item| Value::String((*item).to_string()))
                .collect(),
        )
    }

    const TUN_INTENT_FIXTURE: &str = "\
enable: true
stack: mixed
auto-route: true
strict-route: true
dns-hijack:
- any:53
- tcp://any:53
route-exclude-address:
- 224.0.0.0/4
";

    const OVERRIDE_ONLY_ROUTE_EXCLUDE: &str = "\
tun:
  route-exclude-address:
  - 239.0.0.0/8
";

    // 裁决验收口径：完整性断言解析 YAML 比对**整个 TUN map**（而非只看 `tun:`/`enable`）。
    const EXPECTED_TUN_AFTER_ROUTE_EXCLUDE_OVERRIDE: &str = "\
enable: true
stack: mixed
auto-route: true
strict-route: true
dns-hijack:
- any:53
- tcp://any:53
route-exclude-address:
- 239.0.0.0/8
";

    const EXPECTED_TUN_AFTER_ROUTE_EXCLUDE_CLEARED: &str = "\
enable: true
stack: mixed
auto-route: true
strict-route: true
dns-hijack:
- any:53
- tcp://any:53
route-exclude-address: []
";

    fn assert_whole_tun_map(tun: &serde_yaml::Value, expected: &str, context: &str) {
        let expected: serde_yaml::Value =
            serde_yaml::from_str(expected).expect("valid expected tun map");
        assert_eq!(tun, &expected, "the whole TUN map must match ({context})");
    }

    fn assert_tun_control_keys_kept(tun: &serde_yaml::Value, context: &str) {
        assert_eq!(
            tun["enable"],
            serde_yaml::Value::Bool(true),
            "tun.enable must survive override import ({context})"
        );
        assert_eq!(
            tun["stack"],
            serde_yaml::Value::String("mixed".to_string()),
            "tun.stack must survive override import ({context})"
        );
        assert_eq!(
            tun["auto-route"],
            serde_yaml::Value::Bool(true),
            "tun.auto-route must survive override import ({context})"
        );
        assert_eq!(
            tun["strict-route"],
            serde_yaml::Value::Bool(true),
            "tun.strict-route must survive override import ({context})"
        );
        assert_eq!(
            tun["dns-hijack"],
            seq(&["any:53", "tcp://any:53"]),
            "tun.dns-hijack must survive override import ({context})"
        );
    }

    /// 缺陷 2：`override import` 只带 `tun.route-exclude-address` 时，
    /// tun 块必须**深合并**（控制键保留、显式键覆盖），不得整块替换。
    #[test]
    fn issue041_override_import_deep_merges_tun_with_active_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        let sub_id = "sub-041";
        std::fs::write(
            paths.subscription_file_path(sub_id),
            "proxies:\n  - name: HK01\n    type: http\n    server: hk.example.com\n    port: 443\nrules:\n  - MATCH,DIRECT\n",
        )
        .unwrap();
        set_active_id_at(&paths, sub_id).unwrap();
        merge_user_config_at(&paths).unwrap();

        // `tun on` committed intent: full tun block (incl. route-exclude-address).
        let mut doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(paths.config_path()).unwrap()).unwrap();
        doc.as_mapping_mut().unwrap().insert(
            serde_yaml::Value::String("tun".to_string()),
            serde_yaml::from_str(TUN_INTENT_FIXTURE).unwrap(),
        );
        std::fs::write(paths.config_path(), serde_yaml::to_string(&doc).unwrap()).unwrap();

        std::fs::write(paths.override_path(), OVERRIDE_ONLY_ROUTE_EXCLUDE).unwrap();
        merge_user_config_at(&paths).unwrap();

        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let tun = &config["tun"];
        assert_tun_control_keys_kept(tun, "active subscription");
        assert_whole_tun_map(
            tun,
            EXPECTED_TUN_AFTER_ROUTE_EXCLUDE_OVERRIDE,
            "active subscription",
        );
        assert_eq!(
            tun["route-exclude-address"],
            seq(&["239.0.0.0/8"]),
            "override 显式给出的键覆盖原值; config was:\n{text}"
        );
    }

    /// 缺陷 2（无 active subscription 的 legacy 分支同样必须深合并）。
    #[test]
    fn issue041_override_import_deep_merges_tun_without_subscription() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let mut doc: serde_yaml::Value = serde_yaml::from_str(
            "mixed-port: 7897\nproxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n",
        )
        .unwrap();
        doc.as_mapping_mut().unwrap().insert(
            serde_yaml::Value::String("tun".to_string()),
            serde_yaml::from_str(TUN_INTENT_FIXTURE).unwrap(),
        );
        std::fs::write(paths.config_path(), serde_yaml::to_string(&doc).unwrap()).unwrap();
        std::fs::write(paths.override_path(), OVERRIDE_ONLY_ROUTE_EXCLUDE).unwrap();
        merge_user_config_at(&paths).unwrap();

        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let tun = &config["tun"];
        assert_tun_control_keys_kept(tun, "legacy config");
        assert_whole_tun_map(
            tun,
            EXPECTED_TUN_AFTER_ROUTE_EXCLUDE_OVERRIDE,
            "legacy config",
        );
        assert_eq!(
            tun["route-exclude-address"],
            seq(&["239.0.0.0/8"]),
            "override 显式给出的键覆盖原值; config was:\n{text}"
        );
    }

    /// 缺陷 2：重生成时读不到上一份 intent，不能退化成「没有 TUN 意图」
    /// 继续写（那会把 tun 控制键静默删除）；必须 fail-closed 且不改动 intent。
    #[cfg(unix)]
    #[test]
    fn issue041_generation_fails_closed_when_previous_intent_unreadable() {
        if unsafe { libc::geteuid() } == 0 {
            return; // root 无视 0o000，无法构造不可读
        }
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        let sub_id = "sub-041-unreadable";
        std::fs::write(
            paths.subscription_file_path(sub_id),
            "proxies:\n  - name: HK01\n    type: http\n    server: hk.example.com\n    port: 443\nrules:\n  - MATCH,DIRECT\n",
        )
        .unwrap();
        set_active_id_at(&paths, sub_id).unwrap();
        std::fs::write(
            paths.config_path(),
            "mode: rule\nrules:\n  - MATCH,DIRECT\n",
        )
        .unwrap();
        std::fs::write(paths.override_path(), OVERRIDE_ONLY_ROUTE_EXCLUDE).unwrap();
        std::fs::set_permissions(paths.config_path(), std::fs::Permissions::from_mode(0o000))
            .unwrap();

        let error =
            merge_user_config_at(&paths).expect_err("unreadable intent must not be overwritten");
        assert!(
            error.to_string().contains("TUN intent carry-over"),
            "{error}"
        );
        let meta = std::fs::metadata(paths.config_path()).unwrap();
        assert_eq!(
            meta.len(),
            "mode: rule\nrules:\n  - MATCH,DIRECT\n".len() as u64
        );
    }

    /// 缺陷 2（裁决定稿）：数组整体替换、不追加；`[]` 是显式清空。
    #[test]
    fn issue041_override_tun_arrays_replace_not_append() {
        let cases: [(&str, Vec<&str>); 2] = [
            (
                "tun:\n  route-exclude-address:\n  - 239.0.0.0/8\n",
                vec!["239.0.0.0/8"],
            ),
            ("tun:\n  route-exclude-address: []\n", vec![]),
        ];
        for (override_text, expected) in cases {
            let tmp = TempDir::new().unwrap();
            let paths = AppPaths::for_test(tmp.path());
            std::fs::create_dir_all(paths.config_dir()).unwrap();
            let mut doc: serde_yaml::Value =
                serde_yaml::from_str("mixed-port: 7897\nproxies: []\nrules:\n  - MATCH,DIRECT\n")
                    .unwrap();
            doc.as_mapping_mut().unwrap().insert(
                serde_yaml::Value::String("tun".to_string()),
                serde_yaml::from_str(TUN_INTENT_FIXTURE).unwrap(),
            );
            std::fs::write(paths.config_path(), serde_yaml::to_string(&doc).unwrap()).unwrap();
            std::fs::write(paths.override_path(), override_text).unwrap();
            merge_user_config_at(&paths).unwrap();

            let text = std::fs::read_to_string(paths.config_path()).unwrap();
            let config: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
            let tun = &config["tun"];
            assert_tun_control_keys_kept(tun, override_text);
            assert_whole_tun_map(
                tun,
                if expected.is_empty() {
                    EXPECTED_TUN_AFTER_ROUTE_EXCLUDE_CLEARED
                } else {
                    EXPECTED_TUN_AFTER_ROUTE_EXCLUDE_OVERRIDE
                },
                override_text,
            );
            assert_eq!(
                tun["route-exclude-address"],
                seq(&expected),
                "array must be replaced wholesale (no append); override={override_text} config was:\n{text}"
            );
        }
    }

    /// 缺陷 2：重复 import 幂等——同一 override 连续应用两次，结果字节一致。
    #[test]
    fn issue041_override_import_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let mut doc: serde_yaml::Value =
            serde_yaml::from_str("mixed-port: 7897\nproxies: []\nrules:\n  - MATCH,DIRECT\n")
                .unwrap();
        doc.as_mapping_mut().unwrap().insert(
            serde_yaml::Value::String("tun".to_string()),
            serde_yaml::from_str(TUN_INTENT_FIXTURE).unwrap(),
        );
        std::fs::write(paths.config_path(), serde_yaml::to_string(&doc).unwrap()).unwrap();
        std::fs::write(paths.override_path(), OVERRIDE_ONLY_ROUTE_EXCLUDE).unwrap();

        merge_user_config_at(&paths).unwrap();
        let first = std::fs::read_to_string(paths.config_path()).unwrap();
        merge_user_config_at(&paths).unwrap();
        let second = std::fs::read_to_string(paths.config_path()).unwrap();
        assert_eq!(first, second, "repeated imports must converge");
    }

    /// 缺陷 2：TUN 控制键由事务独占——override 写 `enable: false` / `stack`
    /// / `strict-route` / `dns-hijack`（含数组）一律不生效。
    #[test]
    fn issue041_override_cannot_change_tun_control_keys() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let mut doc: serde_yaml::Value =
            serde_yaml::from_str("mixed-port: 7897\nproxies: []\nrules:\n  - MATCH,DIRECT\n")
                .unwrap();
        doc.as_mapping_mut().unwrap().insert(
            serde_yaml::Value::String("tun".to_string()),
            serde_yaml::from_str(TUN_INTENT_FIXTURE).unwrap(),
        );
        std::fs::write(paths.config_path(), serde_yaml::to_string(&doc).unwrap()).unwrap();
        std::fs::write(
            paths.override_path(),
            "tun:\n  enable: false\n  stack: system\n  strict-route: false\n  dns-hijack:\n  - 1.1.1.1:53\n  mtu: 9000\n",
        )
        .unwrap();

        merge_user_config_at(&paths).unwrap();
        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        let tun = &config["tun"];
        assert_tun_control_keys_kept(tun, "control-key override");
        assert_eq!(
            tun["mtu"],
            serde_yaml::Value::Number(serde_yaml::Number::from(9000_i64)),
            "non-control keys still flow through override; config was:\n{text}"
        );
    }

    /// 缺陷 2：类型绕过（`tun: null` / 标量 / 数组）必须在写入前拒绝，
    /// intent 保持原样；`tun: {}` 不是删除。
    #[test]
    fn issue041_override_non_mapping_tun_is_rejected() {
        for bad in ["tun: null\n", "tun: 5\n", "tun: disabled\n", "tun:\n- a\n"] {
            let tmp = TempDir::new().unwrap();
            let paths = AppPaths::for_test(tmp.path());
            std::fs::create_dir_all(paths.config_dir()).unwrap();
            let mut doc: serde_yaml::Value =
                serde_yaml::from_str("mixed-port: 7897\nproxies: []\nrules:\n  - MATCH,DIRECT\n")
                    .unwrap();
            doc.as_mapping_mut().unwrap().insert(
                serde_yaml::Value::String("tun".to_string()),
                serde_yaml::from_str(TUN_INTENT_FIXTURE).unwrap(),
            );
            let before = serde_yaml::to_string(&doc).unwrap();
            std::fs::write(paths.config_path(), &before).unwrap();
            std::fs::write(paths.override_path(), bad).unwrap();

            let error = merge_user_config_at(&paths)
                .expect_err("a non-mapping `tun` must be refused before writing");
            assert!(
                error.to_string().contains("must be a YAML mapping"),
                "{bad}: {error}"
            );
            assert_eq!(
                std::fs::read_to_string(paths.config_path()).unwrap(),
                before,
                "intent must be untouched for override `{bad}`"
            );
        }

        // `tun: {}` 是空映射（不是删除）：控制键原样保留。
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let mut doc: serde_yaml::Value =
            serde_yaml::from_str("mixed-port: 7897\nproxies: []\nrules:\n  - MATCH,DIRECT\n")
                .unwrap();
        doc.as_mapping_mut().unwrap().insert(
            serde_yaml::Value::String("tun".to_string()),
            serde_yaml::from_str(TUN_INTENT_FIXTURE).unwrap(),
        );
        std::fs::write(paths.config_path(), serde_yaml::to_string(&doc).unwrap()).unwrap();
        std::fs::write(paths.override_path(), "tun: {}\n").unwrap();
        merge_user_config_at(&paths).unwrap();
        let text = std::fs::read_to_string(paths.config_path()).unwrap();
        let config: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
        assert_tun_control_keys_kept(&config["tun"], "empty tun mapping");
    }

    /// 缺陷 2：reconcile（restart/apply 入口的 override 收敛）同样拒绝类型绕过。
    #[test]
    fn issue041_reconcile_rejects_non_mapping_tun() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        let before = format!("mode: rule\nrules:\n  - MATCH,DIRECT\ntun:\n{TUN_INTENT_FIXTURE}");
        std::fs::write(paths.config_path(), &before).unwrap();
        std::fs::write(paths.override_path(), "tun: null\n").unwrap();

        let error = reconcile_override_into_intent_checked(&paths, None)
            .err()
            .expect("reconcile must refuse a non-mapping tun");
        assert!(
            error.to_string().contains("must be a YAML mapping"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            before,
            "intent must be untouched"
        );
    }
}

#[cfg(test)]
mod yaml_structural_equality_tests {
    use super::yaml_structurally_equal;

    fn parse(yaml: &str) -> serde_yaml::Value {
        serde_yaml::from_str(yaml).expect("valid YAML")
    }

    #[test]
    fn mapping_key_order_does_not_break_equality() {
        let left = parse("a: 1\nb: 2\n");
        let right = parse("b: 2\na: 1\n");
        assert!(yaml_structurally_equal(&left, &right));
    }

    #[test]
    fn sequence_order_still_matters() {
        let left = parse("- 1\n- 2\n");
        let right = parse("- 2\n- 1\n");
        assert!(!yaml_structurally_equal(&left, &right));
    }

    #[test]
    fn missing_key_is_a_difference() {
        let intent = parse("tun:\n  enable: true\nrules:\n- MATCH,DIRECT\n");
        let merged =
            parse("tun:\n  enable: true\n  dns-hijack:\n  - any:53\nrules:\n- MATCH,DIRECT\n");
        assert!(!yaml_structurally_equal(&intent, &merged));
        assert!(yaml_structurally_equal(&merged, &merged));
    }
}

#[cfg(test)]
mod override_reconcile_tests {
    use super::*;

    fn temp_paths(tag: &str) -> AppPaths {
        let dir =
            std::env::temp_dir().join(format!("mihomo-reconcile-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        AppPaths::new(dir)
    }

    /// B：reconcile 只在 override 派生键确实缺失时才写 intent。
    /// 键序差异不得触发重写（会破坏“tun 为最后一个顶层键”的契约）。
    #[test]
    fn reconcile_does_not_rewrite_when_only_key_order_differs() {
        let paths = temp_paths("order");
        std::fs::write(
            paths.config_path(),
            "mixed-port: 7890\nrules:\n- MATCH,DIRECT\nsniffer:\n  enable: true\nexternal-controller-unix: /tmp/x.sock\ntun:\n  enable: true\n",
        )
        .unwrap();
        std::fs::write(paths.override_path(), "sniffer:\n  enable: true\n").unwrap();

        let changed = reconcile_override_into_intent_checked(&paths, None)
            .expect("reconcile must not fail")
            .changed;
        assert!(
            !changed,
            "key-order-only difference must not rewrite intent"
        );
        let after = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(
            after.contains("external-controller-unix: /tmp/x.sock\ntun:"),
            "file untouched (controller must stay before tun): {after}"
        );
        let _ = std::fs::remove_dir_all(paths.config_dir());
    }

    /// 真实缺失 override 派生键时必须写回，并返回 true。
    #[test]
    fn reconcile_writes_when_override_keys_are_missing() {
        let paths = temp_paths("missing");
        std::fs::write(
            paths.config_path(),
            "mixed-port: 7890\nrules:\n- MATCH,DIRECT\n",
        )
        .unwrap();
        std::fs::write(paths.override_path(), "sniffer:\n  enable: true\n").unwrap();

        let changed = reconcile_override_into_intent_checked(&paths, None)
            .expect("reconcile must not fail")
            .changed;
        assert!(changed, "missing override key must be written back");
        let after = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(after.contains("sniffer"), "override key merged into intent");
        let _ = std::fs::remove_dir_all(paths.config_dir());
    }

    /// review BLOCK-1：override 里的 runtime 端点字段绝不能被 reconcile 写进 intent，
    /// 否则 restart 提交的 payload 会让 Core 在 0.0.0.0 上开放 API 并挂载任意 external-ui。
    #[test]
    fn reconcile_rejects_runtime_endpoint_keys_from_override() {
        let paths = temp_paths("endpoint");
        std::fs::write(
            paths.config_path(),
            "mixed-port: 7890\nrules:\n- MATCH,DIRECT\nexternal-controller-unix: /tmp/x.sock\n",
        )
        .unwrap();
        std::fs::write(
            paths.override_path(),
            "external-controller: 0.0.0.0:9090\nexternal-ui: /tmp/ui\nsniffer:\n  enable: true\n",
        )
        .unwrap();

        let changed = reconcile_override_into_intent_checked(&paths, None)
            .expect("reconcile must not fail")
            .changed;
        assert!(changed, "sniffer key must still be merged");
        let after = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(
            !after.contains("0.0.0.0:9090"),
            "override 的 external-controller 必须被拒绝: {after}"
        );
        assert!(
            !after.contains("/tmp/ui"),
            "external-ui 必须被拒绝: {after}"
        );
        assert!(
            after.contains("external-controller-unix: /tmp/x.sock"),
            "受管 unix 端点必须保留: {after}"
        );
        assert!(after.contains("sniffer"), "非管辖键正常合并: {after}");
        let _ = std::fs::remove_dir_all(paths.config_dir());
    }

    /// review MAJOR-1：override 的 `tun.enable` 不得绕过 TUN 事务翻转意图。
    #[test]
    fn reconcile_rejects_tun_block_from_override() {
        let paths = temp_paths("tunblock");
        std::fs::write(
            paths.config_path(),
            "rules:\n- MATCH,DIRECT\ntun:\n  enable: false\n",
        )
        .unwrap();
        std::fs::write(
            paths.override_path(),
            "tun:\n  enable: true\n  stack: system\n  route-exclude-address:\n  - 169.254.0.0/16\nsniffer:\n  enable: true\n",
        )
        .unwrap();

        let changed = reconcile_override_into_intent_checked(&paths, None)
            .expect("reconcile must not fail")
            .changed;
        assert!(changed, "override keys must still be merged");
        let after = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(
            after.contains("enable: false"),
            "intent 的 tun.enable 必须保持事务提交的 false: {after}"
        );
        assert!(
            !after.contains("stack: system"),
            "override 的 tun.stack 是控制键，必须被拒绝: {after}"
        );
        assert!(
            after.contains("route-exclude-address"),
            "非控制键必须照常采纳（ADR-22，review 回归-3）: {after}"
        );
        assert!(after.contains("sniffer"), "非管辖键正常合并: {after}");
        let _ = std::fs::remove_dir_all(paths.config_dir());
    }

    /// review MAJOR-2：reconcile 必须与生成流程同序应用悬空规则兜底，且第二次调用不重写。
    #[test]
    fn reconcile_applies_dangling_rule_fallback_and_stays_idempotent() {
        let paths = temp_paths("dangling");
        std::fs::write(
            paths.config_path(),
            "rules:\n- DOMAIN-SUFFIX,foo.example,RemovedGroup\n",
        )
        .unwrap();
        std::fs::write(paths.override_path(), "sniffer:\n  enable: true\n").unwrap();

        let first = reconcile_override_into_intent_checked(&paths, None)
            .expect("reconcile must not fail")
            .changed;
        assert!(first, "override key must be merged on first pass");
        let after = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(
            !after.contains("RemovedGroup"),
            "悬空规则必须兜底成 DIRECT: {after}"
        );
        assert!(
            after.contains("DOMAIN-SUFFIX,foo.example,DIRECT"),
            "{after}"
        );

        let second = reconcile_override_into_intent_checked(&paths, None)
            .expect("reconcile must not fail")
            .changed;
        assert!(!second, "second pass must be a no-op (idempotent)");
        let _ = std::fs::remove_dir_all(paths.config_dir());
    }

    /// review Round 1 MAJOR-2 的**原场景**：悬空规则来自 override（旧组名），
    /// 合并后必须兜底成 DIRECT，且第二次 reconcile 不得把 override 的旧组名再写回来。
    #[test]
    fn reconcile_falls_back_rules_that_dangle_from_override() {
        let paths = temp_paths("dangling-override");
        std::fs::write(paths.config_path(), "rules:\n- MATCH,DIRECT\n").unwrap();
        std::fs::write(
            paths.override_path(),
            "rules:\n- DOMAIN-SUFFIX,foo.example,DeletedGroup\n",
        )
        .unwrap();

        let first = reconcile_override_into_intent_checked(&paths, None)
            .expect("reconcile must not fail")
            .changed;
        assert!(first, "override rules must be merged");
        let after = std::fs::read_to_string(paths.config_path()).unwrap();
        assert!(
            !after.contains("DeletedGroup"),
            "override 里的旧组名必须兜底: {after}"
        );
        assert!(
            after.contains("DOMAIN-SUFFIX,foo.example,DIRECT"),
            "{after}"
        );

        let second = reconcile_override_into_intent_checked(&paths, None)
            .expect("reconcile must not fail")
            .changed;
        assert!(
            !second,
            "override 的悬空组名不得被再次写回（幂等）: {}",
            std::fs::read_to_string(paths.config_path()).unwrap()
        );
        let _ = std::fs::remove_dir_all(paths.config_dir());
    }
}

#[cfg(test)]
mod a1_tombstone_tests {
    use super::*;
    use tempfile::TempDir;

    fn meta(id: &str, url: &str) -> SubscriptionMeta {
        SubscriptionMeta {
            id: id.to_string(),
            url: url.to_string(),
            updated: Utc::now(),
            user_agent: None,
            user_agent_mode: Some(UserAgentMode::Auto),
            retired: false,
            retired_at: None,
        }
    }

    #[test]
    fn revive_same_url_keeps_identity_and_selections() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let id = "sub-aaaa1111";
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::write(paths.subscription_file_path(id), "proxies: []").unwrap();
        save_subscriptions_at(&paths, &[meta(id, "https://a.com")]).unwrap();
        let sel = paths.selection_state_path_for_subscription(id);
        std::fs::create_dir_all(paths.selections_dir()).unwrap();
        std::fs::write(&sel, "Test: DIRECT\n").unwrap();

        remove_subscription_at(&paths, id).unwrap();
        assert!(!paths.subscription_file_path(id).exists());

        let commit = commit_subscription_matched_at(
            &paths,
            "https://a.com",
            "proxies: []\n",
            None,
            Some(false),
        )
        .unwrap();
        assert_eq!(commit.id, id);
        assert!(matches!(commit.kind, SubscriptionCommitKind::Revive { .. }));

        let subs = load_subscriptions_at(&paths).unwrap();
        assert_eq!(subs.len(), 1);
        assert!(subs[0].is_alive());
        assert!(subs[0].retired_at.is_none());
        assert_eq!(subs[0].url, "https://a.com");
        // Identity-adjacent state survives revive.
        assert!(paths.subscription_file_path(id).exists());
        assert_eq!(std::fs::read_to_string(&sel).unwrap(), "Test: DIRECT\n");
    }

    #[test]
    fn living_same_url_reuses_identity() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let id = "sub-bbbb2222";
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::write(paths.subscription_file_path(id), "proxies: []").unwrap();
        save_subscriptions_at(&paths, &[meta(id, "https://b.com")]).unwrap();

        let commit = commit_subscription_matched_at(
            &paths,
            "https://b.com",
            "proxies: []\n",
            None,
            Some(false),
        )
        .unwrap();
        assert_eq!(commit.id, id);
        assert_eq!(commit.kind, SubscriptionCommitKind::Reuse);
        assert_eq!(load_subscriptions_at(&paths).unwrap().len(), 1);
    }

    #[test]
    fn ambiguous_living_urls_fail_fast_with_convergence_guidance() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        save_subscriptions_at(
            &paths,
            &[
                meta("sub-cccc1111", "https://c.com"),
                meta("sub-dddd2222", "https://c.com"),
            ],
        )
        .unwrap();

        let err = commit_subscription_matched_at(
            &paths,
            "https://c.com",
            "proxies: []\n",
            None,
            Some(false),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("multiple living subscriptions"), "{err}");
        assert!(err.contains("--switch sub-cccc1111"), "{err}");
        assert!(err.contains("--remove sub-dddd2222"), "{err}");
        // No identity was created or modified.
        let subs = load_subscriptions_at(&paths).unwrap();
        assert_eq!(subs.len(), 2);
        assert!(!paths.subscription_file_path("sub-cccc1111").exists());
    }

    #[test]
    fn reuse_failure_rollback_restores_preexisting_cache() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let id = "sub-eeee3333";
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        let cache = paths.subscription_file_path(id);
        std::fs::write(&cache, "old-cache").unwrap();
        save_subscriptions_at(&paths, &[meta(id, "https://e.com")]).unwrap();

        // Reuse identity resolution inside commit_subscription_entry_at fails
        // after the cache write (id absent from the metadata handed in), so the
        // rollback must restore the pre-existing cache byte-for-byte instead of
        // deleting it (A1' restore-only rollback contract).
        let mut orphan_subs = vec![meta("sub-other00", "https://other.com")];
        let err = commit_subscription_entry_at(
            &paths,
            id,
            SubscriptionCommitKind::Reuse,
            "new-cache",
            &mut orphan_subs,
            "https://e.com",
            None,
            Some(false),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Subscription not found"), "{err}");
        assert_eq!(std::fs::read_to_string(&cache).unwrap(), "old-cache");
        // Metadata untouched.
        let subs = load_subscriptions_at(&paths).unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].id, id);
        assert!(subs[0].is_alive());
    }

    #[test]
    fn checked_commit_failure_does_not_cover_later_commit() {
        // N1: T1 succeeds, T2 fails → T1's data must survive intact.
        // Tests the after_commit failure path in commit_subscription_matched_checked_at.
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        // T1: commit_subscription_matched_at succeeds with URL "https://a.com"
        let t1 = commit_subscription_matched_at(
            &paths,
            "https://a.com",
            "t1-cache-content",
            None,
            Some(false),
        )
        .unwrap();
        let t1_id = t1.id.clone();
        let t1_cache = paths.subscription_file_path(&t1_id);
        assert_eq!(
            std::fs::read_to_string(&t1_cache).unwrap(),
            "t1-cache-content"
        );
        let t1_subs = load_subscriptions_at(&paths).unwrap();
        assert_eq!(t1_subs.len(), 1);
        assert_eq!(t1_subs[0].id, t1_id);

        // T2: commit_subscription_matched_checked_at with URL "https://b.com"
        // succeeds in commit but after_commit returns Err → must rollback T2's
        // own pre-image, NOT overwrite T1's data.
        let t2_result = commit_subscription_matched_checked_at(
            &paths,
            "https://b.com",
            "t2-cache-content",
            None,
            Some(false),
            |_commit, _pre_images| {
                // Simulate after_commit failure (e.g., merge validation failed)
                anyhow::bail!("simulated after_commit failure")
            },
        );
        assert!(t2_result.is_err(), "T2 must fail");

        // T1's data must survive intact
        assert_eq!(
            std::fs::read_to_string(&t1_cache).unwrap(),
            "t1-cache-content",
            "T1 cache must not be overwritten by T2 rollback"
        );
        let final_subs = load_subscriptions_at(&paths).unwrap();
        assert_eq!(final_subs.len(), 1, "T1 metadata must survive");
        assert_eq!(final_subs[0].id, t1_id);
        assert_eq!(final_subs[0].url, "https://a.com");

        // T2's cache must not exist (it was new, so pre-image was None → removed)
        let t2_caches: Vec<_> = std::fs::read_dir(paths.subscriptions_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path() != t1_cache
                    && e.path().extension().and_then(|s| s.to_str()) == Some("yaml")
            })
            .collect();
        assert!(
            t2_caches.is_empty(),
            "T2 cache must be removed on rollback (new identity has no pre-image)"
        );
    }

    #[test]
    fn tombstone_fifo_evicts_oldest_beyond_limit() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();

        let base = Utc::now() - chrono::Duration::days(2);
        let mut subs: Vec<SubscriptionMeta> = (0..RETIRED_TOMBSTONE_LIMIT)
            .map(|i| {
                let mut entry = meta(&format!("sub-{i:08}"), &format!("https://t{i}.com"));
                entry.retired = true;
                entry.retired_at = Some(base + chrono::Duration::minutes(i as i64));
                entry
            })
            .collect();
        let living = "sub-9999eeee";
        subs.push(meta(living, "https://live.com"));
        save_subscriptions_at(&paths, &subs).unwrap();

        remove_subscription_at(&paths, living).unwrap();

        let after = load_subscriptions_at(&paths).unwrap();
        assert_eq!(after.len(), RETIRED_TOMBSTONE_LIMIT);
        // Oldest tombstone evicted, newest retained.
        assert!(!after.iter().any(|s| s.id == "sub-00000000"));
        assert!(after.iter().any(|s| s.id == living && s.retired));
        let last_kept = format!("sub-{:08}", RETIRED_TOMBSTONE_LIMIT - 1);
        assert!(after.iter().any(|s| s.id == last_kept));
    }

    #[tokio::test]
    async fn retired_entries_are_invisible_to_switch_and_refresh() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        let id = "sub-ffff4444";
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::write(paths.subscription_file_path(id), "proxies: []").unwrap();
        save_subscriptions_at(&paths, &[meta(id, "https://f.com")]).unwrap();
        remove_subscription_at(&paths, id).unwrap();

        let err = switch_subscription_at(&paths, id).unwrap_err().to_string();
        assert!(err.contains("retired"), "{err}");

        let err = refresh_subscription_at(&paths, id)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("retired"), "{err}");
    }

    #[test]
    fn first_living_add_activates_even_with_tombstones_present() {
        let tmp = TempDir::new().unwrap();
        let paths = AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        let mut tomb = meta("sub-gone0000", "https://gone.com");
        tomb.retired = true;
        tomb.retired_at = Some(Utc::now());
        save_subscriptions_at(&paths, &[tomb]).unwrap();

        let commit =
            commit_subscription_matched_at(&paths, "https://new.com", "proxies: []\n", None, None)
                .unwrap();
        assert_eq!(commit.kind, SubscriptionCommitKind::New);
        assert_eq!(get_active_id_at(&paths).unwrap(), Some(commit.id));
    }

    #[test]
    fn test_validate_override_proxies_append_shape_matrix() {
        // Valid case
        let valid_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
            proxies-append:
              - name: SG-corp-host
                type: socks5
                server: 127.0.0.1
                port: 11080
              - name: HK-vps
                type: ss
                server: 1.2.3.4
                port: 8388
            "#,
        )
        .unwrap();
        assert!(validate_override_proxies_append_shape(&valid_yaml).is_ok());

        // Absent proxies-append is ok
        let no_append: serde_yaml::Value = serde_yaml::from_str(
            "tun: {}
",
        )
        .unwrap();
        assert!(validate_override_proxies_append_shape(&no_append).is_ok());

        // Non-sequence
        let bad_seq: serde_yaml::Value = serde_yaml::from_str(
            "proxies-append: true
",
        )
        .unwrap();
        assert!(validate_override_proxies_append_shape(&bad_seq)
            .unwrap_err()
            .to_string()
            .contains("must be a YAML sequence"));

        // Item non-mapping
        let bad_item: serde_yaml::Value = serde_yaml::from_str(
            "proxies-append: [123]
",
        )
        .unwrap();
        assert!(validate_override_proxies_append_shape(&bad_item)
            .unwrap_err()
            .to_string()
            .contains("must be a YAML mapping"));

        // Missing name
        let no_name: serde_yaml::Value = serde_yaml::from_str(
            "proxies-append: [{type: socks5}]
",
        )
        .unwrap();
        assert!(validate_override_proxies_append_shape(&no_name)
            .unwrap_err()
            .to_string()
            .contains("must have a non-empty `name`"));

        // Empty name
        let empty_name: serde_yaml::Value = serde_yaml::from_str(
            "proxies-append: [{name: ' ', type: socks5}]
",
        )
        .unwrap();
        assert!(validate_override_proxies_append_shape(&empty_name)
            .unwrap_err()
            .to_string()
            .contains("cannot be empty"));

        // Missing type
        let no_type: serde_yaml::Value = serde_yaml::from_str(
            "proxies-append: [{name: 'node1'}]
",
        )
        .unwrap();
        assert!(validate_override_proxies_append_shape(&no_type)
            .unwrap_err()
            .to_string()
            .contains("must have a non-empty `type`"));

        // Duplicate name
        let dup: serde_yaml::Value = serde_yaml::from_str(
            r#"
            proxies-append:
              - name: SG-node
                type: socks5
              - name: SG-node
                type: ss
            "#,
        )
        .unwrap();
        assert!(validate_override_proxies_append_shape(&dup)
            .unwrap_err()
            .to_string()
            .contains("duplicate proxy name `SG-node`"));
    }

    #[test]
    fn test_merge_proxies_append_appends_and_overwrites_by_name() {
        let mut config: serde_yaml::Value = serde_yaml::from_str(
            r#"
            proxies:
              - name: SubNode1
                type: ss
                server: 1.1.1.1
              - name: SharedNode
                type: vmess
                server: 2.2.2.2
            "#,
        )
        .unwrap();

        let mut override_val: serde_yaml::Value = serde_yaml::from_str(
            r#"
            proxies-append:
              - name: SharedNode
                type: socks5
                server: 127.0.0.1
              - name: CustomNode
                type: socks5
                server: 127.0.0.1
                port: 1080
            "#,
        )
        .unwrap();

        merge_proxies_append(&mut config, &mut override_val);

        // proxies-append must be removed from override_val and absent in config
        assert!(override_val.get("proxies-append").is_none());
        assert!(config.get("proxies-append").is_none());

        let proxies = config.get("proxies").and_then(|v| v.as_sequence()).unwrap();
        assert_eq!(proxies.len(), 3);
        assert_eq!(proxies[0]["name"].as_str().unwrap(), "SubNode1");
        assert_eq!(proxies[1]["name"].as_str().unwrap(), "SharedNode");
        assert_eq!(proxies[1]["type"].as_str().unwrap(), "socks5"); // updated
        assert_eq!(proxies[2]["name"].as_str().unwrap(), "CustomNode"); // appended
    }

    #[test]
    fn test_extract_override_proxies_and_providers() {
        let override_val: serde_yaml::Value = serde_yaml::from_str(
            r#"
            proxies-append:
              - name: SG-corp-host
                type: socks5
            proxy-providers:
              corp-host:
                type: inline
                payload:
                  - name: SG-corp-host
                    type: socks5
            "#,
        )
        .unwrap();

        let proxies = extract_override_proxies(&override_val);
        assert_eq!(proxies, vec!["SG-corp-host"]);

        let providers = extract_override_providers(&override_val);
        assert_eq!(providers, vec!["corp-host"]);

        let payload_nodes = extract_inline_provider_payload_nodes(&override_val);
        assert_eq!(
            payload_nodes.get("SG-corp-host").map(|s| s.as_str()),
            Some("corp-host")
        );
    }

    #[test]
    fn test_intranet_guard_linter_flags_dangerous_topology() {
        let dangerous_yaml: serde_yaml::Value = serde_yaml::from_str(
            r#"
sniffer:
  enable: true
  override-destination: true
tun:
  enable: true
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
  - MATCH,Proxy
"#,
        )
        .unwrap();
        let warnings = lint_intranet_guard(&dangerous_yaml);
        assert!(
            !warnings.is_empty(),
            "Dangerous topology must produce an Intranet Guard warning"
        );
        assert!(warnings[0].message.contains("Sniffer"));

        let safe_with_exclude: serde_yaml::Value = serde_yaml::from_str(
            r#"
sniffer:
  enable: true
  override-destination: true
tun:
  enable: true
  route-exclude-address:
    - 10.0.0.0/8
    - 172.16.0.0/12
    - 192.168.0.0/16
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
  - MATCH,Proxy
"#,
        )
        .unwrap();
        assert!(lint_intranet_guard(&safe_with_exclude).is_empty());

        let safe_with_direct_match: serde_yaml::Value = serde_yaml::from_str(
            r#"
sniffer:
  enable: true
  override-destination: true
tun:
  enable: true
rules:
  - MATCH,DIRECT
"#,
        )
        .unwrap();
        assert!(lint_intranet_guard(&safe_with_direct_match).is_empty());
    }
}

#[cfg(test)]
mod public_sync_063_fix_tests {
    use super::*;

    fn guard_config(excludes: &str) -> serde_yaml::Value {
        serde_yaml::from_str(&format!(
            r#"
sniffer:
  enable: true
  override-destination: true
tun:
  enable: true
  route-exclude-address: {excludes}
rules:
  - MATCH,Proxy
"#
        ))
        .unwrap()
    }

    #[test]
    fn intranet_guard_requires_tun_enabled() {
        let no_tun: serde_yaml::Value = serde_yaml::from_str(
            r#"
sniffer:
  enable: true
  override-destination: true
rules:
  - MATCH,Proxy
"#,
        )
        .unwrap();
        assert!(lint_intranet_guard(&no_tun).is_empty());

        let tun_disabled: serde_yaml::Value = serde_yaml::from_str(
            r#"
sniffer:
  enable: true
  override-destination: true
tun:
  enable: false
rules:
  - MATCH,Proxy
"#,
        )
        .unwrap();
        assert!(lint_intranet_guard(&tun_disabled).is_empty());
    }

    #[test]
    fn intranet_guard_rejects_non_private_172_range() {
        let config = guard_config("[10.0.0.0/8, 172.64.0.0/13, 192.168.0.0/16]");
        assert!(
            !lint_intranet_guard(&config).is_empty(),
            "172.64.0.0/13 is not RFC1918 and must not suppress the warning"
        );
    }

    #[test]
    fn intranet_guard_rejects_partial_private_range() {
        let config = guard_config("[10.0.0.0/9, 172.16.0.0/12, 192.168.0.0/16]");
        assert!(
            !lint_intranet_guard(&config).is_empty(),
            "10.0.0.0/9 alone does not cover the whole 10.0.0.0/8 private range"
        );
    }

    #[test]
    fn intranet_guard_accepts_supernet_and_union_coverage() {
        let supernet = guard_config("[10.0.0.0/8, 172.0.0.0/8, 192.0.0.0/8]");
        assert!(lint_intranet_guard(&supernet).is_empty());
        let union = guard_config("[10.0.0.0/9, 10.128.0.0/9, 172.16.0.0/12, 192.168.0.0/16]");
        assert!(lint_intranet_guard(&union).is_empty());
    }

    #[test]
    fn dangling_fallback_leaves_logical_rule_intact() {
        let rule = "AND,((NETWORK,udp),(DST-PORT,443)),REJECT";
        let yaml = format!(
            "proxy-groups:\n  - name: Proxy\n    type: select\n    proxies: [DIRECT]\nrules:\n  - '{rule}'\n  - MATCH,DIRECT\n"
        );
        let output = apply_dangling_rule_fallback_to_config_text(&yaml).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&output).unwrap();
        assert_eq!(value["rules"][0].as_str(), Some(rule));
    }

    #[test]
    fn dangling_fallback_still_rewrites_simple_rules() {
        let yaml = "proxy-groups:\n  - name: Proxy\n    type: select\n    proxies: [DIRECT]\nrules:\n  - DOMAIN-SUFFIX,x.com,Missing\n  - MATCH,Missing\n";
        let output = apply_dangling_rule_fallback_to_config_text(yaml).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&output).unwrap();
        assert_eq!(
            value["rules"][0].as_str(),
            Some("DOMAIN-SUFFIX,x.com,DIRECT")
        );
        assert_eq!(value["rules"][1].as_str(), Some("MATCH,DIRECT"));
    }
}
