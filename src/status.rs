//! 共享状态快照模块
//!
//! 本模块定义 `StatusSnapshot`，统一采集所有用户可见的运行态和配置意图。
//! 所有状态查询命令（`status`、`status --json`、`tun status`、`doctor`）必须消费同一个快照，
//! 不得自行读取 Core API、配置文件或 daemon IPC 以组装状态。

use crate::instance::{InstanceContext, InstanceMode};
use crate::mihomo_api;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 三值状态：可观察的布尔值或 unknown
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriState {
    True,
    False,
    Unknown,
}

impl TriState {
    pub fn from_option(value: Option<bool>) -> Self {
        match value {
            Some(true) => TriState::True,
            Some(false) => TriState::False,
            None => TriState::Unknown,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            TriState::True => Some(true),
            TriState::False => Some(false),
            TriState::Unknown => None,
        }
    }
}

impl std::fmt::Display for TriState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriState::True => write!(f, "true"),
            TriState::False => write!(f, "false"),
            TriState::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum JournalState {
    Prepared,
    PromotionPending,
    SnapshotPromoted,
    CoreApplied,
    RollbackPending,
    IntentCommitted,
    RolledBack,
    RecoveryRequired,
    Unknown,
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TunVerdict {
    TunRunning,
    TunRunningUnattested,
    TunDisabled,
    TunStateUnknown,
}

fn tun_verdict(
    runtime_tun: TriState,
    launched_snapshot_revision: Option<&str>,
    active_snapshot_revision: Option<&str>,
    active_intent_revision: Option<&str>,
    journal_state: JournalState,
    runtime_attested: bool,
) -> TunVerdict {
    if runtime_tun == TriState::False {
        return if matches!(
            journal_state,
            JournalState::Unknown | JournalState::IntentCommitted | JournalState::RolledBack
        ) {
            TunVerdict::TunDisabled
        } else {
            TunVerdict::TunRunningUnattested
        };
    }
    if runtime_tun != TriState::True {
        return TunVerdict::TunStateUnknown;
    }
    let revisions_match = matches!(
        (
            launched_snapshot_revision,
            active_snapshot_revision,
            active_intent_revision,
        ),
        (Some(launched), Some(active), Some(intent)) if launched == active && active == intent
    );
    if revisions_match
        && matches!(
            journal_state,
            JournalState::IntentCommitted | JournalState::Unknown
        )
        && runtime_attested
    {
        TunVerdict::TunRunning
    } else if launched_snapshot_revision.is_some()
        || active_snapshot_revision.is_some()
        || active_intent_revision.is_some()
        || !matches!(
            journal_state,
            JournalState::Unknown | JournalState::RolledBack
        )
    {
        TunVerdict::TunRunningUnattested
    } else {
        TunVerdict::TunStateUnknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationVerdict {
    Applied,
    OutOfDate,
    Unknown,
}

pub fn configuration_verdict(
    intent_exists: bool,
    core_running: TriState,
    api_reachable: bool,
    runtime_tun: TriState,
    runtime_attested: bool,
    launched_revision: Option<&str>,
    intent_revision: Option<&str>,
) -> ConfigurationVerdict {
    if !intent_exists
        || core_running != TriState::True
        || !api_reachable
        || (runtime_tun == TriState::True && !runtime_attested)
        || launched_revision.is_none()
        || intent_revision.is_none()
    {
        return ConfigurationVerdict::Unknown;
    }

    if launched_revision == intent_revision {
        ConfigurationVerdict::Applied
    } else {
        ConfigurationVerdict::OutOfDate
    }
}

/// 状态快照：所有用户可见状态的单一事实源
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSnapshot {
    /// system daemon transport/auth state as observed by its single GetStatus request.
    /// `Unknown` means the request did not yield a valid status response.
    pub daemon_reachable: TriState,
    /// 配置意图：下一次配置应用所请求的 TUN 值
    pub configured_tun: TriState,
    /// 运行态：当前 Core 观察到的 TUN 值（仅来自 Core API `/configs`）
    pub runtime_tun: TriState,
    /// Core 进程状态
    pub core_running: TriState,
    /// Core API 可达性
    pub api_reachable: bool,
    /// 流量模式（仅来自 Core API）
    pub rule_mode: String,
    /// Core PID（仅 daemon 提供）
    pub core_pid: Option<u32>,
    /// 活跃配置文件路径（仅 daemon 提供）
    pub active_config_path: Option<std::path::PathBuf>,
    /// Revision observed for the snapshot used to launch Core.
    pub launched_snapshot_revision: Option<String>,
    /// Revision observed for the active protected system snapshot.
    pub active_snapshot_revision: Option<String>,
    /// Revision observed for the active user intent.
    pub active_intent_revision: Option<String>,
    /// Current Core configuration application verdict.
    pub configuration_verdict: ConfigurationVerdict,
    /// Durable TUN transaction journal state.
    pub journal_state: JournalState,
    /// Diagnostic from the daemon when the active journal is unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_error: Option<String>,
    /// Whether the current Core/API observation is attested to the revisions.
    pub runtime_attested: bool,
    /// User-visible TUN convergence verdict.
    pub tun_verdict: TunVerdict,
    /// 系统代理状态（来自平台原生查询）
    pub system_proxy: crate::system_proxy::SystemProxyState,
    /// Shell 代理状态（来自当前进程环境变量）
    pub shell_proxy: crate::system_proxy::ShellProxyState,
    /// 用户意图配置是否存在
    pub intent_config_exists: bool,
    /// Core 二进制是否存在
    pub core_binary_exists: bool,
}

impl StatusSnapshot {
    /// 采集状态快照
    ///
    /// - system 模式：通过 daemon IPC 获取 Core 进程信息，通过受权 Core API 获取运行态
    /// - user 模式：直接通过 user endpoint 获取 Core API 信息
    pub async fn collect(ctx: &InstanceContext) -> Self {
        // 1. 获取 Core 进程信息（system 模式通过 daemon，user 模式通过 API 探测）
        let (
            daemon_reachable,
            core_running,
            core_pid,
            active_config_path,
            daemon_snapshot_revision,
            launched_config_revision,
            daemon_journal_state,
            daemon_journal_error,
        ) = if ctx.mode == InstanceMode::System {
            match crate::ipc::send_command(&crate::ipc::DaemonCommand::GetStatus { token: None })
                .await
            {
                Ok(crate::ipc::DaemonResponse::Status {
                    running,
                    core_pid,
                    config_path,
                    tun_snapshot_revision,
                    launched_config_revision,
                    tun_journal_state,
                    tun_journal_error,
                    ..
                }) => (
                    TriState::True,
                    TriState::from_option(Some(running)),
                    core_pid,
                    config_path,
                    tun_snapshot_revision,
                    launched_config_revision,
                    tun_journal_state,
                    tun_journal_error,
                ),
                _ => (
                    TriState::Unknown,
                    TriState::Unknown,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
            }
        } else {
            // user 模式：尝试连接 API 判断 Core 是否运行
            match mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs").await {
                Ok(_) => (
                    TriState::Unknown,
                    TriState::True,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
                Err(_) => (
                    TriState::Unknown,
                    TriState::Unknown,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                ),
            }
        };

        // 2. 获取 Core API 运行态（仅当 Core 运行时）
        let (runtime_tun, rule_mode, api_reachable) =
            if core_running.as_bool() == Some(true) || ctx.mode == InstanceMode::User {
                match mihomo_api::get_config_for_instance(ctx.mode, &ctx.paths.api_endpoint).await {
                    Ok(config) => {
                        let tun = config["tun"]["enable"].as_bool();
                        let mode = config["mode"]
                            .as_str()
                            .map(|m| m.to_ascii_lowercase())
                            .filter(|m| matches!(m.as_str(), "rule" | "global" | "direct"))
                            .unwrap_or_else(|| "unknown".to_string());
                        (TriState::from_option(tun), mode, true)
                    }
                    Err(_) => (TriState::Unknown, "unknown".to_string(), false),
                }
            } else {
                (TriState::Unknown, "unknown".to_string(), false)
            };

        // 3. 读取用户配置意图。活动运行时快照是派生输入，不得反向作为 intent。
        let configured_tun = read_configured_tun_intent(&ctx.paths.intent_config_file);

        // 4. 读取事务证据，只有完整一致时才将运行态提升为已证明。
        let (
            launched_snapshot_revision,
            active_snapshot_revision,
            active_intent_revision,
            journal_state,
            runtime_attested,
        ) = if ctx.mode == InstanceMode::System {
            let revision = |path: &Path| {
                crate::utils::open_regular_file_no_follow(path)
                    .ok()
                    .and_then(|mut file| {
                        let mut bytes = Vec::new();
                        std::io::Read::read_to_end(&mut file, &mut bytes)
                            .ok()
                            .map(|_| crate::tun_transaction::content_revision(&bytes))
                    })
            };
            let launched = launched_config_revision.clone();
            let active = daemon_snapshot_revision.clone();
            let intent = revision(&ctx.paths.intent_config_file);
            let journal = crate::tun_transaction::read_active_journal(ctx)
                .ok()
                .flatten();
            let journal_state = daemon_journal_state
                .map(|phase| match phase {
                    crate::tun_transaction::JournalPhase::Prepared => JournalState::Prepared,
                    crate::tun_transaction::JournalPhase::PromotionPending => {
                        JournalState::PromotionPending
                    }
                    crate::tun_transaction::JournalPhase::SnapshotPromoted => {
                        JournalState::SnapshotPromoted
                    }
                    crate::tun_transaction::JournalPhase::CoreApplied => JournalState::CoreApplied,
                    crate::tun_transaction::JournalPhase::RollbackPending => {
                        JournalState::RollbackPending
                    }
                    crate::tun_transaction::JournalPhase::IntentCommitted => {
                        JournalState::IntentCommitted
                    }
                    crate::tun_transaction::JournalPhase::RolledBack => JournalState::RolledBack,
                    crate::tun_transaction::JournalPhase::RecoveryRequired => {
                        JournalState::RecoveryRequired
                    }
                })
                .or_else(|| {
                    journal.as_ref().map(|journal| match journal.phase {
                        crate::tun_transaction::JournalPhase::Prepared => JournalState::Prepared,
                        crate::tun_transaction::JournalPhase::PromotionPending => {
                            JournalState::PromotionPending
                        }
                        crate::tun_transaction::JournalPhase::SnapshotPromoted => {
                            JournalState::SnapshotPromoted
                        }
                        crate::tun_transaction::JournalPhase::CoreApplied => {
                            JournalState::CoreApplied
                        }
                        crate::tun_transaction::JournalPhase::RollbackPending => {
                            JournalState::RollbackPending
                        }
                        crate::tun_transaction::JournalPhase::IntentCommitted => {
                            JournalState::IntentCommitted
                        }
                        crate::tun_transaction::JournalPhase::RolledBack => {
                            JournalState::RolledBack
                        }
                        crate::tun_transaction::JournalPhase::RecoveryRequired => {
                            JournalState::RecoveryRequired
                        }
                    })
                })
                .unwrap_or(JournalState::Unknown);
            let attested = runtime_tun == TriState::True
                && core_running.as_bool() == Some(true)
                && api_reachable
                && launched.is_some()
                && launched == active
                && active == intent
                && matches!(
                    journal_state,
                    JournalState::IntentCommitted | JournalState::Unknown
                );
            (launched, active, intent, journal_state, attested)
        } else {
            (None, None, None, JournalState::Unknown, false)
        };

        let tun_verdict = tun_verdict(
            runtime_tun,
            launched_snapshot_revision.as_deref(),
            active_snapshot_revision.as_deref(),
            active_intent_revision.as_deref(),
            journal_state,
            runtime_attested,
        );
        let configuration_verdict = configuration_verdict(
            ctx.paths.intent_config_file.exists(),
            core_running,
            api_reachable,
            runtime_tun,
            runtime_attested,
            launched_config_revision.as_deref(),
            active_intent_revision.as_deref(),
        );

        // 5. 查询系统代理和 shell 代理状态
        let system_proxy = crate::system_proxy::query_system_proxy();
        let shell_proxy = crate::system_proxy::query_shell_proxy();

        StatusSnapshot {
            daemon_reachable,
            configured_tun,
            runtime_tun,
            core_running,
            api_reachable,
            rule_mode,
            core_pid,
            active_config_path,
            launched_snapshot_revision,
            active_snapshot_revision,
            active_intent_revision,
            configuration_verdict,
            journal_state,
            journal_error: daemon_journal_error,
            runtime_attested,
            tun_verdict,
            system_proxy,
            shell_proxy,
            intent_config_exists: ctx.paths.intent_config_file.exists(),
            core_binary_exists: ctx.paths.core_binary.exists(),
        }
    }
}

/// 读取用户配置中声明的 TUN 目标状态。
///
/// 运行时快照是从 intent 派生的受保护输入，不得作为 intent 的替代来源。
fn read_configured_tun_intent(intent_config_path: &Path) -> TriState {
    read_tun_from_config(intent_config_path)
}

/// 安全读取配置文件中的 TUN 意图
///
/// 使用 no-follow regular-file 边界，拒绝 symlink、hardlink、非普通文件。
/// 读取失败或字段缺失返回 `TriState::Unknown`。
pub fn read_tun_from_config(path: &Path) -> TriState {
    let file = match crate::utils::open_regular_file_no_follow(path) {
        Ok(f) => f,
        Err(_) => return TriState::Unknown,
    };
    let config: serde_yaml::Value = match serde_yaml::from_reader(file) {
        Ok(c) => c,
        Err(_) => return TriState::Unknown,
    };
    TriState::from_option(config["tun"]["enable"].as_bool())
}

/// 检查配置意图与运行态是否一致
///
/// 只有两者均可观察且不同时，才报告不一致。
/// 任一值为 unknown 时，返回 `TriState::Unknown`。
pub fn check_tun_consistency(snapshot: &StatusSnapshot) -> TriState {
    match (
        snapshot.configured_tun.as_bool(),
        snapshot.runtime_tun.as_bool(),
    ) {
        (Some(config), Some(runtime)) => {
            if config == runtime {
                TriState::True
            } else {
                TriState::False
            }
        }
        _ => TriState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tun_verdict_requires_matching_revisions_and_an_attested_terminal_state() {
        assert_eq!(
            tun_verdict(
                TriState::True,
                Some("r1"),
                Some("r1"),
                Some("r1"),
                JournalState::IntentCommitted,
                true,
            ),
            TunVerdict::TunRunning
        );
        assert_eq!(
            tun_verdict(
                TriState::True,
                Some("r1"),
                Some("r1"),
                Some("r1"),
                JournalState::Unknown,
                true,
            ),
            TunVerdict::TunRunning
        );
    }

    #[test]
    fn missing_launched_revision_cannot_attest_running_tun() {
        assert_eq!(
            tun_verdict(
                TriState::True,
                None,
                Some("r1"),
                Some("r1"),
                JournalState::IntentCommitted,
                false,
            ),
            TunVerdict::TunRunningUnattested
        );
    }

    #[test]
    fn mismatched_launched_revision_cannot_attest_running_tun() {
        assert_eq!(
            tun_verdict(
                TriState::True,
                Some("launched"),
                Some("active"),
                Some("active"),
                JournalState::IntentCommitted,
                false,
            ),
            TunVerdict::TunRunningUnattested
        );
    }

    #[test]
    fn enabled_runtime_without_attestation_is_unattested() {
        assert_eq!(
            tun_verdict(
                TriState::True,
                Some("r1"),
                Some("r1"),
                None,
                JournalState::IntentCommitted,
                true,
            ),
            TunVerdict::TunRunningUnattested
        );
    }

    #[test]
    fn unknown_runtime_or_revision_is_unknown() {
        assert_eq!(
            tun_verdict(
                TriState::Unknown,
                None,
                None,
                None,
                JournalState::Unknown,
                false,
            ),
            TunVerdict::TunStateUnknown
        );
    }

    #[test]
    fn disabled_runtime_with_pending_journal_requires_recovery() {
        for journal_state in [
            JournalState::Prepared,
            JournalState::PromotionPending,
            JournalState::SnapshotPromoted,
            JournalState::CoreApplied,
            JournalState::RollbackPending,
            JournalState::RecoveryRequired,
        ] {
            assert_eq!(
                tun_verdict(TriState::False, None, None, None, journal_state, false,),
                TunVerdict::TunRunningUnattested,
                "journal state {journal_state:?} must not be hidden by disabled runtime",
            );
        }
    }

    #[test]
    fn disabled_runtime_is_not_running_when_observed() {
        assert_eq!(
            tun_verdict(
                TriState::False,
                Some("r1"),
                Some("r1"),
                Some("r1"),
                JournalState::IntentCommitted,
                true,
            ),
            TunVerdict::TunDisabled
        );
    }

    #[test]
    fn tri_state_from_option() {
        assert_eq!(TriState::from_option(Some(true)), TriState::True);
        assert_eq!(TriState::from_option(Some(false)), TriState::False);
        assert_eq!(TriState::from_option(None), TriState::Unknown);
    }

    #[test]
    fn tri_state_as_bool() {
        assert_eq!(TriState::True.as_bool(), Some(true));
        assert_eq!(TriState::False.as_bool(), Some(false));
        assert_eq!(TriState::Unknown.as_bool(), None);
    }

    #[test]
    fn tri_state_display() {
        assert_eq!(format!("{}", TriState::True), "true");
        assert_eq!(format!("{}", TriState::False), "false");
        assert_eq!(format!("{}", TriState::Unknown), "unknown");
    }

    #[test]
    fn missing_config_reports_unknown_tun_intent() {
        let path = std::env::temp_dir().join(format!(
            "mihomo-status-missing-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        assert_eq!(read_tun_from_config(&path), TriState::Unknown);
    }

    #[test]
    fn configured_tun_reads_user_intent_not_runtime_snapshot() {
        let dir = std::env::temp_dir().join(format!(
            "mihomo-status-intent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let intent = dir.join("config.yaml");
        let runtime_snapshot = dir.join("tun-config.yaml");
        std::fs::write(&intent, "tun:\n  enable: false\n").unwrap();
        std::fs::write(&runtime_snapshot, "tun:\n  enable: true\n").unwrap();

        assert_eq!(
            read_configured_tun_intent(&intent),
            TriState::False,
            "configured intent must not be inferred from the opposite runtime snapshot"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn normal_configuration_requires_matching_api_ready_launch_receipt() {
        assert_eq!(
            configuration_verdict(
                true,
                TriState::True,
                true,
                TriState::False,
                false,
                Some("r1"),
                Some("r1"),
            ),
            ConfigurationVerdict::Applied
        );
        assert_eq!(
            configuration_verdict(
                true,
                TriState::True,
                true,
                TriState::False,
                false,
                Some("old"),
                Some("new"),
            ),
            ConfigurationVerdict::OutOfDate
        );
        assert_eq!(
            configuration_verdict(
                true,
                TriState::True,
                true,
                TriState::False,
                false,
                None,
                Some("r1"),
            ),
            ConfigurationVerdict::Unknown
        );
        assert_eq!(
            configuration_verdict(
                true,
                TriState::True,
                true,
                TriState::True,
                false,
                Some("r1"),
                Some("r1"),
            ),
            ConfigurationVerdict::Unknown
        );
    }

    #[test]
    fn check_tun_consistency_both_true() {
        let snapshot = StatusSnapshot {
            daemon_reachable: TriState::True,
            configured_tun: TriState::True,
            runtime_tun: TriState::True,
            core_running: TriState::True,
            api_reachable: true,
            rule_mode: "rule".to_string(),
            core_pid: None,
            active_config_path: None,
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: ConfigurationVerdict::Unknown,
            journal_state: JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: TunVerdict::TunStateUnknown,
            system_proxy: crate::system_proxy::SystemProxyState::Disabled,
            shell_proxy: crate::system_proxy::ShellProxyState::NotConfigured,
            intent_config_exists: true,
            core_binary_exists: true,
        };
        assert_eq!(check_tun_consistency(&snapshot), TriState::True);
    }

    #[test]
    fn check_tun_consistency_mismatch() {
        let snapshot = StatusSnapshot {
            daemon_reachable: TriState::True,
            configured_tun: TriState::True,
            runtime_tun: TriState::False,
            core_running: TriState::True,
            api_reachable: true,
            rule_mode: "rule".to_string(),
            core_pid: None,
            active_config_path: None,
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: ConfigurationVerdict::Unknown,
            journal_state: JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: TunVerdict::TunStateUnknown,
            system_proxy: crate::system_proxy::SystemProxyState::Disabled,
            shell_proxy: crate::system_proxy::ShellProxyState::NotConfigured,
            intent_config_exists: true,
            core_binary_exists: true,
        };
        assert_eq!(check_tun_consistency(&snapshot), TriState::False);
    }

    #[test]
    fn check_tun_consistency_unknown() {
        let snapshot = StatusSnapshot {
            daemon_reachable: TriState::Unknown,
            configured_tun: TriState::True,
            runtime_tun: TriState::Unknown,
            core_running: TriState::False,
            api_reachable: false,
            rule_mode: "unknown".to_string(),
            core_pid: None,
            active_config_path: None,
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: ConfigurationVerdict::Unknown,
            journal_state: JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: TunVerdict::TunStateUnknown,
            system_proxy: crate::system_proxy::SystemProxyState::Disabled,
            shell_proxy: crate::system_proxy::ShellProxyState::NotConfigured,
            intent_config_exists: true,
            core_binary_exists: true,
        };
        assert_eq!(check_tun_consistency(&snapshot), TriState::Unknown);
    }
}
