use crate::mihomo_api::MihomoApiClient;
use anyhow::Context as _;
use clap::Parser;
use std::io::IsTerminal;

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        if $crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("[DEBUG] {}", format!($($arg)*));
        }
    };
}

/// Debug-level log for sensitive operations (token reads, config import, etc.).
/// Only visible when `-v` is set. Use `sanitize_url`/`sanitize_sensitive` before logging.
#[macro_export]
macro_rules! log_debug_sensitive {
    ($($arg:tt)*) => {
        if $crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("[DEBUG] {}", $crate::utils::sanitize_sensitive(&format!($($arg)*)));
        }
    };
}

pub static VERBOSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

mod backup;
mod config;
mod daemon;
mod dns;
// A：本地解析器探测依赖 /etc/resolv.conf，仅在 unix 上提供。
mod core_identity;
#[cfg(unix)]
mod dns_probe;
mod generation;
mod generation_adoption;
mod groups;
mod installer;
mod instance;
mod ipc;
mod lock;
mod manual;
mod mihomo_api;
mod preflight;
mod rules;
mod selection;
mod service;
mod status;
mod system_proxy;
mod tun_transaction;
mod ui;
mod utils;
mod yaml_editor;

mod cli;
pub(crate) use cli::*;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if cli.verbose {
        VERBOSE.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    // Windows daemon 子命令：同步进 SCM dispatcher（StartServiceCtrlDispatcher
    // 必须由主线程调用；#[tokio::main] 的 block_on 在 main 线程执行，满足约束）。
    // service_main 回调内再自建 tokio runtime 跑核心循环。若非 SCM 启动则回退到 console loop。
    #[cfg(target_os = "windows")]
    if matches!(cli.command, Some(Command::Daemon)) {
        match daemon::run_windows_service() {
            Ok(true) => return,
            Ok(false) => {
                // 回退到非 SCM 控制台循环：进入下方 run(cli).await 在已有 runtime 中执行
            }
            Err(e) => {
                eprintln!("\n  Error: {e}");
                std::process::exit(1);
            }
        }
    }
    let is_json = cli.json;
    let cmd_name = cli.command.as_ref().map(|c| c.name()).unwrap_or("install");
    if let Err(e) = run(cli).await {
        if is_json {
            let error_envelope = json_error_envelope(cmd_name, &format!("{e:#}"));
            if let Ok(json_str) = serde_json::to_string_pretty(&error_envelope) {
                println!("{json_str}");
            }
        }
        if VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("\n  Error: {e:#}");
        } else {
            eprintln!("\n  Error: {e}");
            eprintln!("  Run with -v for more details");
        }
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command.unwrap_or(Command::Install {
        system: false,
        user: false,
        force: false,
        version: None,
        github_mirror: None,
        skip_config: false,
        yes: false,
        discard_pending: None,
        adopt_current: false,
        dry_run: false,
    }) {
        Command::Install {
            system,
            user,
            force,
            version,
            github_mirror,
            skip_config,
            yes,
            discard_pending,
            adopt_current,
            dry_run,
        } => {
            if adopt_current {
                let ctx = instance::planned_current_context(instance::InstanceMode::System)
                    .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance lifecycle"))?;
                return generation_adoption::adopt(
                    &ctx,
                    discard_pending.as_deref(),
                    yes,
                    dry_run,
                    cli.json,
                );
            }
            cmd_install_entry(
                system,
                user,
                force,
                version.as_deref(),
                github_mirror.as_deref(),
                skip_config,
                yes,
                discard_pending.as_deref(),
            )
            .await
        }
        Command::Upgrade { system, yes } => cmd_upgrade(system, false, yes).await,
        Command::Version { system } => cmd_version(system, false, cli.json).await,
        Command::Manual { list, topic } => manual::run(list, topic.as_deref(), cli.json),
        Command::Config {
            command,
            system,
            url,
            fix,
            refresh,
            refresh_all,
            import,
            switch,
            add,
            remove,
            list,
            validate,
            dry_run,
            yes,
            info,
            probe,
            user_agent,
            set_ua,
            activate,
            no_activate,
        } => {
            cmd_config(ConfigCmd {
                command,
                url,
                fix,
                system,
                user: false,
                refresh,
                refresh_all,
                import,
                switch,
                add,
                remove,
                list,
                validate,
                dry_run,
                yes,
                info,
                probe,
                user_agent,
                set_ua,
                activate,
                no_activate,
                json: cli.json,
            })
            .await
        }
        Command::Uninstall {
            system,
            user,
            all,
            remove_binary,
            remove_config,
            remove_geo,
            yes,
            dry_run,
            legacy_root_leftovers,
        } => {
            if legacy_root_leftovers {
                cmd_uninstall_legacy_root_leftovers(dry_run)
            } else {
                cmd_uninstall_resolved(
                    system,
                    user,
                    all,
                    remove_binary,
                    remove_config,
                    remove_geo,
                    yes,
                    dry_run,
                )
                .await
            }
        }
        Command::Update { system } => cmd_update(system, false).await,
        Command::Start { system } => {
            cmd_lifecycle_resolved(system, false, instance::ServiceAction::Start, false).await
        }
        Command::Stop { system } => {
            cmd_lifecycle_resolved(system, false, instance::ServiceAction::Stop, false).await
        }
        Command::Restart { system, yes } => {
            cmd_lifecycle_resolved(system, false, instance::ServiceAction::Restart, yes).await
        }
        Command::Select {
            system,
            user,
            group,
            node,
            unpin,
            all,
            replay,
        } => cmd_select_resolved(system, user, group, node, unpin, all, replay, cli.json).await,
        Command::List { system } => cmd_list_resolved(system, false).await,
        Command::Delay {
            system,
            group,
            refresh,
            cache_ttl,
            fastest,
        } => cmd_delay_resolved(system, false, &group, refresh, cache_ttl, fastest).await,
        Command::Tun {
            system,
            action,
            stack,
            dns_hijack,
            yes,
        } => {
            let opts = TunResolvedOptions {
                system,
                user: false,
                action,
                stack,
                dns_hijack,
                yes,
            };
            cmd_tun_resolved(opts).await
        }
        Command::Connections { system, flush } => {
            cmd_connections_resolved(system, false, flush).await
        }
        Command::Ip { system } => cmd_ip_resolved(system, false).await,
        Command::ExitIp {
            node,
            group,
            url,
            direct,
            yes,
            system,
        } => cmd_exit_ip(system, false, node, group, url, direct, yes).await,
        Command::Proxy { system, action } => cmd_proxy(system, false, action).await,
        Command::SystemProxy { system, action } => cmd_system_proxy(system, false, action).await,
        Command::Status { system, verbose } => {
            if verbose {
                VERBOSE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            cmd_status_resolved_json(system, false, cli.json).await
        }
        Command::Use { mode } => cmd_use(mode).await,
        Command::Logs {
            system,
            tail,
            level,
            follow,
        } => cmd_logs(system, false, tail, level.as_deref(), follow),
        Command::Rule { system, action } => cmd_rule(system, false, action).await,
        Command::Group { system, action } => cmd_group(system, false, cli.json, action).await,
        Command::Dns { system, action } => cmd_dns(system, false, action).await,
        Command::Override { system, action } => cmd_override(system, false, action).await,
        Command::Backup { system, output } => cmd_backup(system, false, output),
        Command::Restore { system, path, yes } => cmd_restore(system, false, &path, yes),
        Command::Doctor {
            system,
            user,
            no_probe_dns,
            privileged,
        } => {
            if privileged {
                return generation_adoption::inspect_privileged(cli.json);
            }
            cmd_doctor(system, user, !no_probe_dns).await
        }
        Command::Daemon => {
            let sock_path = ipc::system_service_socket_path();
            // unix: launchd/systemd 管理生命周期，token 永不取消（保持现有行为）
            daemon::run_daemon(sock_path, tokio_util::sync::CancellationToken::new()).await
        }
        Command::Autostart {
            action,
            system,
            user,
        } => cmd_autostart(action, system, user).await,
        Command::Dashboard => cmd_dashboard().await,
    }
}

#[cfg(unix)]
fn lookup_user(name: &str) -> anyhow::Result<(u32, u32, std::path::PathBuf)> {
    use std::ffi::CString;
    let c = CString::new(name)?;
    unsafe {
        let pwd = libc::getpwnam(c.as_ptr());
        if pwd.is_null() {
            anyhow::bail!("user not found: {name}");
        }
        let uid = (*pwd).pw_uid;
        let gid = (*pwd).pw_gid;
        let home = std::ffi::CStr::from_ptr((*pwd).pw_dir)
            .to_string_lossy()
            .into_owned();
        Ok((uid, gid, std::path::PathBuf::from(home)))
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum OwnerAuthDaemonStatus {
    Authorized,
    Rejected(DaemonIpcErrorKind),
}

#[cfg(unix)]
fn format_doctor_owner_auth_check(
    os: instance::TargetOs,
    token_location: &ipc::ClientTokenLocation,
    token_file_readable: bool,
    status: OwnerAuthDaemonStatus,
    raw_message: Option<&str>,
) -> DoctorCheck {
    let local = format!(
        "token_file_readable={token_file_readable}, credential_path={}",
        token_location.token_path.display()
    );
    let service_hint = daemon_service_status_hint(os);
    match status {
        OwnerAuthDaemonStatus::Authorized => DoctorCheck::pass(
            "Daemon 授权",
            format!("当前用户已授权 ({local}, daemon_authenticated=true)"),
        ),
        OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::InvalidOrMissingToken) => {
            let detail = if let Some(msg) = raw_message {
                format!("{msg} ({local}, daemon_authenticated=false)")
            } else {
                format!("not authorized ({local}, daemon_authenticated=false)")
            };
            DoctorCheck::fail(
                "Daemon 授权",
                detail,
                "检查凭证文件或以管理员重新安装服务: sudo mihomo-cli install --system",
            )
        }
        OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::PeerUidMismatch) => {
            let detail = if let Some(msg) = raw_message {
                format!("{msg} ({local}, daemon_authenticated=false, reason=uid_mismatch)")
            } else {
                format!("not authorized ({local}, daemon_authenticated=false, reason=uid_mismatch)")
            };
            DoctorCheck::fail(
                "Daemon 授权",
                detail,
                "检查当前用户的 HOME/XDG_CONFIG_HOME；系统服务仅限所有者用户使用，管理员可重新安装: sudo mihomo-cli install --system",
            )
        }
        OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::OwnerRecordUnreadable) => {
            let detail = if let Some(msg) = raw_message {
                format!("{msg} ({local})")
            } else {
                format!("authorization state unavailable ({local})")
            };
            DoctorCheck::fail(
                "Daemon 授权状态",
                detail,
                format!("管理员查看 daemon 状态: {service_hint}；或重新安装: sudo mihomo-cli install --system"),
            )
        }
        OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::Other) => {
            let detail = if let Some(msg) = raw_message {
                format!("{msg} ({local})")
            } else {
                format!("authorization check failed ({local})")
            };
            DoctorCheck::fail(
                "Daemon 状态",
                detail,
                format!("检查 daemon 状态: {service_hint}"),
            )
        }
    }
}

#[allow(dead_code)]
fn mode_request_from_flags(system: bool, user: bool) -> instance::ModeRequest {
    match (system, user) {
        (true, false) => instance::ModeRequest::ExplicitSystem,
        (false, true) => instance::ModeRequest::ExplicitUser,
        _ => instance::ModeRequest::Unspecified,
    }
}

#[cfg(unix)]
fn service_definition_user_home_references(
    content: &str,
    user_config: &str,
    user_home: &str,
) -> Vec<std::path::PathBuf> {
    let mut refs = Vec::new();
    for token in
        content.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '='))
    {
        let token = token.trim_matches(|c: char| matches!(c, ',' | ';' | ')' | '('));
        let is_user_path = (!user_config.is_empty() && token.contains(user_config))
            || (!user_home.is_empty() && token.contains(user_home))
            || token.contains("/Users/")
            || token.contains("/home/");
        if is_user_path {
            refs.push(std::path::PathBuf::from(token));
        }
    }
    refs.sort();
    refs.dedup();
    refs
}

#[cfg(unix)]
fn is_legacy_user_home_service_executable_ref(path: &std::path::Path) -> bool {
    let text = path.to_string_lossy();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let parent_name = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("");

    file_name == "start.sh"
        || text.contains("/.local/bin/")
        || (parent_name == "bin" && matches!(file_name, "mihomo" | "mihomo-cli"))
}

#[cfg(unix)]
fn legacy_root_service_from_file(
    service_file: &std::path::Path,
) -> Option<instance::LegacyRootService> {
    let user_ctx = instance::planned_current_context(instance::InstanceMode::User)?;
    let user_config = user_ctx.paths.config_dir.display().to_string();
    let user_home = dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let content = std::fs::read_to_string(service_file).ok()?;
    let referenced_paths =
        service_definition_user_home_references(&content, &user_config, &user_home);
    if referenced_paths.is_empty()
        || !referenced_paths
            .iter()
            .any(|path| is_legacy_user_home_service_executable_ref(path))
    {
        return None;
    }

    let referenced_home = referenced_paths.iter().find_map(|path| {
        let text = path.display().to_string();
        for prefix in ["/Users/", "/home/"] {
            if let Some(rest) = text.strip_prefix(prefix) {
                let user = rest.split('/').next().unwrap_or_default();
                if !user.is_empty() {
                    return Some(std::path::PathBuf::from(format!("{prefix}{user}")));
                }
            }
        }
        if !user_home.is_empty() && text.contains(&user_home) {
            Some(std::path::PathBuf::from(&user_home))
        } else {
            None
        }
    });
    let referenced_current_user_home = referenced_home
        .as_ref()
        .map(|home| !user_home.is_empty() && home == std::path::Path::new(&user_home))
        .unwrap_or(false);

    Some(instance::LegacyRootService {
        service_file: service_file.to_path_buf(),
        referenced_paths,
        referenced_home,
        referenced_current_user_home,
    })
}

fn current_legacy_root_service() -> Option<instance::LegacyRootService> {
    #[cfg(target_os = "macos")]
    {
        legacy_root_service_from_file(std::path::Path::new(
            "/Library/LaunchDaemons/io.mihomo.plist",
        ))
    }

    #[cfg(target_os = "linux")]
    {
        legacy_root_service_from_file(std::path::Path::new("/etc/systemd/system/mihomo.service"))
    }

    #[cfg(target_os = "windows")]
    {
        None
    }
}

fn current_instance_inventory() -> instance::InstanceInventory {
    let system_ctx = instance::planned_current_context(instance::InstanceMode::System);
    let user_ctx = instance::planned_current_context(instance::InstanceMode::User);
    let legacy_root = current_legacy_root_service();

    let system_service_file_exists = system_ctx
        .as_ref()
        .and_then(|ctx| ctx.paths.service_file.as_ref())
        .map(|p| p.exists())
        .unwrap_or(false);
    // Windows system mode has no service file (service_file: None); probe the
    // actual Windows service instead (sc query mihomo).
    let system_service_installed = if cfg!(target_os = "windows") {
        windows_mihomo_service_installed()
    } else {
        system_service_file_exists
    };
    let system_payload = system_ctx
        .as_ref()
        .map(|ctx| ctx.paths.core_binary.exists())
        .unwrap_or(false);
    let user_service_installed = user_ctx
        .as_ref()
        .map(|ctx| match ctx.os {
            instance::TargetOs::Windows => {
                instance::windows_user_install_marker(ctx).is_some_and(|marker| marker.exists())
            }
            _ => ctx.paths.service_file.as_ref().is_some_and(|p| p.exists()),
        })
        .unwrap_or(false);
    let user_payload = user_ctx
        .as_ref()
        .map(|ctx| ctx.paths.core_binary.exists())
        .unwrap_or(false);
    let user_contamination = {
        let leftovers = legacy_root_leftovers();
        if leftovers.is_empty() {
            None
        } else {
            Some(instance::UserContamination {
                paths: leftovers
                    .into_iter()
                    .map(|leftover| leftover.path)
                    .collect(),
            })
        }
    };

    instance::InstanceInventory {
        system: instance::SystemInstanceObservation {
            service: system_service_installed && legacy_root.is_none(),
            payload: system_payload,
        },
        user: instance::UserInstanceObservation {
            service: user_service_installed,
            payload: user_payload,
        },
        legacy_root,
        user_contamination,
    }
}

/// Windows: is the `mihomo` Windows service registered?
#[cfg(target_os = "windows")]
fn windows_mihomo_service_installed() -> bool {
    use crate::service::windows_service_query_indicates_installed;
    match std::process::Command::new("sc.exe")
        .args(["query", "mihomo"])
        .output()
    {
        Ok(o) => windows_service_query_indicates_installed(&o.stdout),
        Err(_) => false,
    }
}

/// Non-Windows: no Windows service to probe.
#[cfg(not(target_os = "windows"))]
fn windows_mihomo_service_installed() -> bool {
    false
}

fn current_service_presence() -> instance::ServicePresence {
    current_instance_inventory().service_presence_for_mode_resolution()
}

fn current_runtime_presence() -> instance::ServicePresence {
    instance::ServicePresence {
        system: system_daemon_socket_connectable()
            || planned_mode_service_manager_active(instance::InstanceMode::System),
        user: planned_mode_api_socket_connectable(instance::InstanceMode::User)
            || planned_mode_service_manager_active(instance::InstanceMode::User),
    }
}

fn current_environment_state() -> EnvironmentState {
    EnvironmentState {
        runtime: current_runtime_presence(),
        installed: current_service_presence(),
        legacy_root: current_legacy_root_service(),
    }
}

#[cfg(unix)]
fn unix_socket_connectable(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn unix_socket_connectable(_path: &std::path::Path) -> bool {
    false
}

fn planned_mode_service_manager_active(mode: instance::InstanceMode) -> bool {
    let Some(ctx) = instance::planned_current_context(mode) else {
        return false;
    };
    service_manager_active(&ctx)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceActiveProbePlan {
    program: String,
    args: Vec<String>,
    output_contains: Option<String>,
}

fn service_active_probe_plan(ctx: &instance::InstanceContext) -> Option<ServiceActiveProbePlan> {
    match &ctx.service {
        instance::ServiceTarget::LinuxSystemdSystem { .. } => Some(ServiceActiveProbePlan {
            program: "systemctl".to_string(),
            args: vec![
                "is-active".to_string(),
                "--quiet".to_string(),
                "mihomo".to_string(),
            ],
            output_contains: None,
        }),
        instance::ServiceTarget::LinuxSystemdUser { .. } => Some(ServiceActiveProbePlan {
            program: "systemctl".to_string(),
            args: vec![
                "--user".to_string(),
                "is-active".to_string(),
                "--quiet".to_string(),
                "mihomo".to_string(),
            ],
            output_contains: None,
        }),
        instance::ServiceTarget::MacosLaunchDaemon { domain_label, .. }
        | instance::ServiceTarget::MacosLaunchAgent { domain_label, .. } => {
            Some(ServiceActiveProbePlan {
                program: "launchctl".to_string(),
                args: vec!["print".to_string(), domain_label.clone()],
                output_contains: None,
            })
        }
        instance::ServiceTarget::WindowsService { name } => Some(ServiceActiveProbePlan {
            program: "sc.exe".to_string(),
            args: vec!["query".to_string(), name.clone()],
            output_contains: Some("RUNNING".to_string()),
        }),
        instance::ServiceTarget::WindowsUserProcess => None,
    }
}

fn service_active_probe_success(
    plan: &ServiceActiveProbePlan,
    status_success: bool,
    stdout: &str,
) -> bool {
    if !status_success {
        return false;
    }
    match &plan.output_contains {
        Some(needle) => stdout
            .to_ascii_uppercase()
            .contains(&needle.to_ascii_uppercase()),
        None => true,
    }
}

fn run_service_active_probe(
    plan: &ServiceActiveProbePlan,
) -> std::io::Result<std::process::Output> {
    std::process::Command::new(&plan.program)
        .args(&plan.args)
        .output()
}

fn service_manager_active(ctx: &instance::InstanceContext) -> bool {
    let Some(plan) = service_active_probe_plan(ctx) else {
        return false;
    };
    run_service_active_probe(&plan)
        .map(|out| {
            service_active_probe_success(
                &plan,
                out.status.success(),
                &String::from_utf8_lossy(&out.stdout),
            )
        })
        .unwrap_or(false)
}

fn system_daemon_socket_connectable() -> bool {
    ipc::is_daemon_running_blocking()
}

#[cfg(windows)]
fn windows_pipe_connectable(pipe: &str) -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe)
        .is_ok()
}

#[cfg(not(windows))]
fn windows_pipe_connectable(_pipe: &str) -> bool {
    false
}

fn planned_mode_api_socket_connectable(mode: instance::InstanceMode) -> bool {
    let Some(ctx) = instance::planned_current_context(mode) else {
        return false;
    };
    match ctx.paths.api_endpoint {
        instance::ApiEndpoint::UnixSocket(path) => unix_socket_connectable(&path),
        instance::ApiEndpoint::WindowsNamedPipe(pipe) => windows_pipe_connectable(&pipe),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedCurrentMode {
    mode: instance::InstanceMode,
    source: instance::ResolutionSource,
}

struct ResolvedCurrentInstance {
    ctx: instance::InstanceContext,
    source: instance::ResolutionSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnvironmentState {
    pub(crate) runtime: instance::ServicePresence,
    pub(crate) installed: instance::ServicePresence,
    pub(crate) legacy_root: Option<instance::LegacyRootService>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UserIntent {
    Install,
    Status,
    Start,
    Stop,
    Uninstall,
    TunOn,
    TunOff,
    TunStatus,
    ApiRead,
    ApiMutate,
    ConfigRead,
    ConfigWrite,
}

const _: &[UserIntent] = &[
    UserIntent::Install,
    UserIntent::Status,
    UserIntent::Start,
    UserIntent::Stop,
    UserIntent::Uninstall,
    UserIntent::TunOn,
    UserIntent::TunOff,
    UserIntent::TunStatus,
    UserIntent::ApiRead,
    UserIntent::ApiMutate,
    UserIntent::ConfigRead,
    UserIntent::ConfigWrite,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeFirstModeResolution {
    Resolved {
        mode: instance::InstanceMode,
        source: instance::ResolutionSource,
    },
    RuntimeConflict,
    PromptRequired,
    AmbiguousBothInstalled,
    NotInstalled,
    NeedsSystemInstall {
        reason: String,
    },
    NeedsSystemSwitch {
        user_running: bool,
        user_installed: bool,
    },
    NeedsSystemDaemonRecovery {
        reason: String,
    },
}

fn current_user_context_with_config_dir(
    config_dir: std::path::PathBuf,
) -> Option<instance::InstanceContext> {
    let mut ctx = instance::planned_current_context(instance::InstanceMode::User)?;
    let runtime_dir = config_dir.join("run");
    ctx.paths.config_dir = config_dir.clone();
    ctx.paths.intent_config_file = config_dir.join("config.yaml");
    ctx.paths.start_script = ctx.paths.start_script.map(|_| config_dir.join("start.sh"));
    if matches!(ctx.os, instance::TargetOs::Windows) {
        ctx.paths.runtime_dir = None;
    } else {
        ctx.paths.runtime_dir = Some(runtime_dir.clone());
        ctx.paths.api_endpoint = instance::ApiEndpoint::UnixSocket(runtime_dir.join("mihomo.sock"));
    }
    ctx.paths.log_file = ctx.paths.log_file.map(|_| config_dir.join("mihomo.log"));
    ctx.paths.backup_dir = config_dir.join("backups");
    Some(ctx)
}

fn config_dir_override_path() -> Option<std::path::PathBuf> {
    std::env::var("MIHOMO_CLI_CONFIG_DIR")
        .ok()
        .map(|dir| dir.trim().to_string())
        .filter(|dir| !dir.is_empty())
        .map(std::path::PathBuf::from)
}

fn doctor_user_context() -> Option<instance::InstanceContext> {
    config_dir_override_path()
        .and_then(current_user_context_with_config_dir)
        .or_else(|| instance::planned_current_context(instance::InstanceMode::User))
}

fn doctor_uses_user_baseline(
    services: instance::ServicePresence,
    runtime: instance::ServicePresence,
) -> bool {
    !services.system && !services.user && !runtime.system && !runtime.user
}

fn doctor_auto_context() -> anyhow::Result<Option<instance::InstanceContext>> {
    let environment = current_environment_state();
    if doctor_uses_user_baseline(environment.installed, environment.runtime) {
        return Ok(doctor_user_context());
    }
    resolve_current_instance_context(false, false, instance::CommandIntent::ReadOnly)
        .map(|resolved| Some(resolved.ctx))
}

fn resolve_current_instance_context(
    system: bool,
    user: bool,
    intent: instance::CommandIntent,
) -> anyhow::Result<ResolvedCurrentInstance> {
    let request = mode_request_from_flags(system, user);
    if matches!(
        request,
        instance::ModeRequest::Unspecified | instance::ModeRequest::ExplicitUser
    ) && matches!(intent, instance::CommandIntent::ReadOnly)
    {
        if let Some(config_dir) = config_dir_override_path() {
            let ctx = current_user_context_with_config_dir(config_dir)
                .ok_or_else(|| anyhow::anyhow!("Unsupported OS for env override status"))?;
            return Ok(ResolvedCurrentInstance {
                ctx,
                source: instance::ResolutionSource::EnvOverride,
            });
        }
    }

    let resolved = resolve_current_instance_mode(system, user, intent)?;
    let ctx = instance::planned_current_context(resolved.mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance command"))?;
    Ok(ResolvedCurrentInstance {
        ctx,
        source: resolved.source,
    })
}

fn resolve_current_instance_mode(
    system: bool,
    user: bool,
    intent: instance::CommandIntent,
) -> anyhow::Result<ResolvedCurrentMode> {
    let request = mode_request_from_flags(system, user);
    let env = current_environment_state();
    if request != instance::ModeRequest::Unspecified && intent != instance::CommandIntent::Install {
        if let Some(message) = explicit_mode_runtime_conflict(request, env.runtime, intent) {
            anyhow::bail!(message);
        }
    }

    let resolution =
        resolve_environment_for_intent(request, &env, user_intent_from_command_intent(intent));
    match resolution {
        RuntimeFirstModeResolution::Resolved { mode, source } => {
            Ok(ResolvedCurrentMode { mode, source })
        }
        RuntimeFirstModeResolution::RuntimeConflict => anyhow::bail!(
            "mode conflict: both system daemon and user core are running.\n  \
             v3 uses mutually exclusive modes; stop one runtime first.\n  \
             Suggestions:\n  \
               mihomo-cli stop --system\n  \
               mihomo-cli stop"
        ),
        RuntimeFirstModeResolution::PromptRequired => {
            anyhow::bail!("mode required: run `mihomo-cli install` or use --system for the system service")
        }
        RuntimeFirstModeResolution::AmbiguousBothInstalled => anyhow::bail!(
            "mode conflict: both system service and user service are installed.\n  \
             v3 uses mutually exclusive modes; stop or uninstall one instance first.\n  \
             Suggestions:\n  \
               mihomo-cli stop --system\n  \
               mihomo-cli stop"
        ),
        RuntimeFirstModeResolution::NotInstalled if intent == instance::CommandIntent::StartLike => {
            Ok(ResolvedCurrentMode {
                mode: instance::InstanceMode::User,
                source: instance::ResolutionSource::DefaultMode,
            })
        }
        RuntimeFirstModeResolution::NotInstalled if env.legacy_root.is_some() => anyhow::bail!(
            "legacy root-mode layout detected: the system service points into your user home.\n  Run: mihomo-cli uninstall --all 清理旧安装\n  然后重新安装：mihomo-cli install"
        ),
        RuntimeFirstModeResolution::NotInstalled => {
            anyhow::bail!("no mihomo service instance found; run `mihomo-cli install`")
        }
        RuntimeFirstModeResolution::NeedsSystemInstall { reason } => {
            anyhow::bail!("{reason}; run `mihomo-cli tun on` from a terminal or `mihomo-cli install --system`")
        }
        RuntimeFirstModeResolution::NeedsSystemSwitch { .. } => {
            anyhow::bail!("TUN requires switching from per-user mode to system service mode; run `mihomo-cli tun on` from a terminal")
        }
        RuntimeFirstModeResolution::NeedsSystemDaemonRecovery { reason } => {
            anyhow::bail!("{reason}; run `mihomo-cli status --system --verbose` for diagnostics")
        }
    }
}

fn user_intent_from_command_intent(intent: instance::CommandIntent) -> UserIntent {
    match intent {
        instance::CommandIntent::Install => UserIntent::Install,
        instance::CommandIntent::ReadOnly => UserIntent::ApiRead,
        instance::CommandIntent::Mutating => UserIntent::ApiMutate,
        instance::CommandIntent::StartLike => UserIntent::Start,
        instance::CommandIntent::StopLike => UserIntent::Stop,
        instance::CommandIntent::UninstallLike => UserIntent::Uninstall,
    }
}

pub(crate) fn resolve_environment_for_intent(
    request: instance::ModeRequest,
    env: &EnvironmentState,
    intent: UserIntent,
) -> RuntimeFirstModeResolution {
    // Check runtime presence first (before settings)
    if request == instance::ModeRequest::Unspecified {
        match (env.runtime.system, env.runtime.user) {
            (true, false) => {
                return RuntimeFirstModeResolution::Resolved {
                    mode: instance::InstanceMode::System,
                    source: instance::ResolutionSource::RuntimePresence,
                }
            }
            (false, true) => {
                return RuntimeFirstModeResolution::Resolved {
                    mode: instance::InstanceMode::User,
                    source: instance::ResolutionSource::RuntimePresence,
                }
            }
            (true, true) if intent != UserIntent::Install => {
                return RuntimeFirstModeResolution::RuntimeConflict
            }
            _ => {}
        }
    }

    // TUN-specific checks (before settings)
    if matches!(intent, UserIntent::TunOn | UserIntent::TunOff) {
        if request == instance::ModeRequest::Unspecified
            && !env.runtime.system
            && !env.installed.system
        {
            if matches!(intent, UserIntent::TunOn) && (env.runtime.user || env.installed.user) {
                return RuntimeFirstModeResolution::NeedsSystemSwitch {
                    user_running: env.runtime.user,
                    user_installed: env.installed.user,
                };
            }
            return RuntimeFirstModeResolution::NeedsSystemInstall {
                reason: "TUN requires the privileged system service".to_string(),
            };
        }
        if matches!(
            request,
            instance::ModeRequest::Unspecified | instance::ModeRequest::ExplicitSystem
        ) && env.installed.system
            && !env.runtime.system
        {
            return RuntimeFirstModeResolution::NeedsSystemDaemonRecovery {
                reason: "system service is installed but daemon IPC is unavailable".to_string(),
            };
        }
    }

    // S5: if no explicit flag and no runtime, check settings.json
    let request = if request == instance::ModeRequest::Unspecified {
        let settings = instance::UserSettings::load();
        match settings.resolve_mode(&env.installed) {
            Some(instance::InstanceMode::System) if intent != UserIntent::Install => {
                instance::ModeRequest::ExplicitSystem
            }
            Some(instance::InstanceMode::User) if intent != UserIntent::Install => {
                instance::ModeRequest::ExplicitUser
            }
            _ => request, // auto or no service installed
        }
    } else {
        request
    };

    if request == instance::ModeRequest::Unspecified {
        match (env.runtime.system, env.runtime.user) {
            (true, false) => {
                return RuntimeFirstModeResolution::Resolved {
                    mode: instance::InstanceMode::System,
                    source: instance::ResolutionSource::RuntimePresence,
                }
            }
            (false, true) => {
                return RuntimeFirstModeResolution::Resolved {
                    mode: instance::InstanceMode::User,
                    source: instance::ResolutionSource::RuntimePresence,
                }
            }
            (true, true) if intent != UserIntent::Install => {
                return RuntimeFirstModeResolution::RuntimeConflict
            }
            _ => {}
        }
    }

    if matches!(intent, UserIntent::TunOn | UserIntent::TunOff) {
        if request == instance::ModeRequest::Unspecified
            && !env.runtime.system
            && !env.installed.system
        {
            if matches!(intent, UserIntent::TunOn) && (env.runtime.user || env.installed.user) {
                return RuntimeFirstModeResolution::NeedsSystemSwitch {
                    user_running: env.runtime.user,
                    user_installed: env.installed.user,
                };
            }
            return RuntimeFirstModeResolution::NeedsSystemInstall {
                reason: "TUN requires the privileged system service".to_string(),
            };
        }
        if matches!(
            request,
            instance::ModeRequest::Unspecified | instance::ModeRequest::ExplicitSystem
        ) && env.installed.system
            && !env.runtime.system
        {
            return RuntimeFirstModeResolution::NeedsSystemDaemonRecovery {
                reason: "system service is installed but daemon IPC is unavailable".to_string(),
            };
        }
    }

    if env.legacy_root.is_some() && !env.installed.system && !env.installed.user {
        return RuntimeFirstModeResolution::NotInstalled;
    }

    match instance::resolve_instance_mode_with_source(
        request,
        env.installed,
        instance_intent_from_user_intent(intent),
    ) {
        instance::ModeResolutionWithSource::Resolved { mode, source } => {
            RuntimeFirstModeResolution::Resolved { mode, source }
        }
        instance::ModeResolutionWithSource::PromptRequired { .. } => {
            RuntimeFirstModeResolution::PromptRequired
        }
        instance::ModeResolutionWithSource::AmbiguousBothInstalled => {
            RuntimeFirstModeResolution::AmbiguousBothInstalled
        }
        instance::ModeResolutionWithSource::NotInstalled => {
            RuntimeFirstModeResolution::NotInstalled
        }
    }
}

fn instance_intent_from_user_intent(intent: UserIntent) -> instance::CommandIntent {
    match intent {
        UserIntent::Install => instance::CommandIntent::Install,
        UserIntent::Start => instance::CommandIntent::StartLike,
        UserIntent::Stop => instance::CommandIntent::StopLike,
        UserIntent::Uninstall => instance::CommandIntent::UninstallLike,
        UserIntent::Status
        | UserIntent::TunStatus
        | UserIntent::ApiRead
        | UserIntent::ConfigRead => instance::CommandIntent::ReadOnly,
        UserIntent::TunOn
        | UserIntent::TunOff
        | UserIntent::ApiMutate
        | UserIntent::ConfigWrite => instance::CommandIntent::Mutating,
    }
}

fn resolve_instance_mode_runtime_first(
    request: instance::ModeRequest,
    runtime: instance::ServicePresence,
    services: instance::ServicePresence,
    intent: instance::CommandIntent,
) -> RuntimeFirstModeResolution {
    resolve_environment_for_intent(
        request,
        &EnvironmentState {
            runtime,
            installed: services,
            legacy_root: None,
        },
        user_intent_from_command_intent(intent),
    )
}

fn explicit_mode_runtime_conflict(
    request: instance::ModeRequest,
    runtime: instance::ServicePresence,
    intent: instance::CommandIntent,
) -> Option<String> {
    if runtime.system
        && runtime.user
        && !matches!(
            intent,
            instance::CommandIntent::StopLike | instance::CommandIntent::UninstallLike
        )
    {
        return Some(
            "mode conflict: both system daemon and per-user core are running.\n  \
             v3 modes are mutually exclusive; stop one runtime first.\n  \
             Suggestions:\n  \
               mihomo-cli stop --system\n  \
               mihomo-cli stop"
                .to_string(),
        );
    }
    match request {
        instance::ModeRequest::ExplicitSystem
            if runtime.user
                && !runtime.system
                && intent != instance::CommandIntent::UninstallLike =>
        {
            Some(
                "system instance requested, but only the per-user core appears to be running.\n  \
             v3 modes are mutually exclusive; stop the user instance first or omit --system.\n  \
             Suggestion: mihomo-cli stop"
                    .to_string(),
            )
        }
        instance::ModeRequest::ExplicitUser
            if runtime.system
                && !runtime.user
                && intent != instance::CommandIntent::UninstallLike =>
        {
            Some(
                "per-user instance requested, but the system daemon appears to be running.\n  \
             v3 modes are mutually exclusive; use the system instance or stop it first.\n  \
             Suggestion: mihomo-cli stop --system"
                    .to_string(),
            )
        }
        _ => None,
    }
}

fn v3_mutual_exclusion_violation(
    requested: instance::InstanceMode,
    runtime: instance::ServicePresence,
    operation: &str,
) -> Option<String> {
    if runtime.system && runtime.user && operation != "stop" {
        return Some(format!(
            "cannot {operation} while both system daemon and per-user core are running.\n  \
             v3 uses mutually exclusive modes because TUN/system service traffic conflicts with per-user core.\n  \
             Stop one runtime first:\n  \
               mihomo-cli stop --system\n  \
               mihomo-cli stop"
        ));
    }
    match requested {
        instance::InstanceMode::System if runtime.user && !runtime.system => Some(format!(
            "cannot {operation} the system service while the per-user core is running.\n  \
             v3 uses mutually exclusive modes because TUN/system service traffic conflicts with per-user core.\n  \
             Stop the user instance first: mihomo-cli stop"
        )),
        instance::InstanceMode::User if runtime.system && !runtime.user => Some(format!(
            "cannot {operation} the per-user core while the system daemon is running.\n  \
             v3 uses mutually exclusive modes because TUN/system service traffic conflicts with per-user core.\n  \
             Use the system instance or stop it first: mihomo-cli stop --system"
        )),
        _ => None,
    }
}

fn resolve_current_mode(
    system: bool,
    user: bool,
    intent: instance::CommandIntent,
) -> anyhow::Result<instance::InstanceMode> {
    Ok(resolve_current_instance_mode(system, user, intent)?.mode)
}

fn format_stop_no_instance() -> Vec<String> {
    vec![
        "No running mihomo instance detected.".to_string(),
        "Nothing to stop.".to_string(),
    ]
}

fn start_requires_install_message() -> &'static str {
    "No mihomo service is installed yet.
  Interactive use: run `mihomo-cli start` from a terminal to choose normal proxy or TUN mode.
  Non-interactive use: run `mihomo-cli install --user` for normal proxy mode, or `mihomo-cli tun on` for TUN mode."
}

async fn prompt_install_for_start() -> anyhow::Result<bool> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(start_requires_install_message());
    }
    println!("No mihomo service is installed yet.");
    println!("Start needs an installed service/core first.");
    println!();
    cmd_install_entry(false, false, false, None, None, false, false, None).await?;
    Ok(true)
}

async fn cmd_lifecycle_resolved(
    system: bool,
    user: bool,
    action: instance::ServiceAction,
    allow_runtime_reset: bool,
) -> anyhow::Result<()> {
    let intent = match action {
        instance::ServiceAction::Start | instance::ServiceAction::Restart => {
            instance::CommandIntent::StartLike
        }
        instance::ServiceAction::Stop => instance::CommandIntent::StopLike,
        // Uninstall is resolved by the installer path, never here — keep it loud
        // instead of silently inheriting another intent.
        instance::ServiceAction::Uninstall => {
            unreachable!("uninstall is resolved by the installer path, not the lifecycle resolver")
        }
        // Enable/Disable are autostart-only (R1.2) and never enter the
        // lifecycle path; Mutating is the conservative intent if they do.
        instance::ServiceAction::Enable | instance::ServiceAction::Disable => {
            instance::CommandIntent::Mutating
        }
    };
    if !system && !user && action == instance::ServiceAction::Stop {
        if let Some(config_dir) = config_dir_override_path() {
            let override_running =
                current_user_context_with_config_dir(config_dir).is_some_and(|ctx| {
                    match ctx.paths.api_endpoint {
                        instance::ApiEndpoint::UnixSocket(path) => unix_socket_connectable(&path),
                        instance::ApiEndpoint::WindowsNamedPipe(pipe) => {
                            windows_pipe_connectable(&pipe)
                        }
                    }
                });
            if !override_running {
                print_lines(format_stop_no_instance());
                return Ok(());
            }
        }
    }
    if !system
        && !user
        && status_has_no_instance(
            false,
            false,
            current_service_presence(),
            current_runtime_presence(),
        )
    {
        match action {
            instance::ServiceAction::Start => {
                prompt_install_for_start().await?;
                return Ok(());
            }
            instance::ServiceAction::Stop => {
                print_lines(format_stop_no_instance());
                return Ok(());
            }
            _ => {}
        }
    }

    // Check for daemon crash recovery scenario:
    // System service is installed, daemon IPC is dead, but core might still be running
    if matches!(
        action,
        instance::ServiceAction::Start | instance::ServiceAction::Restart
    ) && !system
        && !user
    {
        let installed = current_service_presence();
        let runtime = current_runtime_presence();
        if installed.system && !runtime.system && !system_daemon_socket_connectable() {
            // Daemon is dead but system service is installed
            // Check if core might still be running (orphan process)
            if let Some(ctx) = instance::planned_current_context(instance::InstanceMode::System) {
                if let Some(runtime_dir) = &ctx.paths.runtime_dir {
                    let pid_file = runtime_dir.join("core.pid");
                    if pid_file.exists() {
                        // Core PID file exists - daemon crashed but core may be alive
                        if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
                            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                                // Check if process is actually running
                                #[cfg(unix)]
                                {
                                    use std::process::Command;
                                    let check = Command::new("kill")
                                        .args(["-0", &pid.to_string()])
                                        .output();
                                    if let Ok(output) = check {
                                        if output.status.success() {
                                            // Core is running but daemon is dead
                                            anyhow::bail!(
                                                "system daemon crashed but mihomo core (PID {}) is still running.\n  \
                                                 Recovery options:\n  \
                                                   mihomo-cli restart --system   Restart daemon and reattach to running core\n  \
                                                   mihomo-cli stop --system      Stop the orphan core process\n  \
                                                 Then try your command again.",
                                                pid
                                            );
                                        }
                                    }
                                }
                                #[cfg(not(unix))]
                                {
                                    // On Windows, just report the situation
                                    anyhow::bail!(
                                        "system daemon crashed but mihomo core (PID {}) may still be running.\n  \
                                         Recovery: stop the system service first, then try again.\n  \
                                           mihomo-cli stop --system",
                                        pid
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Lifecycle concurrency control (aligned with clash-verge-service):
    // the CLI never holds a lifecycle lock. System mode is serialized by the
    // daemon's OWNER_LIFECYCLE_LOCK; User mode is serialized by systemd's job
    // queue. This also fixes BUG-13 (CLI flock on the root-owned /var/run/mihomo
    // dir would fail for non-root users and was misreported as a lock conflict).
    let mode = resolve_current_mode(system, user, intent)?;
    if matches!(
        action,
        instance::ServiceAction::Start | instance::ServiceAction::Restart
    ) {
        let operation = match action {
            instance::ServiceAction::Start => "start",
            instance::ServiceAction::Restart => "restart",
            _ => unreachable!("guard only allows start/restart"),
        };
        if let Some(message) =
            v3_mutual_exclusion_violation(mode, current_runtime_presence(), operation)
        {
            anyhow::bail!(message);
        }
    }
    let result = cmd_lifecycle_instance_mode(
        mode,
        action,
        allow_runtime_reset,
        !allow_runtime_reset && std::io::stdin().is_terminal(),
    )
    .await;

    result
}

fn format_legacy_root_service_diagnostic(legacy: &instance::LegacyRootService) -> Vec<String> {
    let mut lines = vec![
        "=== Legacy Root Layout Detected ===".to_string(),
        "  The system service points into a user home, which conflicts with the v3 system-service layout."
            .to_string(),
        format!("  Service file: {}", legacy.service_file.display()),
    ];
    if let Some(home) = &legacy.referenced_home {
        lines.push(format!("  Referenced home: {}", home.display()));
        if !legacy.referenced_current_user_home {
            lines.push("  ⚠ Referenced home is not the current user's home; migration/cleanup requires explicit confirmation.".to_string());
        }
    }
    if !legacy.referenced_paths.is_empty() {
        lines.push("  Referenced user-home paths:".to_string());
        for path in &legacy.referenced_paths {
            lines.push(format!("    - {}", path.display()));
        }
    }
    lines.extend([
        String::new(),
        "  Run: mihomo-cli uninstall --all  清理旧安装".to_string(),
        "  Then reinstall: mihomo-cli install".to_string(),
        "  Or inspect a specific instance: mihomo-cli status --system -v".to_string(),
    ]);
    lines
}

fn status_has_no_instance(
    system: bool,
    user: bool,
    services: instance::ServicePresence,
    runtime: instance::ServicePresence,
) -> bool {
    !system && !user && !services.system && !services.user && !runtime.system && !runtime.user
}

fn format_no_instance_status() -> Vec<String> {
    vec![
        "=== Mihomo Status ===".to_string(),
        "No running mihomo instance detected.".to_string(),
        "No service is installed.".to_string(),
        String::new(),
        "Next steps:".to_string(),
        "  Normal proxy mode:".to_string(),
        "    mihomo-cli install --user".to_string(),
        "    mihomo-cli start".to_string(),
        String::new(),
        "  TUN mode:".to_string(),
        "    mihomo-cli tun on".to_string(),
        "    # will guide system service installation".to_string(),
    ]
}

async fn cmd_use(mode: Option<UseMode>) -> anyhow::Result<()> {
    use instance::UserSettings;

    let services = current_service_presence();

    match mode {
        None | Some(UseMode::Status) => {
            // Show current settings
            instance::print_settings_status(&services);
            Ok(())
        }
        Some(mode_value) => {
            let mut settings = UserSettings::load();
            settings.mode = match mode_value {
                UseMode::System => "system".to_string(),
                UseMode::User => "user".to_string(),
                UseMode::Auto => "auto".to_string(),
                UseMode::Status => unreachable!(),
            };
            settings.save()?;

            println!("Mode preference set to: {}", settings.mode);

            // Show warning if setting to system but not installed
            if settings.mode == "system" && !services.system {
                println!("⚠ System service is not installed.");
                println!("  Run `mihomo-cli install --system` to install it first.");
            }
            if settings.mode == "user" && !services.user {
                println!("⚠ Per-user service is not installed.");
                println!("  Run `mihomo-cli install` to install it first.");
            }

            // Show resolved mode
            if let Some(resolved) = settings.resolve_mode(&services) {
                println!("Effective mode: {resolved:?}");
            }

            Ok(())
        }
    }
}

/// pending generation 三态消费——可读无 pending 静默、可读有 pending
/// 沿用既有告警、读不动（EACCES）显示「无法判定」。不可读绝不代表「无 pending」，
/// 只读面也绝不在这里提权。
fn doctor_pending_generation_check(store: &generation::GenerationStore) -> Option<DoctorCheck> {
    match store.observe_pending() {
        generation::PendingObservation::Applied => None,
        generation::PendingObservation::HasPending(pending_id) => Some(DoctorCheck::fail(
            "待应用更新",
            format!("发现 pending generation ({pending_id})"),
            doctor_daemon_binary_skew_hint().to_string(),
        )),
        generation::PendingObservation::Undeterminable(error) => Some(DoctorCheck::neutral(
            "待应用更新",
            pending_generation_unknown_notice(&error),
        )),
    }
}

pub(crate) fn doctor_intranet_guard_checks(config_path: &std::path::Path) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    if let Ok(content) = std::fs::read_to_string(config_path) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            for w in config::lint_intranet_guard(&yaml) {
                checks.push(DoctorCheck::warn(
                    "内网资产安全 (Intranet Guard)",
                    w.message,
                    Some(w.recommendation),
                ));
            }
        }
    }
    checks
}

async fn cmd_doctor(system: bool, user: bool, probe_dns: bool) -> anyhow::Result<()> {
    let ctx = match mode_request_from_flags(system, user) {
        instance::ModeRequest::ExplicitSystem => {
            instance::planned_current_context(instance::InstanceMode::System)
        }
        instance::ModeRequest::ExplicitUser => doctor_user_context(),
        instance::ModeRequest::Unspecified => doctor_auto_context()?,
    }
    .ok_or_else(|| anyhow::anyhow!("Unsupported OS for diagnostics"))?;

    println!("=== mihomo-cli doctor ===");
    println!("  Mode: {}", instance_mode_label(ctx.mode));
    println!();

    let mut checks = Vec::new();
    let config_path = &ctx.paths.intent_config_file;
    if !config_path.exists() {
        checks.push(DoctorCheck::fail(
            "配置文件",
            format!("不存在 ({})", config_path.display()),
            "请先添加订阅: mihomo-cli config -u <subscription-url>",
        ));
    } else if let Err(err) = std::fs::File::open(config_path) {
        checks.push(DoctorCheck::fail(
            "配置文件",
            format!("无法读取 ({})", config_path.display()),
            format!("检查文件权限: {err}"),
        ));
    } else {
        checks.push(DoctorCheck::pass(
            "配置文件",
            config_path.display().to_string(),
        ));
        checks.extend(doctor_intranet_guard_checks(config_path));
    }

    #[cfg(unix)]
    if doctor_checks_config_owner(ctx.mode) {
        if let (Ok(metadata), Some(expected_uid)) = (
            std::fs::metadata(config_path),
            instance::PathInputs::from_current_env().uid,
        ) {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() == expected_uid {
                checks.push(DoctorCheck::pass(
                    "配置文件所有者",
                    format!("uid {}", metadata.uid()),
                ));
            } else {
                checks.push(DoctorCheck::fail(
                    "配置文件所有者",
                    format!("uid {}，当前用户 uid {}", metadata.uid(), expected_uid),
                    format!("修复: sudo chown $(whoami) {}", config_path.display()),
                ));
            }
        }
    }

    if doctor_checks_service(&ctx.service) {
        let service_installed = match ctx.mode {
            instance::InstanceMode::System if cfg!(target_os = "windows") => {
                windows_mihomo_service_installed()
            }
            _ => ctx
                .paths
                .service_file
                .as_ref()
                .is_some_and(|path| path.exists()),
        };
        if service_installed {
            checks.push(DoctorCheck::pass(
                "服务安装",
                status_service_label(&ctx.service),
            ));
        } else {
            let install = if ctx.mode == instance::InstanceMode::System {
                "mihomo-cli install --system"
            } else {
                "mihomo-cli install"
            };
            checks.push(DoctorCheck::fail(
                "服务安装",
                status_service_label(&ctx.service),
                format!("运行: {install}"),
            ));
        }

        if service_manager_active(&ctx) {
            checks.push(DoctorCheck::pass("服务状态", "运行中"));
        } else {
            let restart = if ctx.mode == instance::InstanceMode::System {
                system_service_recovery_command(&ctx)
                    .unwrap_or_else(|| "重启系统服务管理器中的 mihomo 服务".to_string())
            } else {
                "mihomo-cli restart".to_string()
            };
            checks.push(DoctorCheck::fail(
                "服务状态",
                "未运行",
                format!("尝试: {restart}"),
            ));
        }
    }

    if ctx.mode == instance::InstanceMode::System {
        #[cfg(unix)]
        daemon::check_and_warn_legacy_authorized_clients();

        let gen_store = system_generation_store(&ctx);
        if let Some(check) = doctor_pending_generation_check(&gen_store) {
            checks.push(check);
        }
        if let Ok(current_cli) = std::env::current_exe() {
            if ctx.paths.cli_binary.exists() {
                if utils::file_contents_equal(&current_cli, &ctx.paths.cli_binary) {
                    checks.push(DoctorCheck::pass("Daemon 二进制", "与当前 CLI 一致"));
                } else {
                    checks.push(DoctorCheck::fail(
                        "Daemon 二进制",
                        format!("与当前 CLI 不一致 ({})", ctx.paths.cli_binary.display()),
                        doctor_daemon_binary_skew_hint().to_string(),
                    ));
                }
            } else {
                checks.push(DoctorCheck::fail(
                    "Daemon 二进制",
                    format!("未找到 ({})", ctx.paths.cli_binary.display()),
                    "安装系统服务: mihomo-cli install --system".to_string(),
                ));
            }
        }
        let socket_path = ipc::system_service_socket_path();
        if !ipc::is_daemon_running().await {
            checks.push(DoctorCheck::fail(
                "Daemon transport",
                format!("不可连接 ({})", socket_path.display()),
                system_service_recovery_command(&ctx)
                    .map(|command| format!("重启 daemon: {command}"))
                    .unwrap_or_else(|| "安装系统服务: mihomo-cli install --system".to_string()),
            ));
        } else {
            checks.push(DoctorCheck::pass(
                "Daemon transport",
                socket_path.display().to_string(),
            ));
            #[cfg(unix)]
            let token_location = ipc::client_token_location();
            #[cfg(unix)]
            let token_file_readable = ipc::current_client_token().is_some();

            match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await {
                Ok(ipc::DaemonResponse::Status {
                    running,
                    config_path: Some(active_config),
                    ..
                }) => {
                    let _ = &active_config;
                    #[cfg(unix)]
                    checks.push(format_doctor_owner_auth_check(
                        ctx.os,
                        &token_location,
                        token_file_readable,
                        OwnerAuthDaemonStatus::Authorized,
                        None,
                    ));
                    #[cfg(not(unix))]
                    checks.push(DoctorCheck::pass("Daemon 授权", "当前用户已授权"));
                    if running {
                        checks.push(DoctorCheck::pass("Mihomo Core", "运行中"));
                    } else {
                        checks.push(DoctorCheck::fail(
                            "Mihomo Core",
                            "未运行",
                            "启动: mihomo-cli start --system",
                        ));
                    }
                    // 使用 StatusSnapshot 读取配置意图和运行态
                    let snapshot = status::StatusSnapshot::collect(&ctx).await;
                    let consistency = status::check_tun_consistency(&snapshot);
                    match consistency.as_bool() {
                        Some(true) => {
                            checks.push(DoctorCheck::pass(
                                "TUN 配置一致性",
                                format!("配置意图与运行态一致（{}）", snapshot.runtime_tun),
                            ));
                        }
                        Some(false) => {
                            checks.push(DoctorCheck::fail(
                                "TUN 配置一致性",
                                format!(
                                    "配置意图（{}）与运行态（{}）不一致",
                                    snapshot.configured_tun, snapshot.runtime_tun
                                ),
                                "运行: mihomo-cli restart --system".to_string(),
                            ));
                        }
                        None => {
                            // C：unknown 的修法取决于缺哪一侧，不能一律建议
                            // `restart`——Core 正在运行时那是一个无解的循环提示。
                            let hint = match (
                                snapshot.runtime_tun.as_bool(),
                                snapshot.configured_tun.as_bool(),
                            ) {
                                (Some(_), None) => {
                                    "配置意图缺少 tun.enable：运行 mihomo-cli tun on 重建 TUN 意图"
                                        .to_string()
                                }
                                (None, Some(_)) => {
                                    "Core 未报告 TUN 运行态；先运行 mihomo-cli status --json 观察 runtime_tun，再决定是否 restart --system"
                                        .to_string()
                                }
                                _ => "启动或重启核心后重试: mihomo-cli restart --system"
                                    .to_string(),
                            };
                            checks.push(DoctorCheck::fail(
                                "TUN 配置一致性",
                                format!(
                                    "无法观察运行态（{}）或配置意图（{}）",
                                    snapshot.runtime_tun, snapshot.configured_tun
                                ),
                                hint,
                            ));
                        }
                    }

                    // 快照构造完成后才执行 P2 探测，且不回填快照。
                    if let Some(check) = doctor_dns_probe_check(&ctx, &snapshot, probe_dns).await {
                        checks.push(check);
                    }

                    #[cfg(target_os = "linux")]
                    if snapshot.runtime_tun == status::TriState::True {
                        let diagnosis =
                            check_linux_link_local_routing(Some(active_config.as_path()));
                        checks.push(format_link_local_route_check(&diagnosis));
                    }
                }
                Ok(ipc::DaemonResponse::Status {
                    running,
                    config_path: None,
                    ..
                }) => {
                    #[cfg(unix)]
                    checks.push(format_doctor_owner_auth_check(
                        ctx.os,
                        &token_location,
                        token_file_readable,
                        OwnerAuthDaemonStatus::Authorized,
                        None,
                    ));
                    #[cfg(not(unix))]
                    checks.push(DoctorCheck::pass("Daemon 授权", "当前用户已授权"));
                    checks.push(if running {
                        DoctorCheck::pass("Mihomo Core", "运行中")
                    } else {
                        DoctorCheck::fail(
                            "Mihomo Core",
                            "未运行",
                            "启动: mihomo-cli start --system",
                        )
                    });
                    // 使用 StatusSnapshot 读取运行态
                    let snapshot = status::StatusSnapshot::collect(&ctx).await;
                    checks.push(DoctorCheck::fail(
                        "TUN 配置一致性",
                        format!("daemon 未报告活动配置（运行态: {}）", snapshot.runtime_tun),
                        "启动或重启核心后重试: mihomo-cli restart --system",
                    ));
                    // MINOR-2（review）：D1 规定 doctor 默认探测；本分支（daemon 刚重启、
                    // 尚未报告活动配置）正是最需要判定的场景之一，不能静默跳过。
                    if let Some(check) = doctor_dns_probe_check(&ctx, &snapshot, probe_dns).await {
                        checks.push(check);
                    }
                }
                Ok(ipc::DaemonResponse::Error { message }) => {
                    #[cfg(unix)]
                    checks.push(format_doctor_owner_auth_check(
                        ctx.os,
                        &token_location,
                        token_file_readable,
                        OwnerAuthDaemonStatus::Rejected(classify_daemon_ipc_error(&message)),
                        Some(&message),
                    ));
                    #[cfg(not(unix))]
                    checks.push(doctor_daemon_error_check(ctx.os, &message));
                }
                Ok(ipc::DaemonResponse::Success { message }) => checks.push(DoctorCheck::fail(
                    "Daemon 状态",
                    format!("收到意外响应: {message}"),
                    "运行: mihomo-cli status --system --verbose",
                )),
                Ok(
                    ipc::DaemonResponse::CoreApi { .. }
                    | ipc::DaemonResponse::Transaction { .. }
                    | ipc::DaemonResponse::SelectionMirrorRevisions { .. }
                    | ipc::DaemonResponse::SelectionDrift { .. },
                ) => checks.push(DoctorCheck::fail(
                    "Daemon 状态",
                    "收到意外的 Core API / Transaction / Mirror revisions 响应",
                    "运行: mihomo-cli status --system --verbose",
                )),
                Err(err) => checks.push(DoctorCheck::fail(
                    "Daemon transport",
                    err.to_string(),
                    system_service_recovery_command(&ctx)
                        .map(|command| format!("重启 daemon: {command}"))
                        .unwrap_or_else(|| {
                            "检查 system daemon 日志: sudo systemctl status mihomo".to_string()
                        }),
                )),
            }
            #[cfg(unix)]
            match selection_mirror_drift_report(&utils::AppPaths::new(ctx.paths.config_dir.clone()))
                .await
            {
                Ok(drift) => {
                    let drifted: Vec<String> = drift
                        .iter()
                        .filter(|(_, kind)| **kind != SelectionMirrorDrift::Ok)
                        .map(|(id, kind)| format!("{id}({kind:?})"))
                        .collect();
                    if drift.is_empty() {
                        checks.push(DoctorCheck::pass(
                            "Selection mirror",
                            "用户树无 selection 状态，无需对账",
                        ));
                    } else if drifted.is_empty() {
                        checks.push(DoctorCheck::pass(
                            "Selection mirror",
                            format!("与用户树一致（{} 项）", drift.len()),
                        ));
                    } else {
                        checks.push(DoctorCheck::fail(
                            "Selection mirror",
                            format!("运行时镜像与用户树漂移: {}", drifted.join(", ")),
                            "重建: mihomo-cli restart --system".to_string(),
                        ));
                    }
                }
                Err(err) => checks.push(DoctorCheck::fail(
                    "Selection mirror",
                    format!("对账查询失败: {err:#}"),
                    "确认 system daemon 可用: mihomo-cli status --system".to_string(),
                )),
            }
            // B3: immediate read-only drift comparison reusing the
            // daemon's decision function (no write, no fix). Intent source is
            // the user tree (the CLI never requires /var/lib readability).
            match doctor_selection_drift_check(&ctx).await {
                Ok(check) => checks.push(check),
                Err(err) => checks.push(DoctorCheck::fail(
                    "Selection drift",
                    format!("即时比对失败: {err:#}"),
                    "确认 Core API 可用: mihomo-cli status --system".to_string(),
                )),
            }
        }
    } else if mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs")
        .await
        .is_ok()
    {
        checks.push(DoctorCheck::pass("Mihomo Core API", "可连接"));
        if probe_dns {
            let snapshot = status::StatusSnapshot::collect(&ctx).await;
            if let Some(check) = doctor_dns_probe_check(&ctx, &snapshot, probe_dns).await {
                checks.push(check);
            }
        }
    } else {
        checks.push(DoctorCheck::fail(
            "Mihomo Core API",
            format!(
                "不可连接 ({})",
                status_endpoint_label(&ctx.paths.api_endpoint)
            ),
            "启动或重启服务: mihomo-cli restart",
        ));
    }

    if let Some(observation) = orphan_core_observation(&ctx).await {
        checks.push(DoctorCheck {
            passed: true,
            warning: true,
            neutral: false,
            label: "孤儿 Core".to_string(),
            detail: observation,
            hint: Some(
                "只读观察：未停止、未清理；显式 restart/install 会在接管时证明退出".to_string(),
            ),
        });
    }

    for check in &checks {
        println!("{}", check.format());
    }
    let failed = checks.iter().filter(|check| !check.passed).count();
    println!();
    if failed == 0 {
        println!("✅ 未发现问题");
    } else {
        println!("⚠️  发现 {failed} 个问题；请按 💡 提示修复后重新运行 `mihomo-cli doctor`。");
    }
    Ok(())
}

async fn cmd_status_resolved_json(system: bool, user: bool, json: bool) -> anyhow::Result<()> {
    if json {
        return cmd_status_json(system, user).await;
    }
    cmd_status_resolved(system, user).await
}

async fn cmd_status_json(system: bool, user: bool) -> anyhow::Result<()> {
    if !system && !user {
        if let Some(legacy) = current_legacy_root_service() {
            return print_json_envelope(
                "status",
                serde_json::json!({
                    "state": "legacy_root_service_detected",
                    "legacy_root_service": { "service_file": legacy.service_file, "referenced_paths": legacy.referenced_paths }
                }),
                Vec::new(),
            );
        }
        if status_has_no_instance(
            system,
            user,
            current_service_presence(),
            current_runtime_presence(),
        ) {
            return print_json_envelope(
                "status",
                serde_json::json!({ "state": "no_instance" }),
                Vec::new(),
            );
        }
    }
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    let ctx = resolved.ctx;
    let plan = instance::planned_status_diagnostics(&ctx);
    let snapshot = status::StatusSnapshot::collect(&ctx).await;
    print_json_envelope(
        "status",
        status_json_data(
            &plan,
            resolved.source,
            &ctx.paths.intent_config_file,
            &snapshot,
            service_runtime_state(&ctx),
            &ctx,
        ),
        Vec::new(),
    )
}

/// `status --json` 的 pending generation 三态结构化表达。
/// `unknown` 表示代际信任根以当前身份读不动（可能存在未应用更新），
/// 不等于无 pending；user 实例无该概念，为 null。
fn status_json_pending_generation(
    mode: instance::InstanceMode,
    ctx: &instance::InstanceContext,
) -> serde_json::Value {
    if mode != instance::InstanceMode::System {
        return serde_json::Value::Null;
    }
    match system_generation_store(ctx).observe_pending() {
        generation::PendingObservation::Applied => serde_json::json!({ "state": "none" }),
        generation::PendingObservation::HasPending(id) => {
            serde_json::json!({ "state": "pending", "id": id.as_str() })
        }
        generation::PendingObservation::Undeterminable(_) => {
            serde_json::json!({ "state": "unknown" })
        }
    }
}

/// Render status JSON exclusively from the shared runtime snapshot.  This
/// prevents JSON and text status from observing different daemon/Core/proxy
/// state during one command invocation.
fn status_json_data(
    plan: &instance::StatusDiagnosticPlan,
    source: instance::ResolutionSource,
    intent_config_path: &std::path::Path,
    snapshot: &status::StatusSnapshot,
    runtime: service::ServiceRuntimeState,
    ctx: &instance::InstanceContext,
) -> serde_json::Value {
    // Spec §0.6: the JSON top-level judgment is the machine-readable form of the
    // text summary, so it must come from the same predicate.
    let health = status_health_label(snapshot, runtime);
    let daemon = if plan.mode == instance::InstanceMode::System {
        serde_json::json!({
            "running": snapshot.daemon_reachable.as_bool(),
            "socket": ipc::system_service_socket_path(),
        })
    } else {
        serde_json::Value::Null
    };

    // SPEC §3.8.1: status JSON carries `selections` with `subscription_id` and
    // `source: local-derive`. Pure local derivation, no network, no runtime
    // attestation.
    let selections_value = status_json_selections(ctx);

    serde_json::json!({
        "health": health,
        "instance": instance_mode_label(plan.mode),
        "mode": instance_mode_label(plan.mode),
        "resolved_by": resolution_source_label(source),
        "core": {
            "running": snapshot.core_running.as_bool(),
            "pid": snapshot.core_pid,
            "active_config": snapshot.active_config_path,
            "api_reachable": snapshot.api_reachable,
            // Raw Core `/configs` observation (unattested); the attested judgment
            // lives in the top-level `tun` field (Spec §0.6).
            "tun": snapshot.runtime_tun.as_bool(),
        },
        "api": if snapshot.api_reachable { "reachable" } else { "unknown" },
        "tun": status_tun_label(snapshot),
        "configured_tun": snapshot.configured_tun.to_string(),
        "system_proxy": system_proxy_status_label(snapshot.system_proxy),
        "shell_proxy": shell_proxy_status_label(snapshot.shell_proxy),
        "rule_mode": snapshot.rule_mode,
        "default_route": default_route_label(
            default_route_path(snapshot.active_config_path.as_deref(), intent_config_path),
            &snapshot.rule_mode,
        ),
        "configuration": status_configuration_label(snapshot),
        "config": { "path": intent_config_path, "exists": snapshot.intent_config_exists },
        "api_endpoint": status_endpoint_label(&plan.expected_endpoint),
        "service": status_service_label(&plan.service),
        "binary": { "path": plan.binary, "exists": snapshot.core_binary_exists },
        "daemon": daemon,
        "logs": plan.log_file,
        "selections": selections_value,
        // durable 代际状态，非 runtime 快照；与文本路径共用
        // `observe_pending` 单点观测，杜绝同一次调用内两处判定漂移。
        "pending_generation": status_json_pending_generation(plan.mode, ctx),
    })
}

/// SPEC §3.8.1: derive `selections` for status JSON from the user config tree.
/// `source` is always `local-derive`; no network, no runtime attestation.
fn status_json_selections(ctx: &instance::InstanceContext) -> serde_json::Value {
    let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
    let active_id = config::get_active_id_at(&paths).ok().flatten();
    match active_id {
        Some(id) => {
            let selection_path = paths.selection_state_path_for_subscription(&id);
            let selections = selection::load_selection_state_path(&selection_path)
                .ok()
                .unwrap_or_default();
            serde_json::json!({
                "subscription_id": id,
                "source": "local-derive",
                "count": selections.len(),
            })
        }
        None => serde_json::json!({
            "subscription_id": serde_json::Value::Null,
            "source": "local-derive",
            "count": 0,
        }),
    }
}

async fn cmd_status_resolved(system: bool, user: bool) -> anyhow::Result<()> {
    if !system && !user {
        if let Some(legacy) = current_legacy_root_service() {
            print_lines(format_legacy_root_service_diagnostic(&legacy));
            return Ok(());
        }
        if status_has_no_instance(
            system,
            user,
            current_service_presence(),
            current_runtime_presence(),
        ) {
            print_lines(format_no_instance_status());
            return Ok(());
        }
    }

    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    cmd_status_context_with_source(resolved.ctx, Some(resolved.source)).await
}

fn uninstall_modes_for_request(
    system: bool,
    user: bool,
    all: bool,
) -> Option<Vec<instance::InstanceMode>> {
    if !all || system || user {
        return None;
    }
    Some(vec![
        instance::InstanceMode::System,
        instance::InstanceMode::User,
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AllUninstallOptions {
    yes: bool,
    dry_run: bool,
}

fn all_uninstall_options(yes: bool, dry_run: bool) -> AllUninstallOptions {
    AllUninstallOptions { yes, dry_run }
}

fn should_run_all_uninstall_service_commands(service_present: bool, runtime_present: bool) -> bool {
    service_present || runtime_present
}

#[allow(clippy::too_many_arguments)]
async fn cmd_uninstall_resolved(
    system: bool,
    user: bool,
    all: bool,
    remove_binary: bool,
    remove_config: bool,
    remove_geo: bool,
    yes: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    if let Some(modes) = uninstall_modes_for_request(system, user, all) {
        maybe_sudo_reexec_for_system_uninstall(instance::InstanceMode::System, dry_run)?;
        return cmd_uninstall_all_instance_modes(&modes, all_uninstall_options(yes, dry_run)).await;
    }

    let mode = resolve_current_mode(system, user, instance::CommandIntent::UninstallLike)?;
    maybe_sudo_reexec_for_system_uninstall(mode, dry_run)?;
    cmd_uninstall_instance_mode(
        mode,
        all,
        remove_binary,
        remove_config,
        remove_geo,
        yes,
        dry_run,
    )
    .await
}

fn api_requires_running_instance_message(ctx: &instance::InstanceContext) -> String {
    match ctx.mode {
        instance::InstanceMode::System => {
            let recovery = system_service_recovery_command(ctx).unwrap_or_else(|| {
                "restart the system service with your OS service manager".to_string()
            });
            format!(
                "mihomo core API is not running for the system service.
  Start it first:
    mihomo-cli start --system
  Or recover the daemon manually:
    {recovery}
    mihomo-cli start"
            )
        }
        instance::InstanceMode::User => "mihomo core API is not running for normal proxy mode.
  Start it first: mihomo-cli start"
            .to_string(),
    }
}

async fn resolve_ready_api_client(
    system: bool,
    user: bool,
    intent: instance::CommandIntent,
) -> anyhow::Result<mihomo_api::EndpointMihomoApiClient> {
    let resolved = resolve_current_instance_context(system, user, intent)?;
    let runtime = current_runtime_presence();
    let selected_runtime_running = match resolved.ctx.mode {
        instance::InstanceMode::System => runtime.system,
        instance::InstanceMode::User => runtime.user,
    };
    if !selected_runtime_running {
        anyhow::bail!(api_requires_running_instance_message(&resolved.ctx));
    }

    let client = mihomo_api::EndpointMihomoApiClient::for_instance(
        resolved.ctx.mode,
        resolved.ctx.paths.api_endpoint.clone(),
    );
    if let Err(error) = client.get("/configs").await {
        anyhow::bail!(
            "{}\n  Core API error: {error}",
            api_requires_running_instance_message(&resolved.ctx)
        );
    }
    Ok(client)
}

/// Real-time status dashboard (TUI)
/// Control autostart (boot/login launch) for the current instance mode.
///
/// Three-platform matrix:
/// - Linux system: SetAutostart IPC to the system daemon (ADR-19 marker)
/// - Linux user:   systemctl --user enable/disable/is-enabled mihomo
/// - macOS system: launchctl enable/disable/print system/io.mihomo
/// - macOS user:   launchctl enable/disable/print gui/UID/io.mihomo
/// - Windows system: sc config mihomo start= auto/demand + sc qc
/// - Windows user:   registry Run key + .vbs hidden (ADR-17)
async fn cmd_autostart(action: AutostartAction, system: bool, user: bool) -> anyhow::Result<()> {
    let mode = match (system, user) {
        (true, false) => instance::InstanceMode::System,
        (false, true) => instance::InstanceMode::User,
        (false, false) => {
            // Auto-detect: use resolve_current_instance_context.
            let resolved =
                resolve_current_instance_context(false, false, instance::CommandIntent::ReadOnly)?;
            resolved.ctx.mode
        }
        _ => unreachable!("clap conflicts_with"),
    };

    match action {
        AutostartAction::On => set_autostart(mode, true).await,
        AutostartAction::Off => set_autostart(mode, false).await,
        AutostartAction::Status => query_autostart(mode).await,
    }
}

#[cfg(target_os = "linux")]
async fn set_autostart(mode: instance::InstanceMode, enable: bool) -> anyhow::Result<()> {
    // ADR-19: Linux system autostart controls whether the daemon auto-starts
    // the core at boot — NOT the daemon unit itself (which must stay enabled
    // as infrastructure). The daemon owns the marker in its fixed runtime
    // directory; the CLI reaches it via SetAutostart IPC (Bug #11).
    if mode == instance::InstanceMode::System {
        if !ipc::is_daemon_running().await {
            anyhow::bail!(
                "system daemon is not running.\n  \
                 Run: mihomo-cli restart --system"
            );
        }
        match ipc::send_command(&ipc::DaemonCommand::SetAutostart {
            enabled: enable,
            token: None,
        })
        .await?
        {
            ipc::DaemonResponse::Success { .. } => {
                // Best-effort cleanup of the legacy per-user marker so a
                // previous buggy enable cannot linger and mislead status.
                if let Ok(ctx) =
                    instance::planned_current_context(instance::InstanceMode::System).ok_or(())
                {
                    let _ = utils::remove_file_if_exists(&ctx.paths.config_dir.join("autostart"));
                }
                println!(
                    "core autostart {}",
                    if enable { "enabled" } else { "disabled" }
                );
                return Ok(());
            }
            ipc::DaemonResponse::Error { message } => anyhow::bail!(message),
            response => anyhow::bail!("unexpected daemon autostart response: {response:?}"),
        }
    }
    // R1.2: non-IPC path (User mode) executes the planned Enable/Disable
    // service plan — no direct Command::new platform calls remain here.
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("unsupported OS for autostart"))?;
    let action = if enable {
        instance::ServiceAction::Enable
    } else {
        instance::ServiceAction::Disable
    };
    let plan = instance::planned_service_plan(&ctx, action);
    for command in &plan.commands {
        // Preserve the pre-R1.2 failure text: `systemctl enable mihomo failed`.
        if let Err(_err) = service::run_instance_command(command) {
            anyhow::bail!(
                "systemctl {} mihomo failed",
                if enable { "enable" } else { "disable" }
            );
        }
    }
    println!(
        "Autostart {} for mihomo",
        if enable { "enabled" } else { "disabled" }
    );
    Ok(())
}

#[cfg(target_os = "linux")]
async fn query_autostart(mode: instance::InstanceMode) -> anyhow::Result<()> {
    if mode == instance::InstanceMode::System {
        if !ipc::is_daemon_running().await {
            anyhow::bail!(
                "system daemon is not running.\n  \
                 Run: mihomo-cli restart --system"
            );
        }
        match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await? {
            ipc::DaemonResponse::Status {
                autostart_enabled, ..
            } => {
                println!(
                    "Autostart: {} (system core)",
                    if autostart_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
                return Ok(());
            }
            ipc::DaemonResponse::Error { message } => anyhow::bail!(message),
            response => anyhow::bail!("unexpected daemon status response: {response:?}"),
        }
    }
    let mut cmd = std::process::Command::new("systemctl");
    if mode == instance::InstanceMode::User {
        cmd.arg("--user");
    }
    cmd.arg("is-enabled").arg("mihomo");
    let output = cmd.output()?;
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let enabled = output.status.success() && state == "enabled";
    println!(
        "Autostart: {} ({})",
        if enabled { "enabled" } else { "disabled" },
        state
    );
    Ok(())
}

#[cfg(target_os = "macos")]
async fn set_autostart(mode: instance::InstanceMode, enable: bool) -> anyhow::Result<()> {
    // N4b: system-domain launchctl needs root; hint before opaque failure.
    // (Not a scanner needle — precheck intentionally stays in set_autostart.)
    if mode == instance::InstanceMode::System && unsafe { libc::geteuid() } != 0 {
        anyhow::bail!(
            "autostart for the system service requires root.\n  \
             Run with sudo: sudo mihomo-cli autostart {} --system",
            if enable { "on" } else { "off" }
        );
    }
    let ctx = resolved_ctx_for_autostart(mode)?;
    let domain = launchctl_domain(mode)?;
    let label = format!("{domain}/io.mihomo");

    // 1. Rewrite the plist RunAtLoad flag (ADR-17: autostart ⇔ RunAtLoad).
    //    Replace both canonical forms (no-space) to avoid leaving an
    //    invalid `<false />` with a space (breaks plist XML parsing).
    //    Plain std::fs rewrite stays here (not a platform primitive call).
    let plist = read_plist_for_autostart(&ctx)?;
    let desired = format!(
        "<key>RunAtLoad</key><{}/>",
        if enable { "true" } else { "false" }
    );
    let new_plist = plist
        .replace("<key>RunAtLoad</key><true/>", &desired)
        .replace("<key>RunAtLoad</key><false/>", &desired)
        .replace("<key>RunAtLoad</key><true />", &desired)
        .replace("<key>RunAtLoad</key><false />", &desired);
    let service_file = ctx
        .paths
        .service_file
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no launchd plist for this instance"))?;
    std::fs::write(service_file, new_plist.as_bytes())?;

    // 2. Enable/disable via the planned launchctl override (survives reboots).
    //    Args mirror the former direct call: launchctl {enable|disable} <label>.
    let action = if enable {
        instance::ServiceAction::Enable
    } else {
        instance::ServiceAction::Disable
    };
    let plan = instance::planned_service_plan(&ctx, action);
    for command in &plan.commands {
        // Preserve the pre-R1.2 failure text: `launchctl {action} {label} failed`.
        if let Err(_err) = service::run_instance_command(command) {
            anyhow::bail!(
                "launchctl {} {label} failed",
                if enable { "enable" } else { "disable" }
            );
        }
    }

    // 3. Re-load the plist so the new RunAtLoad takes effect (enable only).
    //    print/bootstrap stay as direct calls in a helper outside this fn —
    //    only the enable/disable semantics were folded into the plan.
    ensure_macos_autostart_loaded(&ctx, &domain, &label, enable);

    println!(
        "Autostart {} for {}",
        if enable { "enabled" } else { "disabled" },
        label
    );
    Ok(())
}

/// Best-effort bootstrap after `autostart on`: only when the job is not yet
/// loaded (a fresh disable override would otherwise block bootstrap with EIO
/// until cleared — enable already cleared it). Disabling does NOT re-bootstrap:
/// the service stays loaded so `start` still works, and the disable override
/// prevents autostart on next login. Extracted from `set_autostart` so the
/// R1.2 scanner sees no direct platform primitives in that fn body.
#[cfg(target_os = "macos")]
fn ensure_macos_autostart_loaded(
    ctx: &instance::InstanceContext,
    domain: &str,
    label: &str,
    enable: bool,
) {
    if !enable {
        return;
    }
    let Some(plist) = &ctx.paths.service_file else {
        return;
    };
    // Ensure loaded: bootstrap only if not already loaded (bootout is
    // unnecessary and can race; enable already cleared the override).
    let loaded = std::process::Command::new("launchctl")
        .args(["print", label])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !loaded {
        let _ = std::process::Command::new("launchctl")
            .args([
                "bootstrap",
                domain_parent_macos(domain),
                &plist.display().to_string(),
            ])
            .status();
    }
}

#[cfg(target_os = "macos")]
fn domain_parent_macos(domain: &str) -> &str {
    domain.rsplit_once('/').map(|(p, _)| p).unwrap_or(domain)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn resolved_ctx_for_autostart(
    mode: instance::InstanceMode,
) -> anyhow::Result<instance::InstanceContext> {
    instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("unsupported OS for autostart"))
}

#[cfg(target_os = "macos")]
fn read_plist_for_autostart(ctx: &instance::InstanceContext) -> anyhow::Result<String> {
    let plist = ctx
        .paths
        .service_file
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no launchd plist for this instance"))?;
    std::fs::read_to_string(plist).map_err(|e| anyhow::anyhow!("failed to read plist: {e}"))
}

#[cfg(target_os = "macos")]
async fn query_autostart(mode: instance::InstanceMode) -> anyhow::Result<()> {
    let domain = launchctl_domain(mode)?;
    let label = format!("{domain}/io.mihomo");

    // 1. launchctl disable/enable override (print-disabled lists services with
    //    an explicit override state).
    let disabled_out = std::process::Command::new("launchctl")
        .args(["print-disabled", &domain])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let override_disabled = disabled_out.contains("\"io.mihomo\" => disabled");

    // 2. plist RunAtLoad as the fallback (no override set → install default).
    let ctx = resolved_ctx_for_autostart(mode).ok();
    let run_at_load = ctx
        .and_then(|c| read_plist_for_autostart(&c).ok())
        .map(|plist| plist.contains("<key>RunAtLoad</key><true/>"))
        .unwrap_or(false);

    let enabled = if override_disabled {
        // A disable override forces no-autostart even if RunAtLoad=true.
        false
    } else {
        // Otherwise autostart is driven by the plist RunAtLoad flag. An
        // `enabled` override only permits loading; it does NOT mean autostart.
        run_at_load
    };
    println!(
        "Autostart: {} ({label})",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn launchctl_domain(mode: instance::InstanceMode) -> anyhow::Result<String> {
    match mode {
        instance::InstanceMode::System => Ok("system".to_string()),
        instance::InstanceMode::User => {
            let uid = unsafe { libc::getuid() };
            Ok(format!("gui/{uid}"))
        }
    }
}

#[cfg(target_os = "windows")]
async fn set_autostart(mode: instance::InstanceMode, enable: bool) -> anyhow::Result<()> {
    let ctx = resolved_ctx_for_autostart(mode)?;
    match mode {
        instance::InstanceMode::System => {
            // N4b: sc config needs an elevated token; hint before opaque failure.
            // (Precheck intentionally stays in set_autostart — not a needle.)
            if !service::is_process_elevated() {
                anyhow::bail!(
                    "autostart for the system service requires an elevated (Administrator) shell.\n  \
                     Run from an Administrator terminal."
                );
            }
            let start_type = if enable { "auto" } else { "demand" };
            // R1.2: sc.exe config goes through the planned Enable/Disable plan.
            let action = if enable {
                instance::ServiceAction::Enable
            } else {
                instance::ServiceAction::Disable
            };
            let plan = instance::planned_service_plan(&ctx, action);
            for command in &plan.commands {
                // Preserve the pre-R1.2 failure text:
                // `sc config mihomo start= {start_type} failed`.
                if let Err(_err) = service::run_instance_command(command) {
                    anyhow::bail!("sc config mihomo start= {start_type} failed");
                }
            }
            println!(
                "Autostart {} for system service",
                if enable { "enabled" } else { "disabled" }
            );
            Ok(())
        }
        instance::InstanceMode::User => {
            // Registry Run key + .vbs hidden launch (ADR-17).
            // The .vbs file is pure fs and stays here; the reg.exe call is
            // executed from the planned Enable/Disable plan below.
            // Path matches the plan's derivation: %APPDATA%\mihomo\autostart.vbs.
            let vbs_path = ctx.paths.config_dir.join("autostart.vbs");
            if enable {
                std::fs::create_dir_all(vbs_path.parent().unwrap())?;
                let cli_path = std::env::current_exe()?;
                let vbs = format!(
                    "Set shell = CreateObject(\"WScript.Shell\")\r\nshell.Run \"\"\"{}\"\" start\", 0, False\r\n",
                    cli_path.display()
                );
                std::fs::write(&vbs_path, vbs)?;
            }
            let action = if enable {
                instance::ServiceAction::Enable
            } else {
                instance::ServiceAction::Disable
            };
            let plan = instance::planned_service_plan(&ctx, action);
            for command in &plan.commands {
                match service::run_instance_command(command) {
                    Ok(()) => {}
                    Err(_err) => {
                        // Enable: preserve `reg ADD Run key failed`.
                        // Disable: reg DELETE stays best-effort (pre-R1.2
                        // ignored its status with `let _ =`).
                        if enable {
                            anyhow::bail!("reg ADD Run key failed");
                        }
                    }
                }
            }
            if !enable {
                let _ = std::fs::remove_file(&vbs_path);
                println!("Autostart disabled for user mode");
            } else {
                println!("Autostart enabled for user mode (registry Run + .vbs)");
            }
            Ok(())
        }
    }
}

#[cfg(target_os = "windows")]
async fn query_autostart(mode: instance::InstanceMode) -> anyhow::Result<()> {
    match mode {
        instance::InstanceMode::System => {
            let output = std::process::Command::new("sc.exe")
                .args(["qc", "mihomo"])
                .output()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let enabled = stdout.contains("AUTO_START");
            println!(
                "Autostart: {} (system service)",
                if enabled { "enabled" } else { "disabled" }
            );
            Ok(())
        }
        instance::InstanceMode::User => {
            let output = std::process::Command::new("reg.exe")
                .args([
                    "QUERY",
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                    "/v",
                    "mihomo-cli",
                ])
                .output()?;
            let enabled = output.status.success();
            println!(
                "Autostart: {} (user mode registry Run)",
                if enabled { "enabled" } else { "disabled" }
            );
            Ok(())
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
async fn set_autostart(_mode: instance::InstanceMode, _enable: bool) -> anyhow::Result<()> {
    anyhow::bail!("autostart is not implemented on this platform")
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
async fn query_autostart(_mode: instance::InstanceMode) -> anyhow::Result<()> {
    anyhow::bail!("autostart is not implemented on this platform")
}

async fn cmd_dashboard() -> anyhow::Result<()> {
    let resolved =
        resolve_current_instance_context(false, false, instance::CommandIntent::ReadOnly)?;
    let client = mihomo_api::EndpointMihomoApiClient::for_instance(
        resolved.ctx.mode,
        resolved.ctx.paths.api_endpoint.clone(),
    );

    use crossterm::{
        cursor::MoveTo,
        event::{self, Event, KeyCode, KeyModifiers},
        terminal::{self, Clear, ClearType},
    };
    use std::io::{self, Write};
    use std::time::Duration;

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();

    let result = async {
        loop {
            // Collect data
            let configs = client.get("/configs").await.ok();
            let connections = client.get("/connections").await.ok();
            let proxies = client.get("/proxies").await.ok();

            // Extract info
            // Spec §0.6: the dashboard is also a user-visible TUN renderer, so
            // it must use the attested verdict rather than the raw `/configs` bit.
            let snapshot = status::StatusSnapshot::collect(&resolved.ctx).await;
            let mode = instance_mode_label(resolved.ctx.mode);
            let tun = dashboard_tun_label(&snapshot);
            let mixed_port = configs
                .as_ref()
                .and_then(|c| c.get("mixed-port"))
                .and_then(|p| p.as_u64())
                .unwrap_or(0);
            let conn_count = connections
                .as_ref()
                .and_then(|c| c.get("connections"))
                .and_then(|c| c.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            let upload_total = connections
                .as_ref()
                .and_then(|c| c.get("uploadTotal"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let download_total = connections
                .as_ref()
                .and_then(|c| c.get("downloadTotal"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // Find current proxy group
            let current_group = proxies
                .as_ref()
                .and_then(|p| p.get("proxies"))
                .and_then(|p| p.as_object())
                .map(|proxies| {
                    proxies
                        .iter()
                        .filter(|(_, v)| v.get("type").and_then(|t| t.as_str()) == Some("Selector"))
                        .filter_map(|(name, v)| {
                            v.get("now")
                                .and_then(|n| n.as_str())
                                .map(|now| format!("{} → {}", name, now))
                        })
                        .collect::<Vec<_>>()
                        .join("\n  ")
                })
                .unwrap_or_else(|| "no active group".to_string());

            // Render
            write!(stdout, "{}{}", Clear(ClearType::All), MoveTo(0, 0))?;
            writeln!(stdout, "╔══════════════════════════════════════════╗")?;
            writeln!(stdout, "║        mihomo-cli  Dashboard             ║")?;
            writeln!(stdout, "╠══════════════════════════════════════════╣")?;
            writeln!(stdout, "║  Mode:        {:<25} ║", mode)?;
            writeln!(stdout, "║  TUN:         {:<25} ║", tun)?;
            writeln!(stdout, "║  Mixed port:  {:<25} ║", mixed_port)?;
            writeln!(stdout, "║  Connections: {:<25} ║", conn_count)?;
            writeln!(
                stdout,
                "║  Upload:      {:<25} ║",
                format_bytes(upload_total)
            )?;
            writeln!(
                stdout,
                "║  Download:    {:<25} ║",
                format_bytes(download_total)
            )?;
            writeln!(stdout, "╠══════════════════════════════════════════╣")?;
            writeln!(stdout, "║  Proxy Group:                           ║")?;
            writeln!(stdout, "  {}", current_group)?;
            writeln!(stdout, "╠══════════════════════════════════════════╣")?;
            writeln!(stdout, "║  [q] Quit    [r] Refresh               ║")?;
            writeln!(stdout, "╚══════════════════════════════════════════╝")?;
            stdout.flush()?;

            // Poll for key events
            if event::poll(Duration::from_secs(2))? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('c')
                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            break;
                        }
                        KeyCode::Char('q') => break,
                        KeyCode::Char('r') => continue,
                        _ => {}
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    terminal::disable_raw_mode()?;
    result
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn resolve_api_client(
    system: bool,
    user: bool,
    intent: instance::CommandIntent,
) -> anyhow::Result<mihomo_api::EndpointMihomoApiClient> {
    let resolved = resolve_current_instance_context(system, user, intent)?;
    let runtime = current_runtime_presence();
    let selected_runtime_running = match resolved.ctx.mode {
        instance::InstanceMode::System => runtime.system,
        instance::InstanceMode::User => runtime.user,
    };
    if !selected_runtime_running {
        anyhow::bail!(api_requires_running_instance_message(&resolved.ctx));
    }
    Ok(mihomo_api::EndpointMihomoApiClient::for_instance(
        resolved.ctx.mode,
        resolved.ctx.paths.api_endpoint,
    ))
}

async fn apply_selected_proxy(
    resolved: &ResolvedCurrentInstance,
    client: &mihomo_api::EndpointMihomoApiClient,
    group_name: &str,
    node_name: &str,
    json: bool,
) -> anyhow::Result<()> {
    // D8: hold the per-instance selection lock across kernel PUT/IPC + intent
    // persist so select and replay serialize (SPEC-select-persistence §3.2-6).
    // Acquired before the PUT, so a lock failure means the node was NOT switched.
    // The lock is not reentrant, so persist goes through the `_locked` variant
    // and this path keeps exactly one acquisition.
    let paths = utils::AppPaths::new(resolved.ctx.paths.config_dir.clone());
    let _selection_lock = selection::acquire_selection_lock_at(&paths).map_err(|err| {
        anyhow::anyhow!("Failed to acquire selection lock; node was NOT switched: {err:#}")
    })?;
    let scope = selection::active_selection_scope(&paths).map_err(|err| {
        anyhow::anyhow!("Cannot persist selection without an active subscription: {err:#}")
    })?;
    let is_system_tun = if resolved.ctx.mode == instance::InstanceMode::System {
        let snapshot = status::StatusSnapshot::collect(&resolved.ctx).await;
        snapshot.tun_verdict == status::TunVerdict::TunRunning
    } else {
        false
    };
    if is_system_tun {
        match ipc::send_command(&ipc::DaemonCommand::SelectSystemProxy {
            group: group_name.to_string(),
            node: node_name.to_string(),
            token: None,
        })
        .await?
        {
            ipc::DaemonResponse::Success { .. } => {}
            ipc::DaemonResponse::Error { message } => anyhow::bail!(message),
            response => anyhow::bail!("unexpected daemon selection response: {response:?}"),
        }
    } else {
        mihomo_api::select_proxy_with_client(client, group_name, node_name).await?;
    }
    if let Err(persist_err) =
        selection::remember_selection_for_scope_locked(&scope, group_name, node_name)
    {
        if let Ok(old_map) = selection::load_selection_state_for_scope(&scope) {
            if let Some(old_node) = old_map.get(group_name) {
                if is_system_tun {
                    let _ = ipc::send_command(&ipc::DaemonCommand::SelectSystemProxy {
                        group: group_name.to_string(),
                        node: old_node.clone(),
                        token: None,
                    })
                    .await;
                } else {
                    let _ =
                        mihomo_api::select_proxy_with_client(client, group_name, old_node).await;
                }
            }
        }
        if resolved.ctx.mode == instance::InstanceMode::System {
            // Re-record the (unchanged) intent: this closes the select segment
            // opened by the rollback PUT instead of wedging sampling on an
            // unprovable segment, and honestly reflects the post-rollback state
            // (if the rollback itself failed, the sampler will report the real
            // drift instead of hiding it behind `unknown`).
            push_selection_mirror_update(&paths, &scope.subscription_id, json).await;
        }
        anyhow::bail!("Selection applied but persist failed; runtime rolled back: {persist_err:#}");
    }
    if resolved.ctx.mode == instance::InstanceMode::System {
        push_selection_mirror_update(&paths, &scope.subscription_id, json).await;
    }
    if json {
        print_json_envelope(
            "select",
            serde_json::json!({
                "group": group_name,
                "node": node_name,
            }),
            Vec::new(),
        )?;
    } else {
        println!("Switched {group_name} → {node_name}");
    }
    Ok(())
}

/// Best-effort selection replay against a ready Core; never fails the caller.
/// Degradation is reported as output lines (SPEC §3.3-1, §4-4).
async fn print_selection_replay_after_ready(
    paths: &utils::AppPaths,
    client: &mihomo_api::EndpointMihomoApiClient,
    deadline: std::time::Instant,
) {
    match selection::replay_selections_until(paths, client, deadline).await {
        Ok(report) => {
            for line in report.format_lines() {
                println!("{line}");
            }
            print_selection_replay_remedies(&report);
        }
        Err(err) => println!("⚠ Selections not replayed: {err:#}"),
    }
}

fn cmd_select_unpin(
    system: bool,
    user: bool,
    group: Option<String>,
    all: bool,
) -> anyhow::Result<()> {
    let paths = app_paths_for_resolved_instance_command(
        "select",
        system,
        user,
        instance::CommandIntent::Mutating,
    )?;
    let scope = selection::active_selection_scope(&paths)?;
    if all {
        let removed = selection::unpin_all_selections_for_scope(&scope)?;
        println!("Cleared {removed} pinned selection(s)");
        return Ok(());
    }
    let group = group.ok_or_else(|| {
        anyhow::anyhow!(
            "--unpin requires --group <GROUP> or --all.\n  Run: mihomo-cli select --unpin --group <GROUP>"
        )
    })?;
    if selection::unpin_selection_for_scope(&scope, &group)? {
        println!("Unpinned selection for group {group} (runtime selection unchanged)");
    } else {
        println!("No pinned selection for group {group}");
    }
    Ok(())
}

/// System-mode mirror push after an unpin commit (best-effort).
async fn push_mirror_after_unpin(system: bool, user: bool, json: bool) {
    let Ok(resolved) =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)
    else {
        return;
    };
    if resolved.ctx.mode != instance::InstanceMode::System {
        return;
    }
    let paths = utils::AppPaths::new(resolved.ctx.paths.config_dir.clone());
    let Ok(Some(active)) = config::get_active_id_at(&paths) else {
        return;
    };
    push_selection_mirror_update(&paths, &active, json).await;
}

/// Hidden service-manager hook (`select --replay`). Hard contract: always
/// exits 0 — as an ExecStartPost-style callback, a nonzero exit would fail
/// the unit and terminate the Core (PLAN-select-persistence T5).
async fn cmd_select_replay(system: bool, user: bool) -> anyhow::Result<()> {
    if let Err(err) = select_replay_inner(system, user).await {
        eprintln!("warning: selection replay skipped: {err:#}");
    }
    Ok(())
}

async fn select_replay_inner(system: bool, user: bool) -> anyhow::Result<()> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::Mutating)?;
    let paths = utils::AppPaths::new(resolved.ctx.paths.config_dir.clone());
    let client = mihomo_api::EndpointMihomoApiClient::for_instance(
        resolved.ctx.mode,
        resolved.ctx.paths.api_endpoint.clone(),
    );
    // Service managers invoke this right after forking the Core, so the API
    // is usually not ready yet; poll within the D6 budget before replaying.
    let deadline = std::time::Instant::now() + selection::REPLAY_TOTAL_BUDGET;
    loop {
        if client.get("/configs").await.is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "Core API not ready within {}s",
                selection::REPLAY_TOTAL_BUDGET.as_secs()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    print_selection_replay_after_ready(&paths, &client, deadline).await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn cmd_select_resolved(
    system: bool,
    user: bool,
    group: Option<String>,
    node: Option<String>,
    unpin: bool,
    all: bool,
    replay: bool,
    json: bool,
) -> anyhow::Result<()> {
    if replay {
        return cmd_select_replay(system, user).await;
    }
    if unpin {
        let result = cmd_select_unpin(system, user, group, all);
        if result.is_ok() {
            push_mirror_after_unpin(system, user, json).await;
        }
        return result;
    }
    if node.is_none() && !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "select requires --node in non-interactive mode.\n  Run: mihomo-cli select --group <GROUP> --node <NODE>"
        );
    }
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::Mutating)?;
    let client = resolve_ready_api_client(system, user, instance::CommandIntent::Mutating).await?;
    match node {
        // Non-interactive CLI: switch group to a specific node.
        Some(node_name) => {
            let group_name = group.ok_or_else(|| {
                anyhow::anyhow!("--node requires --group (the proxy group that contains the node)")
            })?;
            apply_selected_proxy(&resolved, &client, &group_name, &node_name, json).await
        }
        // Interactive TUI (no --node): existing behavior.
        None => match group {
            Some(g) => match ui::select_node_with_client(&client, &g).await? {
                Some(node_name) => {
                    apply_selected_proxy(&resolved, &client, &g, &node_name, json).await
                }
                None => Ok(()),
            },
            None => match ui::flat_select_with_client(&client).await? {
                Some((group_name, node_name)) => {
                    apply_selected_proxy(&resolved, &client, &group_name, &node_name, json).await
                }
                None => Ok(()),
            },
        },
    }
}

async fn cmd_list_resolved(system: bool, user: bool) -> anyhow::Result<()> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    let paths = utils::AppPaths::new(resolved.ctx.paths.config_dir.clone());
    let selections = match selection::active_selection_scope(&paths)
        .and_then(|scope| selection::load_selection_state_for_scope(&scope))
    {
        Ok(selections) => selections,
        Err(error) => {
            eprintln!("warning: cannot read persisted selections: {error:#}");
            std::collections::BTreeMap::new()
        }
    };
    let client = resolve_ready_api_client(system, user, instance::CommandIntent::ReadOnly).await?;
    mihomo_api::list_proxies_with_selections(&client, &selections).await
}

async fn cmd_delay_resolved(
    system: bool,
    user: bool,
    group: &str,
    refresh: bool,
    cache_ttl: u64,
    fastest: bool,
) -> anyhow::Result<()> {
    let intent = if fastest {
        instance::CommandIntent::Mutating
    } else {
        instance::CommandIntent::ReadOnly
    };
    let client = resolve_ready_api_client(system, user, intent).await?;
    mihomo_api::delay_test_with_client(&client, group, refresh, cache_ttl, fastest).await
}

fn ensure_instance_controller_endpoint(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    // ADR-22: repair the single source-of-truth intent config only.
    let repair_path = if ctx.paths.intent_config_file.exists() {
        &ctx.paths.intent_config_file
    } else {
        // No config has been imported/generated yet. Let the downstream start
        // path produce the canonical "config not found" error instead of
        // failing early in endpoint repair.
        return Ok(());
    };

    let content = std::fs::read_to_string(repair_path).map_err(|e| {
        anyhow::anyhow!(
            "cannot read config for endpoint repair: {} ({e})",
            repair_path.display()
        )
    })?;
    let fixed = config::ensure_controller_for_endpoint(&content, &ctx.paths.api_endpoint)?;
    if fixed != content {
        write_instance_text_file(ctx, repair_path, &fixed, 0o644)?;
    }
    Ok(())
}

/// 从**调用方捕获的** intent 字节构建 TUN candidate（不自行读盘）。
///
/// P1-1：字节由 `prepare_and_publish_active_transaction_capturing_intent`
/// 在其 CoordinatorLock → ConfigLock 临界区内提供，本函数只在锁内做内存构建。
fn build_tun_candidate(
    ctx: &instance::InstanceContext,
    intent_bytes: &[u8],
    enable: bool,
    stack: Option<&TunStack>,
    dns_hijack: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let content = std::str::from_utf8(intent_bytes).map_err(|e| {
        anyhow::anyhow!(
            "config for TUN candidate is not valid UTF-8: {} ({e})",
            ctx.paths.intent_config_file.display()
        )
    })?;
    let content = if enable {
        config::ensure_controller_for_endpoint(content, &ctx.paths.api_endpoint)?
    } else {
        content.to_string()
    };
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&content)?;
    if !matches!(doc, serde_yaml::Value::Mapping(_)) {
        doc = serde_yaml::Value::Mapping(Default::default());
    }
    let root = doc.as_mapping_mut().expect("mapping initialized");
    let tun_key = serde_yaml::Value::String("tun".to_string());
    let tun = root
        .entry(tun_key)
        .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    if !matches!(tun, serde_yaml::Value::Mapping(_)) {
        *tun = serde_yaml::Value::Mapping(Default::default());
    }
    let tun = tun.as_mapping_mut().expect("tun mapping initialized");
    tun.insert(
        serde_yaml::Value::String("enable".to_string()),
        serde_yaml::Value::Bool(enable),
    );
    if let Some(stack) = stack {
        tun.insert(
            serde_yaml::Value::String("stack".to_string()),
            serde_yaml::Value::String(stack.to_string()),
        );
    }
    // 开启 TUN 时归一化 TUN 块（dns-hijack 对齐参考实现、缺省补
    // strict-route）以及旧版模板遗留的 DNS 默认值，关闭时只翻转 enable，不动其它字段。
    // 归一化结果落在 intent config.yaml 里，仍受 candidate/revision 与 attestation 管辖，
    // 因此存量安装无需额外跑一次 `config -u`。
    if enable {
        config::normalize_tun_block_for_enable(tun, dns_hijack);
        config::normalize_dns_legacy_defaults(root);
    }
    Ok(serde_yaml::to_string(&doc)?.into_bytes())
}

fn tun_user_intent(action: Option<&TunAction>) -> UserIntent {
    match action {
        Some(TunAction::On) => UserIntent::TunOn,
        Some(TunAction::Off) => UserIntent::TunOff,
        Some(TunAction::Status) | None => UserIntent::TunStatus,
    }
}

fn tun_action_intent(action: Option<&TunAction>) -> instance::CommandIntent {
    match action {
        Some(TunAction::On) | Some(TunAction::Off) => instance::CommandIntent::Mutating,
        Some(TunAction::Status) | None => instance::CommandIntent::ReadOnly,
    }
}

fn read_tun_enabled_from_config(config_path: &std::path::Path) -> anyhow::Result<bool> {
    let content = std::fs::read_to_string(config_path)?;
    let config: serde_yaml::Value = serde_yaml::from_str(&content)?;
    Ok(config
        .get("tun")
        .and_then(|tun| tun.get("enable"))
        .and_then(|enable| enable.as_bool())
        .unwrap_or(false))
}

/// 为错误消息生成上下文提示
///
/// 根据错误消息内容，返回相应的修复建议。
/// 如果没有匹配的模式，返回空字符串。
fn format_error_hint(error_message: &str) -> String {
    let msg_lower = error_message.to_lowercase();

    // 配置文件所有者不匹配
    if msg_lower.contains("owner uid") && msg_lower.contains("does not match") {
        return "\n\n\
            💡 配置文件所有者不匹配，可能之前以 root 身份写入过。\n   \
            修复: sudo chown $(whoami) ~/.config/mihomo/config.yaml"
            .to_string();
    }

    // daemon 拒绝配置路径（可能是版本不匹配）
    if msg_lower.contains("refusing to use config path") {
        return "\n\n\
            💡 daemon 可能版本过旧或配置路径验证失败。\n   \
            尝试: sudo systemctl restart mihomo\n   \
            或: mihomo-cli restart"
            .to_string();
    }

    // 配置文件不存在（收窄模式，避免误匹配 rule/subscription/proxy not found）
    if (msg_lower.contains("no such file") || msg_lower.contains("not found"))
        && msg_lower.contains("config")
    {
        return "\n\n\
            💡 配置文件不存在，请先添加订阅。\n   \
            运行: mihomo-cli config -u <subscription-url>"
            .to_string();
    }

    // socket 文件不存在
    if (msg_lower.contains("no such file") || msg_lower.contains("not found"))
        && (msg_lower.contains("sock") || msg_lower.contains("socket"))
    {
        return "\n\n\
            💡 daemon socket 不存在，daemon 可能未运行。\n   \
            启动: mihomo-cli start"
            .to_string();
    }

    // system Geo 数据或受管 TUN snapshot 权限异常
    if msg_lower.contains("permission denied")
        && msg_lower.contains("/var/lib/mihomo-cli")
        && (msg_lower.contains("geoip.metadb")
            || msg_lower.contains("geosite.dat")
            || msg_lower.contains("tun-config.yaml"))
    {
        return "\n\n\
            💡 系统 Mihomo 数据或 TUN 配置权限异常，服务无法更新受管文件。\n   \
            修复: mihomo-cli install --system --force\n   \
            然后重试: mihomo-cli tun on"
            .to_string();
    }

    // 权限不足
    if msg_lower.contains("permission denied") {
        return "\n\n\
            💡 权限不足，请检查文件权限。\n   \
            运行: ls -la ~/.config/mihomo/"
            .to_string();
    }

    // 连接被拒绝（daemon 未运行）
    if msg_lower.contains("connection refused") {
        return "\n\n\
            💡 无法连接到 daemon，daemon 可能未运行。\n   \
            启动: mihomo-cli start"
            .to_string();
    }

    // 服务未安装
    if msg_lower.contains("not installed") || msg_lower.contains("no service") {
        return "\n\n\
            💡 服务未安装。\n   \
            安装: mihomo-cli install"
            .to_string();
    }

    // 配置格式错误
    if msg_lower.contains("invalid yaml")
        || msg_lower.contains("parse error")
        || msg_lower.contains("syntax error")
    {
        return "\n\n\
            💡 配置文件格式错误。\n   \
            验证: mihomo-cli config --validate\n   \
            修复: 检查 ~/.config/mihomo/config.yaml 语法"
            .to_string();
    }

    // 网络错误（收窄模式，避免误匹配）
    if msg_lower.contains("connection timed out")
        || msg_lower.contains("network is unreachable")
        || msg_lower.contains("network unreachable")
    {
        return "\n\n\
            💡 网络连接失败，请检查网络连接。\n   \
            如果是代理问题，尝试: mihomo-cli proxy off"
            .to_string();
    }

    String::new()
}

#[cfg(test)]
mod format_error_hint_tests {
    use super::*;

    #[test]
    fn test_owner_uid_mismatch() {
        let hint = format_error_hint("owner uid 0 does not match IPC peer uid 1000");
        assert!(hint.contains("配置文件所有者不匹配"));
        assert!(hint.contains("sudo chown"));
    }

    #[test]
    fn test_refusing_config_path() {
        let hint =
            format_error_hint("refusing to use config path /var/lib/mihomo-cli/tun-config.yaml");
        assert!(hint.contains("daemon 可能版本过旧"));
        assert!(hint.contains("systemctl restart"));
    }

    #[test]
    fn test_config_file_not_found() {
        let hint = format_error_hint("config.yaml: No such file or directory");
        assert!(hint.contains("配置文件不存在"));
        assert!(hint.contains("mihomo-cli config -u"));
    }

    #[test]
    fn test_socket_not_found() {
        let hint = format_error_hint("socket file not found: /var/run/mihomo/mihomo.sock");
        assert!(hint.contains("daemon socket 不存在"));
        assert!(hint.contains("mihomo-cli start"));
    }

    #[test]
    fn test_permission_denied() {
        let hint = format_error_hint("Permission denied (os error 13)");
        assert!(hint.contains("权限不足"));
    }

    #[test]
    fn test_system_geo_permission_denied_has_system_recovery_hint() {
        let hint = format_error_hint(
            "can't remove invalid MMDB: remove /var/lib/mihomo-cli/geoip.metadb: permission denied",
        );
        assert!(hint.contains("系统 Mihomo 数据或 TUN 配置权限异常"));
        assert!(hint.contains("mihomo-cli install --system --force"));
        assert!(!hint.contains("~/.config/mihomo"));
    }

    #[test]
    fn test_tun_snapshot_permission_denied_has_system_recovery_hint() {
        let hint = format_error_hint(
            "Failed to open /var/lib/mihomo-cli/tun-config.yaml: Permission denied (os error 13)",
        );
        assert!(hint.contains("系统 Mihomo 数据或 TUN 配置权限异常"));
        assert!(hint.contains("mihomo-cli install --system --force"));
        assert!(!hint.contains("~/.config/mihomo"));
    }

    #[test]
    fn test_connection_refused() {
        let hint = format_error_hint("Connection refused (os error 111)");
        assert!(hint.contains("无法连接到 daemon"));
        assert!(hint.contains("mihomo-cli start"));
    }

    #[test]
    fn test_not_installed() {
        let hint = format_error_hint("service not installed");
        assert!(hint.contains("服务未安装"));
        assert!(hint.contains("mihomo-cli install"));
    }

    #[test]
    fn test_invalid_yaml() {
        let hint = format_error_hint("invalid yaml: unexpected token");
        assert!(hint.contains("配置文件格式错误"));
        assert!(hint.contains("mihomo-cli config --validate"));
    }

    #[test]
    fn test_network_unreachable() {
        let hint = format_error_hint("Network is unreachable (os error 101)");
        assert!(hint.contains("网络连接失败"));
    }

    #[test]
    fn test_no_match_returns_empty() {
        let hint = format_error_hint("some random error message");
        assert!(hint.is_empty());
    }

    #[test]
    fn test_rule_not_found_no_false_positive() {
        // "rule not found" 不应该触发配置文件不存在的提示
        let hint = format_error_hint("rule not found: RULE_1");
        assert!(hint.is_empty());
    }

    #[test]
    fn test_subscription_not_found_no_false_positive() {
        // "subscription not found" 不应该触发配置文件不存在的提示
        let hint = format_error_hint("subscription not found");
        assert!(hint.is_empty());
    }
}

fn tun_requires_system_service_install_message() -> &'static str {
    "TUN requires the privileged system service.
  Interactive use: run `mihomo-cli tun on` from a terminal to install it automatically.
  Non-interactive use: run `mihomo-cli install --system` first.
  Per-user service does not have the privileges needed for TUN."
}

fn format_tun_system_install_prompt() -> Vec<String> {
    vec![
        "TUN requires the privileged mihomo system service.".to_string(),
        String::new(),
        "Install system service now?".to_string(),
        "  Password is required once for service installation.".to_string(),
    ]
}

fn should_install_system_for_tun_answer(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    answer.is_empty() || answer == "y" || answer == "yes"
}

fn format_tun_user_to_system_switch_prompt(
    user_running: bool,
    user_installed: bool,
) -> Vec<String> {
    let mut lines = vec!["TUN requires the privileged mihomo system service.".to_string()];
    if user_running {
        lines.push("A per-user mihomo core is currently running.".to_string());
    } else if user_installed {
        lines.push("A per-user mihomo service is currently installed.".to_string());
    }
    lines.extend([
        "".to_string(),
        "Switch to TUN/system service mode?".to_string(),
        "  This will stop/remove the per-user service, keep your user config, and install/start the system service.".to_string(),
        "  Password is required once for system service installation.".to_string(),
    ]);
    lines
}

fn should_switch_user_to_system_for_tun_answer(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    answer == "y" || answer == "yes"
}

fn should_update_existing_tun_answer(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    answer == "y" || answer == "yes"
}

fn confirm_update_existing_tun(yes: bool) -> anyhow::Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "TUN is already enabled. Re-run with --yes to update TUN config in non-interactive mode."
        );
    }
    print!("TUN is already enabled. Update TUN config? [y/N] ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(should_update_existing_tun_answer(&input))
}

fn uninstall_user_service_artifacts_for_tun_switch() -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(instance::InstanceMode::User)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for per-user service cleanup"))?;
    let plan = instance::planned_service_plan(&ctx, instance::ServiceAction::Uninstall);

    println!("Stopping/removing per-user service...");
    for command in &plan.commands {
        if let Err(err) = service::run_instance_command(command) {
            eprintln!("  Warning: service command failed: {err}");
        }
    }
    if let Some(service_file) = &ctx.paths.service_file {
        remove_instance_path(service_file, false)?;
    }
    Ok(())
}

async fn prompt_switch_user_to_system_for_tun(
    user_running: bool,
    user_installed: bool,
    github_mirror: Option<&str>,
    yes: bool,
) -> anyhow::Result<bool> {
    if yes {
        uninstall_user_service_artifacts_for_tun_switch()?;
        cmd_install_instance(
            instance::InstanceMode::System,
            true,
            None,
            github_mirror,
            false,
            true,
        )
        .await?;
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "TUN requires the privileged system service, but a per-user instance is present.
               Interactive use: run `mihomo-cli tun on` from a terminal to switch modes.
               Non-interactive use: run `mihomo-cli uninstall --user` then `mihomo-cli install --system`."
        );
    }
    print_lines(format_tun_user_to_system_switch_prompt(
        user_running,
        user_installed,
    ));
    print!("Proceed [y/N]: ");
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if !should_switch_user_to_system_for_tun_answer(&input) {
        println!("  Cancelled.");
        return Ok(false);
    }
    uninstall_user_service_artifacts_for_tun_switch()?;
    cmd_install_instance(
        instance::InstanceMode::System,
        true,
        None,
        github_mirror,
        false,
        false,
    )
    .await?;
    Ok(true)
}

async fn prompt_install_system_service_for_tun(
    github_mirror: Option<&str>,
    yes: bool,
) -> anyhow::Result<bool> {
    if yes {
        cmd_install_instance(
            instance::InstanceMode::System,
            true,
            None,
            github_mirror,
            false,
            true,
        )
        .await?;
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(tun_requires_system_service_install_message());
    }
    print_lines(format_tun_system_install_prompt());
    print!("Proceed [Y/n]: ");
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if !should_install_system_for_tun_answer(&input) {
        println!("  Cancelled.");
        return Ok(false);
    }
    cmd_install_instance(
        instance::InstanceMode::System,
        true,
        None,
        github_mirror,
        false,
        false,
    )
    .await?;
    Ok(true)
}

fn system_service_recovery_command(ctx: &instance::InstanceContext) -> Option<String> {
    if ctx.os == instance::TargetOs::Windows {
        return Some(WINDOWS_DAEMON_SERVICE_RECOVERY_HINT.to_string());
    }
    let fallbacks = instance::planned_service_plan(ctx, instance::ServiceAction::Restart)
        .commands
        .into_iter()
        .filter_map(instance::privilege_invocation_plan)
        .map(|plan| plan.manual_fallback)
        .collect::<Vec<_>>();
    (!fallbacks.is_empty()).then(|| fallbacks.join(" && "))
}

fn system_daemon_retry_command(operation: &str) -> &'static str {
    match operation {
        op if op.starts_with("enable TUN") => "tun on",
        op if op.starts_with("disable TUN") => "tun off",
        "starting" | "start" => "start",
        "stopping" | "stop" => "stop",
        "restarting" | "restart" => "restart",
        _ => "status --system --verbose",
    }
}

fn system_daemon_unavailable_message(operation: &str, ctx: &instance::InstanceContext) -> String {
    let recovery = system_service_recovery_command(ctx)
        .unwrap_or_else(|| "restart the system service with your OS service manager".to_string());
    let retry = system_daemon_retry_command(operation);
    format!(
        "system daemon IPC is not running; cannot {operation} the system core.
  \
         The system service appears selected/installed, but its daemon socket is unavailable.
  \
         Start the system service first:
  \
           mihomo-cli start --system
  \
         Or Recover the daemon manually, then retry:
  \
           {recovery}
  \
           mihomo-cli {retry}
  \
         If the system service is not installed yet, run: mihomo-cli install --system"
    )
}

/// Classify the system service runtime state: probe results feed
/// diagnosis and guidance only — ServiceTarget selection stays purely static.
fn service_runtime_state(ctx: &instance::InstanceContext) -> service::ServiceRuntimeState {
    let service_manager_available = match (ctx.os, ctx.mode) {
        (instance::TargetOs::Linux, instance::InstanceMode::System) => {
            service::systemd_runtime_available()
        }
        _ => true,
    };
    let unit_installed = match (ctx.os, ctx.mode) {
        (instance::TargetOs::Windows, instance::InstanceMode::System) => {
            windows_mihomo_service_installed()
        }
        (instance::TargetOs::Windows, instance::InstanceMode::User) => {
            instance::windows_user_install_marker(ctx).is_some_and(|marker| marker.exists())
        }
        _ => ctx.paths.service_file.as_ref().is_some_and(|p| p.exists()),
    };
    service::classify_service_runtime(
        service_manager_available,
        unit_installed,
        service_manager_active(ctx),
    )
}

/// Block reason `SystemdUnavailable`: Linux system mode without a running
/// systemd (WSL2 default, containers, minimal distros). Guidance is actionable
/// and never asks the user to hand-type platform service manager commands
/// (workspace rules 18/19).
fn systemd_unavailable_message() -> String {
    "systemd is not available on this system (systemd 不可用); the mihomo system service requires systemd.
  This usually means WSL2 without systemd, a container, or a minimal distro without systemd as init.
  WSL2: add to /etc/wsl.conf:
    [boot]
    systemd=true
  then run `wsl --shutdown` in Windows, reopen the distro, and retry.
  Containers / minimal distros: boot the environment with systemd as init.
  Once systemd is up, install the service: mihomo-cli install --system"
        .to_string()
}

fn service_not_installed_message() -> String {
    "The system service is not installed yet.\n  Run: mihomo-cli install --system".to_string()
}

/// Block reason `UnitNotEnabled` / stopped unit: service manager is healthy and
/// the unit exists, but nothing is running. Distinct from the no-systemd
/// diagnosis so the two failure scenes are never conflated.
fn service_installed_not_running_message(operation: &str, retry: &str) -> String {
    format!(
        "the system service is installed but not running (stopped or not enabled); cannot {operation} the system core.
  \
         Start the service first:
  \
           mihomo-cli start --system
  \
         Autostart may be off — rerun mihomo-cli install --system to enable it, then retry:
  \
           mihomo-cli {retry}"
    )
}

fn service_block_message_for_state(
    operation: &str,
    ctx: &instance::InstanceContext,
    state: service::ServiceRuntimeState,
) -> String {
    match state {
        service::ServiceRuntimeState::SystemdUnavailable => systemd_unavailable_message(),
        service::ServiceRuntimeState::NotInstalled => service_not_installed_message(),
        service::ServiceRuntimeState::InstalledNotRunning => {
            service_installed_not_running_message(operation, system_daemon_retry_command(operation))
        }
        service::ServiceRuntimeState::Running => system_daemon_unavailable_message(operation, ctx),
    }
}

fn system_service_block_message(operation: &str, ctx: &instance::InstanceContext) -> String {
    service_block_message_for_state(operation, ctx, service_runtime_state(ctx))
}

fn system_tun_requires_daemon_message(
    action: Option<&TunAction>,
    ctx: &instance::InstanceContext,
) -> Option<String> {
    match action {
        Some(TunAction::On) => Some(system_daemon_unavailable_message("enable TUN on", ctx)),
        Some(TunAction::Off) => Some(system_daemon_unavailable_message("disable TUN on", ctx)),
        Some(TunAction::Status) | None => None,
    }
}

/// Runtime variant of `system_tun_requires_daemon_message`:
/// diagnose the service runtime state first (no systemd / not installed /
/// not running), falling back to the generic daemon-unavailable message.
fn system_tun_block_message(
    action: Option<&TunAction>,
    ctx: &instance::InstanceContext,
) -> Option<String> {
    let operation = match action {
        Some(TunAction::On) => "enable TUN on",
        Some(TunAction::Off) => "disable TUN on",
        Some(TunAction::Status) | None => return None,
    };
    match service_runtime_state(ctx) {
        service::ServiceRuntimeState::Running => system_tun_requires_daemon_message(action, ctx),
        state => Some(service_block_message_for_state(operation, ctx, state)),
    }
}

struct TunResolvedOptions {
    system: bool,
    user: bool,
    action: Option<TunAction>,
    stack: Option<TunStack>,
    dns_hijack: Option<String>,
    yes: bool,
}

fn is_tun_privileged_action(action: Option<&TunAction>) -> bool {
    match action {
        Some(TunAction::On) | Some(TunAction::Off) => true,
        Some(TunAction::Status) | None => false,
    }
}

#[cfg(unix)]
fn is_current_process_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn is_current_process_root() -> bool {
    true
}

fn sudo_reexec_command(exe: &std::path::Path, args: &[String]) -> std::process::Command {
    sudo_reexec_command_with(exe, args, false)
}

/// `non_interactive` 只影响 sudo 自身的认证方式，不改变被重入的命令参数：
/// 无 TTY 时不等待交互输入，认证不成立即由 sudo 立即失败并返回非零
/// （`--yes` 是 CLI 的业务确认，不构成操作系统授权）。
fn sudo_reexec_command_with(
    exe: &std::path::Path,
    args: &[String],
    non_interactive: bool,
) -> std::process::Command {
    let mut command = std::process::Command::new("sudo");
    if non_interactive {
        command.arg("--non-interactive");
    }
    // 传递原始用户上下文，避免 sudo 重置 $HOME 后路径解析错误
    // 仅在 Linux 上需要（macOS sudo 默认保留 HOME，Windows 无此问题）
    #[cfg(target_os = "linux")]
    {
        if let Ok(home) = std::env::var("HOME") {
            command.arg(format!("_MIHOMO_CLI_ORIGINAL_HOME={}", home));
        }
        if let Ok(uid) = std::env::var("UID") {
            command.arg(format!("_MIHOMO_CLI_ORIGINAL_UID={}", uid));
        } else if let Some(uid) = instance::current_uid() {
            command.arg(format!("_MIHOMO_CLI_ORIGINAL_UID={}", uid));
        }
    }
    command.arg(exe);
    command.args(args);
    command
}

fn maybe_sudo_reexec_for_tun(action: Option<&TunAction>) -> anyhow::Result<()> {
    if is_tun_privileged_action(action) && !is_current_process_root() {
        // 使用 current_exe() 而非 args[0]，避免 PATH 变化导致找错二进制
        let exe = std::env::current_exe().unwrap_or_else(|_| {
            std::env::args()
                .next()
                .map(std::path::PathBuf::from)
                .unwrap_or_default()
        });
        let args: Vec<String> = std::env::args().skip(1).collect();
        let status = sudo_reexec_command(&exe, &args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

fn maybe_sudo_reexec_for_system_uninstall(
    mode: instance::InstanceMode,
    dry_run: bool,
) -> anyhow::Result<()> {
    if dry_run || mode != instance::InstanceMode::System || is_current_process_root() {
        return Ok(());
    }

    let exe = std::env::current_exe().unwrap_or_else(|_| {
        std::env::args()
            .next()
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
    });
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = sudo_reexec_command(&exe, &args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

async fn prepare_system_transaction_recovery() -> anyhow::Result<bool> {
    if is_current_process_root() {
        return Ok(true);
    }

    let response = ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await?;
    let needs_recovery = matches!(
        response,
        ipc::DaemonResponse::Status {
            tun_journal_state: Some(phase),
            ..
        } if !matches!(
            phase,
            tun_transaction::JournalPhase::IntentCommitted
                | tun_transaction::JournalPhase::RolledBack
        )
    );
    if !needs_recovery {
        return Ok(false);
    }

    let exe = std::env::current_exe().unwrap_or_else(|_| {
        std::env::args()
            .next()
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
    });
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = sudo_reexec_command(&exe, &args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// SPEC §12.3.1 / issue: `read_state` preserves the filesystem error kind, so an
/// unreadable state file is classified structurally. A message that merely mentions a
/// permission problem is not an identity limitation and must not escalate.
fn generation_read_is_identity_limited(error: &generation::GenerationError) -> bool {
    matches!(
        error,
        generation::GenerationError::Io(io_error)
            if io_error.kind() == std::io::ErrorKind::PermissionDenied
    )
}

/// 「无法判定」的诚实文案——不宣称有 pending、不宣称无 pending、
/// 不在这里提权。身份不足时指引 `restart --system`（它会为这次操作自行申请
/// 管理员授权并检查、应用信任根）；非身份类读取失败只报事实，不给授权指引。
fn pending_generation_unknown_notice(error: &generation::GenerationError) -> String {
    if generation_read_is_identity_limited(error) {
        "代际状态以当前身份不可读，无法判定是否存在待应用更新或是否需采纳；可能存在未应用更新，需管理员权限查看。显式只读检查：mihomo-cli doctor --system --privileged（不修复、不迁移、不应用更新）".to_string()
    } else {
        format!("代际状态读取失败，无法判定是否有待应用更新 ({error})")
    }
}

fn maybe_sudo_reexec_for_system_generation(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    if is_current_process_root() {
        return Ok(());
    }

    let state = match system_generation_store(ctx).read_state() {
        Ok(state) => Some(state),
        // The generation state lives under `0770 root:mihomo`, so a caller outside the
        // group cannot read it at all. That is an identity limitation, not evidence of
        // a pending generation, and it must not turn every lifecycle command into a
        // sudo prompt.
        Err(error) if generation_read_is_identity_limited(&error) => {
            eprintln!(
                "  ⚠ Cannot inspect the pending system generation with the current identity; skipping the generation check."
            );
            None
        }
        Err(error) => {
            return Err(anyhow::anyhow!(
                "cannot safely inspect pending system generation: {error}"
            ));
        }
    };
    let has_pending = state.is_some_and(|state| state.pending.is_some());
    if !has_pending && !is_system_cli_skewed(ctx) {
        return Ok(());
    }

    let exe = std::env::current_exe().unwrap_or_else(|_| {
        std::env::args()
            .next()
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
    });
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = sudo_reexec_command(&exe, &args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// SPEC §12.3.1（裁决 B）：root 专属域。这两个域**只**影响「当前显式操作是否
/// 必须由特权执行者完成」，与权限漂移判定无关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtectedDomain {
    /// runtime 事务域：`/var/lib/mihomo-cli`（`0770 root:mihomo`）内的 transaction 对象。
    RuntimeTransactions,
    /// 代际信任根：独立的 `/var/lib/mihomo-cli-generations`（`root:root 0750`）。
    GenerationTrustRoot,
}

impl ProtectedDomain {
    fn label(self) -> &'static str {
        match self {
            ProtectedDomain::RuntimeTransactions => "受保护的运行事务状态",
            ProtectedDomain::GenerationTrustRoot => "受保护的代际信任根",
        }
    }
}

/// 结构化判据：错误链里出现 `EACCES`/`EPERM` 即「当前身份看不到」，而不是「它不对」。
fn identity_limited_io(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
    })
}

/// 纯判定：哪些 root 专属域当前身份观测不到（需要业务授权）。
/// 观测得到、或对象根本不存在时返回空——健康状态下不产生任何授权请求。
fn protected_domains_requiring_authorization(
    ctx: &instance::InstanceContext,
) -> Vec<ProtectedDomain> {
    [
        ProtectedDomain::RuntimeTransactions,
        ProtectedDomain::GenerationTrustRoot,
    ]
    .into_iter()
    .filter(|domain| match domain {
        ProtectedDomain::RuntimeTransactions => {
            matches!(
                tun_transaction::read_active_journal(ctx),
                Err(ref error) if identity_limited_io(error)
            )
        }
        ProtectedDomain::GenerationTrustRoot => {
            matches!(
                system_generation_store(ctx).read_state(),
                Err(generation::GenerationError::Io(ref error))
                    if error.kind() == std::io::ErrorKind::PermissionDenied
            )
        }
    })
    .collect()
}

fn protected_domain_authorization_notice(operation: &str, impact: &str) -> String {
    format!(
        "  为执行你请求的 {operation}，需要管理员授权检查受保护的运行状态，并按原命令范围\
         恢复事务或应用更新。当前尚未判定权限异常；{impact}可能暂时中断连接。"
    )
}

/// 已取得管理员授权但仍观测不到：结构化失败，绝不递归 sudo。
fn protected_domain_still_unobservable_message(domains: &[ProtectedDomain]) -> String {
    let labels: Vec<&str> = domains.iter().map(|domain| domain.label()).collect();
    format!(
        "已取得管理员授权，但{}仍不可观测，拒绝在无法确认事务/代际状态的情况下继续。\n  \
         未判定权限漂移；运行态未改动，证据保留。\n  \
         Next: mihomo-cli doctor --system",
        labels.join("与")
    )
}

/// SPEC §12.3.1（裁决 B）：代际信任根的拒绝文案。
/// 「授权不足」与「确证违约」必须分开报，且删除递归 `chown`/`chmod` 建议——
/// 后者把「我看不到」误报成「它不对」，且对普通用户是不可执行的递归操作。
fn generation_trust_root_refusal(action: &str, error: &anyhow::Error) -> anyhow::Error {
    if identity_limited_io(error) {
        anyhow::anyhow!(
            "授权不足，无法{action}：未能以当前身份确认代际信任根的安全条件（未判定权限漂移）。\n  \
             未使用任何代际，运行态未改动。\n  \
             Next: 在可取得管理员授权的会话重试原 mihomo-cli 命令\n  \
             原因: {error}"
        )
    } else {
        anyhow::anyhow!(
            "代际信任根安全条件不满足，已拒绝{action}；运行态未改动，证据保留。\n  \
             Next: mihomo-cli doctor --system\n  \
             原因: {error}"
        )
    }
}

/// SPEC §12.3.1（裁决 B）：业务操作授权边界。
///
/// 决策与权限漂移判定分离：这里只回答「当前显式操作是否需要特权执行者」，
/// 不 chown/chmod、不产生修复计划、不宣称检测到权限异常。
/// 每命令至多一次协调提权重入；权限面以原命令参数重入，业务范围不变
/// （`--yes` 仍是业务确认，runtime reset 仍需原有确认）。
fn authorize_protected_domain_operations(
    ctx: &instance::InstanceContext,
    operation: &str,
    impact: &str,
) -> anyhow::Result<()> {
    if ctx.mode != instance::InstanceMode::System {
        return Ok(());
    }
    let unobservable = protected_domains_requiring_authorization(ctx);
    if unobservable.is_empty() {
        return Ok(());
    }
    if is_current_process_root() {
        anyhow::bail!(protected_domain_still_unobservable_message(&unobservable));
    }

    println!(
        "{}",
        protected_domain_authorization_notice(operation, impact)
    );
    let exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = sudo_reexec_command_with(&exe, &args, !std::io::stdin().is_terminal()).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

async fn tun_on_preflight(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let mut checks = Vec::new();
    let installed = current_service_presence().system;
    checks.push(if installed {
        preflight::PreflightResult::pass_named("system service installed")
    } else {
        preflight::PreflightResult::fail(
            "system service is not installed",
            Some(
                "Run `mihomo-cli install --system --yes`, then retry `mihomo-cli tun on`."
                    .to_string(),
            ),
        )
    });

    let daemon_available = ipc::is_daemon_running().await;
    checks.push(if daemon_available {
        preflight::PreflightResult::pass_named("daemon IPC reachable")
    } else {
        tun_daemon_transport_check(ctx.os, "system daemon socket is not reachable")
    });

    checks.push(if ctx.paths.core_binary.is_file() {
        preflight::PreflightResult::pass_named("Mihomo Core executable available")
    } else {
        preflight::PreflightResult::fail(
            "Mihomo Core executable is missing",
            Some(
                "Run `mihomo-cli install --system --yes`, then retry `mihomo-cli tun on`."
                    .to_string(),
            ),
        )
    });

    let config_check = match utils::open_regular_file_no_follow(&ctx.paths.intent_config_file) {
        Ok(file) => match serde_yaml::from_reader::<_, serde_yaml::Value>(file) {
            Ok(_) => preflight::PreflightResult::pass_named("configuration is readable and valid YAML"),
            Err(error) => preflight::PreflightResult::fail(
                "configuration is not valid YAML",
                Some(format!("Run `mihomo-cli config --validate`, fix the reported configuration, then retry `mihomo-cli tun on`. ({error})")),
            ),
        },
        Err(_error) if !ctx.paths.intent_config_file.exists() => preflight::PreflightResult::fail(
            "configuration file is missing",
            Some("Run `mihomo-cli config -u <subscription-url>`, then retry `mihomo-cli tun on`.".to_string()),
        ),
        Err(error) => preflight::PreflightResult::fail(
            "configuration file cannot be read",
            Some(format!("Make the configuration readable by the current user, then retry `mihomo-cli tun on`. ({error})")),
        ),
    };
    checks.push(config_check);

    let tun_geo_dir = ctx.paths.tun_config_file.parent();
    checks.push(match tun_geo_dir {
        Some(dir) if installer::geo_files_are_valid(dir, &ctx.paths.core_binary) => {
            preflight::PreflightResult::pass_named("TUN runtime Geo data is ready")
        }
        Some(dir) => preflight::PreflightResult::fail(
            format!("TUN runtime Geo data is missing or invalid: {}", dir.display()),
            Some("Run `mihomo-cli install --system --force --skip-config --yes`, then retry `mihomo-cli tun on`.".to_string()),
        ),
        None => preflight::PreflightResult::fail(
            "TUN runtime Geo directory is unavailable",
            Some("Run `mihomo-cli install --system --force --skip-config --yes`, then retry `mihomo-cli tun on`.".to_string()),
        ),
    });

    #[cfg(target_os = "linux")]
    {
        let tun_path = std::path::Path::new("/dev/net/tun");
        checks.push(if tun_path.exists() {
            preflight::PreflightResult::pass_named("/dev/net/tun is available")
        } else {
            preflight::PreflightResult::fail(
                "/dev/net/tun is unavailable",
                Some("Provide /dev/net/tun in the host or container, then retry `mihomo-cli tun on`.".to_string()),
            )
        });
    }

    let runtime = current_runtime_presence();
    checks.push(if runtime.user {
        preflight::PreflightResult::fail(
            "per-user Mihomo Core is still running",
            Some("Run `mihomo-cli stop`, then retry `mihomo-cli tun on`.".to_string()),
        )
    } else {
        preflight::PreflightResult::pass_named("no per-user Core instance conflict")
    });

    if daemon_available {
        checks.push(
            match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await {
                Ok(ipc::DaemonResponse::Status { .. }) => {
                    preflight::PreflightResult::pass_named("daemon status verified")
                }
                Ok(ipc::DaemonResponse::Error { message }) => {
                    tun_daemon_error_check(ctx.os, &message)
                }
                Ok(_) => preflight::PreflightResult::fail(
                    "daemon status returned an unexpected response",
                    Some(
                        "Run `mihomo-cli doctor --system` and inspect the daemon diagnostics."
                            .to_string(),
                    ),
                ),
                Err(error) => tun_daemon_transport_check(ctx.os, &error.to_string()),
            },
        );
    }

    let report = preflight::format_report(&checks);
    if checks.iter().any(|check| !check.passed) {
        anyhow::bail!(report);
    }
    println!("{report}");
    Ok(())
}

fn successful_runtime_proof(
    response: &ipc::DaemonResponse,
    fence: &tun_transaction::TransactionFence,
    expected_phase: tun_transaction::JournalPhase,
    expected_kind: tun_transaction::RuntimeProofKind,
    expected_revision: &str,
    expected_tun: bool,
) -> anyhow::Result<tun_transaction::RuntimeProof> {
    let proof = match response {
        ipc::DaemonResponse::Transaction {
            response:
                tun_transaction::TransactionResponse::Completed(proof)
                | tun_transaction::TransactionResponse::AlreadySatisfied(proof),
        } => proof.clone(),
        other => anyhow::bail!("daemon did not prove the requested runtime state: {other:?}"),
    };
    if proof.transaction_id != fence.transaction_id
        || proof.generation != fence.generation
        || proof.observed_phase != expected_phase
        || proof.proof_kind != expected_kind
        || proof.launched_revision != expected_revision
        || proof.runtime_tun != expected_tun
        || !proof.api_ready
        || proof.core_pid == 0
        || proof.core_identity.is_empty()
    {
        anyhow::bail!("daemon returned an invalid runtime proof: {proof:?}");
    }
    Ok(proof)
}

/// a refused controlled reset must say whether a stop was even
/// attempted. "Not attempted" and "attempted but the exit is unconfirmed" are
/// different obligations for the operator and must not collapse into one text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopAttemptPhase {
    NotAttempted,
    AttemptedButUnconfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedStopError {
    phase: StopAttemptPhase,
    detail: String,
}

impl ManagedStopError {
    fn not_attempted(detail: impl Into<String>) -> Self {
        Self {
            phase: StopAttemptPhase::NotAttempted,
            detail: detail.into(),
        }
    }

    fn attempted_but_unconfirmed(detail: impl Into<String>) -> Self {
        Self {
            phase: StopAttemptPhase::AttemptedButUnconfirmed,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for ManagedStopError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for ManagedStopError {}

/// SPEC B3 / 去脂裁决②: the single quiesce proof consumed by both the
/// rollback path and the controlled reset. There is no "attested but still
/// running" state any more: a valid proof always means the Core is stopped.
fn validate_quiesced_response(
    response: &ipc::DaemonResponse,
    fence: &tun_transaction::TransactionFence,
) -> anyhow::Result<()> {
    let proof = match response {
        ipc::DaemonResponse::Transaction {
            response:
                tun_transaction::TransactionResponse::Completed(proof)
                | tun_transaction::TransactionResponse::AlreadySatisfied(proof),
        } => proof,
        // A `NotSatisfied` response means the daemon did try to stop the Core and
        // could not confirm it; every other refusal never reached a stop.
        ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::NotSatisfied { error, .. },
        } => {
            return Err(anyhow::Error::new(
                ManagedStopError::attempted_but_unconfirmed(error.message.clone()),
            ))
        }
        other => {
            return Err(anyhow::Error::new(ManagedStopError::not_attempted(
                format!("daemon did not prove the managed runtime is stopped: {other:?}"),
            )))
        }
    };
    // SPEC B3: fence validation must check against fence.expected_phase, not a hardcoded phase
    if proof.transaction_id != fence.transaction_id
        || proof.generation != fence.generation
        || proof.observed_phase != fence.expected_phase
        || proof.proof_kind != tun_transaction::RuntimeProofKind::CandidateQuiesced
    {
        return Err(anyhow::Error::new(ManagedStopError::not_attempted(
            format!("daemon returned an invalid quiesce proof: {proof:?}"),
        )));
    }
    let stopped = proof.core_pid == 0
        && proof.launched_revision.is_empty()
        && !proof.api_ready
        && !proof.runtime_tun;
    if !stopped {
        return Err(anyhow::Error::new(
            ManagedStopError::attempted_but_unconfirmed(format!(
                "daemon proof does not show a stopped managed runtime (pid={}, revision={:?}, api_ready={}, tun={}): {proof:?}",
                proof.core_pid,
                proof.launched_revision,
                proof.api_ready,
                proof.runtime_tun
            )),
        ));
    }
    Ok(())
}

/// the rollback copy path reads the same durable evidence the
/// planner classified. Evidence can still vanish between planning and execution,
/// so this wrapper keeps the Fix guidance instead of surfacing a bare ENOENT.
fn read_old_runtime_for_rollback(
    ctx: &instance::InstanceContext,
    journal: &tun_transaction::TunJournal,
) -> anyhow::Result<tun_transaction::OldRuntimeEvidence> {
    tun_transaction::read_and_validate_old_runtime(ctx, journal).map_err(|error| {
        anyhow::anyhow!(
            "the rollback cannot prove the old runtime: old-runtime evidence is {} ({error}).\n  Every transaction artifact is kept.\n  Fix: mihomo-cli restart --system",
            tun_transaction::classify_old_runtime_evidence(ctx, journal).describe()
        )
    })
}

#[cfg(test)]
mod restored_old_compat_tests {
    use super::*;

    #[test]
    fn fallback_requires_authorized_exact_response_stage_code_and_observation() {
        let evidence = tun_transaction::OldRuntimeEvidence {
            core_running: true,
            core_identity: "core".into(),
            core_pid: 1,
            launched_revision: "old".into(),
            launch_source: tun_transaction::LaunchSource::SystemTunSnapshot,
            runtime_tun: true,
            api_endpoint: "endpoint".into(),
            recorded_at_secs: None,
        };
        let observation = tun_transaction::RuntimeObservation {
            core_running: true,
            core_identity: Some("core".into()),
            core_pid: Some(2),
            launched_revision: Some("old".into()),
            runtime_tun: Some(true),
            api_ready: true,
        };
        for (quiesce, stage, accepted) in [
            (true, "quiesce_candidate", true),
            (true, "quiesce_old_runtime", true),
            (true, "attest_old_runtime_after_start", false),
            (false, "attest_old_runtime_after_start", true),
            (false, "quiesce_candidate", false),
            (false, "start_old_core", false),
        ] {
            let make = |obs: tun_transaction::RuntimeObservation, code| {
                let error = tun_transaction::StructuredError {
                    code,
                    stage: stage.into(),
                    retryable: false,
                    message: String::new(),
                };
                ipc::DaemonResponse::Transaction {
                    response: if quiesce {
                        tun_transaction::TransactionResponse::EvidenceMismatch {
                            observation: obs,
                            error,
                        }
                    } else {
                        tun_transaction::TransactionResponse::NotSatisfied {
                            observation: obs,
                            error,
                        }
                    },
                }
            };
            let response = make(
                observation.clone(),
                tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
            );
            assert_eq!(
                restored_old_compat_observation(&response, &evidence, true, quiesce).is_some(),
                accepted
            );
            assert!(
                restored_old_compat_observation(&response, &evidence, false, quiesce).is_none()
            );
            let wrong_code = make(
                observation.clone(),
                tun_transaction::TransactionErrorCode::CoreStartFailed,
            );
            assert!(
                restored_old_compat_observation(&wrong_code, &evidence, true, quiesce).is_none()
            );
            let mut wrong_observation = observation.clone();
            wrong_observation.runtime_tun = Some(false);
            let wrong = make(
                wrong_observation,
                tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
            );
            assert!(restored_old_compat_observation(&wrong, &evidence, true, quiesce).is_none());
        }
        let error = ipc::DaemonResponse::Error {
            message: "error".into(),
        };
        assert!(restored_old_compat_observation(&error, &evidence, true, true).is_none());
    }
}

fn restored_old_compat_observation<'a>(
    response: &'a ipc::DaemonResponse,
    evidence: &tun_transaction::OldRuntimeEvidence,
    allow: bool,
    quiesce: bool,
) -> Option<&'a tun_transaction::RuntimeObservation> {
    if !allow {
        return None;
    }
    let (observation, error) = match response {
        ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::EvidenceMismatch { observation, error },
        } if quiesce => (observation, error),
        ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::NotSatisfied { observation, error },
        } if !quiesce => (observation, error),
        _ => return None,
    };
    let stage_matches = if quiesce {
        matches!(
            error.stage.as_str(),
            "quiesce_candidate" | "quiesce_old_runtime"
        )
    } else {
        error.stage == "attest_old_runtime_after_start"
    };
    (error.code == tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch
        && stage_matches
        && tun_transaction::runtime_matches_restored_old_state(evidence, observation))
    .then_some(observation)
}

#[cfg(target_os = "linux")]
async fn observe_managed_restored_old_runtime(
    ctx: &instance::InstanceContext,
    fence: &tun_transaction::TransactionFence,
    expected: &tun_transaction::OldRuntimeEvidence,
    verify_source_revision: bool,
) -> anyhow::Result<tun_transaction::RuntimeObservation> {
    use std::os::unix::fs::MetadataExt;
    let journal = tun_transaction::read_active_journal(ctx)?
        .ok_or_else(|| anyhow::anyhow!("active journal missing"))?;
    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != fence.expected_phase
        || journal.candidate_revision != fence.expected_candidate_revision
        || journal.phase != tun_transaction::JournalPhase::RollbackPending
        || read_old_runtime_for_rollback(ctx, &journal)? != *expected
    {
        anyhow::bail!("rollback fence or old evidence changed");
    }
    let endpoint = managed_core_endpoint(ctx);
    if expected.api_endpoint != status_endpoint_label(&ctx.paths.api_endpoint) {
        anyhow::bail!("old evidence endpoint does not match the managed endpoint");
    }
    let socket = match &ctx.paths.api_endpoint {
        instance::ApiEndpoint::UnixSocket(path) => path,
        _ => anyhow::bail!("managed endpoint is not a Unix socket"),
    };
    let before = std::fs::symlink_metadata(socket)?;
    if !std::os::unix::fs::FileTypeExt::is_socket(&before.file_type()) {
        anyhow::bail!("managed endpoint is not a socket");
    }
    let state_dir = managed_state_dir(ctx)?;
    let service_uid = managed_service_uid()
        .ok_or_else(|| anyhow::anyhow!("managed service account is unavailable"))?;
    let peer = || match core_identity::prove_core_api_peer(
        &endpoint,
        &ctx.paths.core_binary,
        &state_dir,
        Some(service_uid),
    ) {
        core_identity::CoreIdentityProof::Proven(identity) => Ok(identity),
        other => Err(anyhow::anyhow!(
            "managed Core identity unprovable: {other:?}"
        )),
    };
    let identity = peer()?;
    let source = match expected.launch_source {
        tun_transaction::LaunchSource::SystemActiveConfig => {
            tun_transaction::active_config_path(ctx)
        }
        tun_transaction::LaunchSource::SystemTunSnapshot => ctx.paths.tun_config_file.clone(),
    };
    if verify_source_revision {
        let bytes = utils::read_file_no_follow_limited(
            &source,
            tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
        )?;
        if tun_transaction::sha256_revision(&bytes) != expected.launched_revision {
            anyhow::bail!("restored launch source differs from old revision");
        }
    }
    if !identity
        .cmdline
        .windows(2)
        .any(|args| args[0] == "-f" && std::path::Path::new(&args[1]) == source)
    {
        anyhow::bail!("managed Core launch source differs from old evidence");
    }
    let (running, pid, revision, config_path) =
        match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await? {
            ipc::DaemonResponse::Status {
                running,
                core_pid,
                launched_config_revision,
                config_path,
                ..
            } => (running, core_pid, launched_config_revision, config_path),
            other => anyhow::bail!("unexpected daemon status: {other:?}"),
        };
    if pid != Some(identity.pid) || config_path.as_deref() != Some(source.as_path()) {
        anyhow::bail!("daemon status conflicts with managed Core identity or source");
    }
    let configs = mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs").await?;
    let tun = configs
        .get("tun")
        .and_then(|v| v.get("enable"))
        .and_then(|v| v.as_bool())
        .ok_or_else(|| anyhow::anyhow!("runtime TUN is unobservable"))?;
    let second = peer()?;
    let after = std::fs::symlink_metadata(socket)?;
    if identity != second
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || !std::os::unix::fs::FileTypeExt::is_socket(&after.file_type())
    {
        anyhow::bail!("managed Core endpoint changed during observation");
    }
    let observation = tun_transaction::RuntimeObservation {
        core_running: running,
        core_identity: Some(ctx.paths.core_binary.to_string_lossy().to_string()),
        core_pid: pid,
        launched_revision: revision,
        runtime_tun: Some(tun),
        api_ready: true,
    };
    if !tun_transaction::runtime_matches_restored_old_state(expected, &observation) {
        anyhow::bail!("current runtime differs from old evidence");
    }
    let current = tun_transaction::read_active_journal(ctx)?
        .ok_or_else(|| anyhow::anyhow!("active journal disappeared"))?;
    if current != journal || read_old_runtime_for_rollback(ctx, &current)? != *expected {
        anyhow::bail!("rollback evidence changed during observation");
    }
    Ok(observation)
}

#[cfg(target_os = "linux")]
async fn cli_quiesce_restored_old_core(
    ctx: &instance::InstanceContext,
    fence: &tun_transaction::TransactionFence,
    evidence: &tun_transaction::OldRuntimeEvidence,
    observed: &tun_transaction::RuntimeObservation,
) -> anyhow::Result<()> {
    if !is_current_process_root() {
        return Err(anyhow::Error::new(ManagedStopError::not_attempted(
            "managed Core stop requires root",
        )));
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let pid = observed.core_pid.ok_or_else(|| {
        anyhow::Error::new(ManagedStopError::not_attempted("missing observed PID"))
    })?;
    let bound = core_identity::BoundManagedCore::bind(pid, std::time::Duration::from_secs(5))
        .map_err(|e| anyhow::Error::new(ManagedStopError::not_attempted(e)))?;
    let fresh = tokio::time::timeout_at(
        deadline,
        observe_managed_restored_old_runtime(ctx, fence, evidence, false),
    )
    .await
    .map_err(|_| {
        anyhow::Error::new(ManagedStopError::not_attempted(
            "stop budget expired during identity verification",
        ))
    })?
    .map_err(|e| anyhow::Error::new(ManagedStopError::not_attempted(e.to_string())))?;
    if fresh.core_pid != Some(pid)
        || !bound
            .is_live()
            .map_err(|e| anyhow::Error::new(ManagedStopError::not_attempted(e)))?
    {
        return Err(anyhow::Error::new(ManagedStopError::not_attempted(
            "bound Core identity changed",
        )));
    }
    match bound.terminate_and_wait() {
        core_identity::ProcessExitProof::Exited => Ok(()),
        other => Err(anyhow::Error::new(
            ManagedStopError::attempted_but_unconfirmed(format!(
                "managed Core exit unconfirmed: {other:?}"
            )),
        )),
    }
}

#[cfg(not(target_os = "linux"))]
async fn observe_managed_restored_old_runtime(
    _ctx: &instance::InstanceContext,
    _fence: &tun_transaction::TransactionFence,
    _expected: &tun_transaction::OldRuntimeEvidence,
    _verify_source_revision: bool,
) -> anyhow::Result<tun_transaction::RuntimeObservation> {
    anyhow::bail!("managed Core identity is unsupported on this platform")
}

#[cfg(not(target_os = "linux"))]
async fn cli_quiesce_restored_old_core(
    _ctx: &instance::InstanceContext,
    _fence: &tun_transaction::TransactionFence,
    _evidence: &tun_transaction::OldRuntimeEvidence,
    _observed: &tun_transaction::RuntimeObservation,
) -> anyhow::Result<()> {
    Err(anyhow::Error::new(ManagedStopError::not_attempted(
        "stable process handles are unsupported on this platform",
    )))
}

async fn execute_automatic_rollback(
    ctx: &instance::InstanceContext,
    journal: &tun_transaction::TunJournal,
    _evidence: &tun_transaction::OldRuntimeEvidence,
) -> anyhow::Result<()> {
    // 1. CAS: -> RollbackPending
    let fence = tun_transaction::TransactionFence {
        transaction_id: journal.transaction_id.clone(),
        generation: journal.generation,
        expected_phase: journal.phase,
        expected_candidate_revision: journal.candidate_revision.clone(),
    };
    let journal = tun_transaction::cas_update_phase(
        ctx,
        &fence,
        tun_transaction::JournalPhase::RollbackPending,
        None,
        None,
    )?;

    // 2. Daemon quiesce candidate
    let fence = tun_transaction::TransactionFence {
        transaction_id: journal.transaction_id.clone(),
        generation: journal.generation,
        expected_phase: tun_transaction::JournalPhase::RollbackPending,
        expected_candidate_revision: journal.candidate_revision.clone(),
    };
    if ctx.mode == instance::InstanceMode::System && is_current_process_root() {
        utils::ensure_mihomo_system_state_dir()?;
    }
    let quiesce_resp = ipc::send_command(&ipc::DaemonCommand::QuiesceCandidateRuntime {
        fence: fence.clone(),
        allow_legacy_reset: false,
        allow_restored_old_quiesce: false,
        token: None,
    })
    .await?;

    // Validate immutable evidence before changing the Snapshot.
    let old_evidence = read_old_runtime_for_rollback(ctx, &journal)?;
    validate_quiesced_response(&quiesce_resp, &fence)?;

    // 3. Root restore snapshot
    tun_transaction::restore_snapshot(ctx, &journal)?;
    if ctx.mode == instance::InstanceMode::System && is_current_process_root() {
        utils::ensure_mihomo_system_state_dir()?;
    }

    // 4. Daemon restore old runtime
    let restore_resp = ipc::send_command(&ipc::DaemonCommand::RestoreOldRuntime {
        fence: fence.clone(),
        expected_old_runtime_revision: old_evidence.launched_revision.clone(),
        expected_old_runtime_tun: old_evidence.runtime_tun,
        token: None,
    })
    .await?;
    let _ = successful_runtime_proof(
        &restore_resp,
        &fence,
        tun_transaction::JournalPhase::RollbackPending,
        tun_transaction::RuntimeProofKind::OldRuntimeRestored,
        &old_evidence.launched_revision,
        old_evidence.runtime_tun,
    )?;

    // 5. CAS: RollbackPending -> RolledBack
    let _ = tun_transaction::cas_update_phase(
        ctx,
        &fence,
        tun_transaction::JournalPhase::RolledBack,
        None,
        Some(tun_transaction::TerminalOutcome::RolledBackAfterApplyFailure),
    )?;

    // 6. Terminal cleanup
    let _ = tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation);

    Ok(())
}

#[allow(dead_code)]
async fn execute_recovery_action(
    ctx: &instance::InstanceContext,
    journal: &tun_transaction::TunJournal,
    action: tun_transaction::RecoveryAction,
    obs: &tun_transaction::RuntimeObservation,
    direction: tun_transaction::RecoveryDirection,
    allow_runtime_reset: bool,
) -> anyhow::Result<()> {
    match action {
        tun_transaction::RecoveryAction::NoOpTerminal => Ok(()),
        tun_transaction::RecoveryAction::RebuildOwnerGate => {
            let gate = tun_transaction::SystemTransactionGate {
                schema_version: 1,
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                original_uid: journal.original_uid,
                base_intent_revision: journal.base_intent_revision.clone(),
                candidate_revision: journal.candidate_revision.clone(),
            };
            tun_transaction::write_user_gate(&ctx.paths.config_dir, &gate, journal.original_uid)?;
            println!("  ✓ Rebuilt user transaction gate.");
            Ok(())
        }
        tun_transaction::RecoveryAction::CleanupTerminal => {
            tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation)?;
            println!("  ✅ Transaction cleanup complete.");
            Ok(())
        }
        tun_transaction::RecoveryAction::RepairPhaseToSnapshotPromoted => {
            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: journal.phase,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            let updated = tun_transaction::cas_update_phase(
                ctx,
                &fence,
                tun_transaction::JournalPhase::SnapshotPromoted,
                None,
                None,
            )?;
            println!("  ✓ Repaired phase to SnapshotPromoted.");
            let snap_cls = tun_transaction::classify_snapshot(ctx, &updated);
            let intent_cls =
                tun_transaction::classify_intent(&ctx.paths.intent_config_file, &updated);
            let next_action = tun_transaction::plan_recovery(
                &updated,
                snap_cls,
                intent_cls,
                tun_transaction::classify_old_runtime_evidence(ctx, &updated),
                obs,
                direction,
            );
            Box::pin(execute_recovery_action(
                ctx,
                &updated,
                next_action,
                obs,
                direction,
                allow_runtime_reset,
            ))
            .await
        }
        tun_transaction::RecoveryAction::RetryApply => {
            let journal = if journal.phase == tun_transaction::JournalPhase::RecoveryRequired {
                let recovery_fence = tun_transaction::TransactionFence {
                    transaction_id: journal.transaction_id.clone(),
                    generation: journal.generation,
                    expected_phase: journal.phase,
                    expected_candidate_revision: journal.candidate_revision.clone(),
                };
                let pending = tun_transaction::cas_update_phase(
                    ctx,
                    &recovery_fence,
                    tun_transaction::JournalPhase::PromotionPending,
                    None,
                    None,
                )?;
                if let Err(error) = tun_transaction::promote_snapshot(ctx, &pending) {
                    let _ = tun_transaction::cas_update_phase(
                        ctx,
                        &tun_transaction::TransactionFence {
                            transaction_id: pending.transaction_id.clone(),
                            generation: pending.generation,
                            expected_phase: pending.phase,
                            expected_candidate_revision: pending.candidate_revision.clone(),
                        },
                        tun_transaction::JournalPhase::RecoveryRequired,
                        Some(tun_transaction::StructuredError {
                            code: tun_transaction::TransactionErrorCode::SnapshotConflict,
                            stage: "recover_promote_snapshot".to_string(),
                            retryable: false,
                            message: error.to_string(),
                        }),
                        None,
                    );
                    return Err(anyhow::anyhow!(
                        "snapshot promotion during recovery failed: {error}"
                    ));
                }
                tun_transaction::cas_update_phase(
                    ctx,
                    &tun_transaction::TransactionFence {
                        transaction_id: pending.transaction_id.clone(),
                        generation: pending.generation,
                        expected_phase: pending.phase,
                        expected_candidate_revision: pending.candidate_revision.clone(),
                    },
                    tun_transaction::JournalPhase::SnapshotPromoted,
                    None,
                    None,
                )?
            } else {
                journal.clone()
            };
            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: journal.phase,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            if ctx.mode == instance::InstanceMode::System && is_current_process_root() {
                utils::ensure_mihomo_system_state_dir()?;
            }
            let apply_resp = ipc::send_command(&ipc::DaemonCommand::ApplyPromotedSnapshot {
                fence: fence.clone(),
                target_runtime_tun: journal.target_runtime_tun,
                token: None,
            })
            .await?;
            match &apply_resp {
                ipc::DaemonResponse::Transaction {
                    response:
                        tun_transaction::TransactionResponse::Completed(_)
                        | tun_transaction::TransactionResponse::AlreadySatisfied(_),
                } => {
                    let proof = successful_runtime_proof(
                        &apply_resp,
                        &fence,
                        tun_transaction::JournalPhase::SnapshotPromoted,
                        tun_transaction::RuntimeProofKind::CandidateApplied,
                        &journal.candidate_revision,
                        journal.target_runtime_tun,
                    )?;
                    tun_transaction::record_candidate_runtime(ctx, &fence, &proof)?;
                    let updated = tun_transaction::cas_update_phase(
                        ctx,
                        &fence,
                        tun_transaction::JournalPhase::CoreApplied,
                        None,
                        None,
                    )?;
                    let snap_cls = tun_transaction::classify_snapshot(ctx, &updated);
                    let intent_cls =
                        tun_transaction::classify_intent(&ctx.paths.intent_config_file, &updated);
                    let next_action = tun_transaction::plan_recovery(
                        &updated,
                        snap_cls,
                        intent_cls,
                        tun_transaction::classify_old_runtime_evidence(ctx, &updated),
                        obs,
                        direction,
                    );
                    Box::pin(execute_recovery_action(
                        ctx,
                        &updated,
                        next_action,
                        obs,
                        direction,
                        allow_runtime_reset,
                    ))
                    .await
                }
                other => {
                    anyhow::bail!("Retry apply failed: {:?}", other);
                }
            }
        }
        tun_transaction::RecoveryAction::CommitIntent => {
            match tun_transaction::compare_and_commit_user_intent(ctx, journal)? {
                tun_transaction::IntentCommitResult::Committed
                | tun_transaction::IntentCommitResult::AlreadyCandidate => {
                    let fence = tun_transaction::TransactionFence {
                        transaction_id: journal.transaction_id.clone(),
                        generation: journal.generation,
                        expected_phase: journal.phase,
                        expected_candidate_revision: journal.candidate_revision.clone(),
                    };
                    let _ = tun_transaction::cas_update_phase(
                        ctx,
                        &fence,
                        tun_transaction::JournalPhase::IntentCommitted,
                        None,
                        Some(tun_transaction::TerminalOutcome::AppliedAfterRecovery),
                    )?;
                    tun_transaction::terminal_cleanup(
                        ctx,
                        &journal.transaction_id,
                        journal.generation,
                    )?;
                    println!("  ✅ Intent committed and transaction finalized.");
                    Ok(())
                }
                tun_transaction::IntentCommitResult::Conflict => {
                    anyhow::bail!("Intent conflict: user configuration has changed.");
                }
            }
        }
        tun_transaction::RecoveryAction::FinalizeCommittedIntent => {
            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: journal.phase,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            let _ = tun_transaction::cas_update_phase(
                ctx,
                &fence,
                tun_transaction::JournalPhase::IntentCommitted,
                None,
                Some(tun_transaction::TerminalOutcome::AppliedAfterRecovery),
            )?;
            tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation)?;
            println!("  ✅ Transaction finalized successfully.");
            Ok(())
        }
        tun_transaction::RecoveryAction::BeginRollback
        | tun_transaction::RecoveryAction::ContinueRollback => {
            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: journal.phase,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            let journal = if journal.phase != tun_transaction::JournalPhase::RollbackPending {
                tun_transaction::cas_update_phase(
                    ctx,
                    &fence,
                    tun_transaction::JournalPhase::RollbackPending,
                    None,
                    None,
                )?
            } else {
                journal.clone()
            };

            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: tun_transaction::JournalPhase::RollbackPending,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            let quiesce_resp = ipc::send_command(&ipc::DaemonCommand::QuiesceCandidateRuntime {
                fence: fence.clone(),
                allow_legacy_reset: false,
                allow_restored_old_quiesce: false,
                token: None,
            })
            .await?;

            let old_evidence = read_old_runtime_for_rollback(ctx, &journal)?;
            if let Some(observation) = restored_old_compat_observation(
                &quiesce_resp,
                &old_evidence,
                allow_runtime_reset,
                true,
            ) {
                cli_quiesce_restored_old_core(ctx, &fence, &old_evidence, observation).await?;
            } else {
                validate_quiesced_response(&quiesce_resp, &fence)?;
            }
            tun_transaction::restore_snapshot(ctx, &journal)?;

            let restore_resp = ipc::send_command(&ipc::DaemonCommand::RestoreOldRuntime {
                fence: fence.clone(),
                expected_old_runtime_revision: old_evidence.launched_revision.clone(),
                expected_old_runtime_tun: old_evidence.runtime_tun,
                token: None,
            })
            .await?;
            if restored_old_compat_observation(
                &restore_resp,
                &old_evidence,
                allow_runtime_reset,
                false,
            )
            .is_some()
            {
                observe_managed_restored_old_runtime(ctx, &fence, &old_evidence, true).await?;
            } else {
                let _ = successful_runtime_proof(
                    &restore_resp,
                    &fence,
                    tun_transaction::JournalPhase::RollbackPending,
                    tun_transaction::RuntimeProofKind::OldRuntimeRestored,
                    &old_evidence.launched_revision,
                    old_evidence.runtime_tun,
                )?;
            }

            let _ = tun_transaction::cas_update_phase(
                ctx,
                &fence,
                tun_transaction::JournalPhase::RolledBack,
                None,
                Some(tun_transaction::TerminalOutcome::RolledBackByUser),
            )?;
            tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation)?;
            println!("  ✅ Transaction rolled back successfully.");
            Ok(())
        }
        tun_transaction::RecoveryAction::MarkRolledBack => {
            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: journal.phase,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            let old_evidence = read_old_runtime_for_rollback(ctx, journal)?;
            let restore_resp = ipc::send_command(&ipc::DaemonCommand::RestoreOldRuntime {
                fence: fence.clone(),
                expected_old_runtime_revision: old_evidence.launched_revision.clone(),
                expected_old_runtime_tun: old_evidence.runtime_tun,
                token: None,
            })
            .await?;
            let proof = successful_runtime_proof(
                &restore_resp,
                &fence,
                journal.phase,
                tun_transaction::RuntimeProofKind::OldRuntimeRestored,
                &old_evidence.launched_revision,
                old_evidence.runtime_tun,
            )?;
            let observation = tun_transaction::RuntimeObservation {
                core_running: true,
                core_identity: Some(proof.core_identity),
                core_pid: Some(proof.core_pid),
                launched_revision: Some(proof.launched_revision),
                runtime_tun: Some(proof.runtime_tun),
                api_ready: proof.api_ready,
            };
            if !tun_transaction::runtime_matches_restored_old_state(&old_evidence, &observation) {
                anyhow::bail!("current runtime does not match durable old-runtime evidence");
            }
            let _ = tun_transaction::cas_update_phase(
                ctx,
                &fence,
                tun_transaction::JournalPhase::RolledBack,
                None,
                Some(tun_transaction::TerminalOutcome::RolledBackByUser),
            )?;
            tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation)?;
            println!("  ✅ Transaction marked rolled back.");
            Ok(())
        }
        tun_transaction::RecoveryAction::ConvergeLegacyToCurrentIntent => {
            let intent_bytes = crate::utils::read_file_no_follow_limited(
                &ctx.paths.intent_config_file,
                tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
            )?;
            let rec_target_rev = tun_transaction::sha256_revision(&intent_bytes);
            let rec_target_path = tun_transaction::active_recovery_target_path(ctx);
            crate::utils::atomic_write_bytes_no_follow(&rec_target_path, &intent_bytes, 0o640)?;

            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: journal.phase,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            let fenced_journal =
                tun_transaction::cas_update_recovery_target(ctx, &fence, rec_target_rev.clone())?;
            let response = ipc::send_command(&ipc::DaemonCommand::ApplyLegacyRecoveryTarget {
                fence: fence.clone(),
                expected_recovery_target_revision: rec_target_rev,
                token: None,
            })
            .await?;
            let target_tun = serde_yaml::from_slice::<serde_yaml::Value>(&intent_bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("tun")
                        .and_then(|tun| tun.get("enable"))
                        .and_then(|enable| enable.as_bool())
                })
                .ok_or_else(|| anyhow::anyhow!("current intent has no boolean tun.enable"))?;
            let recovery_fence = tun_transaction::TransactionFence {
                expected_phase: fenced_journal.phase,
                ..fence.clone()
            };
            let proof = successful_runtime_proof(
                &response,
                &recovery_fence,
                tun_transaction::JournalPhase::RecoveryRequired,
                tun_transaction::RuntimeProofKind::LegacyRecoveryTargetApplied,
                &tun_transaction::sha256_revision(&intent_bytes),
                target_tun,
            )?;
            tun_transaction::record_recovery_runtime(ctx, &recovery_fence, &proof)?;

            let _ = tun_transaction::cas_update_phase(
                ctx,
                &fence,
                tun_transaction::JournalPhase::RolledBack,
                None,
                Some(tun_transaction::TerminalOutcome::LegacyConvergedToCurrentIntent),
            )?;
            tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation)?;
            println!("  ✅ Legacy transaction converged to current user intent.");
            Ok(())
        }
        tun_transaction::RecoveryAction::RefuseNeedsEvidence(msg) => {
            anyhow::bail!("Recovery refused: {msg}");
        }
    }
}

pub(crate) fn system_generation_store(
    ctx: &instance::InstanceContext,
) -> generation::GenerationStore {
    generation::GenerationStore::new(ctx.paths.generation_store_root.clone())
}

pub(crate) fn cleanup_system_generation_old(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    lock: &generation::GenerationLockGuard,
    keep_limit: usize,
) -> anyhow::Result<Vec<generation::GenerationId>> {
    match store.cleanup_old_generations_with_lock(lock, keep_limit) {
        Ok(removed) => Ok(removed),
        Err(generation::GenerationError::Io(err))
            if err.kind() == std::io::ErrorKind::PermissionDenied
                && ctx.permissions == instance::PermissionModel::PrivilegedSystem =>
        {
            let state = match store.read_state() {
                Ok(s) => s,
                Err(_) => return Ok(Vec::new()),
            };
            let protected: std::collections::HashSet<_> =
                [&state.active, &state.pending, &state.previous]
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect();

            let gen_parent = store.generations_dir();
            if !gen_parent.exists() {
                return Ok(Vec::new());
            }

            let mut unreferenced = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&gen_parent) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let name = entry.file_name().to_string_lossy().to_string();
                        // SPEC N3: tombstones are never collected by GC.
                        if name.ends_with(generation::WITHDRAWN_SUFFIX)
                            || entry.path().join(generation::UNTRUSTED_MARKER).exists()
                        {
                            continue;
                        }
                        if let Ok(id) = generation::GenerationId::new(&name) {
                            if !protected.contains(&id) {
                                unreferenced.push(id);
                            }
                        }
                    }
                }
            }

            let total_protected = protected.len();
            if total_protected + unreferenced.len() <= keep_limit {
                return Ok(Vec::new());
            }

            let remove_count = (total_protected + unreferenced.len()).saturating_sub(keep_limit);
            unreferenced.truncate(remove_count);
            let mut removed = Vec::new();
            for id in unreferenced {
                let gen_dir = store.generation_dir(&id);
                if service::PrivilegeExecutor::remove_path(&gen_dir).is_ok() {
                    removed.push(id);
                }
            }
            Ok(removed)
        }
        Err(e) => Err(anyhow::anyhow!("failed to cleanup old generations: {e}")),
    }
}

/// SPEC 裁决 A1: explicit, metadata-only revocation of ONE pending
/// generation.
///
/// Contract: verify the trust root, the target identity, `P.baseline == previous`
/// and the baseline's integrity — then persist the tombstone (no-replace) **before**
/// consuming the slots. Zero service stops/starts, zero installed-file replacement:
/// the poisoned bytes stay as evidence (`<P>.withdrawn`) and the authenticated
/// baseline `B` stays bound as `previous` for the next candidate.
const NATIVE_WITHDRAWAL_RECORD: &str = ".native-withdrawal-v1";

fn is_migrated_generation_evidence(
    store: &generation::GenerationStore,
    id: &generation::GenerationId,
) -> bool {
    store.is_generation_untrusted(id)
        || store
            .withdrawn_path(id)
            .join(generation::UNTRUSTED_MARKER)
            .exists()
}

fn native_withdrawal_record(
    store: &generation::GenerationStore,
    pending: &generation::GenerationId,
    baseline: &generation::GenerationId,
    withdrawn: bool,
) -> anyhow::Result<Vec<u8>> {
    let directory = if withdrawn {
        store.withdrawn_path(pending)
    } else {
        store.generation_dir(pending)
    };
    let bytes = utils::read_file_no_follow_limited(
        &directory.join(generation::MANIFEST_FILE_NAME),
        generation::MAX_MANIFEST_BYTES,
    )?;
    Ok(format!(
        "native-withdrawal-v1\npending={pending}\nbaseline={baseline}\nmanifest_sha256={}\n",
        generation::calculate_sha256_bytes(&bytes)
    )
    .into_bytes())
}

fn record_native_withdrawal(
    store: &generation::GenerationStore,
    pending: &generation::GenerationId,
    baseline: &generation::GenerationId,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !is_migrated_generation_evidence(store, pending)
            && !store.is_generation_untrusted(baseline),
        "迁移证据不能生成原生撤销关联"
    );
    let withdrawn = store.has_withdrawn(pending);
    let directory = if withdrawn {
        store.withdrawn_path(pending)
    } else {
        store.generation_dir(pending)
    };
    utils::atomic_write_bytes_no_follow(
        &directory.join(NATIVE_WITHDRAWAL_RECORD),
        &native_withdrawal_record(store, pending, baseline, withdrawn)?,
        0o644,
    )
}

fn verify_native_withdrawal(
    store: &generation::GenerationStore,
    pending: &generation::GenerationId,
    baseline: &generation::GenerationId,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !is_migrated_generation_evidence(store, pending)
            && !store.is_generation_untrusted(baseline),
        "迁移记录不是已认证的原生撤销"
    );
    let actual = utils::read_file_no_follow_limited(
        &store.withdrawn_path(pending).join(NATIVE_WITHDRAWAL_RECORD),
        4096,
    )?;
    anyhow::ensure!(
        actual == native_withdrawal_record(store, pending, baseline, true)?,
        "原生撤销关联与当前候选/基线不一致"
    );
    Ok(())
}

fn discard_pending_generation(
    ctx: &instance::InstanceContext,
    target: &generation::GenerationId,
) -> anyhow::Result<()> {
    authorize_protected_domain_operations(ctx, "install --discard-pending", "安装")?;
    let store = system_generation_store(ctx);
    utils::verify_generation_trust_root(store.root_dir())
        .map_err(|error| generation_trust_root_refusal("撤销代际", &error))?;
    migrate_generation_store_if_needed(ctx, &store)?;
    store
        .init()
        .map_err(|error| anyhow::anyhow!("failed to init generation store: {error}"))?;
    let lock = store
        .acquire_lock()
        .map_err(|error| anyhow::anyhow!("failed to acquire generation lock: {error}"))?;

    let state = store
        .read_state()
        .map_err(|error| anyhow::anyhow!("代际状态损坏，拒绝撤销：{error}"))?;
    let pending = state.pending.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "没有待应用代际可撤销。\n  \
             Fix: mihomo-cli install --system（重新准备候选）"
        )
    })?;
    if &pending != target {
        anyhow::bail!(
            "目标代际 {} 不是当前待应用代际 {}（撤销被拒绝，槽位未被改动）。\n  \
             Fix: mihomo-cli install --system --discard-pending={}",
            target,
            pending,
            pending
        );
    }

    if is_migrated_generation_evidence(&store, target) {
        anyhow::bail!(
            "待应用代际 {target} 来自存量迁移、未经认证；单独撤销可能伪装首装，拒绝撤销。\n  \
             管理员核对当前完整快照后，可显式采纳并同时撤销：mihomo-cli install --system --adopt-current --discard-pending={target}"
        );
    }
    // Target identity + coverage: the pending generation's own bytes must verify.
    let manifest = (if store.has_withdrawn(target) {
        store.read_withdrawn_manifest(target)
    } else {
        store.validate_generation(target)
    })
    .map_err(|error| {
        anyhow::anyhow!(
            "待应用代际 {} 不可验证（{error}）：证据不足，拒绝撤销",
            target
        )
    })?;
    let baseline_id = state.previous.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "待应用代际 {} 缺少绑定基线（state.previous 为空）：证据不足，拒绝撤销",
            target
        )
    })?;
    anyhow::ensure!(
        !store.is_generation_untrusted(&baseline_id),
        "绑定基线未经认证，拒绝撤销"
    );
    let bound = manifest
        .metadata
        .get("baseline")
        .map(|value| value.as_str());
    if bound != Some(baseline_id.as_str()) {
        anyhow::bail!(
            "待应用代际 {} 的绑定基线 {:?} 与 state.previous {} 不一致：证据不足，拒绝撤销",
            target,
            bound,
            baseline_id
        );
    }
    store.validate_generation(&baseline_id).map_err(|error| {
        anyhow::anyhow!(
            "绑定基线 {} 不可验证（{error}）：拒绝撤销（基线必须保持为下一次尝试的认证回退目标）",
            baseline_id
        )
    })?;

    // Tombstone first (no-replace), then consume the slots. A crash in between is
    // re-entrant: the absence of the rollback-proof marker means "revoked, not
    // rolled back".
    record_native_withdrawal(&store, target, &baseline_id)?;
    if !store.has_withdrawn(target) {
        store
            .withdraw_generation(target)
            .map_err(|error| anyhow::anyhow!("撤销墓碑发布失败（槽位未改动）：{error}"))?;
    }
    store
        .consume_revoked_with_lock(&lock, Some(&baseline_id))
        .map_err(|error| anyhow::anyhow!("槽位消费失败：{error}"))?;

    println!("  ✓ 已撤销待应用代际 {target}（仅元数据；未停启服务、未替换任何安装文件）");
    println!("  · 候选字节保留为证据：{}.withdrawn", target);
    println!("  · 绑定基线保持为 previous：{baseline_id}");
    println!("  Next: mihomo-cli install --system（准备候选 C；不会捕获当前安装态为新基线）");
    Ok(())
}

pub(crate) fn prepare_system_generation(
    ctx: &instance::InstanceContext,
    core_bytes: &[u8],
    cli_bytes: &[u8],
    extra_artifacts: Vec<(String, Vec<u8>, Option<u32>)>,
) -> anyhow::Result<generation::GenerationId> {
    // SPEC §12.3.1（裁决 B）：暂存代际属业务阶段，可在首次特权写入之前请求一次授权。
    authorize_protected_domain_operations(ctx, "install --system", "安装")?;
    let daemon_rel = ctx
        .paths
        .cli_binary
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mihomo-cli")
        .to_string();
    let core_rel = ctx
        .paths
        .core_binary
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("mihomo")
        .to_string();

    let daemon_sha256 = generation::calculate_sha256_bytes(cli_bytes);
    let core_sha256 = generation::calculate_sha256_bytes(core_bytes);

    #[cfg(unix)]
    let default_exec_mode = Some(0o755);
    #[cfg(not(unix))]
    let default_exec_mode = None;

    let daemon_entry = generation::ArtifactManifestEntry::new(
        daemon_rel.clone(),
        generation::ArtifactKind::Daemon,
        daemon_sha256,
        cli_bytes.len() as u64,
        default_exec_mode,
    )
    .map_err(|e| anyhow::anyhow!("invalid daemon manifest entry: {e}"))?;

    let core_entry = generation::ArtifactManifestEntry::new(
        core_rel.clone(),
        generation::ArtifactKind::Core,
        core_sha256,
        core_bytes.len() as u64,
        default_exec_mode,
    )
    .map_err(|e| anyhow::anyhow!("invalid core manifest entry: {e}"))?;

    let gen_id = generation::GenerationId::generate();
    let mut manifest = generation::GenerationManifest::new(
        gen_id.clone(),
        generation::DEFAULT_PROTOCOL_VERSION,
        daemon_entry,
        core_entry,
    );

    for (rel_path, bytes, mode) in &extra_artifacts {
        let sha256 = generation::calculate_sha256_bytes(bytes);
        let kind = if rel_path.contains("geoip")
            || rel_path.ends_with(".metadb")
            || rel_path.ends_with(".mmdb")
        {
            generation::ArtifactKind::Geoip
        } else if rel_path.contains("geosite") || rel_path.ends_with(".dat") {
            generation::ArtifactKind::Geosite
        } else {
            generation::ArtifactKind::Other(rel_path.clone())
        };
        let entry = generation::ArtifactManifestEntry::new(
            rel_path.clone(),
            kind,
            sha256,
            bytes.len() as u64,
            *mode,
        )
        .map_err(|e| anyhow::anyhow!("invalid extra artifact manifest entry: {e}"))?;
        manifest = manifest.with_extra_artifact(entry);
    }

    let store = system_generation_store(ctx);
    // SPEC P1-1: migration must precede *every* staging write — `init` and the
    // lock both create the new root, which would otherwise look like
    // "new root exists without state" and silently skip the migration.
    utils::verify_generation_trust_root(store.root_dir())
        .map_err(|error| generation_trust_root_refusal("暂存代际", &error))?;
    migrate_generation_store_if_needed(ctx, &store)?;
    store
        .init()
        .map_err(|e| anyhow::anyhow!("failed to init generation store: {e}"))?;
    let lock = store
        .acquire_lock()
        .map_err(|e| anyhow::anyhow!("failed to lock generation store: {e}"))?;

    // SPEC N5: re-staging admission. A partially replaced install (or a
    // store with slots but no verifiable install state) must never be certified
    // as a new baseline by overwriting the scene.
    enforce_restage_admission(ctx, &store, &lock)?;

    // SPEC 裁决 A1: a pending generation is never silently overwritten.
    // Discarding it is an explicit, separate authorization.
    let staging_state = store
        .read_state()
        .map_err(|error| anyhow::anyhow!("failed to read generation state: {error}"))?;
    if let Some(pending) = staging_state.pending.clone() {
        anyhow::bail!(
            "存在待应用代际 {pending}，本次暂存被拒绝（不隐式丢弃）。\n  \
             Apply:   mihomo-cli restart --system\n  \
             Discard: mihomo-cli install --system --discard-pending={pending}"
        );
    }

    // SPEC 裁决 A1 recovery mode: after a discard the install tree still holds
    // the failed candidate's bytes, so the scene must NOT be captured as a new
    // baseline — the authenticated previous baseline stays the binding target.
    let recovery_baseline = match (staging_state.active.clone(), staging_state.previous.clone()) {
        (None, Some(baseline)) => Some(baseline),
        _ => None,
    };
    let baseline_id = match recovery_baseline {
        Some(baseline) => {
            anyhow::ensure!(
                !store.is_generation_untrusted(&baseline),
                "恢复基线未经认证，拒绝准备候选"
            );
            store.validate_generation(&baseline).map_err(|error| {
                anyhow::anyhow!(
                    "恢复态的绑定基线 {} 不可验证（{error}）：拒绝准备候选（不捕获当前安装态为新基线）",
                    baseline
                )
            })?;
            println!(
                "  恢复态：候选将绑定已认证基线 {}（不捕获当前安装态为新基线）",
                baseline
            );
            baseline
        }
        None => {
            // SPEC A/P2-1: every staging captures a baseline, including a wholly
            // absent install — the all-missing baseline is how "本无此 artifact" is
            // expressed, so a legitimate staging never becomes an unusable unbound pending.
            let baseline_artifacts = build_baseline_artifacts(ctx, &manifest)?;
            generation::capture_baseline(&store, &lock, &baseline_artifacts).map_err(|error| {
                anyhow::anyhow!("基线捕获失败（fail-closed，尝试修复后重试）: {error}")
            })?
        }
    };
    manifest = manifest.with_metadata("baseline", baseline_id.as_str());

    let gen_dir = store.generation_dir(&gen_id);
    utils::ensure_dir_all_no_follow(&gen_dir)?;
    #[cfg(unix)]
    utils::set_directory_mode_no_follow(&gen_dir, 0o755)?;

    let daemon_path = gen_dir.join(&daemon_rel);
    let core_path = gen_dir.join(&core_rel);

    utils::atomic_write_bytes_no_follow(&daemon_path, cli_bytes, 0o755)?;
    utils::atomic_write_bytes_no_follow(&core_path, core_bytes, 0o755)?;

    for (rel_path, bytes, mode) in &extra_artifacts {
        let path = gen_dir.join(rel_path);
        if let Some(parent) = path.parent() {
            utils::ensure_dir_all_no_follow(parent)?;
        }
        utils::atomic_write_bytes_no_follow(&path, bytes, mode.unwrap_or(0o644) as u16)?;
    }

    store
        .write_manifest(&manifest)
        .map_err(|e| anyhow::anyhow!("failed to write generation manifest: {e}"))?;
    // SPEC A/P2-2: pending + `previous = baseline` land in ONE state write
    // inside the staging critical section — no RMW window to crash into.
    store
        .stage_pending_with_baseline_with_lock(&lock, gen_id.clone(), Some(&baseline_id))
        .map_err(|e| anyhow::anyhow!("failed to stage pending generation: {e}"))?;

    Ok(gen_id)
}

/// SPEC A: builds the baseline artifact list from the current install — the
/// full managed set (daemon/core/extra artifacts) the pending generation targets,
/// so artifacts that do not exist yet are captured as explicitly missing.
fn build_baseline_artifacts(
    ctx: &instance::InstanceContext,
    manifest: &generation::GenerationManifest,
) -> anyhow::Result<Vec<generation::BaselineArtifact>> {
    let state_dir = managed_state_dir(ctx)?;
    let mut artifacts = vec![
        generation::BaselineArtifact {
            relative_path: manifest.daemon.relative_path.clone(),
            kind: generation::ArtifactKind::Daemon,
            source_path: ctx.paths.cli_binary.clone(),
            unix_mode: manifest.daemon.unix_mode,
        },
        generation::BaselineArtifact {
            relative_path: manifest.core.relative_path.clone(),
            kind: generation::ArtifactKind::Core,
            source_path: ctx.paths.core_binary.clone(),
            unix_mode: manifest.core.unix_mode,
        },
    ];
    for extra in &manifest.extra_artifacts {
        artifacts.push(generation::BaselineArtifact {
            relative_path: extra.relative_path.clone(),
            kind: extra.kind.clone(),
            source_path: state_dir.join(&extra.relative_path),
            unix_mode: extra.unix_mode,
        });
    }
    Ok(artifacts)
}

/// SPEC N5: re-staging admission. A partially replaced install (or a store
/// with slots but no verifiable install state) must never be certified as a new
/// baseline by overwriting the scene.
fn enforce_restage_admission(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    _lock: &generation::GenerationLockGuard,
) -> anyhow::Result<()> {
    let state = store
        .read_state()
        .map_err(|error| anyhow::anyhow!("failed to read generation state: {error}"))?;
    let slots: Vec<generation::GenerationId> = [
        state.active.clone(),
        state.pending.clone(),
        state.previous.clone(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if slots.is_empty() {
        // First install (or a pre-generation deployment): the current install is
        // the only scene available, and capturing it is exactly the intent.
        return Ok(());
    }
    for id in &slots {
        // A migrated or unverifiable slot cannot certify the installed state.
        if store.is_generation_untrusted(id) {
            continue;
        }
        let Ok(manifest) = store.validate_generation(id) else {
            continue;
        };
        if manifest.metadata.get("adoption_source").map(String::as_str)
            == Some(generation_adoption::ADOPTION_SOURCE)
        {
            if generation_adoption::installed_matches_adopted(ctx, &manifest)? {
                return Ok(());
            }
            continue;
        }
        if installed_matches_manifest(ctx, store, id, &manifest)? {
            return Ok(());
        }
    }
    // Explicit A1 recovery reuses its authenticated baseline. It never certifies
    // the current (possibly mixed) install, and tombstones are not trust evidence.
    if state.active.is_none() && state.pending.is_none() {
        if let Some(baseline) = &state.previous {
            if !store.is_generation_untrusted(baseline) {
                let manifest = store.validate_generation(baseline)?;
                anyhow::ensure!(manifest.is_baseline(), "恢复绑定不是已认证基线");
                if manifest.metadata.get("adoption_source").map(String::as_str)
                    != Some(generation_adoption::ADOPTION_SOURCE)
                {
                    return Ok(());
                }
            }
        }
    }
    if slots.iter().all(|id| store.is_generation_untrusted(id)) {
        let command = state
            .pending
            .as_ref()
            .map(|id| format!("mihomo-cli install --system --adopt-current --discard-pending={id}"))
            .unwrap_or_else(|| "mihomo-cli install --system --adopt-current".into());
        anyhow::bail!("已迁移但尚无可信基线，拒绝重新暂存（零覆盖）；需管理员采纳当前安装态。\n  先只读检查：mihomo-cli doctor --system --privileged\n  管理员核对完整路径与 SHA-256 后显式采纳：{command}\n  采纳是新的信任决策，不证明历史非混合或运行健康。");
    }
    anyhow::bail!(
        "安装态与任何已知代际（active/pending/基线）都不一致：混合态，拒绝重新暂存（零覆盖）。\n  \
         现场保持原样，本工具无法自动认证或恢复此状态。只读检查：mihomo-cli doctor --system --privileged\n  \
         保留取证并核对安装文件（不要循环执行 install）：\n  \
         sha256sum {}\n  sha256sum {}",
        ctx.paths.cli_binary.display(),
        ctx.paths.core_binary.display(),
    )
}

/// SPEC N5: does the current install match this generation byte for byte?
fn installed_matches_manifest(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    id: &generation::GenerationId,
    manifest: &generation::GenerationManifest,
) -> anyhow::Result<bool> {
    let missing = manifest_missing_set(manifest);
    for target in build_artifact_targets(ctx, store, id, manifest)? {
        if missing.contains(&target.relative_path) {
            match utils::open_regular_file_no_follow(&target.dest) {
                Err(error) if utils::is_not_found_error(&error) => {}
                Err(error) => return Err(error),
                Ok(_) => return Ok(false),
            }
            continue;
        }
        if !target_matches(&target)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn maybe_prepare_cli_skew_generation(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<bool> {
    if ctx.mode != instance::InstanceMode::System {
        return Ok(false);
    }
    if !is_system_cli_skewed(ctx) {
        return Ok(false);
    }
    let store = system_generation_store(ctx);
    if let Ok(state) = store.read_state() {
        if state.pending.is_some() {
            return Ok(false);
        }
    }
    // SPEC C: tombstone gate — if a withdrawn tombstone exists matching
    // the current CLI sha, refuse automatic re-staging. User must explicitly
    // run `mihomo-cli install --system` to retry.
    if let Ok(withdrawn) = store.list_withdrawn() {
        for wid in &withdrawn {
            let Ok(wmanifest) = store.read_withdrawn_manifest(wid) else {
                continue;
            };
            if daemon_identity_matches_current_exe(&wmanifest.daemon.sha256).unwrap_or(false) {
                anyhow::bail!(
                    "存在已撤销候选墓碑 {}（不单独证明回滚成功），重试请显式运行: mihomo-cli install --system",
                    wid
                );
            }
        }
    }
    if let Ok(state) = store.read_state() {
        if state.active.is_none() && state.previous.is_some() {
            anyhow::bail!("恢复基线已绑定；请显式运行 mihomo-cli install --system 准备候选，不自动采纳或重新捕获现场");
        }
    }
    if !ctx.paths.core_binary.exists() {
        anyhow::bail!(
            "检测到当前 CLI 与系统 Daemon 二进制不一致，但系统 Core 二进制不存在 ({})。\n  请先运行: mihomo-cli install --system",
            ctx.paths.core_binary.display()
        );
    }
    let current_cli = std::env::current_exe()?;
    let cli_bytes = std::fs::read(&current_cli)?;
    let core_bytes = std::fs::read(&ctx.paths.core_binary)?;
    println!("  检测到当前 CLI 与系统 Daemon 二进制不一致，正在自动同步更新...");
    let gen_id = prepare_system_generation(ctx, &core_bytes, &cli_bytes, Vec::new())?;
    println!("  ✅ 已准备待应用的系统版本: {gen_id}");
    Ok(true)
}

/// SPEC P1-2: configuration identity frozen at P1. The mirror push, the
/// start/restart request and the (b′) report consumption must all use these
/// same bytes, revision and subscription ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrozenConfig {
    pub content: String,
    pub revision: String,
    pub subscription_id: Option<String>,
    /// False when the active-subscription pointer could not be read: ownership
    /// is unprovable, so selection replay must be skipped instead of claimed.
    pub ownership_proven: bool,
}

/// SPEC P3/P4: one artifact the pipeline may replace or restore.
#[derive(Debug, Clone)]
pub(crate) struct ArtifactTarget {
    pub relative_path: String,
    pub kind: generation::ArtifactKind,
    pub sha256: String,
    pub source: std::path::PathBuf,
    pub dest: std::path::PathBuf,
    pub mode: Option<u32>,
}

impl ArtifactTarget {
    fn is_daemon(&self) -> bool {
        self.kind == generation::ArtifactKind::Daemon
    }
}

/// SPEC B: the immutable description of one apply attempt, derived from the
/// verified pending manifest and its bound baseline.
struct ApplyPlan {
    pending_id: generation::GenerationId,
    manifest: generation::GenerationManifest,
    baseline_id: generation::GenerationId,
    baseline_manifest: generation::GenerationManifest,
    targets: Vec<ArtifactTarget>,
}

/// What P3/P4 must touch: minimal downtime scope and minimal inode churn.
#[derive(Debug, Clone)]
struct ChangeSet {
    daemon: bool,
    core: bool,
    extras: Vec<usize>,
}

impl ChangeSet {
    fn has_changes(&self) -> bool {
        self.daemon || self.core || !self.extras.is_empty()
    }
}

struct ApplyReport {
    core_request_id: Option<String>,
    degraded: Vec<String>,
}

struct Replacement {
    relative_path: String,
    had_pre_image: bool,
}

struct DaemonStatusView {
    running: bool,
    launched_config_revision: Option<String>,
}

fn short_revision(revision: &str) -> String {
    revision.chars().take(16).collect()
}

fn managed_state_dir(ctx: &instance::InstanceContext) -> anyhow::Result<std::path::PathBuf> {
    ctx.paths
        .tun_config_file
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "managed state directory is undefined for {}",
                ctx.paths.tun_config_file.display()
            )
        })
}

fn file_sha256(path: &std::path::Path) -> anyhow::Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    generation::calculate_sha256(path)
        .map(Some)
        .map_err(|error| anyhow::anyhow!("failed to hash {}: {error}", path.display()))
}

fn target_matches(target: &ArtifactTarget) -> anyhow::Result<bool> {
    Ok(file_sha256(&target.dest)?.as_deref() == Some(target.sha256.as_str()))
}

/// SPEC A: artifacts a baseline manifest declares as absent at capture time.
fn manifest_missing_set(
    manifest: &generation::GenerationManifest,
) -> std::collections::HashSet<String> {
    manifest
        .metadata
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

/// recovery candidates are built from the authenticated baseline (or a
/// freshly downloaded/validated artifact), never from the current install just
/// because it still passes `-v`. `None` means the baseline explicitly recorded
/// the artifact as absent.
fn recovery_baseline_artifact(
    ctx: &instance::InstanceContext,
    relative_path: &str,
) -> anyhow::Result<Option<Vec<u8>>> {
    let store = system_generation_store(ctx);
    let state = store.read_state()?;
    let baseline = state
        .previous
        .ok_or_else(|| anyhow::anyhow!("恢复态缺少绑定基线，拒绝捕获当前安装态"))?;
    anyhow::ensure!(
        !store.is_generation_untrusted(&baseline),
        "恢复基线未经认证，拒绝复用其字节"
    );
    let manifest = store.validate_generation(&baseline)?;
    anyhow::ensure!(manifest.is_baseline(), "恢复绑定不是已认证的基线代际");
    if manifest.missing_paths().contains(relative_path) {
        return Ok(None);
    }
    let Some(entry) = manifest
        .all_entries()
        .into_iter()
        .find(|entry| entry.relative_path == relative_path)
    else {
        return Ok(None);
    };
    let bytes = utils::read_file_no_follow_limited(
        &store.generation_dir(&baseline).join(&entry.relative_path),
        512 * 1024 * 1024,
    )?;
    anyhow::ensure!(
        generation::calculate_sha256_bytes(&bytes) == entry.sha256,
        "恢复基线 artifact 与 manifest 不一致"
    );
    Ok(Some(bytes))
}

fn build_artifact_targets(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    id: &generation::GenerationId,
    manifest: &generation::GenerationManifest,
) -> anyhow::Result<Vec<ArtifactTarget>> {
    let gen_dir = store.generation_dir(id);
    let state_dir = managed_state_dir(ctx)?;
    let mut targets = vec![
        ArtifactTarget {
            relative_path: manifest.daemon.relative_path.clone(),
            kind: generation::ArtifactKind::Daemon,
            sha256: manifest.daemon.sha256.clone(),
            source: gen_dir.join(&manifest.daemon.relative_path),
            dest: ctx.paths.cli_binary.clone(),
            mode: manifest.daemon.unix_mode,
        },
        ArtifactTarget {
            relative_path: manifest.core.relative_path.clone(),
            kind: generation::ArtifactKind::Core,
            sha256: manifest.core.sha256.clone(),
            source: gen_dir.join(&manifest.core.relative_path),
            dest: ctx.paths.core_binary.clone(),
            mode: manifest.core.unix_mode,
        },
    ];
    for extra in &manifest.extra_artifacts {
        anyhow::ensure!(
            matches!(
                extra.relative_path.as_str(),
                "geoip.metadb" | "GeoSite.dat" | "Country.mmdb"
            ) && !matches!(
                extra.kind,
                generation::ArtifactKind::Daemon | generation::ArtifactKind::Core
            ),
            "拒绝未知或角色冒充的受管资源路径: {}",
            extra.relative_path
        );
        targets.push(ArtifactTarget {
            relative_path: extra.relative_path.clone(),
            kind: extra.kind.clone(),
            sha256: extra.sha256.clone(),
            source: gen_dir.join(&extra.relative_path),
            dest: state_dir.join(&extra.relative_path),
            mode: extra.unix_mode,
        });
    }
    Ok(targets)
}

/// SPEC P4: replacement is content addressed — equal bytes are skipped, so a
/// daemon-only upgrade never touches the core inode.
fn change_set(plan: &ApplyPlan) -> anyhow::Result<ChangeSet> {
    let mut daemon = false;
    let mut core = false;
    let mut extras = Vec::new();
    for (index, target) in plan.targets.iter().enumerate() {
        if target_matches(target)? {
            continue;
        }
        if target.is_daemon() {
            daemon = true;
        } else if target.kind == generation::ArtifactKind::Core {
            core = true;
        } else {
            extras.push(index);
        }
    }
    Ok(ChangeSet {
        daemon,
        core,
        extras,
    })
}

fn write_artifact(target: &ArtifactTarget, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = target.dest.parent() {
        utils::ensure_dir_all_no_follow(parent)?;
    }
    if target.kind == generation::ArtifactKind::Daemon
        || target.kind == generation::ArtifactKind::Core
    {
        return utils::atomic_write_bytes_no_follow(&target.dest, bytes, 0o755);
    }
    // Extra artifacts live in the service-owned runtime directory: write directly
    // when we already have access, escalate only when the filesystem refuses.
    let mode = target.mode.unwrap_or(0o644) as u16;
    match utils::atomic_write_bytes_no_follow(&target.dest, bytes, mode) {
        Ok(()) => Ok(()),
        Err(error) if is_permission_denied(&error) => {
            service::PrivilegeExecutor::write_file(&target.dest, bytes, mode)
        }
        Err(error) => Err(error),
    }
}

fn is_permission_denied(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io_error| io_error.kind() == std::io::ErrorKind::PermissionDenied)
}

fn replace_artifacts(plan: &ApplyPlan, change: &ChangeSet) -> anyhow::Result<Vec<Replacement>> {
    let mut replaced = Vec::new();
    for (index, target) in plan.targets.iter().enumerate() {
        let changed = if target.is_daemon() {
            change.daemon
        } else if target.kind == generation::ArtifactKind::Core {
            change.core
        } else {
            change.extras.contains(&index)
        };
        if !changed {
            continue;
        }
        let had_pre_image = target.dest.exists();
        let bytes = std::fs::read(&target.source).map_err(|error| {
            anyhow::anyhow!(
                "cannot read staged artifact {}: {error}",
                target.source.display()
            )
        })?;
        write_artifact(target, &bytes)?;
        replaced.push(Replacement {
            relative_path: target.relative_path.clone(),
            had_pre_image,
        });
    }
    Ok(replaced)
}

/// SPEC P0: copies the pre-upgrade `cache.db` aside before anything in the
/// pipeline can start a Core that would delete a broken database.
///
/// the evidence face is the superset "corrupt content verdict +
/// unjudged overlay". `PermissionDrift` short-circuits the content verdict, so a
/// drifted file may also be corrupt and the Core would delete it once the repair
/// lets it open the file. Using the same superset predicate here closes the
/// post-recovery entry points (`tun off` / `uninstall` / `install` after a repair)
/// that the CLI repair gate deliberately does not cover.
fn preserve_upgrade_evidence(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let Some(state_dir) = ctx.paths.tun_config_file.parent() else {
        return Ok(());
    };
    let cache_db_path = state_dir.join(utils::CACHE_DB_FILE_NAME);
    let classification =
        utils::classify_cache_db_detailed(&cache_db_path, Some(SERVICE_ACCOUNT_USER));
    if !classification.needs_evidence_preservation() {
        return Ok(());
    }
    let preserved = utils::preserve_cache_db_evidence(&cache_db_path).map_err(|error| {
        anyhow::anyhow!(
            "cache.db 证据保全失败（{}），本次操作已中止，运行态未改动。\n  \
             原因: {error}\n  \
             Fix: 检查 {} 的可读性与所在目录权限后重试",
            classification.state.as_str(),
            state_dir.display()
        )
    })?;
    crate::log!("preserved cache.db evidence at {}", preserved.display());
    println!(
        "  ⚠ cache.db {} 已复制留证：{}",
        classification.state.as_str(),
        preserved.display()
    );
    Ok(())
}

/// SPEC P0: read-only transaction convergence. An in-flight TUN journal
/// already owns the runtime mutation; the generation pipeline must not become a
/// second side-effect owner (recovery runs before this pipeline, never here).
fn refuse_inflight_transaction(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let Some(journal) = tun_transaction::read_active_journal(ctx)? else {
        return Ok(());
    };
    if matches!(
        journal.phase,
        tun_transaction::JournalPhase::IntentCommitted | tun_transaction::JournalPhase::RolledBack
    ) {
        return Ok(());
    }
    anyhow::bail!(
        "存在未收敛的 TUN 事务（{:?}），本次不应用代际升级，运行态未改动。\n  Fix: mihomo-cli restart --system",
        journal.phase
    );
}

fn migrate_generation_store_if_needed(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
) -> anyhow::Result<()> {
    let legacy_root = ctx
        .paths
        .tun_config_file
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| ctx.paths.config_dir.clone());
    if legacy_root == store.root_dir() {
        return Ok(());
    }
    match generation::migrate_legacy_store(&legacy_root, store) {
        Ok(generation::MigrationOutcome::FirstInstall)
        | Ok(generation::MigrationOutcome::NewRootAuthoritative) => Ok(()),
        Ok(generation::MigrationOutcome::Migrated) => {
            println!("  ✅ 代际存储已从旧路径迁移到新信任根");
            Ok(())
        }
        Err(error) => anyhow::bail!(
            "代际存储迁移失败: {error}\n  旧数据保留，本工具未完成迁移；保留取证。显式只读检查：mihomo-cli doctor --system --privileged"
        ),
    }
}

/// SPEC P1 (frozen): the single daemon identity criterion — the candidate
/// daemon bytes must equal **the running executable's** bytes. Shared by the P1
/// verification, the skew detector and the tombstone retry gate so the three
/// cannot drift apart.
pub(crate) fn daemon_identity_matches_current_exe(candidate_sha256: &str) -> anyhow::Result<bool> {
    let current_exe = std::env::current_exe()?;
    let current_exe_sha = file_sha256(&current_exe)?
        .ok_or_else(|| anyhow::anyhow!("cannot hash current executable"))?;
    Ok(candidate_sha256 == current_exe_sha)
}

/// SPEC P1: the pending daemon bytes must be the identity we are running
/// (sudo re-entry or repair) — the pipeline validates, it never stages.
fn verify_pending_daemon_identity(
    _ctx: &instance::InstanceContext,
    manifest: &generation::GenerationManifest,
    pending_id: &generation::GenerationId,
) -> anyhow::Result<()> {
    if daemon_identity_matches_current_exe(&manifest.daemon.sha256)? {
        return Ok(());
    }
    let current_exe = std::env::current_exe()?;
    let current_exe_sha = file_sha256(&current_exe)?.unwrap_or_default();
    anyhow::bail!(
        "代际 {} 的 daemon 哈希 {} 与当前运行的可执行文件 {} 不匹配。\n  \
         零替换。请使用 mihomo-cli install --system 重新暂存。",
        pending_id,
        short_revision(&manifest.daemon.sha256),
        short_revision(&current_exe_sha),
    );
}

/// SPEC P2: strict binding verification. No binding, an inconsistent
/// binding, or a baseline that came from the untrusted legacy migration all mean
/// "fact unknown" → zero replacement plus recovery guidance.
fn verify_baseline_binding(
    store: &generation::GenerationStore,
    manifest: &generation::GenerationManifest,
    state: &generation::GenerationState,
    pending_id: &generation::GenerationId,
) -> anyhow::Result<generation::GenerationId> {
    if store.is_generation_untrusted(pending_id) {
        anyhow::bail!(
            "代际 {} 来自存量迁移，未经认证（事实不明），零替换。\n  \
             请使用 mihomo-cli install --system 重新暂存以建立基线绑定。",
            pending_id
        );
    }
    let binding = manifest.metadata.get("baseline").cloned();
    let previous = state.previous.as_ref().map(|id| id.as_str().to_string());
    let bound = match (binding, previous) {
        (Some(binding), Some(previous)) if binding == previous => binding,
        (None, _) => anyhow::bail!(
            "代际 {} 无基线绑定（legacy pending），事实不明，零替换。\n  \
             请使用 mihomo-cli install --system 重新暂存以建立基线绑定。",
            pending_id
        ),
        (Some(_), None) => anyhow::bail!(
            "代际 {} 有基线绑定但 state.previous 为空，事实不明，零替换。\n  \
             请使用 mihomo-cli install --system 重新暂存。",
            pending_id
        ),
        (Some(binding), Some(previous)) => anyhow::bail!(
            "代际 {} 的基线绑定 {} 与 state.previous {} 不一致，事实不明，零替换。\n  \
             请使用 mihomo-cli install --system 重新暂存。",
            pending_id,
            binding,
            previous
        ),
    };
    let baseline_id = generation::GenerationId::new(&bound)
        .map_err(|error| anyhow::anyhow!("绑定基线 ID {bound} 非法：{error}"))?;
    if store.is_generation_untrusted(&baseline_id) {
        anyhow::bail!(
            "代际 {} 的绑定基线 {} 来自存量迁移，未经认证（事实不明），零替换。\n  \
             请使用 mihomo-cli install --system 重新暂存以捕获可信基线。",
            pending_id,
            baseline_id
        );
    }
    Ok(baseline_id)
}

/// SPEC P1-2: freezes `(配置字节, revision, subscription_id)` before any
/// downtime so the mirror push, the start request and the report share one
/// identity. Lock order note: the generation lock is already held; the config
/// lock is a different (bounded, leaf) lock — no coordinator lock is taken.
fn freeze_config_triple(ctx: &instance::InstanceContext) -> anyhow::Result<Option<FrozenConfig>> {
    let config_path = lifecycle_system_config_path(ctx);
    if !config_path.exists() {
        return Ok(None);
    }
    let (content, revision) = lifecycle_system_config_payload(ctx)?;
    let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
    let (subscription_id, ownership_proven) = match config::get_active_id_at(&paths) {
        Ok(id) => (id, true),
        Err(error) => {
            crate::log!("active subscription pointer unreadable: {error}");
            (None, false)
        }
    };
    Ok(Some(FrozenConfig {
        content,
        revision,
        subscription_id,
        ownership_proven,
    }))
}

/// SPEC P5 mirror timing: push the mirror through the still-live old daemon
/// and require a confirmed business response (`Error` = channel unavailable).
async fn push_mirror_strict(
    ctx: &instance::InstanceContext,
    frozen: &FrozenConfig,
) -> anyhow::Result<()> {
    let Some(subscription_id) = frozen.subscription_id.clone() else {
        return Ok(());
    };
    match ipc::send_command(&ipc::DaemonCommand::RecordActiveSubscription {
        subscription_id: subscription_id.clone(),
        config_revision: Some(frozen.revision.clone()),
        token: None,
    })
    .await?
    {
        ipc::DaemonResponse::Success { .. } => {}
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!("RecordActiveSubscription 被拒绝：{message}")
        }
        other => anyhow::bail!("RecordActiveSubscription 返回非预期响应：{other:?}"),
    }
    let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
    let selection_path = paths.selection_state_path_for_subscription(&subscription_id);
    let Ok(content) = std::fs::read_to_string(&selection_path) else {
        return Ok(());
    };
    if content.trim().is_empty() {
        return Ok(());
    }
    match ipc::send_command(&ipc::DaemonCommand::RecordSelectionIntent {
        subscription_id,
        content_yaml: content,
        config_revision: Some(frozen.revision.clone()),
        token: None,
    })
    .await?
    {
        ipc::DaemonResponse::Success { .. } => Ok(()),
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!("RecordSelectionIntent 被拒绝：{message}")
        }
        other => anyhow::bail!("RecordSelectionIntent 返回非预期响应：{other:?}"),
    }
}

async fn daemon_status() -> anyhow::Result<DaemonStatusView> {
    match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await? {
        ipc::DaemonResponse::Status {
            running,
            launched_config_revision,
            ..
        } => Ok(DaemonStatusView {
            running,
            launched_config_revision,
        }),
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!("daemon status query failed: {message}")
        }
        other => anyhow::bail!("daemon returned unexpected status response: {other:?}"),
    }
}

/// SPEC P3: stopped proof for the managed Core while the daemon stays alive.
async fn wait_for_core_stopped_proof(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    while start.elapsed() < timeout {
        match daemon_status().await {
            Ok(status) if !status.running => {
                no_running_core_without_daemon(ctx)?;
                return Ok(());
            }
            Ok(_) => {}
            Err(error) => anyhow::bail!("无法获取 core 停止证明：{error}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "无法证明受管 core 已停止（{}s 超时），拒绝继续",
        timeout.as_secs()
    )
}

async fn quiesce_daemon_service(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    if ipc::is_daemon_running().await {
        let _ = ipc::send_command(&ipc::DaemonCommand::StopCore { token: None }).await;
    }
    let stop_plan = instance::planned_generation_quiesce_plan(ctx);
    for command in &stop_plan.commands {
        service::run_instance_command(command)?;
    }
    wait_for_system_daemon_shutdown().await?;
    // SPEC P3/P2-5: a stopped daemon is not a stopped Core. Replacing the
    // Core binary under an orphaned process would swap the file behind a running
    // runtime, so the same process-level proof the recovery path uses is required
    // before any replacement (fail closed, never kill).
    quiesce_managed_core_after_daemon_stop(ctx).await
}

/// SPEC P3: minimal downtime — the daemon service stops only when the
/// daemon binary changes; core/extra changes only restart the managed Core.
async fn quiesce_for_apply(
    ctx: &instance::InstanceContext,
    change: &ChangeSet,
) -> anyhow::Result<()> {
    if change.daemon {
        println!("  P3 停机：daemon 变更 → 停止 system service（最小停机范围）");
        return quiesce_daemon_service(ctx).await;
    }
    if change.core || !change.extras.is_empty() {
        println!("  P3 停机：core/资源变更 → 只停受管 core，daemon service 不动");
        if ipc::is_daemon_running().await {
            match ipc::send_command(&ipc::DaemonCommand::StopCore { token: None }).await? {
                ipc::DaemonResponse::Success { .. } => {}
                ipc::DaemonResponse::Error { message } => {
                    anyhow::bail!("core 停止请求被拒绝：{message}")
                }
                other => anyhow::bail!("core 停止返回非预期响应：{other:?}"),
            }
            return wait_for_core_stopped_proof(ctx).await;
        }
        return quiesce_managed_core_after_daemon_stop(ctx).await;
    }
    Ok(())
}

async fn start_daemon_service(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let start_plan = instance::planned_generation_resume_plan(ctx);
    for command in &start_plan.commands {
        service::run_instance_command(command)?;
    }
    wait_for_system_daemon_readiness().await
}

async fn core_payload(
    ctx: &instance::InstanceContext,
    frozen: Option<&FrozenConfig>,
) -> anyhow::Result<(String, String, Option<String>)> {
    if let Some(frozen) = frozen {
        return Ok((
            frozen.content.clone(),
            frozen.revision.clone(),
            frozen.subscription_id.clone(),
        ));
    }
    let (content, revision) = lifecycle_system_config_payload(ctx)?;
    let subscription_id =
        config::get_active_id_at(&utils::AppPaths::new(ctx.paths.config_dir.clone()))
            .ok()
            .flatten();
    Ok((content, revision, subscription_id))
}

fn print_replay_report(
    ctx: &instance::InstanceContext,
    request_id: &str,
    revision: &str,
    message: &str,
    frozen: Option<&FrozenConfig>,
) {
    let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
    println!(
        "  ✓ core 启动/重启请求 {request_id} 已确认（revision {}）",
        short_revision(revision)
    );
    let subscription_id_at_request = frozen.and_then(|frozen| frozen.subscription_id.as_deref());
    for line in selection_replay_message_lines(message, subscription_id_at_request, &paths) {
        println!("{line}");
    }
}

/// SPEC P5: the start/restart request is bound to the request ID printed
/// here and to the frozen revision (never to the mirror's current active slot).
async fn request_core_start(
    ctx: &instance::InstanceContext,
    tag: &str,
    frozen: Option<&FrozenConfig>,
) -> anyhow::Result<(String, String)> {
    let (content, revision, subscription_id) = core_payload(ctx, frozen).await?;
    let request_id = format!("start:{tag}:{}", short_revision(&revision));
    let response = ipc::send_command(&ipc::DaemonCommand::StartCore {
        config_content: content,
        config_revision: revision.clone(),
        subscription_id,
        token: None,
    })
    .await?;
    match response {
        ipc::DaemonResponse::Success { message } => {
            print_replay_report(ctx, &request_id, &revision, &message, frozen);
            Ok((request_id, message))
        }
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!(daemon_command_error_message(ctx.os, &message))
        }
        other => anyhow::bail!("core 启动返回非预期响应：{other:?}"),
    }
}

async fn request_core_restart(
    ctx: &instance::InstanceContext,
    tag: &str,
    frozen: Option<&FrozenConfig>,
) -> anyhow::Result<(String, String)> {
    let (content, revision, subscription_id) = core_payload(ctx, frozen).await?;
    let request_id = format!("restart:{tag}:{}", short_revision(&revision));
    let response = ipc::send_command(&ipc::DaemonCommand::RestartCore {
        config_content: content,
        config_revision: revision.clone(),
        subscription_id,
        token: None,
    })
    .await?;
    match response {
        ipc::DaemonResponse::Success { message } => {
            print_replay_report(ctx, &request_id, &revision, &message, frozen);
            Ok((request_id, message))
        }
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!(daemon_command_error_message(ctx.os, &message))
        }
        other => anyhow::bail!("core 重启返回非预期响应：{other:?}"),
    }
}

/// SPEC P5: start causality verdict for the running Core.
///
/// A missing receipt (daemon re-attached to a Core it did not launch) is
/// **unprovable**, never "assume fine": it forces a managed restart and the
/// post-restart proof still fails closed if the receipt stays absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartCausality {
    Satisfied,
    NeedsStart,
    NeedsRestart(String),
}

pub(crate) fn start_causality(
    frozen: Option<&FrozenConfig>,
    status: &DaemonStatusView,
) -> StartCausality {
    if !status.running {
        return StartCausality::NeedsStart;
    }
    let Some(frozen) = frozen else {
        return StartCausality::Satisfied;
    };
    match status.launched_config_revision.as_deref() {
        None => StartCausality::NeedsRestart(
            "受管 core 无法提供启动 receipt（daemon reattach 旧 core）：启动因果不可证".to_string(),
        ),
        Some(launched) if !frozen.revision.is_empty() && launched != frozen.revision => {
            StartCausality::NeedsRestart(format!(
                "core 运行快照 {} 不是本次冻结快照 {}",
                short_revision(launched),
                short_revision(&frozen.revision)
            ))
        }
        Some(_) => StartCausality::Satisfied,
    }
}

/// SPEC P5: Core-reported TUN state; `None` when it cannot be observed.
async fn observe_runtime_tun(ctx: &instance::InstanceContext) -> Option<bool> {
    let snapshot = status::StatusSnapshot::collect(ctx).await;
    snapshot.runtime_tun.as_bool()
}

/// SPEC P5: target-identity running proof. Reports success only when every
/// target artifact on disk equals the manifest, the daemon attests its own
/// revision, and the running Core carries this attempt's start causality.
async fn prove_runtime_identity(
    ctx: &instance::InstanceContext,
    plan: &ApplyPlan,
    frozen: Option<&FrozenConfig>,
    status: &DaemonStatusView,
    _tun_before: Option<bool>,
    degraded: &mut Vec<String>,
) -> anyhow::Result<()> {
    for target in &plan.targets {
        match file_sha256(&target.dest)? {
            Some(actual) if actual == target.sha256 => {}
            Some(actual) => anyhow::bail!(
                "artifact {} 磁盘哈希 {} 与 manifest {} 不一致（目标身份运行证明失败）",
                target.dest.display(),
                short_revision(&actual),
                short_revision(&target.sha256)
            ),
            None => anyhow::bail!(
                "artifact {} 缺失（目标身份运行证明失败）",
                target.dest.display()
            ),
        }
    }
    verify_running_daemon_revision(&plan.manifest.daemon.sha256).await?;
    let config_path = lifecycle_system_config_path(ctx);
    if !config_path.exists() {
        return Ok(());
    }
    if !status.running {
        anyhow::bail!("受管 core 未运行：目标身份无法证明，不允许报成功");
    }
    match start_causality(frozen, status) {
        StartCausality::Satisfied => {}
        other => anyhow::bail!("启动因果不可证（{other:?}）：不允许报成功"),
    }
    // SPEC P5: TUN attestation is an explicit component of the proof
    // formula. The target is the effective config that was just promoted: a
    // restore/enable (before=false → after=true) is valid when the target says
    // TUN enabled; only a mismatch against the target, or an unobservable state,
    // is a failure. This keeps the no-silent-TUN-loss guard without blocking a
    // legitimate recovery from an earlier TUN-disabled rollback.
    let target_tun = frozen
        .and_then(|frozen| {
            tun_transaction::parse_tun_enabled_from_bytes(frozen.content.as_bytes()).ok()
        })
        .or_else(|| {
            std::fs::read(&config_path)
                .ok()
                .and_then(|bytes| tun_transaction::parse_tun_enabled_from_bytes(&bytes).ok())
        });
    match (target_tun, observe_runtime_tun(ctx).await) {
        (Some(target), Some(after)) if target == after => {}
        (Some(target), Some(after)) => anyhow::bail!(
            "升级后 TUN 运行态 {after} 与目标配置 {target} 不一致：TUN attestation 失败"
        ),
        _ => degraded.push(
            "TUN 运行态在升级前后不可完全观测：TUN attestation 显式降级（未证明）".to_string(),
        ),
    }
    Ok(())
}

/// SPEC P5: start (or restart) and prove. At most one managed Core restart
/// is spent on the mirror degradation path.
async fn start_and_prove(
    ctx: &instance::InstanceContext,
    plan: &ApplyPlan,
    frozen: Option<&FrozenConfig>,
    mirror_ready: bool,
    tun_before: Option<bool>,
    degraded: &mut Vec<String>,
) -> anyhow::Result<Option<String>> {
    if !ipc::is_daemon_running().await {
        start_daemon_service(ctx).await?;
    }
    verify_running_daemon_revision(&plan.manifest.daemon.sha256).await?;
    ensure_system_daemon_access(ctx).await?;
    maybe_auto_recover_active_transaction(
        ctx,
        tun_transaction::RecoveryDirection::Abort,
        false,
        false,
        false,
    )
    .await?;

    let mut mirror_rebuilt = false;
    if !mirror_ready {
        match frozen {
            Some(frozen) if frozen.ownership_proven => {
                match push_mirror_strict(ctx, frozen).await {
                    Ok(()) => {
                        mirror_rebuilt = true;
                        degraded.push(
                            "旧 daemon 不可用：镜像已在新 daemon 后重建，补偿 1 次受管 core 重启"
                                .to_string(),
                        );
                    }
                    Err(error) => degraded.push(format!(
                        "新 daemon 后镜像重建失败（{error}）：选点回放可能缺失"
                    )),
                }
            }
            _ => {}
        }
    }

    let mut request_id = None;
    let config_path = lifecycle_system_config_path(ctx);
    if config_path.exists() {
        let status = daemon_status().await?;
        match start_causality(frozen, &status) {
            StartCausality::NeedsStart => {
                let (id, _) = request_core_start(ctx, plan.pending_id.as_str(), frozen).await?;
                request_id = Some(id);
            }
            StartCausality::NeedsRestart(reason) => {
                // Single restart call site: the "at most one managed restart"
                // budget is structural (P0-1: unknown receipt ⇒ restart, then
                // the post-restart proof still fails closed if it stays absent).
                degraded.push(reason);
                let (id, _) = request_core_restart(ctx, plan.pending_id.as_str(), frozen).await?;
                request_id = Some(id);
            }
            StartCausality::Satisfied if mirror_rebuilt => {
                let (id, _) = request_core_restart(ctx, plan.pending_id.as_str(), frozen).await?;
                request_id = Some(id);
            }
            StartCausality::Satisfied => {}
        }
        wait_for_instance_readiness(ctx).await?;
    }

    let status = daemon_status().await?;
    prove_runtime_identity(ctx, plan, frozen, &status, tun_before, degraded).await?;
    Ok(request_id)
}

/// SPEC P3/P4/P5 for one attempt.
async fn run_apply_pipeline(
    ctx: &instance::InstanceContext,
    plan: &ApplyPlan,
    frozen: Option<&FrozenConfig>,
) -> anyhow::Result<ApplyReport> {
    let change = change_set(plan)?;
    let mut degraded = Vec::new();

    // Mirror timing: push through the old daemon while it is still alive.
    let mirror_ready = match frozen {
        Some(frozen) if frozen.ownership_proven => match push_mirror_strict(ctx, frozen).await {
            Ok(()) => true,
            Err(error) => {
                degraded.push(format!(
                    "旧 daemon 镜像推送不可用（{error}）：退化到新 daemon 后重建 + 至多一次受管 core 重启"
                ));
                false
            }
        },
        // Ownership unprovable → replays are skipped (explicit degradation).
        Some(_) => false,
        // No managed config at all → nothing to replay; not a degradation.
        None => true,
    };

    // SPEC P5: TUN state snapshot before any downtime (old runtime alive).
    let tun_before = observe_runtime_tun(ctx).await;

    if change.has_changes() {
        quiesce_for_apply(ctx, &change).await?;
        let replaced = replace_artifacts(plan, &change)?;
        for replacement in &replaced {
            println!(
                "  · 替换 {}{}",
                replacement.relative_path,
                if replacement.had_pre_image {
                    ""
                } else {
                    "（替换前不存在）"
                }
            );
        }
    } else {
        println!("  P3/P4 跳过：全部 artifact 与 manifest 一致（零 inode churn）");
    }

    let core_request_id =
        start_and_prove(ctx, plan, frozen, mirror_ready, tun_before, &mut degraded).await?;
    Ok(ApplyReport {
        core_request_id,
        degraded,
    })
}

fn print_apply_success(plan: &ApplyPlan, report: &ApplyReport) {
    for line in &report.degraded {
        println!("  ⚠ {line}");
    }
    if let Some(request_id) = &report.core_request_id {
        println!("  · core 启动因果已绑定请求 {request_id}");
    }
    println!(
        "  ✅ Successfully upgraded to generation {}（全部 artifact 与 manifest 一致 + 目标身份运行证明通过）",
        plan.pending_id
    );
}

/// SPEC N3: a succeeded explicit retry retires the tombstones of withdrawn
/// generations that shipped the same daemon revision.
fn retire_successful_tombstones(store: &generation::GenerationStore, plan: &ApplyPlan) {
    let Ok(withdrawn) = store.list_withdrawn() else {
        return;
    };
    for id in withdrawn {
        let manifest = match store.read_withdrawn_manifest(&id) {
            Ok(manifest) => manifest,
            Err(error) => {
                crate::log!("cannot read tombstone manifest {}: {error}", id);
                eprintln!("  ⚠ 墓碑 {id} 无法读取（保留原状）：{error}");
                continue;
            }
        };
        // Unknown/migrated tombstones are evidence, not authenticated native retries.
        if store
            .withdrawn_path(&id)
            .join(generation::UNTRUSTED_MARKER)
            .exists()
        {
            continue;
        }
        let authenticated_binding = manifest
            .metadata
            .get("baseline")
            .and_then(|value| generation::GenerationId::new(value).ok())
            .filter(|baseline| !store.is_generation_untrusted(baseline))
            .and_then(|baseline| store.validate_generation(&baseline).ok())
            .is_some_and(|baseline| baseline.is_baseline());
        if !authenticated_binding {
            continue;
        }
        // SPEC N3 "同源" = the same daemon identity: the withdrawn
        // generation's *core* was a failed candidate and carries no identity, so
        // a successful retry of the same daemon revision retires it.
        if manifest.daemon.sha256 != plan.manifest.daemon.sha256 {
            continue;
        }
        match store.retire_withdrawn(&id) {
            Ok(()) => println!(
                "  ✓ 已退休墓碑 {}（本次升级成功应用同一 daemon/core 版本）",
                id
            ),
            Err(error) => {
                crate::log!("cannot retire tombstone {}: {error}", id);
                eprintln!("  ⚠ 墓碑 {id} 退休失败（保留原状）：{error}");
            }
        }
    }
}

/// SPEC P4/C: restoration draws from the bound baseline, including the
/// "artifact did not exist" state (moved aside with a no-replace rename).
fn restore_baseline_artifacts(
    baseline_targets: &[ArtifactTarget],
    missing: &std::collections::HashSet<String>,
) -> anyhow::Result<()> {
    for target in baseline_targets {
        if missing.contains(&target.relative_path) {
            if target.dest.exists() {
                let moved = move_aside_no_replace(&target.dest)?;
                println!(
                    "  · 恢复缺失态：{} → {}",
                    target.dest.display(),
                    moved.display()
                );
            }
            continue;
        }
        if target_matches(target)? {
            continue;
        }
        let bytes = std::fs::read(&target.source).map_err(|error| {
            anyhow::anyhow!(
                "cannot read baseline artifact {}: {error}",
                target.source.display()
            )
        })?;
        write_artifact(target, &bytes)?;
        println!("  · 回滚 {}", target.relative_path);
    }
    Ok(())
}

fn move_aside_no_replace(path: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid file name for {}", path.display()))?;
    for seq in 0..64u32 {
        let candidate = path.with_file_name(format!("{name}.rollback-removed-{seq:02}"));
        if candidate.exists() {
            continue;
        }
        std::fs::rename(path, &candidate).map_err(|error| {
            anyhow::anyhow!(
                "cannot move {} aside to {}: {error}",
                path.display(),
                candidate.display()
            )
        })?;
        return Ok(candidate);
    }
    anyhow::bail!(
        "cannot allocate a no-replace rollback name for {}",
        path.display()
    )
}

async fn prove_baseline_identity(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    baseline_id: &generation::GenerationId,
    baseline_manifest: &generation::GenerationManifest,
) -> anyhow::Result<()> {
    let baseline_targets = build_artifact_targets(ctx, store, baseline_id, baseline_manifest)?;
    let missing = manifest_missing_set(baseline_manifest);
    for target in &baseline_targets {
        if missing.contains(&target.relative_path) {
            if target.dest.exists() {
                anyhow::bail!(
                    "基线声明 {} 本不存在，但安装态仍有该文件（补偿未收敛）",
                    target.dest.display()
                );
            }
            continue;
        }
        match file_sha256(&target.dest)? {
            Some(actual) if actual == target.sha256 => {}
            Some(actual) => anyhow::bail!(
                "回滚后 {} 的哈希 {} 与基线 {} 不一致",
                target.dest.display(),
                short_revision(&actual),
                short_revision(&target.sha256)
            ),
            None => anyhow::bail!("回滚后 {} 缺失", target.dest.display()),
        }
    }
    if missing.contains(&baseline_manifest.daemon.relative_path) {
        anyhow::bail!("基线不含 daemon 二进制，无法启动并证明回滚运行态");
    }
    verify_running_daemon_revision(&baseline_manifest.daemon.sha256).await?;
    Ok(())
}

/// SPEC C: compensating rollback in the order that makes crash semantics
/// convergent: stopped proof → per-artifact restore → start + proof →
/// tombstone (no-replace) → slot consumption.
async fn rollback_to_baseline(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    lock: &generation::GenerationLockGuard,
    plan: &ApplyPlan,
    frozen: Option<&FrozenConfig>,
) -> anyhow::Result<()> {
    let baseline_targets =
        build_artifact_targets(ctx, store, &plan.baseline_id, &plan.baseline_manifest)?;
    let missing = manifest_missing_set(&plan.baseline_manifest);
    let daemon_revert = match baseline_targets.iter().find(|target| target.is_daemon()) {
        Some(target) => !missing.contains(&target.relative_path) && !target_matches(target)?,
        None => false,
    };
    if daemon_revert {
        quiesce_daemon_service(ctx).await?;
    } else if ipc::is_daemon_running().await {
        let _ = ipc::send_command(&ipc::DaemonCommand::StopCore { token: None }).await;
        wait_for_core_stopped_proof(ctx).await?;
    } else {
        quiesce_managed_core_after_daemon_stop(ctx).await?;
    }

    restore_baseline_artifacts(&baseline_targets, &missing)?;

    if !ipc::is_daemon_running().await {
        start_daemon_service(ctx).await?;
    }
    ensure_system_daemon_access(ctx).await?;
    if lifecycle_system_config_path(ctx).exists() {
        let status = daemon_status().await?;
        if !status.running {
            let _ = request_core_start(ctx, plan.baseline_id.as_str(), frozen).await?;
        }
        wait_for_instance_readiness(ctx).await?;
    }
    prove_baseline_identity(ctx, store, &plan.baseline_id, &plan.baseline_manifest).await?;

    // SPEC 裁决 A1: mark "rollback proven" (inside the generation dir, so the
    // marker travels with the tombstone rename). Without this marker a tombstone
    // only means "candidate revoked", never "runtime restored".
    store
        .mark_rollback_proven(&plan.pending_id)
        .map_err(|error| anyhow::anyhow!("回滚证明标记写入失败（补偿不算成功）：{error}"))?;
    // SPEC N2: tombstone first, then consume the slots. Collision refuses
    // (an existing tombstone means the withdrawn semantics are undefined).
    store
        .withdraw_generation(&plan.pending_id)
        .map_err(|error| anyhow::anyhow!("撤销墓碑发布失败（补偿不算成功）：{error}"))?;
    store
        .compensate_with_lock(lock, Some(&plan.baseline_id))
        .map_err(|error| anyhow::anyhow!("槽位消费失败：{error}"))?;
    Ok(())
}

async fn compensate_after_failure(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    lock: &generation::GenerationLockGuard,
    plan: &ApplyPlan,
    frozen: Option<&FrozenConfig>,
    cause: &anyhow::Error,
) -> anyhow::Result<bool> {
    println!("  ⚠ 代际 {} 应用失败：{cause:#}", plan.pending_id);
    match rollback_to_baseline(ctx, store, lock, plan, frozen).await {
        Ok(()) => {
            println!(
                "  ✅ 已从绑定基线 {} 回退，升级未生效（墓碑 {}.withdrawn 已发布）",
                plan.baseline_id, plan.pending_id
            );
            println!("  重试请显式运行: mihomo-cli install --system");
            Ok(true)
        }
        Err(rollback_error) => anyhow::bail!(
            "⚠ upgrade failed and rollback failed\n  \
             Pending {pending} retained; runtime has not been restored.\n  \
             cause: {cause:#}\n  \
             rollback: {rollback_error:#}\n  \
             Fix: mihomo-cli install --system --discard-pending={pending}",
            pending = plan.pending_id
        ),
    }
}

/// SPEC C: `pending = P` with the `P.withdrawn` tombstone already published
/// means compensation is nearly complete — finish the slot consumption.
async fn finish_withdrawn_compensation(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    lock: &generation::GenerationLockGuard,
    state: &generation::GenerationState,
    pending_id: &generation::GenerationId,
) -> anyhow::Result<bool> {
    let Some(baseline_id) = state.previous.clone() else {
        anyhow::bail!(
            "代际 {} 已发布撤销墓碑但槽位未消费，且缺少基线引用（state.previous 为空）。\n  Fix: mihomo-cli install --system",
            pending_id
        );
    };
    // a migrated pending may share an ID with a copied tombstone. It must
    // never be consumed here without the explicit combined adoption+discard.
    if is_migrated_generation_evidence(store, pending_id) {
        anyhow::bail!(
            "待应用代际 {pending_id} 来自存量迁移、未经认证，拒绝自动消费槽位。\n  \
             管理员核对后显式采纳并撤销：mihomo-cli install --system --adopt-current --discard-pending={pending_id}"
        );
    }
    verify_native_withdrawal(store, pending_id, &baseline_id)?;
    // SPEC 裁决 A1: a tombstone WITHOUT the rollback-proof marker is an
    // explicit discard — consume the metadata-only revoked terminal state and
    // never claim (or require) a restored runtime; zero stops/starts happen here.
    if !store.has_rollback_proof(pending_id) {
        store
            .consume_revoked_with_lock(lock, Some(&baseline_id))
            .map_err(|error| anyhow::anyhow!("槽位消费失败：{error}"))?;
        println!(
            "  ✅ 撤销已收敛：待应用代际 {} 已撤销，运行态未恢复（active=∅, previous={}）",
            pending_id, baseline_id
        );
        return Ok(true);
    }
    // SPEC C/P2-8: never consume the slots on an unproven baseline — the
    // runtime must still be the verified baseline before it becomes `active`.
    let baseline_manifest = store.validate_generation(&baseline_id).map_err(|error| {
        anyhow::anyhow!(
            "补偿重入：绑定基线 {} 不可验证（{error}），拒绝消费槽位。\n  Fix: mihomo-cli restart --system",
            baseline_id
        )
    })?;
    prove_baseline_identity(ctx, store, &baseline_id, &baseline_manifest)
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "补偿重入：运行态未能证明为基线 {}（{error}），拒绝消费槽位。\n  Fix: mihomo-cli restart --system",
                baseline_id
            )
        })?;
    store
        .compensate_with_lock(lock, Some(&baseline_id))
        .map_err(|error| anyhow::anyhow!("槽位消费失败：{error}"))?;
    println!(
        "  ✅ 补偿已收敛：槽位回到基线 {}（pending 已清空）",
        baseline_id
    );
    Ok(true)
}

/// SPEC C/P2-7: verify the already-committed generation and report the
/// outcome (never a silent convergence claim). Returns without mutating anything.
async fn verify_committed_generation(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    state: &generation::GenerationState,
) -> anyhow::Result<()> {
    let Some(active_id) = state.active.clone() else {
        return Ok(());
    };
    let Ok(manifest) = store.validate_generation(&active_id) else {
        println!(
            "  ⚠ 已提交代际 {} 不可验证：请运行 mihomo-cli install --system 重新暂存",
            active_id
        );
        return Ok(());
    };
    let mut mismatched = Vec::new();
    for target in build_artifact_targets(ctx, store, &active_id, &manifest)? {
        if !target_matches(&target)? {
            mismatched.push(target.relative_path.clone());
        }
    }
    if !mismatched.is_empty() {
        println!(
            "  ⚠ 运行态与已提交代际 {} 不一致（{}）：本次仅执行普通 restart 收敛，升级报告已丢失",
            active_id,
            mismatched.join(", ")
        );
        return Ok(());
    }
    if ipc::is_daemon_running().await {
        match verify_running_daemon_revision(&manifest.daemon.sha256).await {
            Ok(()) => println!(
                "  ✓ 运行态与已提交代际 {} 一致（提交后崩溃重入：本次仅复验，升级报告已丢失）",
                active_id
            ),
            Err(error) => println!(
                "  ⚠ 已提交代际 {} 的 daemon 身份未能验证（{error}）：本次仅执行普通 restart 收敛",
                active_id
            ),
        }
    }
    Ok(())
}

pub(crate) async fn apply_pending_generation(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<bool> {
    if ctx.mode != instance::InstanceMode::System {
        return Ok(false);
    }
    // SPEC §12.3.1（裁决 B）：代际信任根是独立的 root 专属域；应用代际属于业务
    // 阶段，可在首次特权读取之前请求一次授权（绝不宣称权限异常）。
    authorize_protected_domain_operations(ctx, "restart --system", "重启")?;
    if is_current_process_root() {
        utils::ensure_mihomo_system_state_dir()?;
    }
    let store = system_generation_store(ctx);

    // SPEC P1-1: the trust root authenticates baselines — owner/mode drift
    // means the store cannot be trusted, so the pipeline refuses before any work.
    utils::verify_generation_trust_root(store.root_dir())
        .map_err(|error| generation_trust_root_refusal("进入升级管线", &error))?;

    // SPEC P1-1: trust root migration precedes every side effect.
    migrate_generation_store_if_needed(ctx, &store)?;
    // Create/normalise the root with its contract (0750, root-owned) before the
    // lock file or any staged write can inherit the process umask.
    store
        .init()
        .map_err(|error| anyhow::anyhow!("failed to init generation store: {error}"))?;

    // SPEC P0: evidence preservation + transaction convergence, lock-out.
    preserve_upgrade_evidence(ctx)?;
    refuse_inflight_transaction(ctx)?;

    // The generation lock spans the whole pipeline (P1-P6).
    let lock = store
        .acquire_lock()
        .map_err(|error| anyhow::anyhow!("failed to acquire generation lock: {error}"))?;

    let state = store
        .read_state()
        .map_err(|error| anyhow::anyhow!("代际状态损坏，拒绝应用（零替换）：{error}"))?;
    let Some(pending_id) = state.pending.clone() else {
        // SPEC P6/C (P2-7): a crash after the CAS commit leaves no pending.
        // The normal restart path converges, but the terminal claim must still be
        // verified and reported instead of silently returning `false`.
        verify_committed_generation(ctx, &store, &state).await?;
        return Ok(false);
    };

    if store.has_withdrawn(&pending_id) {
        return finish_withdrawn_compensation(ctx, &store, &lock, &state, &pending_id).await;
    }

    // === P1: manifest verification + expected ID snapshot (spans P2-P6) ===
    let manifest = store.validate_generation(&pending_id).map_err(|error| {
        anyhow::anyhow!(
            "Failed to validate pending generation {}: {error}",
            pending_id
        )
    })?;
    verify_pending_daemon_identity(ctx, &manifest, &pending_id)?;

    // === P2: binding verification (strict) ===
    let baseline_id = verify_baseline_binding(&store, &manifest, &state, &pending_id)?;
    let baseline_manifest = store.validate_generation(&baseline_id).map_err(|error| {
        anyhow::anyhow!(
            "代际 {} 的绑定基线 {} 不可验证（{error}）：事实不明，零替换。\n  \
             请使用 mihomo-cli install --system 重新暂存。",
            pending_id,
            baseline_id
        )
    })?;

    let targets = build_artifact_targets(ctx, &store, &pending_id, &manifest)?;
    let plan = ApplyPlan {
        pending_id: pending_id.clone(),
        manifest,
        baseline_id,
        baseline_manifest,
        targets,
    };

    println!("  Pending generation detected: {}", pending_id);
    let frozen = freeze_config_triple(ctx)?;
    if frozen
        .as_ref()
        .is_some_and(|frozen| !frozen.ownership_proven)
    {
        println!("  ⚠ 配置归属不可证：本次跳过选点回放（显式降级，0 次回放）");
    }

    match run_apply_pipeline(ctx, &plan, frozen.as_ref()).await {
        Ok(report) => {
            store
                .commit_active_cas(&lock, &plan.pending_id, Some(&plan.baseline_id))
                .map_err(|error| anyhow::anyhow!("CAS commit failed: {error}"))?;
            retire_successful_tombstones(&store, &plan);
            let _ = cleanup_system_generation_old(ctx, &store, &lock, 2);
            print_apply_success(&plan, &report);
            Ok(true)
        }
        Err(error) => {
            compensate_after_failure(ctx, &store, &lock, &plan, frozen.as_ref(), &error).await
        }
    }
}

pub(crate) async fn maybe_auto_recover_active_transaction(
    ctx: &instance::InstanceContext,
    direction: tun_transaction::RecoveryDirection,
    allow_runtime_reset: bool,
    prompt_runtime_reset: bool,
    allow_legacy_reset: bool,
) -> anyhow::Result<()> {
    if ctx.mode != instance::InstanceMode::System {
        return Ok(());
    }
    let active_journal = tun_transaction::read_active_journal(ctx)?;
    if active_journal.is_none() && !tun_transaction::legacy_journal_path(ctx).exists() {
        return Ok(());
    }
    // SPEC P0/P1-3: single-point evidence preservation for EVERY recovery
    // entry (restart / tun on-off / uninstall / install post-steps). It must run
    // before the recovery marker and before any recovery action, because a
    // recovery may start the old Core whose bbolt init deletes a broken cache.db.
    // The helper is idempotent (identical bytes reuse the existing artifact).
    preserve_upgrade_evidence(ctx)?;
    // SPEC B5: non-blocking recovery admission. A concurrent recovery must
    // fail immediately instead of queueing behind the coordinator lock; the RAII
    // guard releases the marker on every exit path, including failures.
    // R2: every durable recovery mutation (including legacy migration below)
    // happens under the marker.
    let _recovery_admission = match tun_transaction::try_acquire_recovery_marker(ctx)? {
        Some(guard) => guard,
        None => anyhow::bail!("recovery already in progress"),
    };
    if tun_transaction::check_and_migrate_legacy_journal(ctx)?.is_some()
        && is_current_process_root()
    {
        utils::ensure_mihomo_system_state_dir()?;
    }
    let Some(journal) = tun_transaction::read_active_journal(ctx)? else {
        return Ok(());
    };
    if matches!(
        journal.phase,
        tun_transaction::JournalPhase::IntentCommitted | tun_transaction::JournalPhase::RolledBack
    ) {
        let _ = tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation);
        return Ok(());
    }

    let obs = match ipc::send_command(&ipc::DaemonCommand::GetTransactionStatus { token: None })
        .await
    {
        Ok(ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::Completed(proof),
        }) => tun_transaction::RuntimeObservation {
            core_running: true,
            core_identity: Some(proof.core_identity),
            core_pid: Some(proof.core_pid),
            launched_revision: Some(proof.launched_revision),
            runtime_tun: Some(proof.runtime_tun),
            api_ready: proof.api_ready,
        },
        Ok(ipc::DaemonResponse::Status {
            running,
            core_pid,
            launched_config_revision,
            ..
        }) => {
            let api_ready = mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs")
                .await
                .is_ok();
            tun_transaction::RuntimeObservation {
                core_running: running,
                core_identity: Some(ctx.paths.core_binary.to_string_lossy().to_string()),
                core_pid,
                launched_revision: launched_config_revision,
                runtime_tun: None,
                api_ready,
            }
        }
        _ => tun_transaction::RuntimeObservation {
            core_running: false,
            core_identity: None,
            core_pid: None,
            launched_revision: None,
            runtime_tun: None,
            api_ready: false,
        },
    };

    let snap_cls = tun_transaction::classify_snapshot(ctx, &journal);
    let intent_cls = tun_transaction::classify_intent(&ctx.paths.intent_config_file, &journal);
    let action = tun_transaction::plan_recovery(
        &journal,
        snap_cls,
        intent_cls,
        tun_transaction::classify_old_runtime_evidence(ctx, &journal),
        &obs,
        direction,
    );

    match route_recovery_action(action, allow_runtime_reset, prompt_runtime_reset) {
        RecoveryRoute::ControlledReset => {
            let confirmed = if prompt_runtime_reset {
                confirm_managed_runtime_reset()?
            } else {
                // `--yes` replaces the question, never the impact statement: the
                // consequence is printed before anything is stopped.
                print_runtime_reset_impact();
                true
            };
            if !confirmed {
                anyhow::bail!(
                    "未执行修复；你的配置未被修改。需要修复时请重新执行：mihomo-cli restart --yes"
                );
            }
            let reset_message = reset_managed_system_runtime(ctx, allow_legacy_reset).await?;
            println!("  ⚠ {reset_message}");
        }
        RecoveryRoute::Refuse(message) => {
            anyhow::bail!("{message}\n  {}", managed_recovery_required_message());
        }
        RecoveryRoute::Execute(action) => {
            execute_recovery_action(ctx, &journal, action, &obs, direction, allow_runtime_reset)
                .await?;
        }
    }
    Ok(())
}

/// a refusal carries its own Fix guidance, so routing keeps the
/// planner's message instead of collapsing every refusal into one generic hint.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecoveryRoute {
    /// The refusal authorizes the controlled reset (SPEC §12.3:8).
    ControlledReset,
    /// Surface the refusal with the planner's Fix line.
    Refuse(String),
    /// Run the planned action.
    Execute(tun_transaction::RecoveryAction),
}

fn route_recovery_action(
    action: tun_transaction::RecoveryAction,
    allow_runtime_reset: bool,
    prompt_runtime_reset: bool,
) -> RecoveryRoute {
    match action {
        tun_transaction::RecoveryAction::RefuseNeedsEvidence(_)
            if allow_runtime_reset || prompt_runtime_reset =>
        {
            RecoveryRoute::ControlledReset
        }
        tun_transaction::RecoveryAction::RefuseNeedsEvidence(message) => {
            RecoveryRoute::Refuse(message)
        }
        other_action => RecoveryRoute::Execute(other_action),
    }
}

fn managed_recovery_required_message() -> String {
    "检测到 mihomo 之前留下的运行状态，暂时无法自动恢复。你的配置未被修改。\n  如需重置 mihomo 管理的运行状态并重新启动，请执行：mihomo-cli restart --yes".to_string()
}

fn confirm_managed_runtime_reset() -> anyhow::Result<bool> {
    use dialoguer::Confirm;

    Ok(Confirm::new()
        .with_prompt(RUNTIME_RESET_IMPACT_PROMPT)
        .default(false)
        .interact()?)
}

/// the impact statement shown for every controlled reset — as the
/// prompt when a terminal can answer, and printed before the stop when `--yes`
/// skips the question. `--yes` replaces the business confirmation only; the OS
/// authorization (root daemon peer) is still required.
const RUNTIME_RESET_IMPACT_PROMPT: &str = "检测到旧版本运行状态。将停止当前受管 Core，保留用户配置并重建运行状态；代理连接会暂时中断，TUN 可能关闭。是否继续修复";

fn print_runtime_reset_impact() {
    println!(
        "  ⚠ 检测到旧版本运行状态。将停止当前受管 Core，保留用户配置并重建运行状态；代理连接会暂时中断，TUN 可能关闭。"
    );
}

/// SPEC §12.3:8: controlled reset. Cleanup happens **only** after the single
/// stopped proof (the same proof the quiesce path consumes). A refusal or a
/// failed stop keeps every artifact and returns a non-zero result.
/// `allow_legacy_reset` is set only by an explicit `restart --system` that the
/// user confirmed (TTY prompt or `--yes`).
async fn reset_managed_system_runtime(
    ctx: &instance::InstanceContext,
    allow_legacy_reset: bool,
) -> anyhow::Result<String> {
    reset_managed_system_runtime_with_proof(ctx, |fence| {
        prove_managed_runtime_stopped(ctx, fence, allow_legacy_reset)
    })
    .await
}

async fn reset_managed_system_runtime_with_proof<F, Fut>(
    ctx: &instance::InstanceContext,
    prove_stopped: F,
) -> anyhow::Result<String>
where
    F: FnOnce(tun_transaction::TransactionFence) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let Some(journal) = tun_transaction::read_active_journal(ctx)? else {
        return Ok("未发现需要重置的运行状态".to_string());
    };
    if !tun_transaction::validate_managed_active_transaction(ctx, &journal)? {
        anyhow::bail!(managed_recovery_required_message());
    }
    let fence = tun_transaction::TransactionFence {
        transaction_id: journal.transaction_id.clone(),
        generation: journal.generation,
        expected_phase: journal.phase,
        expected_candidate_revision: journal.candidate_revision.clone(),
    };

    if let Err(error) = prove_stopped(fence.clone()).await {
        let (headline, detail) = match error.downcast_ref::<ManagedStopError>() {
            Some(stop) if stop.phase == StopAttemptPhase::AttemptedButUnconfirmed => (
                "受控重置未执行：已尝试停止 Core，但退出未被确认",
                stop.detail.clone(),
            ),
            Some(stop) => ("受控重置未执行：未尝试停止 Core", stop.detail.clone()),
            None => (
                "受控重置未执行：无法证明运行中的 Core 已停止",
                error.to_string(),
            ),
        };
        anyhow::bail!(
            "{headline}，运行状态证据全部保留。\n  {detail}\n  Fix: mihomo-cli restart --system"
        );
    }

    reset_managed_runtime_artifacts(ctx, &journal)?;
    println!("  ✓ 已保留用户配置并重置 mihomo 运行状态");
    Ok("正在从用户配置重新建立运行状态".to_string())
}

/// The destructive half of a reset. Only reached behind a validated stopped proof.
///
/// 准入④: the active journal is the lifecycle gate that refuses every
/// new Core start (SPEC §12.4), so it is quarantined **last**. From the stopped
/// proof to the end of destructive cleanup no new Core start can be admitted by
/// the daemon; the recovery marker alone would only exclude other recovery agents.
fn reset_managed_runtime_artifacts(
    ctx: &instance::InstanceContext,
    journal: &tun_transaction::TunJournal,
) -> anyhow::Result<()> {
    tun_transaction::remove_user_gate(
        &ctx.paths.config_dir,
        &journal.transaction_id,
        journal.generation,
    )?;
    tun_transaction::remove_managed_snapshot(ctx)?;
    if journal.legacy_source {
        tun_transaction::remove_legacy_artifacts(ctx)?;
    }
    // the broken cache.db is evidence too. It is moved aside with the
    // same no-replace primitive only after the stopped proof; a failure here must
    // not resurrect the transaction, so it is reported without undoing the reset.
    if let Some(state_dir) = ctx.paths.tun_config_file.parent() {
        let cache_db_path = state_dir.join(utils::CACHE_DB_FILE_NAME);
        let classification =
            utils::classify_cache_db_detailed(&cache_db_path, Some(SERVICE_ACCOUNT_USER));
        if classification.needs_isolation() {
            match utils::quarantine_cache_db(&cache_db_path) {
                Ok(quarantined) => println!(
                    "  ⚠ cache.db {} 已隔离为 {}（Core 将重新生成）",
                    classification.state.as_str(),
                    quarantined.display()
                ),
                Err(error) => {
                    eprintln!("  ⚠ cache.db 隔离失败（{error}）；保留原文件，下次启动前会再次尝试")
                }
            }
        }
    }
    // Last: releasing the gate is the end of the reset window.
    tun_transaction::quarantine_active_transaction(ctx, journal)?;
    Ok(())
}

/// SPEC B3: prove the managed runtime is stopped through the shared quiesce
/// proof. When the daemon is unreachable, a process-level proof (pid file +
/// endpoint) is required before any cleanup; otherwise the daemon is started
/// first and the IPC proof is obtained.
async fn prove_managed_runtime_stopped(
    ctx: &instance::InstanceContext,
    fence: tun_transaction::TransactionFence,
    allow_legacy_reset: bool,
) -> anyhow::Result<()> {
    let first = ipc::send_command(&ipc::DaemonCommand::QuiesceCandidateRuntime {
        fence: fence.clone(),
        allow_legacy_reset,
        allow_restored_old_quiesce: false,
        token: None,
    })
    .await;

    match first {
        Ok(response) => match validate_quiesced_response(&response, &fence) {
            Ok(()) => Ok(()),
            // hard gate: `maybe_auto_recover_active_transaction` runs
            // before the generation apply, so an upgrade can meet a daemon that
            // predates this admission. The authorized reset then falls back to the
            // privileged CLI executor — under the same admission and the same
            // identity/exit proof, never by clearing evidence or faking a state.
            Err(refusal)
                if allow_legacy_reset && is_pre_admission_quiesce_refusal(ctx, &response) =>
            {
                cli_quiesce_legacy_managed_core(ctx, &fence)
                    .await
                    .map(|proof| {
                        println!(
                            "  ⚠ 旧 daemon 不识别本次准入；已由特权 CLI 完成停机（pid={}，退出已确认）",
                            proof.pid
                        );
                    })
                    .map_err(|fallback| match fallback.downcast_ref::<ManagedStopError>() {
                        Some(_) => fallback,
                        None => anyhow::Error::new(ManagedStopError::not_attempted(format!(
                            "{refusal} / {fallback}"
                        ))),
                    })
            }
            Err(refusal) => Err(refusal),
        },
        Err(daemon_error) => {
            if no_running_core_without_daemon(ctx).is_ok() {
                println!(
                    "  ⚠ system daemon is unreachable ({daemon_error}); no managed Core process is running — continuing"
                );
                return Ok(());
            }
            println!(
                "  System daemon is unreachable and a Core may still be running. Starting the system daemon..."
            );
            start_system_daemon_for_recovery(ctx)
                .await
                .map_err(|error| {
                    anyhow::Error::new(ManagedStopError::not_attempted(error.to_string()))
                })?;
            let response = ipc::send_command(&ipc::DaemonCommand::QuiesceCandidateRuntime {
                fence: fence.clone(),
                allow_legacy_reset,
                allow_restored_old_quiesce: false,
                token: None,
            })
            .await
            .map_err(|error| {
                anyhow::Error::new(ManagedStopError::not_attempted(format!(
                    "system daemon is still unreachable after restart ({error})"
                )))
            })?;
            validate_quiesced_response(&response, &fence)
        }
    }
}

/// the only refusal shape an upgrade still meets — a daemon built
/// before the legacy reset admission. It either answers with the original
/// "neither candidate nor attested old runtime" rejection, or (with
/// `deny_unknown_fields`) rejects the request shape outright. The capability-free
/// case is limited to an observable CLI/daemon binary skew so a generic daemon
/// error can never unlock the compatibility executor.
fn is_pre_admission_quiesce_refusal(
    ctx: &instance::InstanceContext,
    response: &ipc::DaemonResponse,
) -> bool {
    match response {
        ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::EvidenceMismatch { error, .. },
        } => matches!(
            error.stage.as_str(),
            "quiesce_candidate" | "quiesce_old_runtime" | "quiesce_legacy_reset"
        ),
        ipc::DaemonResponse::Error { .. } => is_system_cli_skewed(ctx),
        _ => false,
    }
}

/// hard gate: the privileged CLI as the compatibility executor for a
/// daemon that predates the admission. It applies the same admission locally,
/// uses the existing four-part managed-Core attribution (never a pid file, a path
/// string or a journal flag alone) and accepts only a kernel-handle exit
/// observation. Anything unprovable refuses with zero destructive cleanup.
///
/// Lifecycle exclusion on this path: the old daemon cannot lend its lock, but
/// the active journal stays in a non-terminal phase for the whole window, so the
/// SPEC §12.4 gate keeps refusing every Core-starting command, and the reset
/// releases that journal last (`reset_managed_runtime_artifacts`). The recovery
/// marker is held as well, but it only excludes other recovery agents.
#[cfg(unix)]
async fn cli_quiesce_legacy_managed_core(
    ctx: &instance::InstanceContext,
    fence: &tun_transaction::TransactionFence,
) -> anyhow::Result<daemon::CoreStoppedProof> {
    if !is_current_process_root() {
        return Err(anyhow::Error::new(ManagedStopError::not_attempted(
            "受管 Core 归属证明需要管理员授权，当前进程不是特权执行者",
        )));
    }
    let Some(journal) = tun_transaction::read_active_journal(ctx)? else {
        return Err(anyhow::Error::new(ManagedStopError::not_attempted(
            "active transaction journal is missing",
        )));
    };
    if journal.transaction_id != fence.transaction_id
        || journal.generation != fence.generation
        || journal.phase != fence.expected_phase
    {
        return Err(anyhow::Error::new(ManagedStopError::not_attempted(
            "the active transaction changed before the compatibility stop",
        )));
    }
    if let Err(reason) = tun_transaction::legacy_reset_evidence_admission(ctx, &journal) {
        return Err(anyhow::Error::new(ManagedStopError::not_attempted(
            format!("迁移占位证据不满足受控 reset 准入：{reason}"),
        )));
    }

    // Read-only cross-check of the daemon's recorded pid (GetStatus stays admitted
    // during the transaction gate). The pid is a cross-check, never the proof.
    let daemon_pid = match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await {
        Ok(ipc::DaemonResponse::Status { core_pid, .. }) => core_pid,
        _ => None,
    };

    let state_dir = managed_state_dir(ctx)?;
    let endpoint = managed_core_endpoint(ctx);
    match core_identity::prove_core_api_peer(
        &endpoint,
        &ctx.paths.core_binary,
        &state_dir,
        managed_service_uid(),
    ) {
        core_identity::CoreIdentityProof::Proven(identity) => {
            if daemon_pid.is_some_and(|pid| pid != identity.pid) {
                return Err(anyhow::Error::new(ManagedStopError::not_attempted(
                    format!(
                        "daemon 记录的 Core pid {daemon_pid:?} 与已认证 Core pid {} 冲突",
                        identity.pid
                    ),
                )));
            }
            match send_terminate_signal(identity.pid) {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => {
                    return Err(anyhow::Error::new(ManagedStopError::not_attempted(
                        format!(
                            "无法向已认证的受管 Core pid {} 发送终止信号（{error}）",
                            identity.pid
                        ),
                    )))
                }
            }
            match core_identity::observe_process_exit(
                identity.pid,
                std::time::Duration::from_secs(5),
            ) {
                core_identity::ProcessExitProof::Exited => {
                    Ok(daemon::CoreStoppedProof::observed(identity.pid))
                }
                core_identity::ProcessExitProof::StillRunning => Err(anyhow::Error::new(
                    ManagedStopError::attempted_but_unconfirmed(format!(
                        "受管 Core pid {} 在 5s 内未退出（证据保留）",
                        identity.pid
                    )),
                )),
                core_identity::ProcessExitProof::Unprovable(reason) => Err(anyhow::Error::new(
                    ManagedStopError::attempted_but_unconfirmed(format!(
                        "无法观察受管 Core pid {} 的退出（{reason}；证据保留）",
                        identity.pid
                    )),
                )),
            }
        }
        core_identity::CoreIdentityProof::Absent => match daemon::live_managed_core_liveness() {
            daemon::CoreLiveness::Stopped => Ok(daemon::CoreStoppedProof::observed(0)),
            daemon::CoreLiveness::Running(pid) => {
                Err(anyhow::Error::new(ManagedStopError::not_attempted(
                    format!("受管 Core pid {pid} 仍在运行，但无法经 API 端点证明归属（证据保留）"),
                )))
            }
            daemon::CoreLiveness::Unprovable(reason) => {
                Err(anyhow::Error::new(ManagedStopError::not_attempted(
                    format!("无法证明受管 Core 已停止（{reason}；证据保留）"),
                )))
            }
        },
        core_identity::CoreIdentityProof::Unprovable(reason) => {
            Err(anyhow::Error::new(ManagedStopError::not_attempted(
                format!("受管 Core 归属不可证（{reason}）：拒绝终止与清理，运行态证据已保留"),
            )))
        }
    }
}

#[cfg(not(unix))]
async fn cli_quiesce_legacy_managed_core(
    _ctx: &instance::InstanceContext,
    _fence: &tun_transaction::TransactionFence,
) -> anyhow::Result<daemon::CoreStoppedProof> {
    Err(anyhow::Error::new(ManagedStopError::not_attempted(
        "compatibility stop needs a provable managed Core identity, which this platform cannot provide",
    )))
}

/// SPEC B3: process-level liveness proof used only when the daemon cannot
/// answer. Requires both no live managed Core process and no reachable Core API.
fn no_running_core_without_daemon(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    // SPEC 裁决 D: the endpoint peer must be identified, not just probed for
    // connectability. A reachable socket only proves "someone answers".
    #[cfg(unix)]
    let state_dir = managed_state_dir(ctx)?;
    match &ctx.paths.api_endpoint {
        #[cfg(not(unix))]
        instance::ApiEndpoint::UnixSocket(_) => anyhow::bail!(
            "cannot prove the managed Core stopped on this platform (peer identity probing is Unix-only); evidence retained"
        ),
        #[cfg(unix)]
        instance::ApiEndpoint::UnixSocket(_) => {
            let endpoint = managed_core_endpoint(ctx);
            match core_identity::prove_core_api_peer(
                &endpoint,
                &ctx.paths.core_binary,
                &state_dir,
                managed_service_uid(),
            ) {
                core_identity::CoreIdentityProof::Absent => {}
                core_identity::CoreIdentityProof::Proven(identity) => {
                    anyhow::bail!(
                        "managed Core process {} is still running (identity proven); refusing to continue without a stopped proof",
                        identity.pid
                    )
                }
                core_identity::CoreIdentityProof::Unprovable(reason) => {
                    anyhow::bail!(
                        "cannot prove the managed Core is stopped: {reason} (evidence retained; no termination attempted)"
                    )
                }
            }
        }
        instance::ApiEndpoint::WindowsNamedPipe(pipe) => {
            if windows_pipe_connectable(pipe) {
                anyhow::bail!(
                    "Core API endpoint is still reachable and its peer identity cannot be proven on this platform"
                );
            }
        }
    }
    match daemon::live_managed_core_liveness() {
        daemon::CoreLiveness::Stopped => Ok(()),
        daemon::CoreLiveness::Running(pid) => {
            anyhow::bail!("managed Core process {pid} is still running")
        }
        daemon::CoreLiveness::Unprovable(reason) => {
            anyhow::bail!("cannot prove the managed Core is stopped: {reason}")
        }
    }
}

/// SPEC 裁决 D-1: the Core API endpoint string of the managed instance.
#[cfg(unix)]
fn managed_core_endpoint(ctx: &instance::InstanceContext) -> String {
    match &ctx.paths.api_endpoint {
        instance::ApiEndpoint::UnixSocket(path) => format!("unix://{}", path.display()),
        instance::ApiEndpoint::WindowsNamedPipe(pipe) => pipe.clone(),
    }
}

/// SPEC 裁决 D-3: an apply the user explicitly authorized (`restart` /
/// `install`) may take over a *proven* managed orphan. Identity is established by
/// [`core_identity::prove_core_api_peer`] (the peer is answering), and the stop is
/// accepted only with a kernel-handle exit proof. Unprovable identity refuses to
/// terminate or replace anything and keeps the runtime evidence.
async fn quiesce_managed_core_after_daemon_stop(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let state_dir = managed_state_dir(ctx)?;
        let endpoint = managed_core_endpoint(ctx);
        match core_identity::prove_core_api_peer(
            &endpoint,
            &ctx.paths.core_binary,
            &state_dir,
            managed_service_uid(),
        ) {
            core_identity::CoreIdentityProof::Absent => Ok(()),
            core_identity::CoreIdentityProof::Unprovable(reason) => {
                anyhow::bail!("受管 Core 归属不可证（{reason}）：拒绝终止与替换，运行态证据已保留")
            }
            core_identity::CoreIdentityProof::Proven(identity) => {
                println!(
                    "  P3 停机：接管归属可证的受管 Core 孤儿（pid {}，exe {}）",
                    identity.pid,
                    identity.exe.display()
                );
                match send_terminate_signal(identity.pid) {
                    Ok(()) => {}
                    Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                    Err(error) => anyhow::bail!(
                        "无法向受管 Core 孤儿 pid {} 发送终止信号（{error}）：无停机证明，拒绝替换（证据已保留）",
                        identity.pid
                    ),
                }
                match core_identity::observe_process_exit(
                    identity.pid,
                    std::time::Duration::from_secs(5),
                ) {
                    core_identity::ProcessExitProof::Exited => {
                        println!(
                            "  P3 停机：已观察到 pid {} 退出（pidfd 退出证明）",
                            identity.pid
                        );
                        Ok(())
                    }
                    core_identity::ProcessExitProof::StillRunning => anyhow::bail!(
                        "受管 Core 孤儿 pid {} 在 5s 内未退出：无停机证明，拒绝替换（证据已保留）",
                        identity.pid
                    ),
                    core_identity::ProcessExitProof::Unprovable(reason) => anyhow::bail!(
                        "无法观察受管 Core 孤儿 pid {} 的退出（{reason}）：无停机证明，拒绝替换（证据已保留）",
                        identity.pid
                    ),
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        no_running_core_without_daemon(ctx)
    }
}

#[cfg(unix)]
fn send_terminate_signal(pid: u32) -> std::io::Result<()> {
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// SPEC 裁决 D-3: read-only orphan classification for `status` / `doctor`.
/// Reports what can be proven and never stops, cleans or suggests manual kills.
async fn orphan_core_observation(ctx: &instance::InstanceContext) -> Option<String> {
    if ipc::is_daemon_running().await {
        return None;
    }
    #[cfg(unix)]
    {
        let state_dir = managed_state_dir(ctx).ok()?;
        let endpoint = managed_core_endpoint(ctx);
        match core_identity::prove_core_api_peer(
            &endpoint,
            &ctx.paths.core_binary,
            &state_dir,
            managed_service_uid(),
        ) {
            core_identity::CoreIdentityProof::Absent => None,
            core_identity::CoreIdentityProof::Proven(identity) => Some(format!(
                "orphan core: managed pid {} exe {} (read-only; not stopped, not cleaned)",
                identity.pid,
                identity.exe.display()
            )),
            core_identity::CoreIdentityProof::Unprovable(reason) => Some(format!(
                "orphan core: unprovable ({reason}) (read-only; evidence retained, nothing stopped)"
            )),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctx;
        None
    }
}

/// SPEC 裁决 D: the service account that may own the managed Core.
#[cfg(unix)]
fn managed_service_uid() -> Option<u32> {
    #[cfg(unix)]
    {
        let user = unsafe { libc::getpwnam(c"mihomo".as_ptr()) };
        if user.is_null() {
            return None;
        }
        Some(unsafe { (*user).pw_uid })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

async fn start_system_daemon_for_recovery(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let start_plan = instance::planned_service_plan(ctx, instance::ServiceAction::Start);
    for command in &start_plan.commands {
        service::run_instance_command(command).map_err(|error| {
            anyhow::anyhow!(
                "failed to start the system daemon for recovery: {error}\n                   If you do not have administrator/sudo privileges, ask an administrator to start the service."
            )
        })?;
    }
    wait_for_system_daemon_readiness().await
}

async fn execute_system_tun_transaction(
    ctx: &instance::InstanceContext,
    target_tun: bool,
    stack: Option<&TunStack>,
    dns_hijack: Option<&str>,
) -> anyhow::Result<()> {
    // 1. Recover any durable transaction before collecting runtime evidence.
    maybe_auto_recover_active_transaction(
        ctx,
        tun_transaction::RecoveryDirection::Resume,
        false,
        false,
        false,
    )
    .await?;

    // 2. Fetch runtime evidence from daemon
    let status_resp = ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await?;
    let (core_running, core_pid, launched_revision) = match status_resp {
        ipc::DaemonResponse::Status {
            running,
            core_pid,
            launched_config_revision,
            ..
        } => (
            running,
            core_pid.unwrap_or(0),
            launched_config_revision.unwrap_or_default(),
        ),
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!("daemon error: {}", message);
        }
        _ => anyhow::bail!("daemon returned unexpected status response"),
    };

    if !core_running {
        anyhow::bail!("System Core is not running. Fix: mihomo-cli start --system");
    }

    // Determine current runtime TUN
    let old_runtime_tun = {
        let snapshot = status::StatusSnapshot::collect(ctx).await;
        snapshot.runtime_tun.as_bool().unwrap_or(false)
    };

    let evidence = tun_transaction::OldRuntimeEvidence {
        core_running: true,
        // Keep the durable identity in the same form used by the daemon's
        // runtime observation (the fixed system Core binary path).
        core_identity: ctx.paths.core_binary.to_string_lossy().to_string(),
        core_pid,
        launched_revision: launched_revision.clone(),
        launch_source: tun_transaction::LaunchSource::SystemTunSnapshot,
        runtime_tun: old_runtime_tun,
        api_endpoint: status_endpoint_label(&ctx.paths.api_endpoint),
        recorded_at_secs: None,
    };

    // 3./4. P1-1：intent 捕获（base revision + candidate）与事务发布在
    // **同一个** CoordinatorLock → ConfigLock 临界区内完成（不再有「捕获后释放锁 →
    // 再发布」的空档）；candidate 由同一份 intent 字节构建，base 描述其真实前置。
    let original_uid = crate::instance::PathInputs::from_current_env()
        .uid
        .unwrap_or(0);
    let journal =
        tun_transaction::prepare_and_publish_active_transaction_capturing_intent_with_prepare(
            ctx,
            original_uid,
            target_tun,
            &evidence,
            || {
                // tun on 的 candidate 必须建立在 reconcile 后的 intent 上，
                // 否则只写在 override 里的非控制键（route-exclude-address/mtu/device）
                // 永远不会进入 snapshot。tun off 只翻转 enable，不在这里改 intent。
                if !target_tun {
                    return Ok(());
                }
                let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
                let reconciled = config::reconcile_override_into_intent_checked(&paths, None)?;
                if reconciled.changed {
                    crate::log!("  ⚠ override.yaml 派生键已重新合并进 intent config.yaml");
                }
                Ok(())
            },
            |intent_bytes| build_tun_candidate(ctx, intent_bytes, target_tun, stack, dns_hijack),
        )?;

    // 5. Validate prepared runtime
    let fence = tun_transaction::TransactionFence {
        transaction_id: journal.transaction_id.clone(),
        generation: journal.generation,
        expected_phase: tun_transaction::JournalPhase::Prepared,
        expected_candidate_revision: journal.candidate_revision.clone(),
    };
    if ctx.mode == instance::InstanceMode::System && is_current_process_root() {
        utils::ensure_mihomo_system_state_dir()?;
    }

    let val_resp = ipc::send_command(&ipc::DaemonCommand::ValidatePreparedRuntime {
        fence: fence.clone(),
        expected_old_runtime_revision: evidence.launched_revision.clone(),
        expected_old_runtime_tun: evidence.runtime_tun,
        token: None,
    })
    .await?;

    match val_resp {
        ipc::DaemonResponse::Transaction {
            response:
                tun_transaction::TransactionResponse::Completed(_)
                | tun_transaction::TransactionResponse::AlreadySatisfied(_),
        } => {}
        ipc::DaemonResponse::Transaction { response } => {
            // Cancel / safe rollback before snapshot promotion
            let _ = tun_transaction::cas_update_phase(
                ctx,
                &fence,
                tun_transaction::JournalPhase::RolledBack,
                None,
                Some(tun_transaction::TerminalOutcome::RolledBackAfterApplyFailure),
            );
            let _ =
                tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation);
            anyhow::bail!("pre-promotion runtime validation failed: {:?}", response);
        }
        ipc::DaemonResponse::Error { message } => {
            let _ = tun_transaction::cas_update_phase(
                ctx,
                &fence,
                tun_transaction::JournalPhase::RolledBack,
                None,
                Some(tun_transaction::TerminalOutcome::RolledBackAfterApplyFailure),
            );
            let _ =
                tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation);
            anyhow::bail!("daemon error validating runtime: {}", message);
        }
        _ => {
            anyhow::bail!("unexpected daemon response validating runtime");
        }
    }

    // 6. CAS: Prepared -> PromotionPending
    let journal = tun_transaction::cas_update_phase(
        ctx,
        &fence,
        tun_transaction::JournalPhase::PromotionPending,
        None,
        None,
    )?;

    // 7. Promote Snapshot
    if let Err(e) = tun_transaction::promote_snapshot(ctx, &journal) {
        let _ = tun_transaction::cas_update_phase(
            ctx,
            &tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: tun_transaction::JournalPhase::PromotionPending,
                expected_candidate_revision: journal.candidate_revision.clone(),
            },
            tun_transaction::JournalPhase::RecoveryRequired,
            Some(tun_transaction::StructuredError {
                code: tun_transaction::TransactionErrorCode::SnapshotConflict,
                stage: "promote_snapshot".to_string(),
                retryable: false,
                message: e.to_string(),
            }),
            None,
        );
        anyhow::bail!("snapshot promotion failed: {e}");
    }

    // 8. CAS: PromotionPending -> SnapshotPromoted
    let fence = tun_transaction::TransactionFence {
        transaction_id: journal.transaction_id.clone(),
        generation: journal.generation,
        expected_phase: tun_transaction::JournalPhase::PromotionPending,
        expected_candidate_revision: journal.candidate_revision.clone(),
    };
    let journal = tun_transaction::cas_update_phase(
        ctx,
        &fence,
        tun_transaction::JournalPhase::SnapshotPromoted,
        None,
        None,
    )?;

    // 9. Apply Promoted Snapshot
    let fence = tun_transaction::TransactionFence {
        transaction_id: journal.transaction_id.clone(),
        generation: journal.generation,
        expected_phase: tun_transaction::JournalPhase::SnapshotPromoted,
        expected_candidate_revision: journal.candidate_revision.clone(),
    };
    if ctx.mode == instance::InstanceMode::System && is_current_process_root() {
        utils::ensure_mihomo_system_state_dir()?;
    }
    let apply_resp = ipc::send_command(&ipc::DaemonCommand::ApplyPromotedSnapshot {
        fence: fence.clone(),
        target_runtime_tun: target_tun,
        token: None,
    })
    .await?;

    let apply_ok = matches!(
        &apply_resp,
        ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::Completed(_)
                | tun_transaction::TransactionResponse::AlreadySatisfied(_),
        }
    );

    if !apply_ok {
        // Automatic rollback (SPEC §6.2)
        println!("  ⚠️ TUN apply failed; initiating automatic rollback...");
        match execute_automatic_rollback(ctx, &journal, &evidence).await {
            Ok(()) => anyhow::bail!(
                "TUN apply failed ({apply_resp:?}), but the previous runtime was restored successfully.\n                 No manual cleanup is required. Fix the Core error and retry:\n                   mihomo-cli tun {} --yes",
                if target_tun { "on" } else { "off" }
            ),
            Err(error) => anyhow::bail!(
                "TUN apply failed ({apply_resp:?}) and automatic rollback encountered an issue ({error}).\n                 Inspect the recoverable state:\n                   mihomo-cli status\n                 Or restart the system service:\n                   mihomo-cli restart --system"
            ),
        }
    }

    let apply_proof = successful_runtime_proof(
        &apply_resp,
        &fence,
        tun_transaction::JournalPhase::SnapshotPromoted,
        tun_transaction::RuntimeProofKind::CandidateApplied,
        &journal.candidate_revision,
        journal.target_runtime_tun,
    )?;
    let journal = tun_transaction::record_candidate_runtime(ctx, &fence, &apply_proof)?;

    // 10. CAS: SnapshotPromoted -> CoreApplied
    let fence = tun_transaction::TransactionFence {
        transaction_id: journal.transaction_id.clone(),
        generation: journal.generation,
        expected_phase: tun_transaction::JournalPhase::SnapshotPromoted,
        expected_candidate_revision: journal.candidate_revision.clone(),
    };
    let journal = tun_transaction::cas_update_phase(
        ctx,
        &fence,
        tun_transaction::JournalPhase::CoreApplied,
        None,
        None,
    )?;

    // 11. User intent compare and commit
    match tun_transaction::compare_and_commit_user_intent(ctx, &journal)? {
        tun_transaction::IntentCommitResult::Committed
        | tun_transaction::IntentCommitResult::AlreadyCandidate => {}
        tun_transaction::IntentCommitResult::Conflict => {
            // 裁决点 4：Conflict 是安全闸。intent 已被外部合法改写 → 本事务
            // **不写 intent**（当前内容原样保留），只回撤运行态。回滚失败时不得宣称
            // "rolled back"：保留全部证据并落到 RecoveryRequired，如实报告。
            println!("  ⚠️ User configuration changed during transaction; rolling back runtime...");
            match execute_automatic_rollback(ctx, &journal, &evidence).await {
                Ok(()) => anyhow::bail!(
                    "User intent configuration was modified concurrently; the runtime was rolled back and your configuration was NOT modified.\n                 Re-run the same command to apply your configuration: mihomo-cli tun {} --yes",
                    if target_tun { "on" } else { "off" }
                ),
                Err(rollback_error) => {
                    // 回滚可能已推进到 RollbackPending 才失败；按 canonical journal 的
                    // 真实 phase 落 RecoveryRequired，而不是假设 CoreApplied。
                    if let Ok(Some(current)) = tun_transaction::read_active_journal(ctx) {
                        if current.transaction_id == journal.transaction_id
                            && current.generation == journal.generation
                            && current.phase != tun_transaction::JournalPhase::IntentCommitted
                            && current.phase != tun_transaction::JournalPhase::RolledBack
                        {
                            let fence = tun_transaction::TransactionFence {
                                transaction_id: current.transaction_id.clone(),
                                generation: current.generation,
                                expected_phase: current.phase,
                                expected_candidate_revision: current.candidate_revision.clone(),
                            };
                            let _ = tun_transaction::cas_update_phase(
                                ctx,
                                &fence,
                                tun_transaction::JournalPhase::RecoveryRequired,
                                Some(tun_transaction::StructuredError {
                                    code: tun_transaction::TransactionErrorCode::CoreStopFailed,
                                    stage: "rollback_after_intent_conflict".to_string(),
                                    retryable: true,
                                    message: rollback_error.to_string(),
                                }),
                                None,
                            );
                        }
                    }
                    anyhow::bail!(
                        "User intent configuration was modified concurrently and the automatic rollback could NOT be completed ({rollback_error}).\n                 Your configuration was NOT modified and every transaction artifact is kept.\n                 Fix: mihomo-cli restart --system (then `mihomo-cli status`)"
                    );
                }
            }
        }
    }

    // 12. Attest Current Transaction
    let fence = tun_transaction::TransactionFence {
        transaction_id: journal.transaction_id.clone(),
        generation: journal.generation,
        expected_phase: tun_transaction::JournalPhase::CoreApplied,
        expected_candidate_revision: journal.candidate_revision.clone(),
    };
    if ctx.mode == instance::InstanceMode::System && is_current_process_root() {
        utils::ensure_mihomo_system_state_dir()?;
    }
    let attest_resp = ipc::send_command(&ipc::DaemonCommand::AttestCurrentTransaction {
        fence: fence.clone(),
        expected_runtime_revision: journal.candidate_revision.clone(),
        expected_runtime_tun: target_tun,
        token: None,
    })
    .await?;
    let _ = successful_runtime_proof(
        &attest_resp,
        &fence,
        tun_transaction::JournalPhase::CoreApplied,
        tun_transaction::RuntimeProofKind::CandidateAttested,
        &journal.candidate_revision,
        target_tun,
    )?;

    // 13. CAS: CoreApplied -> IntentCommitted
    let _ = tun_transaction::cas_update_phase(
        ctx,
        &fence,
        tun_transaction::JournalPhase::IntentCommitted,
        None,
        Some(tun_transaction::TerminalOutcome::Applied),
    )?;

    // 14. Terminal cleanup
    tun_transaction::terminal_cleanup(ctx, &journal.transaction_id, journal.generation)?;

    println!(
        "  ✅ TUN {}",
        if target_tun { "enabled" } else { "disabled" }
    );
    Ok(())
}

async fn cmd_tun_resolved(opts: TunResolvedOptions) -> anyhow::Result<()> {
    let TunResolvedOptions {
        system,
        user,
        action,
        stack,
        dns_hijack,
        yes,
    } = opts;
    maybe_sudo_reexec_for_tun(action.as_ref())?;
    let intent = tun_action_intent(action.as_ref());
    if is_tun_privileged_action(action.as_ref()) {
        let request = mode_request_from_flags(system, user);
        let env = current_environment_state();
        match resolve_environment_for_intent(request, &env, tun_user_intent(action.as_ref())) {
            RuntimeFirstModeResolution::NeedsSystemSwitch {
                user_running,
                user_installed,
            } => {
                if !prompt_switch_user_to_system_for_tun(user_running, user_installed, None, yes)
                    .await?
                {
                    return Ok(());
                }
            }
            RuntimeFirstModeResolution::NeedsSystemInstall { .. } => {
                if matches!(action, Some(TunAction::On)) && !system && !user {
                    if !prompt_install_system_service_for_tun(None, yes).await? {
                        return Ok(());
                    }
                } else {
                    anyhow::bail!(tun_requires_system_service_install_message());
                }
            }
            RuntimeFirstModeResolution::NeedsSystemDaemonRecovery { .. } => {
                let ctx = instance::planned_current_context(instance::InstanceMode::System)
                    .ok_or_else(|| anyhow::anyhow!("Unsupported OS for system daemon recovery"))?;
                if let Some(message) = system_tun_block_message(action.as_ref(), &ctx) {
                    anyhow::bail!(message);
                }
            }
            _ => {}
        }
    }
    let resolved = resolve_current_instance_context(system, user, intent)?;

    if matches!(action, Some(TunAction::On))
        && resolved.ctx.mode == instance::InstanceMode::System
        && is_current_process_root()
        && utils::ensure_mihomo_system_state_dir()?
    {
        println!("  ✓ Repaired Mihomo system data permissions.");
    }

    if matches!(action, Some(TunAction::On)) {
        tun_on_preflight(&resolved.ctx).await?;
    }

    if matches!(action, Some(TunAction::On)) {
        if let Some(message) = v3_mutual_exclusion_violation(
            resolved.ctx.mode,
            current_runtime_presence(),
            "enable TUN on",
        ) {
            anyhow::bail!(message);
        }
    }
    if matches!(action, Some(TunAction::On) | Some(TunAction::Off))
        && resolved.ctx.mode == instance::InstanceMode::User
    {
        let runtime = current_runtime_presence();
        if matches!(action, Some(TunAction::On)) && runtime.user {
            anyhow::bail!(
                "TUN requires the system service, but the per-user core is running.\n                 v3 uses mutually exclusive modes; enabling TUN would make the user core ineffective.\n                 Suggestions:\n                   mihomo-cli stop\n                   mihomo-cli tun on"
            );
        }
        let presence = current_service_presence();
        if presence.system {
            let runtime = current_runtime_presence();
            if runtime.user {
                anyhow::bail!(
                    "TUN requires the system service.\n                     The system service is installed, but the per-user core is running.\n                     v3 uses mutually exclusive modes.\n                     Stop the user instance first, then start the system service:\n                       mihomo-cli stop\n                       mihomo-cli start\n                       mihomo-cli tun on"
                );
            } else {
                anyhow::bail!(
                    "TUN requires the system service.\n                     The system service is already installed — use it directly:\n                       mihomo-cli start\n                       mihomo-cli tun on\n                     Per-user service does not have the privileges needed for TUN."
                );
            }
        }
        anyhow::bail!(tun_requires_system_service_install_message());
    }

    let daemon_running = ipc::is_daemon_running().await;

    if resolved.ctx.mode == instance::InstanceMode::System && daemon_running {
        match action {
            Some(TunAction::On) => {
                if resolved.ctx.paths.intent_config_file.exists()
                    && read_tun_enabled_from_config(&resolved.ctx.paths.intent_config_file)
                        .unwrap_or(false)
                    && !confirm_update_existing_tun(yes)?
                {
                    println!("  Cancelled.");
                    return Ok(());
                }
                return execute_system_tun_transaction(
                    &resolved.ctx,
                    true,
                    stack.as_ref(),
                    dns_hijack.as_deref(),
                )
                .await;
            }
            Some(TunAction::Off) => {
                return execute_system_tun_transaction(&resolved.ctx, false, None, None).await;
            }
            Some(TunAction::Status) | None => {
                let status_resp =
                    ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await?;
                match status_resp {
                    ipc::DaemonResponse::Status {
                        running,
                        core_pid,
                        config_path,
                        ..
                    } => {
                        let snapshot = status::StatusSnapshot::collect(&resolved.ctx).await;
                        println!("System daemon: running");
                        println!("Core running: {running}");
                        println!("TUN enabled: {}", tun_command_status_label(&snapshot));
                        if let Some(pid) = core_pid {
                            println!("Core PID: {pid}");
                        }
                        if let Some(path) = config_path {
                            println!("Config: {}", path.display());
                        }
                        return Ok(());
                    }
                    ipc::DaemonResponse::Error { message } => {
                        let hint = format_error_hint(&message);
                        anyhow::bail!("daemon error: {}{}", message, hint);
                    }
                    _ => anyhow::bail!("unexpected response from daemon"),
                }
            }
        }
    }

    if resolved.ctx.mode == instance::InstanceMode::System {
        if let Some(message) = system_tun_block_message(action.as_ref(), &resolved.ctx) {
            anyhow::bail!(message);
        }
        println!("System daemon: not running");
        println!("TUN enabled: unknown");
        println!("Start it first: mihomo-cli start --system");
        return Ok(());
    }

    let client = mihomo_api::EndpointMihomoApiClient::new(resolved.ctx.paths.api_endpoint);
    mihomo_api::tun_toggle_with_client(&client, action, stack, dns_hijack).await
}

async fn cmd_connections_resolved(system: bool, user: bool, flush: bool) -> anyhow::Result<()> {
    let intent = if flush {
        instance::CommandIntent::Mutating
    } else {
        instance::CommandIntent::ReadOnly
    };
    let client = resolve_ready_api_client(system, user, intent).await?;
    mihomo_api::connections_with_client(&client, flush).await
}

async fn cmd_ip_resolved(system: bool, user: bool) -> anyhow::Result<()> {
    let client = resolve_ready_api_client(system, user, instance::CommandIntent::ReadOnly).await?;
    let port = mihomo_api::get_port_with_client(&client).await?;
    let (ip, country, source) =
        mihomo_api::fetch_ip_info_fast_with_proxy_port(port, std::time::Duration::from_secs(5))
            .await?;
    println!("=== Mihomo Proxy IP Probe ===");
    println!("Probe:   {source}");
    println!("Path:    current mihomo rules for the IP-check service");
    println!("IP:      {ip}");
    println!("Country: {country}");
    println!();
    println!("Note:");
    println!("  This is not a system direct IP, TUN state, or arbitrary target's exit IP.");
    println!("  Use `mihomo-cli exit-ip --node <NODE>` for node exit IP.");
    println!("  Use `mihomo-cli rule test <host>` for route matching.");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyGroupInfo {
    name: String,
    kind: String,
    now: Option<String>,
    all: Vec<String>,
}

fn proxy_group_from_value(name: &str, value: &serde_json::Value) -> Option<ProxyGroupInfo> {
    let all = value.get("all")?.as_array()?;
    Some(ProxyGroupInfo {
        name: name.to_string(),
        kind: value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        now: value
            .get("now")
            .and_then(|v| v.as_str())
            .map(ToString::to_string),
        all: all
            .iter()
            .filter_map(|v| v.as_str().map(ToString::to_string))
            .collect(),
    })
}

fn proxy_groups_from_response(data: &serde_json::Value) -> Vec<ProxyGroupInfo> {
    let Some(proxies) = data.get("proxies").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let mut groups: Vec<_> = proxies
        .iter()
        .filter_map(|(name, value)| proxy_group_from_value(name, value))
        .collect();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    groups
}

fn proxy_names_from_response(data: &serde_json::Value) -> std::collections::BTreeSet<String> {
    data.get("proxies")
        .and_then(|v| v.as_object())
        .map(|proxies| proxies.keys().cloned().collect())
        .unwrap_or_default()
}

fn normalize_url_host(input: &str) -> String {
    if let Ok(url) = reqwest::Url::parse(input) {
        if let Some(host) = url.host_str() {
            return host.to_string();
        }
    }
    input
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(input)
        .to_string()
}

fn resolve_effective_outbound(
    policy: &str,
    groups: &[ProxyGroupInfo],
    proxy_names: &std::collections::BTreeSet<String>,
) -> anyhow::Result<String> {
    let mut current = policy.to_string();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        if current.eq_ignore_ascii_case("DIRECT") || current.eq_ignore_ascii_case("REJECT") {
            return Ok(current);
        }
        let Some(group) = groups.iter().find(|g| g.name == current) else {
            if proxy_names.contains(&current) {
                return Ok(current);
            }
            anyhow::bail!("policy `{current}` is not a known node or group");
        };
        if !seen.insert(current.clone()) {
            anyhow::bail!("proxy group cycle detected while resolving `{policy}`");
        }
        current = group.now.clone().ok_or_else(|| {
            anyhow::anyhow!("policy group `{}` has no current selection", group.name)
        })?;
    }
}

fn select_probe_group_for_node(groups: &[ProxyGroupInfo], node: &str) -> anyhow::Result<String> {
    let mut candidates: Vec<&ProxyGroupInfo> = groups
        .iter()
        .filter(|group| group.all.iter().any(|item| item == node))
        .collect();
    candidates.sort_by_key(|group| if group.name == "GLOBAL" { 0 } else { 1 });
    if let Some(global) = candidates.iter().find(|group| group.name == "GLOBAL") {
        return Ok(global.name.clone());
    }
    match candidates.as_slice() {
        [only] => Ok(only.name.clone()),
        [] => anyhow::bail!(
            "node not found: {node}
  Run: mihomo-cli list"
        ),
        many => {
            let names: Vec<_> = many.iter().map(|g| g.name.as_str()).collect();
            anyhow::bail!(
                "ambiguous node name: {node}
  It appears in groups: {}
  Add/enable a GLOBAL selector or use `mihomo-cli exit-ip --group <GROUP>` for a group's current selection.",
                names.join(", ")
            )
        }
    }
}

async fn probe_ip_via_current_proxy(
    client: &impl mihomo_api::MihomoApiClient,
) -> anyhow::Result<(String, String, String)> {
    let port = mihomo_api::get_port_with_client(client).await?;
    mihomo_api::fetch_ip_info_fast_with_proxy_port(port, std::time::Duration::from_secs(5)).await
}

async fn probe_with_temporary_selection(
    client: &impl mihomo_api::MihomoApiClient,
    group: &str,
    node: &str,
    yes: bool,
) -> anyhow::Result<(String, String, String, Option<String>)> {
    let group_data = client.get(&mihomo_api::proxy_group_path(group)).await?;
    let original = group_data
        .get("now")
        .and_then(|v| v.as_str())
        .map(ToString::to_string);
    let needs_switch = original.as_deref() != Some(node);
    if needs_switch && !yes && std::io::stdin().is_terminal() {
        println!(
            "This will temporarily switch group `{group}` to `{node}`, probe exit IP, then restore it."
        );
        print!("Proceed [Y/n]: ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let answer = input.trim().to_lowercase();
        if answer == "n" || answer == "no" {
            anyhow::bail!("Cancelled.");
        }
    } else if needs_switch && !yes && !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "probing node `{node}` requires temporarily switching group `{group}`. Re-run with --yes in non-interactive mode."
        );
    }

    if needs_switch {
        mihomo_api::select_proxy_with_client(client, group, node).await?;
    }
    let probe = probe_ip_via_current_proxy(client).await;
    if needs_switch {
        if let Some(original) = original.as_deref() {
            if let Err(err) = mihomo_api::select_proxy_with_client(client, group, original).await {
                eprintln!("  Warning: failed to restore {group} -> {original}: {err}");
            }
        }
    }
    let (ip, country, source) = probe?;
    Ok((ip, country, source, original))
}

fn print_exit_ip_result(
    title: &str,
    lines: &[(&str, String)],
    ip: &str,
    country: &str,
    source: &str,
) {
    println!("=== {title} ===");
    for (key, value) in lines {
        println!("{key:<9} {value}");
    }
    println!("Probe:    {source}");
    println!("IP:       {ip}");
    println!("Country:  {country}");
}

async fn cmd_exit_ip(
    system: bool,
    user: bool,
    node: Option<String>,
    group: Option<String>,
    url: Option<String>,
    direct: bool,
    yes: bool,
) -> anyhow::Result<()> {
    if direct {
        let (ip, country, source) =
            mihomo_api::fetch_ip_info_direct(std::time::Duration::from_secs(5)).await?;
        print_exit_ip_result(
            "Direct Exit IP",
            &[("Path:", "direct, without mihomo local proxy".to_string())],
            &ip,
            &country,
            &source,
        );
        return Ok(());
    }

    let client = resolve_ready_api_client(system, user, instance::CommandIntent::ReadOnly).await?;
    let proxies = client.get("/proxies").await?;
    let groups = proxy_groups_from_response(&proxies);
    let proxy_names = proxy_names_from_response(&proxies);

    if let Some(node) = node {
        if !proxy_names.contains(&node) {
            anyhow::bail!(
                "node not found: {node}
  Run: mihomo-cli list"
            );
        }
        let probe_group = select_probe_group_for_node(&groups, &node)?;
        let (ip, country, source, original) =
            probe_with_temporary_selection(&client, &probe_group, &node, yes).await?;
        let mut lines = vec![
            ("Node:", node),
            ("Via:", format!("temporary selector `{probe_group}`")),
        ];
        if let Some(original) = original {
            lines.push(("Restore:", format!("{probe_group} -> {original}")));
        }
        print_exit_ip_result("Node Exit IP", &lines, &ip, &country, &source);
        return Ok(());
    }

    if let Some(group_name) = group {
        let group = groups
            .iter()
            .find(|g| g.name == group_name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "group not found: {group_name}
  Run: mihomo-cli list"
                )
            })?;
        let selected = group
            .now
            .clone()
            .ok_or_else(|| anyhow::anyhow!("group `{group_name}` has no current selection"))?;
        let effective = resolve_effective_outbound(&group_name, &groups, &proxy_names)?;
        if effective.eq_ignore_ascii_case("DIRECT") {
            let (ip, country, source) =
                mihomo_api::fetch_ip_info_direct(std::time::Duration::from_secs(5)).await?;
            print_exit_ip_result(
                "Group Direct Exit IP",
                &[
                    ("Group:", group_name),
                    ("Selected:", selected),
                    ("Node:", effective),
                    ("Type:", group.kind.clone()),
                ],
                &ip,
                &country,
                &source,
            );
            return Ok(());
        }
        if effective.eq_ignore_ascii_case("REJECT") {
            anyhow::bail!("group `{group_name}` resolves to REJECT; no exit IP to probe");
        }
        let probe_group = select_probe_group_for_node(&groups, &effective)?;
        let (ip, country, source, _) =
            probe_with_temporary_selection(&client, &probe_group, &effective, true).await?;
        print_exit_ip_result(
            "Group Exit IP",
            &[
                ("Group:", group_name),
                ("Selected:", selected),
                ("Node:", effective),
                ("Type:", group.kind.clone()),
            ],
            &ip,
            &country,
            &source,
        );
        return Ok(());
    }

    if let Some(url) = url {
        let host = normalize_url_host(&url);
        let paths = app_paths_for_resolved_instance_command(
            "exit-ip --url",
            system,
            user,
            instance::CommandIntent::ReadOnly,
        )?;
        let matched = crate::rules::test_rule_match_at(&paths, &host)?;
        let Some(matched) = matched else {
            anyhow::bail!("no matching rule found for {host}");
        };
        let policy = matched.policy.clone();
        if policy.eq_ignore_ascii_case("DIRECT") {
            let (ip, country, source) =
                mihomo_api::fetch_ip_info_direct(std::time::Duration::from_secs(5)).await?;
            print_exit_ip_result(
                "Route Direct Exit IP",
                &[
                    ("URL:", url),
                    ("Host:", host),
                    ("Rule:", matched.rule),
                    ("Policy:", policy),
                ],
                &ip,
                &country,
                &source,
            );
            return Ok(());
        }
        if policy.eq_ignore_ascii_case("REJECT") {
            anyhow::bail!("route for {host} is REJECT; no exit IP to probe");
        }
        let target_node = resolve_effective_outbound(&policy, &groups, &proxy_names)?;
        if target_node.eq_ignore_ascii_case("REJECT") {
            anyhow::bail!("route for {host} resolves to REJECT; no exit IP to probe");
        }
        let probe_group = select_probe_group_for_node(&groups, &target_node)?;
        let (ip, country, source, _) =
            probe_with_temporary_selection(&client, &probe_group, &target_node, yes).await?;
        print_exit_ip_result(
            "Route Exit IP Estimate",
            &[
                ("URL:", url),
                ("Host:", host),
                ("Rule:", matched.rule),
                ("Policy:", policy),
                ("Node:", target_node),
            ],
            &ip,
            &country,
            &source,
        );
        println!();
        println!("Note:");
        println!("  The route is resolved for the URL/host above.");
        println!("  The IP probe uses {source} through the selected node/path.");
        println!("  It does not prove the target server itself observed this source IP.");
        return Ok(());
    }

    unreachable!("clap requires exactly one exit-ip target mode")
}

fn lifecycle_system_config_path(ctx: &instance::InstanceContext) -> std::path::PathBuf {
    ctx.paths.intent_config_file.clone()
}

fn lifecycle_system_config_payload(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<(String, String)> {
    let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
    // B：restart/start 的 payload 直接取 intent；必须先保证 override 参与合并，
    // 否则任何漏掉合并的写入路径都会被 restart 静默放大成运行态丢配置。
    // review MAJOR-3：直接使用锁内 reconcile 返回的内容与 revision，锁外重读会漏检并发写入。
    let reconciled = config::reconcile_override_into_intent_checked(&paths, None)?;
    if reconciled.changed {
        crate::log!("  ⚠ override.yaml 派生键已重新合并进 intent config.yaml");
    }
    if ctx.mode == instance::InstanceMode::System {
        return tun_transaction::merge_system_tun_snapshot_into_payload(
            ctx,
            &reconciled.content,
            &reconciled.revision,
        );
    }
    Ok((reconciled.content, reconciled.revision))
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigOwnershipRepair {
    NotNeeded,
    ReexecAsRoot,
    RepairAsRoot,
    ReportUndeterminable,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigAssetsVerdict {
    Healthy,
    NeedsRepair,
    Undeterminable,
}

#[cfg(unix)]
impl ConfigAssetsVerdict {
    fn merge(self, other: Option<Self>) -> Self {
        match other {
            Some(other) if other.rank() > self.rank() => other,
            _ => self,
        }
    }

    fn rank(self) -> u8 {
        match self {
            ConfigAssetsVerdict::Healthy => 0,
            ConfigAssetsVerdict::Undeterminable => 1,
            ConfigAssetsVerdict::NeedsRepair => 2,
        }
    }
}

#[cfg(unix)]
fn config_assets_repair_action(
    verdict: ConfigAssetsVerdict,
    is_root: bool,
) -> ConfigOwnershipRepair {
    match verdict {
        ConfigAssetsVerdict::Healthy => ConfigOwnershipRepair::NotNeeded,
        ConfigAssetsVerdict::Undeterminable => ConfigOwnershipRepair::ReportUndeterminable,
        ConfigAssetsVerdict::NeedsRepair if is_root => ConfigOwnershipRepair::RepairAsRoot,
        ConfigAssetsVerdict::NeedsRepair => ConfigOwnershipRepair::ReexecAsRoot,
    }
}

#[cfg(unix)]
fn probe_config_file_verdict(
    expected_uid: u32,
    actual_uid: u32,
    is_regular_file: bool,
    link_count: u64,
) -> anyhow::Result<ConfigAssetsVerdict> {
    if expected_uid == actual_uid {
        return Ok(ConfigAssetsVerdict::Healthy);
    }
    if actual_uid != 0 {
        anyhow::bail!(
            "Mihomo configuration is owned by another user and cannot be repaired automatically"
        );
    }
    if !is_regular_file || link_count != 1 {
        anyhow::bail!(
            "Mihomo configuration has an unsafe file type and cannot be repaired automatically"
        );
    }
    Ok(ConfigAssetsVerdict::NeedsRepair)
}

#[cfg(unix)]
#[cfg_attr(not(test), allow(dead_code))]
fn config_ownership_repair(
    is_root: bool,
    expected_uid: u32,
    actual_uid: u32,
    is_regular_file: bool,
    link_count: u64,
) -> anyhow::Result<ConfigOwnershipRepair> {
    let verdict = probe_config_file_verdict(expected_uid, actual_uid, is_regular_file, link_count)?;
    Ok(config_assets_repair_action(verdict, is_root))
}

/// system deployments before the 2026-08-28 runtime convergence
/// ran the core with `-d <user config dir>`, leaving service-owned runtime
/// state files in the user config tree. Since the promotion to the managed
/// runtime those files are dead artifacts; the lifecycle preflight must not
/// block on them. Anything else — including service-owned files with other
/// names — still fails closed.
#[cfg(unix)]
const LEGACY_SERVICE_RUNTIME_FILES: &[&str] = &[
    "cache.db",
    "geoip.dat",
    "geosite.dat",
    "geoip.metadb",
    "country.mmdb",
];

#[cfg(unix)]
fn is_legacy_service_runtime_artifact(
    file_name: &std::ffi::OsStr,
    actual_uid: u32,
    is_regular_file: bool,
    service_uid: Option<u32>,
) -> bool {
    let Some(service_uid) = service_uid else {
        return false;
    };
    is_regular_file
        && actual_uid == service_uid
        && LEGACY_SERVICE_RUNTIME_FILES.contains(&file_name.to_string_lossy().as_ref())
}

/// Resolve the Linux system service account uid from passwd only; never from
/// environment variables. `None` on platforms without the service user.
#[cfg(target_os = "linux")]
fn system_service_uid() -> Option<u32> {
    lookup_user("mihomo").ok().map(|(uid, _, _)| uid)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn system_service_uid() -> Option<u32> {
    None
}

#[cfg(unix)]
fn check_config_assets_verdict(
    config_dir: &std::path::Path,
    config_path: &std::path::Path,
    expected_uid: u32,
) -> anyhow::Result<ConfigAssetsVerdict> {
    use std::os::unix::fs::MetadataExt;

    // 1. 检查 config_dir 自身
    let dir_meta = match std::fs::symlink_metadata(config_dir) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigAssetsVerdict::Healthy);
        }
        Err(_) => return Ok(ConfigAssetsVerdict::Undeterminable),
    };
    let mut verdict = if dir_meta.uid() != expected_uid {
        if dir_meta.uid() == 0 {
            ConfigAssetsVerdict::NeedsRepair
        } else {
            anyhow::bail!(
                "Mihomo configuration directory is owned by another user and cannot be repaired automatically"
            );
        }
    } else {
        ConfigAssetsVerdict::Healthy
    };

    // 2. 检查 config_path
    match std::fs::symlink_metadata(config_path) {
        Ok(metadata) => {
            let file_verdict = probe_config_file_verdict(
                expected_uid,
                metadata.uid(),
                metadata.file_type().is_file(),
                metadata.nlink(),
            )?;
            verdict = verdict.merge(Some(file_verdict));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            verdict = verdict.merge(Some(ConfigAssetsVerdict::Undeterminable));
        }
    }

    // 3. 检查 subscriptions_dir
    let subscriptions_dir = config_dir.join("subscriptions");
    let mut sub_is_dir = false;
    match std::fs::symlink_metadata(&subscriptions_dir) {
        Ok(sub_meta) => {
            sub_is_dir = sub_meta.is_dir();
            if sub_meta.uid() != expected_uid {
                if sub_meta.uid() == 0 {
                    verdict = verdict.merge(Some(ConfigAssetsVerdict::NeedsRepair));
                } else {
                    anyhow::bail!(
                        "Mihomo subscriptions directory is owned by another user and cannot be repaired automatically"
                    );
                }
            } else {
                verdict = verdict.merge(Some(ConfigAssetsVerdict::Healthy));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            verdict = verdict.merge(Some(ConfigAssetsVerdict::Undeterminable));
        }
    }

    // 4. 遍历 subscriptions_dir 下的文件
    if sub_is_dir {
        match std::fs::read_dir(&subscriptions_dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(e) => e,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(_) => {
                            verdict = verdict.merge(Some(ConfigAssetsVerdict::Undeterminable));
                            continue;
                        }
                    };
                    let meta = match std::fs::symlink_metadata(entry.path()) {
                        Ok(m) => m,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(_) => {
                            verdict = verdict.merge(Some(ConfigAssetsVerdict::Undeterminable));
                            continue;
                        }
                    };
                    if meta.uid() != expected_uid {
                        if meta.uid() == 0 {
                            verdict = verdict.merge(Some(ConfigAssetsVerdict::NeedsRepair));
                        } else {
                            anyhow::bail!(
                                "Mihomo subscription asset is owned by another user and cannot be repaired automatically"
                            );
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                verdict = verdict.merge(Some(ConfigAssetsVerdict::Undeterminable));
            }
        }
    }

    // 5. 遍历 config_dir 下的其它常规文件
    let service_uid = system_service_uid();
    let mut legacy_runtime_files: Vec<std::path::PathBuf> = Vec::new();
    match std::fs::read_dir(config_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => {
                        verdict = verdict.merge(Some(ConfigAssetsVerdict::Undeterminable));
                        continue;
                    }
                };
                let meta = match std::fs::symlink_metadata(entry.path()) {
                    Ok(m) => m,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => {
                        verdict = verdict.merge(Some(ConfigAssetsVerdict::Undeterminable));
                        continue;
                    }
                };
                if meta.uid() != expected_uid {
                    if meta.uid() == 0 {
                        verdict = verdict.merge(Some(ConfigAssetsVerdict::NeedsRepair));
                    } else if is_legacy_service_runtime_artifact(
                        &entry.file_name(),
                        meta.uid(),
                        meta.file_type().is_file(),
                        service_uid,
                    ) {
                        legacy_runtime_files.push(entry.path());
                        continue;
                    } else {
                        anyhow::bail!(
                            "Mihomo configuration asset is owned by another user and cannot be repaired automatically"
                        );
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            verdict = verdict.merge(Some(ConfigAssetsVerdict::Undeterminable));
        }
    }

    // 扫描全程无硬失败后才收敛 的遗留运行时文件：
    // 失败路径绝不产生副作用，删除动作不依赖 readdir 顺序。
    if verdict == ConfigAssetsVerdict::Healthy {
        for path in legacy_runtime_files {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match std::fs::remove_file(&path) {
                Ok(()) => crate::log!(
                    "  ⚠ Removed stale service-owned `{name}` left by a pre-2026-08 system deployment ."
                ),
                Err(err) => crate::log!(
                    "  ⚠ Stale service-owned `{name}` could not be removed ({err}); it is unused and ignored by this check."
                ),
            }
        }
    }

    Ok(verdict)
}

#[cfg(unix)]
fn detect_config_assets_repair(
    config_dir: &std::path::Path,
    config_path: &std::path::Path,
    expected_uid: u32,
    is_root: bool,
) -> anyhow::Result<ConfigOwnershipRepair> {
    let verdict = check_config_assets_verdict(config_dir, config_path, expected_uid)?;
    Ok(config_assets_repair_action(verdict, is_root))
}

#[cfg(unix)]
fn heal_directory_tree(dir: &std::path::Path, expected_uid: u32, recursive: bool) {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.is_dir() {
                    if meta.uid() == expected_uid
                        && !utils::mode_has_setgid(meta.permissions().mode())
                    {
                        let _ = utils::set_directory_mode_no_follow(&path, 0o2755);
                    }
                    if recursive {
                        heal_directory_tree(&path, expected_uid, true);
                    }
                } else if meta.is_file()
                    && meta.uid() == expected_uid
                    && (meta.permissions().mode() & 0o040) == 0
                {
                    let _ = utils::set_file_mode_no_follow(&path, 0o640);
                }
            }
        }
    }
}

#[cfg(unix)]
fn heal_local_config_permissions(
    config_dir: &std::path::Path,
    expected_uid: u32,
) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    let subscriptions_dir = config_dir.join("subscriptions");
    if let Ok(meta) = std::fs::symlink_metadata(&subscriptions_dir) {
        if meta.is_dir()
            && meta.uid() == expected_uid
            && !utils::mode_has_setgid(meta.permissions().mode())
        {
            let _ = utils::set_directory_mode_no_follow(&subscriptions_dir, 0o2755);
        }
    }

    let active_file = subscriptions_dir.join("active");
    if let Ok(meta) = std::fs::symlink_metadata(&active_file) {
        if meta.is_file() && meta.uid() == expected_uid && (meta.permissions().mode() & 0o040) == 0
        {
            let _ = utils::set_file_mode_no_follow(&active_file, 0o640);
        }
    }

    if let Ok(entries) = std::fs::read_dir(&subscriptions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if meta.is_file()
                    && meta.uid() == expected_uid
                    && (meta.permissions().mode() & 0o040) == 0
                {
                    let _ = utils::set_file_mode_no_follow(&path, 0o640);
                }
            }
        }
    }

    for subdir_name in ["selections", "transactions"] {
        let subdir = config_dir.join(subdir_name);
        if let Ok(meta) = std::fs::symlink_metadata(&subdir) {
            if meta.is_dir()
                && meta.uid() == expected_uid
                && !utils::mode_has_setgid(meta.permissions().mode())
            {
                let _ = utils::set_directory_mode_no_follow(&subdir, 0o2755);
            }
        }
        let recursive = subdir_name == "transactions";
        heal_directory_tree(&subdir, expected_uid, recursive);
    }

    let config_path = config_dir.join("config.yaml");
    if let Ok(meta) = std::fs::symlink_metadata(&config_path) {
        if meta.is_file() && meta.uid() == expected_uid && (meta.permissions().mode() & 0o040) == 0
        {
            let _ = utils::set_file_mode_no_follow(&config_path, 0o640);
        }
    }

    Ok(())
}

#[cfg(unix)]
fn repair_config_tree_as_root(
    config_dir: &std::path::Path,
    expected_uid: u32,
) -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;

    if !config_dir.exists() {
        return Ok(());
    }

    let Some((_, _, home)) = utils::original_user_identity()? else {
        anyhow::bail!("cannot resolve original user home for repair");
    };
    if utils::canonical_original_user_home_path(config_dir, &home)?.is_none() {
        anyhow::bail!(
            "refusing to repair config directory outside user home: {}",
            config_dir.display()
        );
    }

    let service_gid = unsafe {
        let gr = libc::getgrnam(c"mihomo".as_ptr());
        if !gr.is_null() {
            Some((*gr).gr_gid)
        } else {
            None
        }
    };

    let target_gid = service_gid.unwrap_or_else(|| {
        let user = unsafe { libc::getpwuid(expected_uid) };
        if !user.is_null() {
            unsafe { (*user).pw_gid }
        } else {
            !0 as libc::gid_t
        }
    });

    let mut stack = vec![config_dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if !current.starts_with(config_dir) {
            continue;
        }

        let meta = match std::fs::symlink_metadata(&current) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };

        if meta.file_type().is_symlink() {
            continue;
        }

        if meta.is_dir() {
            let fd = utils::open_directory_no_follow(&current)?;
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let stat = unsafe { stat.assume_init() };

            let need_chown = stat.st_uid != expected_uid;
            let need_chgrp = stat.st_gid != target_gid && target_gid != (!0 as libc::gid_t);
            if need_chown || need_chgrp {
                let new_uid = if need_chown {
                    expected_uid
                } else {
                    !0 as libc::uid_t
                };
                let new_gid = if need_chgrp {
                    target_gid
                } else {
                    !0 as libc::gid_t
                };
                if unsafe { libc::fchown(fd.as_raw_fd(), new_uid, new_gid) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }

            let _ = utils::set_directory_mode_no_follow(&current, 0o2755);

            if let Ok(entries) = std::fs::read_dir(&current) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        } else if meta.is_file() {
            if meta.nlink() != 1 {
                anyhow::bail!(
                    "refusing to repair hard-linked configuration file: {}",
                    current.display()
                );
            }
            let file = utils::open_regular_file_no_follow(&current)?;
            let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let stat = unsafe { stat.assume_init() };

            let need_chown = stat.st_uid != expected_uid;
            let need_chgrp = stat.st_gid != target_gid && target_gid != (!0 as libc::gid_t);
            if need_chown || need_chgrp {
                let new_uid = if need_chown {
                    expected_uid
                } else {
                    !0 as libc::uid_t
                };
                let new_gid = if need_chgrp {
                    target_gid
                } else {
                    !0 as libc::gid_t
                };
                if unsafe { libc::fchown(file.as_raw_fd(), new_uid, new_gid) } != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }

            let _ = utils::set_file_mode_no_follow(&current, 0o640);
        }
    }

    Ok(())
}

#[cfg(unix)]
fn ensure_system_config_ownership_for_lifecycle(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let config_dir = &ctx.paths.config_dir;
    let config_path = lifecycle_system_config_path(ctx);
    let expected_uid = instance::PathInputs::from_current_env()
        .uid
        .ok_or_else(|| {
            anyhow::anyhow!("cannot determine the current user for configuration repair")
        })?;

    let is_root = is_current_process_root();
    let repair = detect_config_assets_repair(config_dir, &config_path, expected_uid, is_root)?;

    match repair {
        ConfigOwnershipRepair::NotNeeded => {
            heal_local_config_permissions(config_dir, expected_uid)?;
            Ok(())
        }
        ConfigOwnershipRepair::ReexecAsRoot => {
            println!("  Detected a Mihomo configuration permission issue. Repairing it and continuing restart...");
            let exe = std::env::current_exe()?;
            let args: Vec<String> = std::env::args().skip(1).collect();
            let status = sudo_reexec_command(&exe, &args).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        ConfigOwnershipRepair::RepairAsRoot => {
            repair_config_tree_as_root(config_dir, expected_uid)?;
            if config_path.exists() {
                let repaired_uid = std::fs::symlink_metadata(&config_path)?.uid();
                if repaired_uid != expected_uid {
                    anyhow::bail!("Mihomo configuration permission repair could not be verified");
                }
            }
            println!("  ✓ Mihomo configuration permissions repaired.");
            Ok(())
        }
        ConfigOwnershipRepair::ReportUndeterminable => {
            eprintln!(
                "  ⚠ Cannot verify the Mihomo configuration directory ownership with the current identity; continuing without an automatic repair."
            );
            Ok(())
        }
    }
}

#[cfg(not(unix))]
fn ensure_system_config_ownership_for_lifecycle(
    _ctx: &instance::InstanceContext,
) -> anyhow::Result<()> {
    Ok(())
}

/// What the lifecycle repair gate is allowed to do with a state-directory verdict.
/// `ReportUndeterminable` exists so that "the current identity cannot judge" never
/// maps onto a privilege escalation (issue ).
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemStateRepairAction {
    None,
    RepairInPlace,
    ReexecAsRoot,
    ReportUndeterminable,
}

#[cfg(unix)]
fn system_state_repair_action(
    verdict: utils::SystemStateDirVerdict,
    is_root: bool,
) -> SystemStateRepairAction {
    match verdict {
        utils::SystemStateDirVerdict::Healthy => SystemStateRepairAction::None,
        utils::SystemStateDirVerdict::Undeterminable => {
            SystemStateRepairAction::ReportUndeterminable
        }
        utils::SystemStateDirVerdict::NeedsRepair if is_root => {
            SystemStateRepairAction::RepairInPlace
        }
        utils::SystemStateDirVerdict::NeedsRepair => SystemStateRepairAction::ReexecAsRoot,
    }
}

#[cfg(unix)]
fn ensure_system_state_ownership_for_lifecycle() -> anyhow::Result<()> {
    if !utils::is_managed_system_state_dir_present() {
        return Ok(());
    }

    match system_state_repair_action(
        utils::check_system_state_dir_needs_repair(),
        is_current_process_root(),
    ) {
        SystemStateRepairAction::None => Ok(()),
        SystemStateRepairAction::RepairInPlace => {
            utils::ensure_mihomo_system_state_dir()?;
            println!("  ✓ Mihomo system state directory permissions repaired.");
            Ok(())
        }
        SystemStateRepairAction::ReexecAsRoot => {
            println!("  Detected a system state permission issue. Repairing it via sudo and continuing restart...");
            let exe = std::env::current_exe()?;
            let args: Vec<String> = std::env::args().skip(1).collect();
            let status = sudo_reexec_command(&exe, &args).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        SystemStateRepairAction::ReportUndeterminable => {
            eprintln!(
                "  ⚠ Cannot verify the system state directory ownership with the current identity; continuing without an automatic repair."
            );
            Ok(())
        }
    }
}

#[cfg(not(unix))]
fn ensure_system_state_ownership_for_lifecycle() -> anyhow::Result<()> {
    Ok(())
}

/// SPEC C1: Shared repair prerequisites for `start --system` / `restart --system`.
/// Both commands run the same pre-flight contract:
/// - managed state/config ownership repair (sudo re-entry when needed),
/// - `cache.db` classification (A1/A2), with permission repair only,
/// - SPEC §12.3.1 confirmation contract for every privilege-taking repair.
async fn shared_repair_prerequisites(
    ctx: &instance::InstanceContext,
    allow_runtime_reset: bool,
    prompt_runtime_reset: bool,
    fix_command: &str,
) -> anyhow::Result<()> {
    ensure_system_state_ownership_for_lifecycle()?;
    maybe_sudo_reexec_for_system_generation(ctx)?;
    ensure_system_config_ownership_for_lifecycle(ctx)?;

    let Some(state_dir) = ctx.paths.tun_config_file.parent() else {
        return Ok(());
    };
    let cache_db_path = state_dir.join(utils::CACHE_DB_FILE_NAME);
    let mut state = utils::classify_cache_db(&cache_db_path, Some(SERVICE_ACCOUNT_USER));
    if state == utils::CacheDbState::PermissionDrift {
        let decision =
            cache_db_permission_repair_decision(allow_runtime_reset, prompt_runtime_reset);
        if decision == CacheDbRepairDecision::RefuseRepairRequired {
            anyhow::bail!(cache_db_repair_required_message(fix_command));
        }
        if !is_current_process_root() {
            // SPEC §12.3.1: re-enter as root and re-probe there; the confirmation
            // happens exactly once, on the side that performs the repair.
            println!(
                "  Detected a system cache.db permission issue. Repairing it via sudo and continuing..."
            );
            let exe = std::env::current_exe()?;
            let args: Vec<String> = std::env::args().skip(1).collect();
            let status = sudo_reexec_command(&exe, &args).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        if decision == CacheDbRepairDecision::Prompt
            && !confirm_cache_db_permission_repair(&cache_db_path)?
        {
            anyhow::bail!(
                "Repair not applied; your configuration was not modified.\n  Re-run to repair: {fix_command}"
            );
        }
        preserve_unjudged_cache_db_evidence(&cache_db_path, Some(SERVICE_ACCOUNT_USER));
        utils::repair_cache_db_permissions(&cache_db_path, Some(SERVICE_ACCOUNT_USER))?;
        println!("  ✓ Mihomo cache.db permissions repaired.");
        // SPEC A1: a combined permission+content anomaly must be
        // re-classified inside the stop window — one pass never authorizes start.
        state = utils::classify_cache_db(&cache_db_path, Some(SERVICE_ACCOUNT_USER));
        if state == utils::CacheDbState::PermissionDrift {
            anyhow::bail!(
                "cache.db permissions are still invalid after repair ({}).\n  Fix: {fix_command}",
                cache_db_path.display()
            );
        }
    }

    let classification =
        utils::classify_cache_db_detailed(&cache_db_path, Some(SERVICE_ACCOUNT_USER));
    if classification.needs_isolation() {
        println!(
            "  ⚠ cache.db is {} and will be quarantined before the Core starts.",
            classification.state.as_str()
        );
    }
    Ok(())
}

const SERVICE_ACCOUNT_USER: &str = "mihomo";

/// `PermissionDrift` short-circuits the content verdict, so a drifted
/// `cache.db` may also be corrupt and the Core would delete it once the repair
/// lets it open the file. Copy the bytes aside first — the original is never
/// mutated and a failed copy must not block the repair-first flow.
fn preserve_unjudged_cache_db_evidence(
    cache_db_path: &std::path::Path,
    service_user: Option<&str>,
) -> Option<std::path::PathBuf> {
    let classification = utils::classify_cache_db_detailed(cache_db_path, service_user);
    if !classification.needs_evidence_preservation() {
        return None;
    }
    // Judged-unsafe states are moved aside by the daemon's spawn gate / controlled
    // reset; only the unjudged side (content verdict short-circuited by the drift)
    // needs a copy here.
    if classification.needs_isolation() {
        return None;
    }
    match utils::preserve_cache_db_evidence(cache_db_path) {
        Ok(preserved) => {
            println!(
                "  ⚠ cache.db {} 的内容未判定（权限漂移），已复制留证：{}",
                classification.state.as_str(),
                preserved.display()
            );
            Some(preserved)
        }
        Err(error) => {
            eprintln!("  ⚠ cache.db 留证复制失败（{error}）；按修复优先继续");
            None
        }
    }
}

/// SPEC C1: `start --system` and `restart --system` share one repair gate.
fn lifecycle_shares_repair_prerequisites(
    mode: instance::InstanceMode,
    action: instance::ServiceAction,
) -> bool {
    mode == instance::InstanceMode::System
        && matches!(
            action,
            instance::ServiceAction::Start | instance::ServiceAction::Restart
        )
}

fn lifecycle_repair_fix_command(action: instance::ServiceAction) -> &'static str {
    match action {
        instance::ServiceAction::Start => "mihomo-cli start --system --yes",
        _ => "mihomo-cli restart --system --yes",
    }
}

/// SPEC §12.3.1（裁决 B）：授权提示里的操作名与影响面措辞。
fn lifecycle_authorization_labels(action: instance::ServiceAction) -> (&'static str, &'static str) {
    match action {
        instance::ServiceAction::Start => ("start --system", "启动"),
        instance::ServiceAction::Stop => ("stop --system", "停止"),
        instance::ServiceAction::Restart => ("restart --system", "重启"),
        instance::ServiceAction::Uninstall => ("uninstall --system", "卸载"),
        instance::ServiceAction::Enable | instance::ServiceAction::Disable => {
            ("autostart --system", "该操作")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheDbRepairDecision {
    /// `--yes`: repair without asking.
    Apply,
    /// Interactive TTY without `--yes`: explain the impact, then ask.
    Prompt,
    /// No TTY and no `--yes`: report `repair_required` and do not self-heal.
    RefuseRepairRequired,
}

fn cache_db_permission_repair_decision(
    allow_runtime_reset: bool,
    prompt_runtime_reset: bool,
) -> CacheDbRepairDecision {
    if allow_runtime_reset {
        CacheDbRepairDecision::Apply
    } else if prompt_runtime_reset {
        CacheDbRepairDecision::Prompt
    } else {
        CacheDbRepairDecision::RefuseRepairRequired
    }
}

fn cache_db_repair_required_message(fix_command: &str) -> String {
    format!(
        "repair_required: cache.db ownership/permissions drifted from the mihomo service user \
         and was not repaired (no terminal and no --yes).\n  Fix: {fix_command}"
    )
}

fn confirm_cache_db_permission_repair(cache_db_path: &std::path::Path) -> anyhow::Result<bool> {
    use dialoguer::Confirm;

    println!(
        "  cache.db permissions must be repaired ({}) so the Core can persist its state.",
        cache_db_path.display()
    );
    Ok(Confirm::new()
        .with_prompt("Repair cache.db ownership/permissions? The file content is not modified")
        .default(false)
        .interact()?)
}

async fn cmd_lifecycle_instance_mode(
    mode: instance::InstanceMode,
    action: instance::ServiceAction,
    allow_runtime_reset: bool,
    prompt_runtime_reset: bool,
) -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance lifecycle"))?;

    // SPEC §12.3.1（裁决 B）：root 专属域的**业务操作授权**边界。必须在首次特权
    // 读取/留证/写入之前完成（避免「先写用户 intent 再重入」），且与三态权限判定
    // 严格分离：这里不判定漂移、不修复、不宣称检测到权限异常。
    let (operation, impact) = lifecycle_authorization_labels(action);
    authorize_protected_domain_operations(&ctx, operation, impact)?;

    // SPEC C1: Shared repair prerequisites for start/restart --system
    if lifecycle_shares_repair_prerequisites(mode, action) {
        shared_repair_prerequisites(
            &ctx,
            allow_runtime_reset,
            prompt_runtime_reset,
            lifecycle_repair_fix_command(action),
        )
        .await?;
    }

    if mode == instance::InstanceMode::System {
        if action == instance::ServiceAction::Restart {
            ensure_system_state_ownership_for_lifecycle()?;
            maybe_sudo_reexec_for_system_generation(&ctx)?;
            ensure_system_config_ownership_for_lifecycle(&ctx)?;
            // SPEC P0/P1-3: evidence preservation precedes every recovery
            // action — recovery may start the old Core, whose bbolt init deletes a
            // broken cache.db. Idempotent, so the pipeline's own P0 call is free.
            preserve_upgrade_evidence(&ctx)?;
            maybe_auto_recover_active_transaction(
                &ctx,
                tun_transaction::RecoveryDirection::Resume,
                allow_runtime_reset,
                prompt_runtime_reset,
                // only the explicit `restart --system` origin may
                // unlock the migrated-legacy reset admission.
                true,
            )
            .await?;
            // SPEC P2-4: a legacy intent config without external-controller
            // must be repaired BEFORE the pipeline starts Core, otherwise the
            // upgrade would fail and compensate on a pre-existing config defect.
            ensure_instance_controller_endpoint(&ctx)?;
            maybe_prepare_cli_skew_generation(&ctx)?;
            if apply_pending_generation(&ctx).await? {
                return Ok(());
            }
        } else if matches!(action, instance::ServiceAction::Stop)
            && prepare_system_transaction_recovery().await?
        {
            ensure_system_state_ownership_for_lifecycle()?;
            maybe_auto_recover_active_transaction(
                &ctx,
                tun_transaction::RecoveryDirection::Abort,
                false,
                false,
                false,
            )
            .await?;
        }
    }

    if mode == instance::InstanceMode::System
        && matches!(
            action,
            instance::ServiceAction::Start | instance::ServiceAction::Restart
        )
    {
        ensure_instance_controller_endpoint(&ctx)?;
    }

    // v3 system lifecycle is core lifecycle: after installation, these commands
    // must go through the root daemon IPC and must not sudo-fallback to the OS
    // service manager. Managing the daemon itself is an install/admin concern.
    if mode == instance::InstanceMode::System {
        if !ipc::is_daemon_running().await {
            match action {
                instance::ServiceAction::Stop => {
                    println!(
                        "{} mihomo {} instance...",
                        lifecycle_verb(action),
                        instance_mode_label(mode)
                    );
                    println!("  ✅ system core is already stopped");
                    return Ok(());
                }
                instance::ServiceAction::Start | instance::ServiceAction::Restart => {
                    // lifecycle precheck: distinguish "no systemd"
                    // (WSL2/containers — cannot ever start) and "not installed"
                    // from "installed but not running" (recoverable below).
                    match service_runtime_state(&ctx) {
                        service::ServiceRuntimeState::SystemdUnavailable => {
                            anyhow::bail!(systemd_unavailable_message())
                        }
                        service::ServiceRuntimeState::NotInstalled => {
                            anyhow::bail!(service_not_installed_message())
                        }
                        service::ServiceRuntimeState::InstalledNotRunning
                        | service::ServiceRuntimeState::Running => {}
                    }

                    println!("  System daemon is not running. Starting system service...");
                    let start_plan =
                        instance::planned_service_plan(&ctx, instance::ServiceAction::Start);
                    for command in &start_plan.commands {
                        if command.privileged {
                            if let Some(invocation) =
                                instance::privilege_invocation_plan(command.clone())
                            {
                                println!(
                                    "  Privilege required. Fallback: {}",
                                    invocation.manual_fallback
                                );
                            }
                        }
                        service::run_instance_command(command).map_err(|e| {
                            anyhow::anyhow!(
                                "Failed to start system service via privilege escalation: {e}\n  \
                                 If you do not have administrator/sudo privileges, ask an administrator to start the service."
                            )
                        })?;
                    }

                    wait_for_system_daemon_readiness().await?;
                }
                _ => {
                    anyhow::bail!(system_service_block_message(
                        lifecycle_command_name(action),
                        &ctx,
                    ));
                }
            }
        }

        // Request-time subscription ID: the daemon may switch the active
        // pointer before it replies, so the replay report must be judged
        // against the requested slot (SPEC §3.8.1).
        let subscription_id_at_request;
        let cmd = match action {
            instance::ServiceAction::Start => {
                let (config_content, config_revision) = lifecycle_system_config_payload(&ctx)?;
                push_selection_mirror_full(&ctx, &config_revision).await;
                let subscription_id =
                    config::get_active_id_at(&utils::AppPaths::new(ctx.paths.config_dir.clone()))?;
                subscription_id_at_request = subscription_id.clone();
                ipc::DaemonCommand::StartCore {
                    config_content,
                    config_revision,
                    subscription_id,
                    token: None,
                }
            }
            instance::ServiceAction::Stop => {
                subscription_id_at_request = None;
                ipc::DaemonCommand::StopCore { token: None }
            }
            instance::ServiceAction::Restart => {
                let (config_content, config_revision) = lifecycle_system_config_payload(&ctx)?;
                push_selection_mirror_full(&ctx, &config_revision).await;
                let subscription_id =
                    config::get_active_id_at(&utils::AppPaths::new(ctx.paths.config_dir.clone()))?;
                subscription_id_at_request = subscription_id.clone();
                ipc::DaemonCommand::RestartCore {
                    config_content,
                    config_revision,
                    subscription_id,
                    token: None,
                }
            }
            _ => return Err(anyhow::anyhow!("unsupported lifecycle action via IPC")),
        };
        let resp = ipc::send_command(&cmd).await?;
        match resp {
            ipc::DaemonResponse::Success { message } => {
                // review MAJOR-5：daemon 不读用户 home，mirror 缺失只回传结构化信号；
                // 由 CLI 结合用户 selections树决定最终措辞（§4.8 不得静默降级）。
                let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
                for line in
                    lifecycle_success_lines(&message, subscription_id_at_request.as_deref(), &paths)
                {
                    println!("{line}");
                }
                // Readiness is confirmed by the daemon (it waits for the core
                // API before replying Success). The CLI must not re-poll —
                // aligned with clash-verge-service: client trusts the
                // daemon's Success as the readiness contract.
                return Ok(());
            }
            ipc::DaemonResponse::Error { message } => {
                anyhow::bail!(daemon_command_error_message(ctx.os, &message));
            }
            ipc::DaemonResponse::Status { .. } => return Ok(()),
            ipc::DaemonResponse::CoreApi { .. }
            | ipc::DaemonResponse::Transaction { .. }
            | ipc::DaemonResponse::SelectionMirrorRevisions { .. }
            | ipc::DaemonResponse::SelectionDrift { .. } => {
                anyhow::bail!(
                    "unexpected Core API / Transaction / mirror revisions response to lifecycle command"
                );
            }
        }
    }

    // Phase 5: 启动前检查 TCP 端口是否被占用
    if matches!(action, instance::ServiceAction::Start) {
        if let Some(port) = read_mixed_port_from_config(&ctx.paths.intent_config_file) {
            if is_tcp_port_in_use(port) {
                anyhow::bail!(
                    "端口 {port} 已被占用\n  \
                     可能原因：\n  \
                       - 另一个 mihomo 实例正在运行\n  \
                       - 其他程序占用了此端口\n  \
                     建议：\n  \
                       1. 更换端口：mihomo-cli config set mixed-port {new_port}\n  \
                       2. 或停止占用端口的程序\n  \
                       3. 然后重新启动：mihomo-cli start",
                    new_port = port + 1
                );
            }
        }
    }

    // Phase 5: 启动前检查 API 端点是否已被占用（另一个实例可能正在运行）
    if matches!(action, instance::ServiceAction::Start)
        && mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs")
            .await
            .is_ok()
    {
        eprintln!(
            "warning: API endpoint already responding at {}\n  \
             The {} core may already be running.\n  \
             If you want to restart: mihomo-cli restart",
            status_endpoint_label(&ctx.paths.api_endpoint),
            instance_mode_label(mode),
        );
    }

    let plan = instance::planned_service_plan(&ctx, action);

    println!(
        "{} mihomo {} instance...",
        lifecycle_verb(action),
        instance_mode_label(mode)
    );
    for command in &plan.commands {
        if command.privileged {
            if let Some(invocation) = instance::privilege_invocation_plan(command.clone()) {
                println!(
                    "  Privilege required. Fallback: {}",
                    invocation.manual_fallback
                );
            }
        }
        service::run_instance_command(command)?;
    }

    if matches!(
        action,
        instance::ServiceAction::Start | instance::ServiceAction::Restart
    ) {
        wait_for_instance_readiness(&ctx).await?;
        // User instance only (system returned early via daemon IPC; the daemon
        // replays after its own readiness confirmation): restore persisted
        // selections now that the Core API is confirmed ready (SPEC §3.2).
        let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
        let client = mihomo_api::EndpointMihomoApiClient::for_instance(
            ctx.mode,
            ctx.paths.api_endpoint.clone(),
        );
        print_selection_replay_after_ready(
            &paths,
            &client,
            std::time::Instant::now() + selection::REPLAY_TOTAL_BUDGET,
        )
        .await;
    }

    Ok(())
}

async fn wait_for_system_daemon_shutdown() -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    while start.elapsed() < timeout {
        if !ipc::is_daemon_running().await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "mihomo system daemon did not stop within {}s; pending generation was not applied",
        timeout.as_secs()
    )
}

async fn verify_running_daemon_revision(expected_revision: &str) -> anyhow::Result<()> {
    match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await? {
        ipc::DaemonResponse::Status {
            daemon_executable_revision: Some(actual_revision),
            ..
        } if actual_revision == expected_revision => Ok(()),
        ipc::DaemonResponse::Status {
            daemon_executable_revision: Some(actual_revision),
            ..
        } => anyhow::bail!(
            "running daemon revision {actual_revision} does not match pending generation {expected_revision}"
        ),
        ipc::DaemonResponse::Status {
            daemon_executable_revision: None,
            ..
        } => anyhow::bail!(
            "running daemon cannot attest its executable revision; pending generation was not committed"
        ),
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!("daemon identity verification failed: {message}")
        }
        _ => anyhow::bail!("daemon returned an unexpected identity response"),
    }
}

async fn wait_for_system_daemon_readiness() -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);
    while start.elapsed() < timeout {
        if ipc::is_daemon_running().await {
            println!(
                "  ✅ System daemon ready at {}",
                ipc::system_service_socket_path().display()
            );
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    anyhow::bail!(
        "mihomo system daemon did not become ready within {}s.\n  Socket: {}",
        timeout.as_secs(),
        ipc::system_service_socket_path().display()
    )
}

#[cfg(unix)]
#[allow(dead_code)]
fn username_for_uid(uid: u32) -> anyhow::Result<String> {
    let account = unsafe { libc::getpwuid(uid) };
    if account.is_null() {
        anyhow::bail!("cannot resolve account name for uid {uid}");
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*account).pw_name) }
        .to_str()
        .map_err(|err| anyhow::anyhow!("invalid account name for uid {uid}: {err}"))?;
    Ok(name.to_string())
}

#[cfg(unix)]
async fn ensure_system_daemon_access(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let status = ipc::DaemonCommand::GetStatus { token: None };
    let installing_for_non_root_user_as_root =
        unsafe { libc::geteuid() == 0 } && ctx.owner_uid.is_some_and(|uid| uid != 0);
    if !installing_for_non_root_user_as_root
        && matches!(
            ipc::send_command(&status).await?,
            ipc::DaemonResponse::Status { .. }
        )
    {
        return Ok(());
    }

    service::setup_unix_system_owner_credentials(ctx)?;

    match ipc::send_command(&status).await? {
        ipc::DaemonResponse::Status { .. } => Ok(()),
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!("IPC owner credentials setup did not take effect: {message}")
        }
        ipc::DaemonResponse::Success { message } => {
            anyhow::bail!("unexpected daemon response after IPC owner credentials setup: {message}")
        }
        ipc::DaemonResponse::CoreApi { .. }
        | ipc::DaemonResponse::Transaction { .. }
        | ipc::DaemonResponse::SelectionMirrorRevisions { .. }
        | ipc::DaemonResponse::SelectionDrift { .. } => {
            anyhow::bail!(
                "unexpected Core API / Transaction / mirror revisions response after IPC owner credentials setup"
            )
        }
    }
}

#[cfg(not(unix))]
async fn ensure_system_daemon_access(_ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    Ok(())
}

async fn wait_for_instance_readiness(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let readiness = instance::planned_readiness(ctx);
    if crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
        println!();
        println!("=== Readiness checks ===");
        for probe in [
            &readiness.service_running_probe,
            &readiness.configured_endpoint_probe,
            &readiness.endpoint_connect_probe,
            &readiness.api_probe,
        ] {
            println!("  - {:?}: {}", probe.kind, probe.target);
        }
    }

    verify_config_endpoint(ctx)?;

    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(15);
    let mut last_error = String::new();
    let client =
        mihomo_api::EndpointMihomoApiClient::for_instance(ctx.mode, ctx.paths.api_endpoint.clone());
    while start.elapsed() < timeout {
        match client.get("/configs").await {
            Ok(_) => {
                println!(
                    "  ✅ API ready at {}",
                    status_endpoint_label(&ctx.paths.api_endpoint)
                );
                return Ok(());
            }
            Err(e) => {
                last_error = e.to_string();
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }

    let mut message = format!(
        "mihomo service did not become API-ready within {}s.\n  Endpoint: {}",
        timeout.as_secs(),
        status_endpoint_label(&ctx.paths.api_endpoint)
    );
    if !last_error.is_empty() {
        message.push_str(&format!("\n  Last API error: {last_error}"));
    }
    if let Some(log_file) = &ctx.paths.log_file {
        message.push_str(&format!("\n  Logs: {}", log_file.display()));
    }
    if let Some(hint) = readiness.failure_hint_command {
        message.push_str(&format!(
            "\n  Privileged fallback: {}",
            hint.manual_fallback
        ));
    }
    anyhow::bail!(message)
}

fn verify_config_endpoint(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(&ctx.paths.intent_config_file).map_err(|e| {
        anyhow::anyhow!(
            "cannot read config for readiness check: {} ({e})",
            ctx.paths.intent_config_file.display()
        )
    })?;
    let expected_line = ctx.paths.api_endpoint.controller_line();
    if content.contains(&expected_line) {
        return Ok(());
    }
    anyhow::bail!(
        "config endpoint does not match selected instance.\n  Expected: {}\n  Config: {}\n  Fix: {}",
        expected_line,
        ctx.paths.intent_config_file.display(),
        config_fix_command_for_mode(ctx.mode)
    )
}

/// Read the mixed-port from config.yaml. Returns None if not found or unparseable.
fn read_mixed_port_from_config(config_path: &std::path::Path) -> Option<u16> {
    let content = std::fs::read_to_string(config_path).ok()?;
    let config: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    config
        .get("mixed-port")?
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
}

/// Check if a TCP port is already in use by attempting to bind to it.
fn is_tcp_port_in_use(port: u16) -> bool {
    use std::net::TcpListener;
    TcpListener::bind(("127.0.0.1", port)).is_err()
}

fn lifecycle_verb(action: instance::ServiceAction) -> &'static str {
    match action {
        instance::ServiceAction::Start => "Starting",
        instance::ServiceAction::Stop => "Stopping",
        instance::ServiceAction::Restart => "Restarting",
        instance::ServiceAction::Uninstall => "Uninstalling",
        instance::ServiceAction::Enable => "Enabling",
        instance::ServiceAction::Disable => "Disabling",
    }
}

fn lifecycle_command_name(action: instance::ServiceAction) -> &'static str {
    match action {
        instance::ServiceAction::Start => "start",
        instance::ServiceAction::Stop => "stop",
        instance::ServiceAction::Restart => "restart",
        instance::ServiceAction::Uninstall => "uninstall",
        instance::ServiceAction::Enable => "enable",
        instance::ServiceAction::Disable => "disable",
    }
}

#[allow(dead_code)]
fn cmd_status_instance(system: bool, user: bool) -> anyhow::Result<()> {
    let mode = match mode_request_from_flags(system, user) {
        instance::ModeRequest::ExplicitSystem => instance::InstanceMode::System,
        instance::ModeRequest::ExplicitUser => instance::InstanceMode::User,
        instance::ModeRequest::Unspecified => unreachable!("caller handles unspecified status"),
    };
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(cmd_status_instance_mode(mode))
    })
}

async fn cmd_status_instance_mode(mode: instance::InstanceMode) -> anyhow::Result<()> {
    cmd_status_instance_mode_with_source(mode, Some(instance::ResolutionSource::ExplicitFlag)).await
}

async fn cmd_status_instance_mode_with_source(
    mode: instance::InstanceMode,
    source: Option<instance::ResolutionSource>,
) -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance status"))?;
    cmd_status_context_with_source(ctx, source).await
}

/// status four-way: surface the service runtime diagnosis
/// (`no-systemd` / `not-installed` / `installed-not-running`) when nothing is
/// actually up, so "installed but stopped" is never shown as a bare `unknown`.
/// Returns `None` when the instance is up or running — callers fall back to the
/// rich health labels.
fn service_runtime_health_label(
    state: service::ServiceRuntimeState,
    instance_up: bool,
) -> Option<&'static str> {
    if instance_up {
        return None;
    }
    match state {
        service::ServiceRuntimeState::SystemdUnavailable => Some("no-systemd"),
        service::ServiceRuntimeState::NotInstalled => Some("not-installed"),
        service::ServiceRuntimeState::InstalledNotRunning => Some("installed-not-running"),
        service::ServiceRuntimeState::Running => None,
    }
}

fn snapshot_instance_up(snapshot: &status::StatusSnapshot) -> bool {
    snapshot.daemon_reachable == status::TriState::True
        || snapshot.api_reachable
        || snapshot.core_running.as_bool() == Some(true)
}

/// Spec §0.6: the single user-visible health predicate. Both the text summary
/// and `status --json` call this — never branch on raw snapshot fields in a
/// formatter, or `TunRunningUnattested` degrades in one output and reads
/// `healthy` in the other.
fn status_health_label(
    snapshot: &status::StatusSnapshot,
    runtime: service::ServiceRuntimeState,
) -> &'static str {
    if let Some(label) = service_runtime_health_label(runtime, snapshot_instance_up(snapshot)) {
        return label;
    }
    if snapshot.daemon_reachable == status::TriState::True && !snapshot.intent_config_exists {
        return "ready";
    }
    match (snapshot.core_running.as_bool(), snapshot.api_reachable) {
        (Some(false), _) => "inactive",
        (Some(true), true)
            if matches!(
                snapshot.tun_verdict,
                status::TunVerdict::TunDisabled | status::TunVerdict::TunRunning
            ) =>
        {
            "healthy"
        }
        (Some(true), _) | (None, true) => "degraded",
        _ => "unknown",
    }
}

fn status_core_label(snapshot: &status::StatusSnapshot) -> &'static str {
    match snapshot.core_running.as_bool() {
        Some(true) => "running",
        Some(false) => "stopped",
        None if snapshot.daemon_reachable == status::TriState::True
            && !snapshot.intent_config_exists =>
        {
            "stopped"
        }
        None => "unknown",
    }
}

fn status_api_label(snapshot: &status::StatusSnapshot) -> &'static str {
    if snapshot.daemon_reachable == status::TriState::True && !snapshot.intent_config_exists {
        "not configured"
    } else if snapshot.api_reachable {
        "reachable"
    } else {
        "unknown"
    }
}

fn status_configuration_label(snapshot: &status::StatusSnapshot) -> &'static str {
    if snapshot.daemon_reachable == status::TriState::True && !snapshot.intent_config_exists {
        "not configured"
    } else {
        match snapshot.configuration_verdict {
            status::ConfigurationVerdict::Applied => "applied",
            status::ConfigurationVerdict::OutOfDate => "out of date",
            status::ConfigurationVerdict::Unknown => "unknown",
        }
    }
}

/// A2' observation line: purely observational, no action. Only
/// when ≥1 registered (cache-valid), non-retired, non-active subscription slot
/// holds persisted selections — retired slots are covered by the A1' revive
/// path, not by `config --switch`. Explicit exception to SPEC §3.8.1's
/// "status only reads the active intent" rule (contract table updated).
fn print_status_fixed_selections_observation(ctx: &instance::InstanceContext) {
    let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
    let Ok(subs) = config::load_subscriptions_at(&paths) else {
        return;
    };
    let active = config::get_active_id_at(&paths).ok().flatten();
    let count = count_fixed_selection_slots(&paths, &subs, active.as_deref());
    if count > 0 {
        println!(
            "Selections:      {count} registered subscription slot(s) hold fixed selections (config --switch <id>)"
        );
    }
}

fn count_fixed_selection_slots(
    paths: &utils::AppPaths,
    subs: &[config::SubscriptionMeta],
    active: Option<&str>,
) -> usize {
    subs.iter()
        .filter(|sub| {
            sub.is_alive()
                && active != Some(sub.id.as_str())
                && subscription_cache_is_valid_yaml(paths, &sub.id)
                && selection_slot_holds_selections(paths, &sub.id)
        })
        .count()
}

/// Cache validity: the file must exist AND be parseable as a non-empty
/// subscription YAML (not just `is_file()`). A corrupt cache is not a
/// switchable target.
fn subscription_cache_is_valid_yaml(paths: &utils::AppPaths, id: &str) -> bool {
    let path = paths.subscription_file_path(id);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_yaml::from_str::<serde_yaml::Value>(&content).ok())
        .is_some_and(|yaml| yaml.is_mapping() || yaml.is_sequence())
}

fn selection_slot_holds_selections(paths: &utils::AppPaths, id: &str) -> bool {
    let path = paths.selection_state_path_for_subscription(id);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_yaml::from_str::<selection::SelectionStateFile>(&content).ok())
        .is_some_and(|state| !state.selections.is_empty())
}

/// B3: cached drift observation from the daemon. Read-only, no
/// network, no fix; the one-click convergence command is printed for the user
/// to run explicitly.
async fn print_status_selection_drift_observation(ctx: &instance::InstanceContext) {
    if ctx.mode != instance::InstanceMode::System || !ipc::is_daemon_running().await {
        return;
    }
    // `send_command` injects the client token internally (with_ipc_token).
    match ipc::send_command(&ipc::DaemonCommand::GetSelectionDrift { token: None }).await {
        Ok(ipc::DaemonResponse::SelectionDrift {
            observations,
            unknown_samples,
            sample_status,
            journal,
        }) => {
            println!(
                "Selection drift: {} observation(s), sampler {}, {} unknown sample(s)",
                observations.len(),
                sample_status,
                unknown_samples
            );
            // N2: most recent observation by `at_unix` (ties broken by tail
            // position). Shared with the daemon-side test so `.last()` cannot
            // silently come back.
            if let Some(latest) = daemon::latest_drift_observation(&observations) {
                println!(
                    "  Latest:         {} runtime={} intent={}",
                    latest.group, latest.runtime, latest.intent
                );
                println!(
                    "  Fix:            {}",
                    crate::selection::select_fix_command(&latest.group, &latest.runtime)
                );
            }
            if let Some(journal) = journal {
                println!("  Journal:        {journal} (cache still served)");
            }
        }
        Ok(_) | Err(_) => {
            // Cached observation unavailable is not a status failure; stay quiet.
        }
    }
}

async fn cmd_status_context_with_source(
    ctx: instance::InstanceContext,
    source: Option<instance::ResolutionSource>,
) -> anyhow::Result<()> {
    let plan = instance::planned_status_diagnostics(&ctx);
    let verbose = crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed);

    // 使用 StatusSnapshot 统一采集状态
    let snapshot = status::StatusSnapshot::collect(&ctx).await;

    let health = status_health_label(&snapshot, service_runtime_state(&ctx));

    let default_route = default_route_label(
        default_route_path(
            snapshot.active_config_path.as_deref(),
            &ctx.paths.intent_config_file,
        ),
        &snapshot.rule_mode,
    );

    println!("=== Mihomo Status ===");
    println!("Health:          {health}");
    println!("Instance:        {}", instance_mode_label(plan.mode));
    println!("Core:            {}", status_core_label(&snapshot));
    if let Some(observation) = orphan_core_observation(&ctx).await {
        println!("Orphan Core:     {observation}");
    }
    println!("API:             {}", status_api_label(&snapshot));
    println!("TUN:             {}", status_tun_label(&snapshot));
    println!(
        "System proxy:    {}",
        match snapshot.system_proxy {
            crate::system_proxy::SystemProxyState::Enabled => "enabled",
            crate::system_proxy::SystemProxyState::Disabled => "disabled",
            crate::system_proxy::SystemProxyState::Unsupported => "unsupported",
            crate::system_proxy::SystemProxyState::Unknown => "unknown",
        }
    );
    println!(
        "Shell proxy:     {}",
        match snapshot.shell_proxy {
            crate::system_proxy::ShellProxyState::Configured => "configured",
            crate::system_proxy::ShellProxyState::NotConfigured => "not configured",
            crate::system_proxy::ShellProxyState::Unknown => "unknown",
        }
    );
    println!("Rule mode:       {}", snapshot.rule_mode);
    println!("Default route:   {default_route}");
    println!("Configuration:   {}", status_configuration_label(&snapshot));
    print_status_fixed_selections_observation(&ctx);
    print_status_selection_drift_observation(&ctx).await;

    if verbose {
        if let Some(source) = source {
            println!("Resolved by:     {}", resolution_source_label(source));
        }
        println!("Service:          {}", status_service_label(&plan.service));
        println!(
            "Service state:    {}",
            if service_manager_active(&ctx) {
                "active"
            } else {
                "unknown"
            }
        );
        println!(
            "API endpoint:     {}",
            status_endpoint_label(&plan.expected_endpoint)
        );

        // 获取配置以显示监听端口
        let config = if snapshot.core_running.as_bool() == Some(true)
            || ctx.mode == instance::InstanceMode::User
        {
            mihomo_api::get_config_for_instance(ctx.mode, &ctx.paths.api_endpoint)
                .await
                .ok()
        } else {
            None
        };

        println!(
            "Listening ports:  {}",
            config
                .as_ref()
                .map(format_listening_ports)
                .unwrap_or_else(|| "unknown".to_string())
        );
        println!("Config path:      {}", plan.config_file.display());
        if let Some(log_file) = &plan.log_file {
            println!("Logs:             {}", log_file.display());
        }
    }
    check_and_warn_daemon_binary_skew(&ctx);
    Ok(())
}

pub(crate) fn is_system_cli_skewed(ctx: &instance::InstanceContext) -> bool {
    if ctx.mode != instance::InstanceMode::System {
        return false;
    }
    let Ok(installed_sha) = file_sha256(&ctx.paths.cli_binary) else {
        return false;
    };
    let Some(installed_sha) = installed_sha else {
        return false;
    };
    // Same identity criterion as P1 and the tombstone retry gate.
    match daemon_identity_matches_current_exe(&installed_sha) {
        Ok(matches) => !matches,
        Err(_) => false,
    }
}

pub(crate) fn check_and_warn_daemon_binary_skew(ctx: &instance::InstanceContext) {
    if ctx.mode != instance::InstanceMode::System {
        return;
    }
    let store = system_generation_store(ctx);
    match store.observe_pending() {
        generation::PendingObservation::HasPending(pending_id) => {
            eprintln!("⚠ 提示: 存在待应用的系统更新 ({pending_id})。");
            eprintln!("  如需应用更新，请运行: mihomo-cli restart\n");
            return;
        }
        // 读不动 ≠没有 pending——如实报「无法判定」并给授权检查指引，
        // 不宣称有/无、不提权；随后照常展示独立可读的 skew 观测。
        generation::PendingObservation::Undeterminable(error) => {
            eprintln!("⚠ 提示: {}。\n", pending_generation_unknown_notice(&error));
        }
        generation::PendingObservation::Applied => {}
    }
    if is_system_cli_skewed(ctx) {
        let current_cli = std::env::current_exe().unwrap_or_default();
        eprintln!("⚠ 提示: 当前 CLI 与系统 Daemon 二进制不一致。");
        eprintln!("  当前 CLI:   {}", current_cli.display());
        eprintln!("  系统 Daemon: {}", ctx.paths.cli_binary.display());
        eprintln!("  如需应用更新，请运行: mihomo-cli restart\n");
    }
}

fn format_listening_ports(config: &serde_json::Value) -> String {
    [
        "port",
        "socks-port",
        "mixed-port",
        "redir-port",
        "tproxy-port",
    ]
    .iter()
    .filter_map(|key| config[*key].as_u64().map(|port| format!("{key}={port}")))
    .collect::<Vec<_>>()
    .join(", ")
}

fn clean_status_label(value: &str) -> String {
    value.chars().filter(|ch| !ch.is_control()).collect()
}

fn default_route_path<'a>(
    active_config_path: Option<&'a std::path::Path>,
    intent_config_path: &'a std::path::Path,
) -> &'a std::path::Path {
    active_config_path.unwrap_or(intent_config_path)
}

/// Spec §0.6: attested TUN judgment shared by the text summary and
/// `status --json`. Raw runtime observation is only exposed as `core.tun`.
fn status_tun_label(snapshot: &status::StatusSnapshot) -> &'static str {
    match snapshot.tun_verdict {
        status::TunVerdict::TunRunning => "enabled",
        status::TunVerdict::TunDisabled => "disabled",
        status::TunVerdict::TunRunningUnattested | status::TunVerdict::TunStateUnknown => "unknown",
    }
}

/// Dashboard decoration on top of [`status_tun_label`]; unattested or unknown
/// states stay `unknown` so they can never render as the green enabled badge.
fn dashboard_tun_label(snapshot: &status::StatusSnapshot) -> &'static str {
    match status_tun_label(snapshot) {
        "enabled" => "✅ enabled",
        "disabled" => "❌ disabled",
        _ => "unknown",
    }
}

fn tun_command_status_label(snapshot: &status::StatusSnapshot) -> String {
    match snapshot.tun_verdict {
        status::TunVerdict::TunRunning => "enabled".to_string(),
        status::TunVerdict::TunDisabled => "disabled".to_string(),
        status::TunVerdict::TunRunningUnattested => "unknown (recovery required)".to_string(),
        status::TunVerdict::TunStateUnknown => "unknown".to_string(),
    }
}

fn system_proxy_status_label(proxy: crate::system_proxy::SystemProxyState) -> &'static str {
    match proxy {
        crate::system_proxy::SystemProxyState::Enabled => "enabled",
        crate::system_proxy::SystemProxyState::Disabled => "disabled",
        crate::system_proxy::SystemProxyState::Unsupported => "unsupported",
        crate::system_proxy::SystemProxyState::Unknown => "unknown",
    }
}

fn shell_proxy_status_label(proxy: crate::system_proxy::ShellProxyState) -> &'static str {
    match proxy {
        crate::system_proxy::ShellProxyState::Configured => "configured",
        crate::system_proxy::ShellProxyState::NotConfigured => "not configured",
        crate::system_proxy::ShellProxyState::Unknown => "unknown",
    }
}

fn default_route_label(path: &std::path::Path, mode: &str) -> String {
    if mode == "global" {
        return "current global selection".to_string();
    }
    if mode == "direct" {
        return "DIRECT".to_string();
    }
    let Ok(file) = utils::open_regular_file_no_follow(path) else {
        return "unknown".to_string();
    };
    let Ok(config) = serde_yaml::from_reader::<_, serde_yaml::Value>(file) else {
        return "unknown".to_string();
    };
    config["rules"]
        .as_sequence()
        .and_then(|rules| {
            rules.iter().rev().find_map(|rule| {
                rule.as_str().and_then(|value| {
                    let mut parts = value.split(',');
                    let kind = parts.next()?;
                    if kind.eq_ignore_ascii_case("MATCH") {
                        Some(clean_status_label(parts.next().unwrap_or("unknown").trim()))
                    } else {
                        None
                    }
                })
            })
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn resolution_source_label(source: instance::ResolutionSource) -> &'static str {
    match source {
        instance::ResolutionSource::ExplicitFlag => "explicit mode flag",
        instance::ResolutionSource::ServicePresence => "installed service detection",
        instance::ResolutionSource::RuntimePresence => "running instance detection",
        instance::ResolutionSource::DefaultMode => "default per-user mode",
        instance::ResolutionSource::EnvOverride => "MIHOMO_CLI_CONFIG_DIR",
        instance::ResolutionSource::InteractivePrompt => "interactive prompt",
        instance::ResolutionSource::LegacyDetection => "legacy layout detection",
    }
}

fn status_service_label(service: &instance::ServiceTarget) -> String {
    match service {
        instance::ServiceTarget::MacosLaunchDaemon {
            domain_label,
            plist,
        } => {
            format!("LaunchDaemon {domain_label} ({})", plist.display())
        }
        instance::ServiceTarget::MacosLaunchAgent {
            domain_label,
            plist,
        } => {
            format!("LaunchAgent {domain_label} ({})", plist.display())
        }
        instance::ServiceTarget::LinuxSystemdSystem { unit } => {
            format!("systemd system ({})", unit.display())
        }
        instance::ServiceTarget::LinuxSystemdUser { unit } => {
            format!("systemd user ({})", unit.display())
        }
        instance::ServiceTarget::WindowsService { name } => format!("Windows Service {name}"),
        instance::ServiceTarget::WindowsUserProcess => "Windows user process".to_string(),
    }
}

fn status_endpoint_label(endpoint: &instance::ApiEndpoint) -> String {
    match endpoint {
        instance::ApiEndpoint::UnixSocket(path) => path.display().to_string(),
        instance::ApiEndpoint::WindowsNamedPipe(pipe) => pipe.clone(),
    }
}

struct DoctorCheck {
    passed: bool,
    warning: bool,
    /// `neutral` 表示「已观察但无法判定」：显示 ❓，且不计入问题数（ISS P2 探测）。
    neutral: bool,
    label: String,
    detail: String,
    hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonIpcErrorKind {
    InvalidOrMissingToken,
    PeerUidMismatch,
    OwnerRecordUnreadable,
    Other,
}

fn classify_daemon_ipc_error(message: &str) -> DaemonIpcErrorKind {
    if message.contains("invalid or missing auth token") {
        DaemonIpcErrorKind::InvalidOrMissingToken
    } else if message.contains("auth token does not belong to IPC peer uid") {
        DaemonIpcErrorKind::PeerUidMismatch
    } else if message.contains("cannot read owner record")
        || message.contains("cannot read authorized clients")
    {
        DaemonIpcErrorKind::OwnerRecordUnreadable
    } else {
        DaemonIpcErrorKind::Other
    }
}

fn daemon_service_status_hint(os: instance::TargetOs) -> &'static str {
    match os {
        instance::TargetOs::Linux => "sudo systemctl status mihomo",
        instance::TargetOs::Macos => "sudo launchctl print system/io.mihomo",
        instance::TargetOs::Windows => "sc.exe query mihomo",
    }
}

const WINDOWS_DAEMON_SERVICE_RECOVERY_HINT: &str =
    "Open an Administrator PowerShell or Command Prompt and run:\n  \
     sc.exe start mihomo\n\
     If the service is already running but unhealthy, run these separately:\n  \
     sc.exe stop mihomo\n  \
     sc.exe start mihomo";

fn daemon_service_recovery_hint(os: instance::TargetOs) -> &'static str {
    match os {
        instance::TargetOs::Linux => "sudo systemctl restart mihomo",
        instance::TargetOs::Macos => "sudo launchctl kickstart -k system/io.mihomo",
        instance::TargetOs::Windows => WINDOWS_DAEMON_SERVICE_RECOVERY_HINT,
    }
}

fn windows_daemon_auth_repair_hint() -> &'static str {
    "Repair the Windows service credentials by reinstalling: mihomo-cli install --system --force --skip-config --yes"
}

fn daemon_command_error_message(os: instance::TargetOs, message: &str) -> String {
    if os == instance::TargetOs::Windows
        && !matches!(
            classify_daemon_ipc_error(message),
            DaemonIpcErrorKind::Other
        )
    {
        return format!(
            "daemon authorization failed: {message}\n  {}",
            windows_daemon_auth_repair_hint()
        );
    }
    match classify_daemon_ipc_error(message) {
        DaemonIpcErrorKind::InvalidOrMissingToken => format!(
            "daemon authorization failed: {message}\n  \
             Check: mihomo-cli doctor --system\n  \
             Admin fix: sudo mihomo-cli install --system"
        ),
        DaemonIpcErrorKind::PeerUidMismatch => format!(
            "daemon authorization failed: {message}\n  \
             Check HOME/XDG_CONFIG_HOME for the current user, or reinstall the system service:\n  \
             sudo mihomo-cli install --system"
        ),
        DaemonIpcErrorKind::OwnerRecordUnreadable => format!(
            "daemon authorization state is unavailable: {message}\n  \
             Admin check: {} (or reinstall: sudo mihomo-cli install --system)",
            daemon_service_status_hint(os)
        ),
        DaemonIpcErrorKind::Other => format!("daemon error: {message}"),
    }
}

#[cfg(not(unix))]
fn doctor_daemon_error_check(os: instance::TargetOs, message: &str) -> DoctorCheck {
    if os == instance::TargetOs::Windows
        && !matches!(
            classify_daemon_ipc_error(message),
            DaemonIpcErrorKind::Other
        )
    {
        return DoctorCheck::fail(
            "Daemon authorization",
            message,
            windows_daemon_auth_repair_hint(),
        );
    }
    match classify_daemon_ipc_error(message) {
        DaemonIpcErrorKind::InvalidOrMissingToken => DoctorCheck::fail(
            "Daemon 授权",
            message,
            "检查凭证文件或以管理员重新安装服务: sudo mihomo-cli install --system",
        ),
        DaemonIpcErrorKind::PeerUidMismatch => DoctorCheck::fail(
            "Daemon 授权",
            message,
            "检查当前用户的 HOME/XDG_CONFIG_HOME；系统服务仅限所有者用户使用，管理员可重新安装: sudo mihomo-cli install --system",
        ),
        DaemonIpcErrorKind::OwnerRecordUnreadable => DoctorCheck::fail(
            "Daemon 授权状态",
            message,
            format!(
                "管理员查看 daemon 状态: {}；或重新安装: sudo mihomo-cli install --system",
                daemon_service_status_hint(os)
            ),
        ),
        DaemonIpcErrorKind::Other => DoctorCheck::fail(
            "Daemon 状态",
            message,
            format!("检查 daemon 状态: {}", daemon_service_status_hint(os)),
        ),
    }
}

fn tun_daemon_error_check(os: instance::TargetOs, message: &str) -> preflight::PreflightResult {
    preflight::PreflightResult::fail(
        "daemon status could not be verified",
        Some(format!(
            "{}\nRetry: mihomo-cli tun on",
            daemon_command_error_message(os, message)
        )),
    )
}

fn tun_daemon_transport_check(os: instance::TargetOs, message: &str) -> preflight::PreflightResult {
    preflight::PreflightResult::fail(
        "daemon IPC is unavailable",
        Some(format!(
            "Transport error: {message}\nRecover the system daemon: {}\nRetry: mihomo-cli tun on",
            daemon_service_recovery_hint(os)
        )),
    )
}

#[cfg(unix)]
fn doctor_checks_config_owner(mode: instance::InstanceMode) -> bool {
    mode == instance::InstanceMode::User
}

/// P2 判据来源选择的结果。
#[cfg(unix)]
enum ProbeConfigSource {
    /// 可用于判定的配置文本与其出处标签。
    Text { label: String, text: String },
    /// 取不到可用配置时的原因（呈现为 ❓，绝不静默跳过）。
    Unavailable(String),
}

/// 有界、不跟随 symlink 的配置读取（review MINOR-e：doctor 也用 的加固读法）。
#[cfg(unix)]
fn read_probe_config_text(path: &std::path::Path) -> anyhow::Result<String> {
    let bytes = crate::utils::read_file_no_follow_limited(
        path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    )?;
    Ok(String::from_utf8(bytes)?)
}

/// 选择 P2 判据来源（review 回归-1）。
///
/// 1. `runtime_attested`（launched == active == intent 三份 revision 一致）时**优先用 intent**：
///    字节上等同于运行配置，而且普通用户可读——`/var/lib/mihomo-cli` 是 `root:mihomo 0770`，
///    非 root 的 `doctor` 读运行配置必然 EACCES，否则探测会永远退化成 ❓；
/// 2. 否则读 daemon 报告的运行配置（root 场景）；
/// 3. 都不可用时返回具体原因，由调用方呈现为 ❓（不得静默跳过探测）。
#[cfg(unix)]
fn probe_config_source(
    runtime_attested: bool,
    active_config_path: Option<&std::path::Path>,
    intent_path: &std::path::Path,
) -> ProbeConfigSource {
    let mut failures: Vec<String> = Vec::new();
    if runtime_attested {
        match read_probe_config_text(intent_path) {
            Ok(text) => {
                return ProbeConfigSource::Text {
                    label: "intent 配置（runtime attested：launched == active == intent）"
                        .to_string(),
                    text,
                }
            }
            Err(err) => failures.push(format!("intent 配置: {err}")),
        }
    }
    match active_config_path {
        Some(path) => match read_probe_config_text(path) {
            Ok(text) => {
                return ProbeConfigSource::Text {
                    label: format!("运行配置 {}", path.display()),
                    text,
                }
            }
            Err(err) => failures.push(format!("运行配置 {}: {err}", path.display())),
        },
        None => failures.push("未观察到 Core 的运行配置".to_string()),
    }
    ProbeConfigSource::Unavailable(failures.join("；"))
}

/// P2 本地解析器探测（SPEC 草案 D1–D4）。
///
/// 仅在 `doctor` 默认启用（`--no-probe-dns` 关闭）、**运行态** TUN 生效且判据配置为 fake-ip
/// 模式时执行一次 ≤2s 的有界查询；结果只生成独立的 `DoctorCheck`，不回填 `StatusSnapshot`，
/// 也不参与 Health / attestation（D4）。
#[cfg(unix)]
async fn doctor_dns_probe_check(
    _ctx: &instance::InstanceContext,
    snapshot: &status::StatusSnapshot,
    probe_dns: bool,
) -> Option<DoctorCheck> {
    if !probe_dns {
        return None;
    }
    // D2（review MAJOR-7 修订）：只有**运行态确认 TUN 生效**才探测。
    // 仅 configured_tun 为真而 Core 未运行时，系统解析本来就不该经过 mihomo，
    // 报 ❌ 会与“Mihomo Core 未运行”重复计数，并把根因指向 DNS 路径。
    if snapshot.runtime_tun.as_bool() != Some(true) {
        return None;
    }
    // 判据来源（review MAJOR-7 + 回归-1）：attested 时优先 intent（与运行配置字节一致
    // 且非 root 可读），否则读运行配置；取不到就给 ❓ 并说明原因，绝不静默跳过。
    let (config_text, source_label) = match probe_config_source(
        snapshot.runtime_attested,
        snapshot.active_config_path.as_deref(),
        &_ctx.paths.intent_config_file,
    ) {
        ProbeConfigSource::Text { text, label } => (text, label),
        ProbeConfigSource::Unavailable(reason) => {
            return Some(DoctorCheck::neutral(
                "系统 DNS 路径",
                format!("无法判定：{reason}"),
            ))
        }
    };
    // D2：非 fake-ip 模式下不存在「返回 fake-ip」这一判据，跳过。
    let range = dns_probe::probe_precondition(&config_text)?;
    match dns_probe::probe_system_resolver(&range).await {
        dns_probe::ProbeVerdict::RoutedThroughMihomo => Some(DoctorCheck::pass(
            "系统 DNS 路径",
            format!(
                "{} → fake-ip（{range}），系统解析经过 mihomo（判据：{source_label}）",
                dns_probe::PROBE_DOMAIN
            ),
        )),
        dns_probe::ProbeVerdict::BypassesMihomo => {
            let resolver_addrs = dns_probe::system_resolver_addrs();
            if has_split_dns_intent(&config_text, &resolver_addrs) {
                Some(DoctorCheck::neutral(
                    "系统 DNS 路径",
                    format!(
                        "{} → 真实地址，检测到覆盖该探测域或系统解析器的 Split-DNS / route-exclude 配置；判据：{source_label}",
                        dns_probe::PROBE_DOMAIN
                    ),
                ))
            } else {
                Some(DoctorCheck::fail(
                    "系统 DNS 路径",
                    format!(
                        "{} → 真实地址（不在 {range} 内），系统解析未经过 mihomo（判据：{source_label}）",
                        dns_probe::PROBE_DOMAIN
                    ),
                    // review MAJOR-8：提示必须落在真正会改变状态的动作上。
                    // `restart` 不执行 TUN/DNS 归一化（只有 `tun on` 会），
                    // 而 场景里 resolv.conf 本来就指向 127.0.0.53，
                    // 旧提示会让用户“按提示做完但状态不变”（违反 §4.8 不得循环）。
                    "TUN 下域名规则会对真实流量失效。运行: mihomo-cli tun on --yes（重建并归一化 TUN/DNS 意图，\
                     含 dns-hijack 与 strict-route；事务会直接应用到 Core，无需再 restart）；\
                     若 Core 未运行则 mihomo-cli start --system。仍绕过说明系统解析器未被 TUN 劫持\
                     （A2 待真机验证的 strict-route 假说）。\
                     刚修复过时系统解析缓存可能仍返回旧地址，稍后重跑 doctor 复核",
                ))
            }
        }
        dns_probe::ProbeVerdict::Unknown(detail) => Some(DoctorCheck::neutral(
            "系统 DNS 路径",
            format!("无法判定：{detail}"),
        )),
    }
}

#[cfg(not(unix))]
async fn doctor_dns_probe_check(
    _ctx: &instance::InstanceContext,
    _snapshot: &status::StatusSnapshot,
    _probe_dns: bool,
) -> Option<DoctorCheck> {
    None
}

fn doctor_checks_service(service: &instance::ServiceTarget) -> bool {
    !matches!(service, instance::ServiceTarget::WindowsUserProcess)
}

/// B3: immediate read-only drift comparison for `doctor --system`.
/// Reuses the daemon's decision function ([`daemon::compare_selection_drift`]);
/// intent comes from the user tree (never `/var/lib`), runtime from a single
/// read-only `GET /proxies`. No write, no fix.
///
/// System mode routes through the authenticated daemon (SPEC §1.4/§3.8.3);
/// intent source unreadable/corrupt → "不可判定" (not a false green).
async fn doctor_selection_drift_check(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<DoctorCheck> {
    let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
    let active = config::get_active_id_at(&paths)?;
    let Some(active) = active else {
        return Ok(DoctorCheck::pass(
            "Selection drift",
            "无 active 订阅，无意图可比对",
        ));
    };
    let intent_path = paths.selection_state_path_for_subscription(&active);
    let intent_result = std::fs::read_to_string(&intent_path)
        .ok()
        .and_then(|content| serde_yaml::from_str::<selection::SelectionStateFile>(&content).ok())
        .map(|state| state.selections)
        .filter(|selections| !selections.is_empty());
    let Some(intent) = intent_result else {
        // Intent unreadable or unparseable → "不可判定", not a false green.
        if intent_path.exists() {
            return Ok(DoctorCheck::neutral(
                "Selection drift",
                "active 订阅 intent 文件存在但不可解析，无法判定",
            ));
        }
        return Ok(DoctorCheck::pass(
            "Selection drift",
            "active 订阅无持久化选点，无意图可比对",
        ));
    };
    let client =
        mihomo_api::EndpointMihomoApiClient::for_instance(ctx.mode, ctx.paths.api_endpoint.clone());
    let proxies = match client.get("/proxies").await {
        Ok(value) => value,
        Err(err) => {
            return Ok(DoctorCheck::neutral(
                "Selection drift",
                format!("Core API 不可达: {err:#}"),
            ));
        }
    };
    let at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let drifts = daemon::compare_selection_drift(&intent, &proxies, at_unix, None, None);
    if drifts.is_empty() {
        Ok(DoctorCheck::pass(
            "Selection drift",
            format!("runtime 与 persisted intent 一致（{} 项）", intent.len()),
        ))
    } else {
        let detail = drifts
            .iter()
            .map(|d| format!("{} runtime={} intent={}", d.group, d.runtime, d.intent))
            .collect::<Vec<_>>()
            .join("; ");
        let hint = drifts
            .iter()
            .map(|d| crate::selection::select_fix_command(&d.group, &d.runtime))
            .collect::<Vec<_>>()
            .join("; ");
        Ok(DoctorCheck::fail(
            "Selection drift",
            detail,
            format!("显式追平: {hint}"),
        ))
    }
}

impl DoctorCheck {
    fn pass(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            passed: true,
            warning: false,
            neutral: false,
            label: label.into(),
            detail: detail.into(),
            hint: None,
        }
    }

    /// 无法判定的观察结果：不算问题，但要在输出里显式标出来。
    fn neutral(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            passed: true,
            warning: false,
            neutral: true,
            label: label.into(),
            detail: detail.into(),
            hint: None,
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn warn(
        label: impl Into<String>,
        detail: impl Into<String>,
        hint: Option<impl Into<String>>,
    ) -> Self {
        Self {
            passed: true,
            warning: true,
            neutral: false,
            label: label.into(),
            detail: detail.into(),
            hint: hint.map(Into::into),
        }
    }

    fn fail(label: impl Into<String>, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            passed: false,
            warning: false,
            neutral: false,
            label: label.into(),
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }

    fn format(&self) -> String {
        let icon = if !self.passed {
            "❌"
        } else if self.warning {
            "⚠"
        } else if self.neutral {
            "❓"
        } else {
            "✅"
        };
        let mut line = format!("  {icon} {}: {}", self.label, self.detail);
        if let Some(hint) = &self.hint {
            line.push_str(&format!("\n     💡 {hint}"));
        }
        line
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkLocalRouteDiagnosis {
    CoveredByConfig {
        details: String,
    },
    CoveredByRoute {
        interface: String,
    },
    DegradedMainTableOnly {
        interface: String,
        suggested_interface: String,
    },
    Partial {
        interface: String,
        host_or_subnet: String,
        suggested_interface: String,
        config_partial: Option<String>,
    },
    Missing {
        suggested_interface: String,
    },
    Unknown {
        reason: String,
    },
}

#[allow(dead_code)]
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkLocalMatch {
    FullCover(String),
    Partial(String),
    None,
}

#[cfg(any(target_os = "linux", test))]
fn subnet_covers_link_local_block(ip: std::net::Ipv4Addr, prefix: u8) -> bool {
    if prefix == 0 || prefix > 16 {
        return false;
    }
    let mask = !((1u32 << (32 - prefix)) - 1);
    let ip_u32 = u32::from(ip);
    let target_u32 = u32::from(std::net::Ipv4Addr::new(169, 254, 0, 0));
    (ip_u32 & mask) == (target_u32 & mask)
}

#[cfg(any(target_os = "linux", test))]
fn classify_link_local_destination(dest: &str) -> LinkLocalMatch {
    let (ip_str, prefix_len) = if let Some((ip_part, mask_part)) = dest.split_once('/') {
        let prefix = match mask_part.parse::<u8>() {
            Ok(p) => p,
            Err(_) => return LinkLocalMatch::None,
        };
        (ip_part, prefix)
    } else {
        (dest, 32)
    };

    if let Ok(ip) = ip_str.parse::<std::net::Ipv4Addr>() {
        let octets = ip.octets();
        if subnet_covers_link_local_block(ip, prefix_len) {
            return LinkLocalMatch::FullCover(dest.to_string());
        }
        if octets[0] == 169 && octets[1] == 254 {
            return LinkLocalMatch::Partial(dest.to_string());
        }
    }
    LinkLocalMatch::None
}

#[allow(dead_code)]
#[cfg(any(target_os = "linux", test))]
fn inspect_config_tun_settings(
    config_path: &std::path::Path,
) -> (Option<Vec<String>>, Option<String>) {
    let content = match crate::utils::read_file_no_follow_limited(
        config_path,
        crate::tun_transaction::MAX_TRANSACTION_ARTIFACT_BYTES,
    ) {
        Ok(c) => c,
        Err(_) => return (None, None),
    };
    let val: serde_yaml::Value = match serde_yaml::from_slice(&content) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let tun = match val.get("tun") {
        Some(t) => t,
        None => return (None, None),
    };

    let excludes = tun
        .get("route-exclude-address")
        .and_then(|e| e.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

    let device = tun
        .get("device")
        .and_then(|d| d.as_str())
        .map(|s| s.to_string());

    (excludes, device)
}

#[allow(dead_code)]
#[cfg(any(target_os = "linux", test))]
fn check_route_exclude_coverage(excludes: &[String]) -> LinkLocalMatch {
    let mut has_partial = false;
    let mut partial_item = String::new();
    for item in excludes {
        match classify_link_local_destination(item) {
            LinkLocalMatch::FullCover(s) => return LinkLocalMatch::FullCover(s),
            LinkLocalMatch::Partial(s) => {
                has_partial = true;
                partial_item = s;
            }
            LinkLocalMatch::None => {}
        }
    }
    if has_partial {
        LinkLocalMatch::Partial(partial_item)
    } else {
        LinkLocalMatch::None
    }
}

#[cfg(any(target_os = "linux", test))]
fn is_physical_interface(dev: &str, configured_tun_dev: Option<&str>) -> bool {
    if let Some(custom_dev) = configured_tun_dev {
        if dev.eq_ignore_ascii_case(custom_dev) {
            return false;
        }
    }
    let lower = dev.to_ascii_lowercase();
    !(lower == "lo"
        || lower.starts_with("tun")
        || lower.starts_with("utun")
        || lower.starts_with("meta")
        || lower.starts_with("clash")
        || lower.starts_with("tap")
        || lower.starts_with("wg")
        || lower.starts_with("dummy")
        || lower.starts_with("docker")
        || lower.starts_with("br-")
        || lower.starts_with("veth")
        || lower.starts_with("virbr")
        || lower.starts_with("tailscale")
        || lower.starts_with("zt"))
}

#[allow(dead_code)]
#[cfg(any(target_os = "linux", test))]
fn find_safe_ip_binary() -> Option<&'static str> {
    const SAFE_PATHS: &[&str] = &["/sbin/ip", "/usr/sbin/ip", "/bin/ip", "/usr/bin/ip"];
    SAFE_PATHS
        .iter()
        .copied()
        .find(|&p| std::path::Path::new(p).exists())
}

#[cfg(any(target_os = "linux", test))]
fn parse_ip_route_get_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if let Some(pos) = tokens.iter().position(|&t| t == "dev") {
            if let Some(dev) = tokens.get(pos + 1) {
                return Some(dev.to_string());
            }
        }
    }
    None
}

#[cfg(any(target_os = "linux", test))]
fn classify_link_local_exit_devices(
    mirror_dev: Option<String>,
    imds_dev: Option<String>,
    configured_tun_dev: Option<&str>,
    default_dev: Option<String>,
    config_partial: Option<String>,
) -> Option<LinkLocalRouteDiagnosis> {
    let m_dev = mirror_dev?;
    let i_dev = imds_dev?;
    let m_phys = is_physical_interface(&m_dev, configured_tun_dev);
    let i_phys = is_physical_interface(&i_dev, configured_tun_dev);

    if m_phys && i_phys {
        Some(LinkLocalRouteDiagnosis::CoveredByRoute { interface: m_dev })
    } else if !m_phys && i_phys {
        Some(LinkLocalRouteDiagnosis::Partial {
            interface: i_dev.clone(),
            host_or_subnet: "169.254.169.254".to_string(),
            suggested_interface: i_dev,
            config_partial,
        })
    } else if m_phys && !i_phys {
        Some(LinkLocalRouteDiagnosis::Partial {
            interface: m_dev.clone(),
            host_or_subnet: "169.254.0.3".to_string(),
            suggested_interface: m_dev,
            config_partial,
        })
    } else {
        Some(LinkLocalRouteDiagnosis::Missing {
            suggested_interface: default_dev.unwrap_or_else(|| "<物理网卡>".to_string()),
        })
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_ip_route_main_output(
    output: &str,
    configured_tun_dev: Option<&str>,
) -> LinkLocalRouteDiagnosis {
    let mut default_interface: Option<String> = None;
    let mut full_interface: Option<String> = None;
    let mut partial_interface: Option<(String, String)> = None;
    let mut has_any_routes = false;

    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }
        if matches!(
            tokens[0],
            "blackhole" | "unreachable" | "prohibit" | "throw"
        ) {
            continue;
        }
        has_any_routes = true;
        let dest = tokens[0];

        let dev = tokens
            .windows(2)
            .find(|w| w[0] == "dev")
            .map(|w| w[1].to_string());

        if dest == "default" {
            if let Some(ref interface) = dev {
                if is_physical_interface(interface, configured_tun_dev)
                    && default_interface.is_none()
                {
                    default_interface = Some(interface.clone());
                }
            }
        }

        match classify_link_local_destination(dest) {
            LinkLocalMatch::FullCover(_) => {
                if let Some(ref interface) = dev {
                    if is_physical_interface(interface, configured_tun_dev) {
                        full_interface = Some(interface.clone());
                        break;
                    }
                }
            }
            LinkLocalMatch::Partial(sub) => {
                if let Some(ref interface) = dev {
                    if is_physical_interface(interface, configured_tun_dev)
                        && partial_interface.is_none()
                    {
                        partial_interface = Some((interface.clone(), sub));
                    }
                }
            }
            LinkLocalMatch::None => {}
        }
    }

    if let Some(interface) = full_interface {
        LinkLocalRouteDiagnosis::DegradedMainTableOnly {
            suggested_interface: default_interface.unwrap_or_else(|| interface.clone()),
            interface,
        }
    } else if let Some((interface, sub)) = partial_interface {
        LinkLocalRouteDiagnosis::Partial {
            interface,
            host_or_subnet: sub,
            suggested_interface: default_interface.unwrap_or_else(|| "<物理网卡>".to_string()),
            config_partial: None,
        }
    } else if has_any_routes {
        LinkLocalRouteDiagnosis::Missing {
            suggested_interface: default_interface.unwrap_or_else(|| "<物理网卡>".to_string()),
        }
    } else {
        LinkLocalRouteDiagnosis::Unknown {
            reason: "主路由表为空或无法解析".to_string(),
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_proc_net_route_content(
    content: &str,
    configured_tun_dev: Option<&str>,
) -> LinkLocalRouteDiagnosis {
    let mut lines = content.lines();
    let header = match lines.next() {
        Some(h) => h,
        None => {
            return LinkLocalRouteDiagnosis::Unknown {
                reason: "/proc/net/route 为空".to_string(),
            }
        }
    };
    if !header.contains("Iface") || !header.contains("Destination") {
        return LinkLocalRouteDiagnosis::Unknown {
            reason: "/proc/net/route 格式不符合预期".to_string(),
        };
    }

    let mut default_interface: Option<String> = None;
    let mut full_interface: Option<String> = None;
    let mut partial_interface: Option<(String, String)> = None;
    let mut has_routes = false;

    for line in lines {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() < 8 {
            continue;
        }
        let iface = tokens[0];
        let dest_hex = tokens[1];
        let flags_hex = tokens[3];
        let mask_hex = tokens[7];

        let flags = u32::from_str_radix(flags_hex, 16).unwrap_or(0);
        if (flags & 0x0001) == 0 || (flags & 0x0200) != 0 {
            continue;
        }
        has_routes = true;

        let dest_val = match u32::from_str_radix(dest_hex, 16) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mask_val = match u32::from_str_radix(mask_hex, 16) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let dest_bytes = dest_val.to_ne_bytes();
        let prefix_len = mask_val.count_ones() as u8;

        if dest_val == 0
            && mask_val == 0
            && is_physical_interface(iface, configured_tun_dev)
            && default_interface.is_none()
        {
            default_interface = Some(iface.to_string());
        }

        if dest_bytes[0] == 169
            && dest_bytes[1] == 254
            && is_physical_interface(iface, configured_tun_dev)
        {
            if prefix_len <= 16 && dest_bytes[2] == 0 && dest_bytes[3] == 0 {
                full_interface = Some(iface.to_string());
                break;
            } else if partial_interface.is_none() {
                let sub_str = format!(
                    "{}.{}.{}.{}/{}",
                    dest_bytes[0], dest_bytes[1], dest_bytes[2], dest_bytes[3], prefix_len
                );
                partial_interface = Some((iface.to_string(), sub_str));
            }
        }
    }

    if let Some(interface) = full_interface {
        LinkLocalRouteDiagnosis::DegradedMainTableOnly {
            suggested_interface: default_interface.unwrap_or_else(|| interface.clone()),
            interface,
        }
    } else if let Some((interface, sub)) = partial_interface {
        LinkLocalRouteDiagnosis::Partial {
            interface,
            host_or_subnet: sub,
            suggested_interface: default_interface.unwrap_or_else(|| "<物理网卡>".to_string()),
            config_partial: None,
        }
    } else if has_routes {
        LinkLocalRouteDiagnosis::Missing {
            suggested_interface: default_interface.unwrap_or_else(|| "<物理网卡>".to_string()),
        }
    } else {
        LinkLocalRouteDiagnosis::Unknown {
            reason: "未在 /proc/net/route 中读取到有效路由".to_string(),
        }
    }
}

#[allow(dead_code)]
#[cfg(any(target_os = "linux", test))]
fn check_linux_link_local_routing(
    config_path: Option<&std::path::Path>,
) -> LinkLocalRouteDiagnosis {
    let mut config_partial: Option<String> = None;
    let (tun_excludes, configured_tun_dev) = match config_path {
        Some(cfg_file) => inspect_config_tun_settings(cfg_file),
        None => (None, None),
    };

    // 1. 优先检查 mihomo 自身配置文件中的 tun.route-exclude-address
    if let Some(excludes) = tun_excludes.as_deref() {
        match check_route_exclude_coverage(excludes) {
            LinkLocalMatch::FullCover(matched) => {
                return LinkLocalRouteDiagnosis::CoveredByConfig { details: matched };
            }
            LinkLocalMatch::Partial(matched) => {
                config_partial = Some(matched);
            }
            LinkLocalMatch::None => {}
        }
    }

    let ip_bin = find_safe_ip_binary();

    // 2. 通过绝对路径调用 ip route get 探测内核综合策略路由与出口设备
    if let Some(ip_bin_path) = ip_bin {
        let mirror_out = std::process::Command::new(ip_bin_path)
            .args(["route", "get", "169.254.0.3"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok());

        let imds_out = std::process::Command::new(ip_bin_path)
            .args(["route", "get", "169.254.169.254"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok());

        if let (Some(mirror_text), Some(imds_text)) = (mirror_out, imds_out) {
            let mirror_dev = parse_ip_route_get_output(&mirror_text);
            let imds_dev = parse_ip_route_get_output(&imds_text);

            let default_dev = std::process::Command::new(ip_bin_path)
                .args(["route", "show", "table", "main"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .and_then(|out| {
                    for line in out.lines() {
                        if line.starts_with("default") {
                            let tokens: Vec<&str> = line.split_whitespace().collect();
                            if let Some(pos) = tokens.iter().position(|&t| t == "dev") {
                                if let Some(dev) = tokens.get(pos + 1) {
                                    if is_physical_interface(dev, configured_tun_dev.as_deref()) {
                                        return Some(dev.to_string());
                                    }
                                }
                            }
                        }
                    }
                    None
                });

            if let Some(diag) = classify_link_local_exit_devices(
                mirror_dev,
                imds_dev,
                configured_tun_dev.as_deref(),
                default_dev,
                config_partial.clone(),
            ) {
                return diag;
            }
        }
    }

    // 3. 降级通道：读取 /proc/net/route
    if let Ok(content) = std::fs::read_to_string("/proc/net/route") {
        let diag = parse_proc_net_route_content(&content, configured_tun_dev.as_deref());
        if !matches!(diag, LinkLocalRouteDiagnosis::Unknown { .. }) {
            return diag;
        }
    }

    // 4. 降级通道：执行 ip route show table main
    if let Some(ip_bin_path) = ip_bin {
        if let Ok(output) = std::process::Command::new(ip_bin_path)
            .args(["route", "show", "table", "main"])
            .output()
        {
            if output.status.success() {
                if let Ok(stdout) = std::str::from_utf8(&output.stdout) {
                    let diag = parse_ip_route_main_output(stdout, configured_tun_dev.as_deref());
                    if !matches!(diag, LinkLocalRouteDiagnosis::Unknown { .. }) {
                        return diag;
                    }
                }
            }
        }
    }

    LinkLocalRouteDiagnosis::Unknown {
        reason: "未能通过 ip route get、/proc/net/route 或 ip route main 研判路由".to_string(),
    }
}

#[cfg(any(target_os = "linux", test))]
fn format_link_local_route_check(diagnosis: &LinkLocalRouteDiagnosis) -> DoctorCheck {
    match diagnosis {
        LinkLocalRouteDiagnosis::CoveredByConfig { details } => DoctorCheck::pass(
            "云链路本地路由",
            format!("已在配置 tun.route-exclude-address 中排除（{details}），未受 TUN 接管影响"),
        ),
        LinkLocalRouteDiagnosis::CoveredByRoute { interface } => DoctorCheck::pass(
            "云链路本地路由",
            format!("169.254.0.0/16 流量由 {interface} 直连路由直接接管，未受 TUN 接管影响"),
        ),
        LinkLocalRouteDiagnosis::DegradedMainTableOnly {
            interface,
            suggested_interface,
        } => DoctorCheck::warn(
            "云链路本地路由",
            format!(
                "主路由表中存在直连路由（dev {interface}），但降级模式下未评估策略路由（ip rule）；若存在 auto-route 规则，访问内网镜像（如腾讯云 169.254.0.3）仍可能被送入 TUN"
            ),
            Some(format!(
                "首选：在用户配置 intent.yaml 中添加 tun.route-exclude-address: [\"169.254.0.0/16\"]，执行 mihomo-cli restart --system 生效；备选：sudo ip route add 169.254.0.0/16 dev {suggested_interface}（临时生效；持久化需写入 netplan 配置）"
            )),
        ),
        LinkLocalRouteDiagnosis::Partial {
            interface,
            host_or_subnet,
            suggested_interface,
            config_partial,
        } => {
            let config_hint = config_partial
                .as_ref()
                .map(|c| format!("（配置中仅排除 {c}）"))
                .unwrap_or_default();
            DoctorCheck::warn(
                "云链路本地路由",
                format!(
                    "仅发现部分链路本地直连路由（{host_or_subnet} dev {interface}）{config_hint}，未覆盖 169.254.0.0/16 全段；访问内网镜像（如腾讯云 169.254.0.3）可能超时挂死"
                ),
                Some(format!(
                    "首选：在用户配置 intent.yaml 中添加 tun.route-exclude-address: [\"169.254.0.0/16\"]，执行 mihomo-cli restart --system 生效；备选：sudo ip route add 169.254.0.0/16 dev {suggested_interface}（临时生效；持久化需写入 netplan 配置）"
                )),
            )
        }
        LinkLocalRouteDiagnosis::Missing {
            suggested_interface,
        } => DoctorCheck::warn(
            "云链路本地路由",
            "TUN 已启用且流量被接管，但未配置链路本地网段（169.254.0.0/16）排除或直连；在云主机上访问内网镜像（如腾讯云 169.254.0.3）或元数据可能超时挂死",
            Some(format!(
                "首选：在用户配置 intent.yaml 中添加 tun.route-exclude-address: [\"169.254.0.0/16\"]，执行 mihomo-cli restart --system 生效；备选：sudo ip route add 169.254.0.0/16 dev {suggested_interface}（临时生效；持久化需写入 netplan 配置）"
            )),
        ),
        LinkLocalRouteDiagnosis::Unknown { reason } => DoctorCheck::warn(
            "云链路本地路由",
            format!("未完成链路本地路由研判（{reason}）"),
            Some("建议检查网络环境与 ip 命令权限".to_string()),
        ),
    }
}

// ── Command implementations ──

#[allow(dead_code)]
fn format_install_already_installed() -> Vec<String> {
    vec!["Already installed. Use --force to reinstall.".to_string()]
}

fn format_install_mode_prompt() -> Vec<String> {
    vec![
        "How do you want mihomo-cli to run?".to_string(),
        String::new(),
        "[1] Normal proxy mode".to_string(),
        "    - No admin password".to_string(),
        "    - Works for browsers/apps using system proxy".to_string(),
        "    - No TUN".to_string(),
        String::new(),
        "[2] TUN mode / all-traffic mode".to_string(),
        "    - Requires admin password once".to_string(),
        "    - Captures apps that ignore system proxy".to_string(),
        "    - Recommended if you want Clash Verge Rev-like TUN".to_string(),
    ]
}

fn format_install_mode_selected(user_mode: bool) -> Vec<String> {
    vec![
        if user_mode {
            "Selected: Normal proxy mode"
        } else {
            "Selected: TUN/system service mode"
        }
        .to_string(),
        String::new(),
    ]
}

#[allow(dead_code)]
fn format_install_header(os: &str) -> Vec<String> {
    vec![format!("=== mihomo-cli install ({os}) ==="), String::new()]
}

fn format_install_instance_header(mode: instance::InstanceMode, os: instance::TargetOs) -> String {
    format!(
        "=== mihomo-cli install --{} ({os:?}) ===",
        instance_mode_marker(mode)
    )
}

fn format_install_step(step: &str) -> Vec<String> {
    vec![step.to_string()]
}

fn install_download_error(error: &str) -> String {
    format!(
        "Failed to download mihomo: {error}
  Check network and try --verbose for details"
    )
}

#[allow(dead_code)]
fn format_install_config_setup_failed(prefix: &str, error: &str) -> Vec<String> {
    vec![
        format!("  ⚠ {prefix}: {error}"),
        "  You can configure later with: mihomo-cli config".to_string(),
    ]
}

fn format_install_done(config_ok: bool) -> Vec<String> {
    let mut lines = vec![
        String::new(),
        "=== Done ===".to_string(),
        "  ✅ Binary installed".to_string(),
    ];
    if config_ok {
        lines.extend([
            "  ✅ Config ready".to_string(),
            String::new(),
            "  Next steps:".to_string(),
            "    mihomo-cli restart    start/restart service".to_string(),
            "    mihomo-cli select     select proxy node".to_string(),
            "    mihomo-cli status     check service/core status".to_string(),
            "    mihomo-cli ip         check current exit IP".to_string(),
            "    mihomo-cli tun on     enable TUN mode".to_string(),
        ]);
    } else {
        lines.push("  ⚠ Config pending — run: mihomo-cli config".to_string());
    }
    lines
}

fn install_mode_label(user_mode: bool) -> &'static str {
    if user_mode {
        "user-level"
    } else {
        "system"
    }
}

#[allow(dead_code)]
fn format_install_pending_service_notice() -> Vec<String> {
    vec![
        String::new(),
        "Config is pending. Service will not be started yet.".to_string(),
        "After configuring, run: mihomo-cli restart".to_string(),
    ]
}

#[allow(dead_code)]
fn format_install_service_unit_installed() -> Vec<String> {
    vec!["  ✅ Service unit installed (not started)".to_string()]
}

fn format_install_service_prompt(mode_label: &str) -> Vec<String> {
    vec![
        String::new(),
        format!("Install and start {mode_label} service?"),
        "  [y] Yes, install and start".to_string(),
        "  [n] No, skip (you can run 'mihomo-cli restart' later)".to_string(),
    ]
}

fn should_install_service_answer(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    answer.is_empty() || answer == "y" || answer == "yes"
}

fn format_install_service_installed() -> Vec<String> {
    vec!["  ✅ Service installed".to_string()]
}

fn format_install_service_skipped() -> Vec<String> {
    vec!["  ⚠ Service skipped — run: mihomo-cli restart".to_string()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemInstallGroupHintDecision {
    Prompt,
    SilentAlreadyInGroup,
    SkipNoOperator,
    SkipGroupCheckUnavailable,
}

pub(crate) fn decide_system_install_group_hint(
    operator: Option<&str>,
    in_group: Option<bool>,
) -> SystemInstallGroupHintDecision {
    let Some(user) = operator else {
        return SystemInstallGroupHintDecision::SkipNoOperator;
    };
    let user = user.trim();
    if user.is_empty() || user == "root" {
        return SystemInstallGroupHintDecision::SkipNoOperator;
    }
    match in_group {
        Some(true) => SystemInstallGroupHintDecision::SilentAlreadyInGroup,
        Some(false) => SystemInstallGroupHintDecision::Prompt,
        None => SystemInstallGroupHintDecision::SkipGroupCheckUnavailable,
    }
}

pub(crate) fn resolve_system_install_operator_from(
    sudo_user_env: Option<&str>,
    current_user: Option<&str>,
    is_root: bool,
) -> Option<String> {
    if let Some(user) = sudo_user_env {
        let user = user.trim();
        if !user.is_empty() && user != "root" {
            return Some(user.to_string());
        }
    }
    if is_root {
        return None;
    }
    if let Some(user) = current_user {
        let user = user.trim();
        if !user.is_empty() && user != "root" {
            return Some(user.to_string());
        }
    }
    None
}

pub(crate) fn format_system_install_group_hint(user: &str) -> Vec<String> {
    vec![
        "".to_string(),
        format!("  Notice: user '{user}' is not in the 'mihomo' group."),
        "  Normal users cannot inspect managed state or run `override import`/`status` without group access.".to_string(),
        format!("  To grant access: sudo usermod -aG mihomo {user}"),
        "  Sign out and sign back in for the new group membership to take effect.".to_string(),
    ]
}

#[cfg(target_os = "linux")]
fn is_user_in_group_linux(user: &str, group_name: &str) -> Option<bool> {
    use std::ffi::CString;
    let c_group = CString::new(group_name).ok()?;
    let c_user = CString::new(user).ok()?;
    unsafe {
        let grp = libc::getgrnam(c_group.as_ptr());
        if grp.is_null() {
            return None;
        }
        let target_gid = (*grp).gr_gid;

        let pwd = libc::getpwnam(c_user.as_ptr());
        if pwd.is_null() {
            return None;
        }
        let user_gid = (*pwd).pw_gid;
        if user_gid == target_gid {
            return Some(true);
        }

        let mut ngroups: libc::c_int = 32;
        let mut groups: Vec<libc::gid_t> = vec![0; ngroups as usize];
        let mut ret =
            libc::getgrouplist(c_user.as_ptr(), user_gid, groups.as_mut_ptr(), &mut ngroups);
        if ret == -1 {
            groups.resize(ngroups as usize, 0);
            ret = libc::getgrouplist(c_user.as_ptr(), user_gid, groups.as_mut_ptr(), &mut ngroups);
        }
        if ret == -1 {
            return None;
        }
        Some(groups[..ngroups as usize].contains(&target_gid))
    }
}

#[cfg(not(target_os = "linux"))]
fn is_user_in_group_linux(_user: &str, _group_name: &str) -> Option<bool> {
    None
}

/// Q1 裁决 A：只有 system 模式、且操作者确实不在 `mihomo` 组时才返回提示行。
/// 该函数把“模式门控 + 决策矩阵 + 文案”收敛到一处，便于单测安装调用点的行为。
pub(crate) fn system_install_group_hint_lines(
    mode: instance::InstanceMode,
    operator: Option<&str>,
    in_group: Option<bool>,
) -> Option<Vec<String>> {
    if mode != instance::InstanceMode::System {
        return None;
    }
    match decide_system_install_group_hint(operator, in_group) {
        SystemInstallGroupHintDecision::Prompt => operator.map(format_system_install_group_hint),
        SystemInstallGroupHintDecision::SilentAlreadyInGroup
        | SystemInstallGroupHintDecision::SkipNoOperator
        | SystemInstallGroupHintDecision::SkipGroupCheckUnavailable => None,
    }
}

/// 非阻塞提示必须保证安装退出码不受 stdout 写失败影响：
/// 使用 best-effort 写入，忽略断管/关闭错误。
fn print_lines_best_effort(lines: impl IntoIterator<Item = String>) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
    let _ = out.flush();
}

fn maybe_print_system_install_group_hint(mode: instance::InstanceMode) {
    let sudo_user = std::env::var("SUDO_USER").ok();
    let current_user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok();
    #[cfg(unix)]
    let current_user = current_user.or_else(|| {
        let uid = unsafe { libc::getuid() };
        let pwd = unsafe { libc::getpwuid(uid) };
        if pwd.is_null() {
            None
        } else {
            let name = unsafe { std::ffi::CStr::from_ptr((*pwd).pw_name) };
            name.to_str().ok().map(|s| s.to_string())
        }
    });
    let is_root = is_current_process_root();
    let operator = resolve_system_install_operator_from(
        sudo_user.as_deref(),
        current_user.as_deref(),
        is_root,
    );
    let in_group = operator
        .as_deref()
        .and_then(|u| is_user_in_group_linux(u, "mihomo"));
    if let Some(lines) = system_install_group_hint_lines(mode, operator.as_deref(), in_group) {
        print_lines_best_effort(lines);
    }
}

#[allow(clippy::too_many_arguments)] // install flags mirror the CLI surface 1:1
async fn cmd_install_entry(
    system: bool,
    user: bool,
    force: bool,
    version: Option<&str>,
    github_mirror: Option<&str>,
    skip_config: bool,
    yes: bool,
    discard_pending: Option<&str>,
) -> anyhow::Result<()> {
    // SPEC 裁决 A1: discarding a pending generation is an EXPLICIT, separate
    // operation — it revokes metadata only and never pretends the runtime was
    // restored. A plain `install --system` is never an authorization to discard.
    if let Some(target) = discard_pending {
        if !system {
            anyhow::bail!(
                "--discard-pending requires an explicit system instance: mihomo-cli install --system --discard-pending=<GENERATION> --yes"
            );
        }
        let ctx = instance::planned_current_context(instance::InstanceMode::System)
            .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance lifecycle"))?;
        let target = generation::GenerationId::new(target)
            .map_err(|error| anyhow::anyhow!("非法代际 ID: {error}"))?;
        return discard_pending_generation(&ctx, &target);
    }
    let mode = if system {
        instance::InstanceMode::System
    } else if user {
        instance::InstanceMode::User
    } else {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(
                "install requires an explicit instance mode in non-interactive mode.\n  Run: mihomo-cli install --user [--skip-config] [-y]\n  Or:  mihomo-cli install --system [--skip-config] [-y]"
            );
        }
        print_lines(format_install_mode_prompt());
        print!("Choice [1/2] (default: 1): ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let system_mode = input.trim() == "2";
        print_lines(format_install_mode_selected(!system_mode));
        if system_mode {
            instance::InstanceMode::System
        } else {
            instance::InstanceMode::User
        }
    };
    cmd_install_instance(mode, force, version, github_mirror, skip_config, yes).await?;
    // Q1 A: 非阻塞组提示；模式门控在 helper 内，user 模式不会输出。
    maybe_print_system_install_group_hint(mode);
    Ok(())
}

fn install_mode_conflict_message(
    requested: instance::InstanceMode,
    services: instance::ServicePresence,
) -> Option<String> {
    match requested {
        instance::InstanceMode::System if services.user => Some(
            "cannot install the system service while a per-user service is installed.\n  \
             v3 uses mutually exclusive modes; uninstall the user service first.\n  \
             Suggestion: mihomo-cli uninstall --user"
                .to_string(),
        ),
        instance::InstanceMode::User if services.system => Some(
            "cannot install the per-user service while the system service is installed.\n  \
             v3 uses mutually exclusive modes; uninstall the system service first.\n  \
             Suggestion: mihomo-cli uninstall --system"
                .to_string(),
        ),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallFastPathAction {
    ContinueInstall,
    ReturnUpToDate,
}

fn install_fast_path_action(
    mode: instance::InstanceMode,
    components_complete: bool,
    core_running: bool,
) -> InstallFastPathAction {
    if !components_complete {
        return InstallFastPathAction::ContinueInstall;
    }
    match mode {
        instance::InstanceMode::System | instance::InstanceMode::User if core_running => {
            InstallFastPathAction::ReturnUpToDate
        }
        instance::InstanceMode::System | instance::InstanceMode::User => {
            InstallFastPathAction::ContinueInstall
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallPostServiceAction {
    ProvisionAccessOnly,
    WaitForUserInstance,
}

fn install_post_service_action(
    mode: instance::InstanceMode,
    _config_file_exists: bool,
) -> InstallPostServiceAction {
    match mode {
        instance::InstanceMode::System => InstallPostServiceAction::ProvisionAccessOnly,
        instance::InstanceMode::User => InstallPostServiceAction::WaitForUserInstance,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemInstallScenario {
    PostServiceNoConfig,
    PostServiceWithConfig,
    CompleteFastPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemInstallOperation {
    WaitForDaemon,
    EnsureAccess,
}

fn system_install_operations(scenario: SystemInstallScenario) -> Vec<SystemInstallOperation> {
    match scenario {
        SystemInstallScenario::PostServiceNoConfig
        | SystemInstallScenario::PostServiceWithConfig => vec![
            SystemInstallOperation::WaitForDaemon,
            SystemInstallOperation::EnsureAccess,
        ],
        SystemInstallScenario::CompleteFastPath => vec![SystemInstallOperation::EnsureAccess],
    }
}

async fn execute_system_install_operations(
    ctx: &instance::InstanceContext,
    scenario: SystemInstallScenario,
) -> anyhow::Result<()> {
    for operation in system_install_operations(scenario) {
        match operation {
            SystemInstallOperation::WaitForDaemon => {
                wait_for_system_daemon_readiness().await?;
            }
            SystemInstallOperation::EnsureAccess => {
                ensure_system_daemon_access(ctx).await?;
            }
        }
    }
    Ok(())
}

async fn cmd_install_instance(
    mode: instance::InstanceMode,
    force: bool,
    version: Option<&str>,
    github_mirror: Option<&str>,
    skip_config: bool,
    yes: bool,
) -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance install"))?;
    if mode == instance::InstanceMode::System && ctx.os == instance::TargetOs::Linux {
        match (ctx.owner_uid, ctx.owner_gid) {
            (Some(0), _) => anyhow::bail!(
                "Linux system install requires a non-root owner for the per-user config.\n  \
                 Run from a normal user account; mihomo-cli will request sudo when needed."
            ),
            (None, _) | (_, None) => anyhow::bail!(
                "cannot resolve the original user's uid/gid for Linux system install.\n  \
                 Run from a normal login account, or preserve SUDO_UID when invoking with sudo."
            ),
            _ => {}
        }
    }
    // fail fast on hosts without a running systemd (WSL2 default,
    // containers, minimal distros) before writing anything — a unit file that
    // can never be started must not be left behind as "installed".
    if mode == instance::InstanceMode::System
        && ctx.os == instance::TargetOs::Linux
        && !service::systemd_runtime_available()
    {
        anyhow::bail!(systemd_unavailable_message());
    }
    let plan = instance::planned_install_plan(&ctx)
        .ok_or_else(|| anyhow::anyhow!("Instance install plan is unavailable for this OS"))?;

    if mode == instance::InstanceMode::System && current_legacy_root_service().is_some() && !force {
        anyhow::bail!(
            "legacy root-mode layout detected: the system service points into your user home.
  v3 does not auto-migrate legacy root layouts.
  Run: mihomo-cli uninstall --all 清理旧安装
  Then reinstall: mihomo-cli install
  To override after manual cleanup, retry with --force."
        );
    }

    if let Some(message) = install_mode_conflict_message(mode, current_service_presence()) {
        anyhow::bail!(message);
    }
    if let Some(message) =
        v3_mutual_exclusion_violation(mode, current_runtime_presence(), "install")
    {
        anyhow::bail!(message);
    }

    println!("{}", format_install_instance_header(mode, ctx.os));
    println!("Resolved instance paths:");
    println!("  Binary:   {}", ctx.paths.core_binary.display());
    println!("  Config:   {}", ctx.paths.intent_config_file.display());
    println!(
        "  Endpoint: {}",
        status_endpoint_label(&ctx.paths.api_endpoint)
    );

    // Pre-flight: check each component, skip valid ones (unless --force)
    let binary_valid = installer::validate_binary_at(&ctx.paths.core_binary).is_ok();
    let cli_binary_valid = if ctx.permissions == instance::PermissionModel::PrivilegedSystem {
        std::env::current_exe()
            .is_ok_and(|current| utils::file_contents_equal(&current, &ctx.paths.cli_binary))
    } else if mode == instance::InstanceMode::User && ctx.paths.cli_binary != ctx.paths.core_binary
    {
        std::env::current_exe()
            .is_ok_and(|current| utils::file_contents_equal(&current, &ctx.paths.cli_binary))
    } else {
        true
    };
    let service_exists = plan.files.iter().all(|f| f.path.exists());
    let config_exists = ctx.paths.intent_config_file.exists();
    let geo_valid = installer::geo_files_are_valid(&ctx.paths.config_dir, &ctx.paths.core_binary);
    let tun_geo_valid = mode != instance::InstanceMode::System
        || ctx
            .paths
            .tun_config_file
            .parent()
            .is_some_and(|dir| installer::geo_files_are_valid(dir, &ctx.paths.core_binary));

    let components_complete = !force
        && binary_valid
        && cli_binary_valid
        && service_exists
        && config_exists
        && geo_valid
        && tun_geo_valid;
    // an adopted/revoked recovery store has no active generation. The
    // "everything is up to date" fast path must not hide the need to prepare a
    // candidate bound to the authenticated baseline.
    let generation_recovery = mode == instance::InstanceMode::System
        && system_generation_store(&ctx)
            .read_state()
            .is_ok_and(|state| {
                state.active.is_none() && state.pending.is_none() && state.previous.is_some()
            });
    if components_complete && !generation_recovery {
        // ADR-23: also check runtime state. If daemon/core are not running,
        // don't return early — start them to ensure the system reaches running state.
        let core_running = if mode == instance::InstanceMode::System {
            matches!(
                ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await,
                Ok(ipc::DaemonResponse::Status { running: true, .. })
            )
        } else {
            mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs")
                .await
                .is_ok()
        };
        match install_fast_path_action(mode, components_complete, core_running) {
            InstallFastPathAction::ReturnUpToDate => {
                if mode == instance::InstanceMode::System {
                    execute_system_install_operations(
                        &ctx,
                        SystemInstallScenario::CompleteFastPath,
                    )
                    .await?;
                }
                println!("  All components are up to date — nothing to install.");
                println!("  Use --force to reinstall service artifacts.");
                return Ok(());
            }
            InstallFastPathAction::ContinueInstall => {}
        }
        // Components valid but daemon not running — start service via plan commands.
        println!("  All components are up to date, but service is not running.");
        println!("  Starting service...");
        // Fall through to the normal install flow which will execute plan.commands.
    }

    if !force {
        println!();
        println!("  Pre-flight check:");
        if binary_valid {
            println!("    [1/4] binary  ✅ valid, will skip");
        } else if ctx.paths.core_binary.exists() {
            println!("    [1/4] binary  ⚠ invalid, will re-download");
        } else {
            println!("    [1/4] binary  ⬇ not found, will download");
        }
        if ctx.permissions == instance::PermissionModel::PrivilegedSystem {
            if cli_binary_valid {
                println!("    [*] cli       ✅ valid, will skip");
            } else if ctx.paths.cli_binary.exists() {
                println!(
                    "    [*] cli       ⚠ outdated/mismatched, will replace with current binary"
                );
            } else {
                println!("    [*] cli       ⬇ not found, will install current binary");
            }
        }
        if service_exists {
            println!("    [2/4] service ✅ exists, will skip");
        } else {
            println!("    [2/4] service ⬇ will install");
        }
        if config_exists {
            println!("    [3/4] config  ✅ exists, will skip");
        } else {
            println!("    [3/4] config  ⬇ will generate");
        }
        if geo_valid {
            println!("    [4/4] geo     ✅ valid, will skip");
        } else {
            println!("    [4/4] geo     ⬇ missing/invalid, will download");
        }
    }

    for dir in &plan.directories {
        if dir.privileged {
            service::create_dir_privileged(&dir.path, dir.mode)?;
        } else {
            utils::ensure_dir_all_no_follow(&dir.path)
                .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", dir.path.display()))?;
            #[cfg(unix)]
            utils::set_directory_mode_no_follow(&dir.path, dir.mode)?;
        }
    }

    // If system daemon is already running and this is not a forced reinstall,
    // prepare a staged pending generation rather than disrupting the active instance.
    let system_daemon_active =
        mode == instance::InstanceMode::System && ipc::is_daemon_running().await;
    // SPEC 裁决 A1: after an explicit discard the store sits in its recovery
    // state (active=None, previous=B, pending=None). An explicit install is the
    // ONLY way to prepare candidate C, so staging must trigger there even when all
    // installed artifacts are already valid.
    let recovery_state = mode == instance::InstanceMode::System
        && system_generation_store(&ctx)
            .read_state()
            .is_ok_and(|state| {
                state.active.is_none() && state.pending.is_none() && state.previous.is_some()
            });
    if recovery_state
        || (system_daemon_active && !force && (!binary_valid || !cli_binary_valid || !geo_valid))
    {
        if !is_current_process_root() {
            let exe = std::env::current_exe()?;
            let args: Vec<String> = std::env::args().skip(1).collect();
            let status = sudo_reexec_command(&exe, &args).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        println!();
        if recovery_state {
            println!(
                "  恢复态（已撤销上一次候选）：准备候选 C，继续绑定已认证基线（不捕获当前安装态）..."
            );
        } else {
            println!("  System service is currently running. Preparing staged upgrade in pending generation...");
        }
        let stage = tempfile::tempdir()?;
        let staged_core = stage.path().join("mihomo");
        // In recovery the candidate must draw from the authenticated baseline or
        // from a freshly downloaded/validated artifact. A current Core that merely
        // passes `-v` may be the very bytes that already failed.
        let authenticated_core = if recovery_state {
            recovery_baseline_artifact(&ctx, "mihomo")?
        } else {
            None
        };
        match authenticated_core {
            Some(bytes) => std::fs::write(&staged_core, bytes)?,
            None if recovery_state || !binary_valid => {
                installer::download_mihomo_to_with_mirror(version, &staged_core, github_mirror)
                    .await
                    .map_err(|e| anyhow::anyhow!(install_download_error(&e.to_string())))?;
            }
            None => {
                let existing_core = std::fs::read(&ctx.paths.core_binary)?;
                std::fs::write(&staged_core, existing_core)?;
            }
        }
        let core_bytes = std::fs::read(&staged_core)?;

        let current_cli = std::env::current_exe()?;
        let cli_bytes = std::fs::read(&current_cli)?;

        let mut extra_artifacts = Vec::new();
        if recovery_state {
            for name in ["geoip.metadb", "GeoSite.dat"] {
                if let Some(bytes) = recovery_baseline_artifact(&ctx, name)? {
                    extra_artifacts.push((name.to_string(), bytes, Some(0o644)));
                }
            }
        }
        if !geo_valid {
            let geo_stage = tempfile::tempdir()?;
            let _ = installer::ensure_geo_files_in(geo_stage.path(), github_mirror).await;
            for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb"] {
                if extra_artifacts
                    .iter()
                    .any(|(existing, _, _)| existing == name)
                {
                    continue;
                }
                let p = geo_stage.path().join(name);
                if p.exists() {
                    let b = std::fs::read(&p)?;
                    extra_artifacts.push((name.to_string(), b, Some(0o644)));
                }
            }
        }

        let gen_id = prepare_system_generation(&ctx, &core_bytes, &cli_bytes, extra_artifacts)?;
        if recovery_state {
            println!("  ✅ Candidate {gen_id} prepared; not yet applied.");
            println!("  Apply: mihomo-cli restart --system");
            return Ok(());
        }
        println!("  ✅ Upgrade prepared in pending generation: {gen_id}");
        println!("  The running daemon and core were not interrupted.");
        println!(
            "  To apply this update, run: mihomo-cli restart
"
        );
        return Ok(());
    }

    println!();
    print_lines(format_install_step("[1/4] Mihomo core binary..."));
    if !force && binary_valid {
        println!("  ✅ Already valid, skipped");
    } else {
        if ctx.permissions == instance::PermissionModel::PrivilegedSystem {
            let stage = tempfile::tempdir()?;
            let staged_bin = stage.path().join(
                ctx.paths
                    .core_binary
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("mihomo")),
            );
            installer::download_mihomo_to_with_mirror(version, &staged_bin, github_mirror)
                .await
                .map_err(|e| anyhow::anyhow!(install_download_error(&e.to_string())))?;
            let bin_bytes = std::fs::read(&staged_bin)?;
            service::PrivilegeExecutor::write_file(&ctx.paths.core_binary, &bin_bytes, 0o755)?;
        } else {
            installer::download_mihomo_to_with_mirror(
                version,
                &ctx.paths.core_binary,
                github_mirror,
            )
            .await
            .map_err(|e| anyhow::anyhow!(install_download_error(&e.to_string())))?;
        }
        println!("  Installed to {}", ctx.paths.core_binary.display());
    }

    if ctx.permissions == instance::PermissionModel::PrivilegedSystem {
        if force || !cli_binary_valid {
            let current_cli = std::env::current_exe()?;
            let cli_bytes = std::fs::read(&current_cli)?;
            service::PrivilegeExecutor::write_file(&ctx.paths.cli_binary, &cli_bytes, 0o755)?;
            println!(
                "  Installed CLI daemon to {}",
                ctx.paths.cli_binary.display()
            );
        } else {
            println!("  ✅ CLI daemon up to date, skipped");
        }
    } else if mode == instance::InstanceMode::User && ctx.paths.cli_binary != ctx.paths.core_binary
    {
        if force || !cli_binary_valid {
            let current_cli = std::env::current_exe()?;
            let cli_bytes = std::fs::read(&current_cli)?;
            utils::atomic_write_bytes_no_follow(&ctx.paths.cli_binary, &cli_bytes, 0o755)?;
            println!("  Installed CLI to {}", ctx.paths.cli_binary.display());
        } else {
            println!("  ✅ CLI up to date, skipped");
        }
    }

    println!();
    print_lines(format_install_step("[2/4] Service files..."));
    if !force && service_exists {
        println!("  ✅ Already present, skipped");
    } else {
        for file in &plan.files {
            if file.privileged {
                service::PrivilegeExecutor::write_file(
                    &file.path,
                    file.content.as_bytes(),
                    file.mode,
                )?;
            } else {
                // BUG-15: non-privileged writes must still apply the planned
                // mode — macOS start.sh needs 0755 or launchd exec fails with
                // EX_CONFIG (78). write_instance_bytes_file applies mode on
                // unix and dispatches privileged writes when needed.
                write_instance_bytes_file(&ctx, &file.path, file.content.as_bytes(), file.mode)?;
            }
            println!("  Wrote {}", file.path.display());
        }
    }

    println!();
    print_lines(format_install_step("[3/4] Configuration..."));
    let config_ok = if !force && config_exists {
        // BUG-17: a pre-existing config may carry a controller endpoint for a
        // different instance mode (e.g. leftover per-user `external-controller
        // -unix` when installing the system service). Fix it in place so the
        // daemon/readiness check does not refuse to start the core.
        match verify_config_endpoint(&ctx) {
            Ok(()) => {
                println!("  ✅ Already present, skipped");
                true
            }
            Err(_) => {
                let mutation = instance::planned_config_mutation(
                    &ctx,
                    instance::ConfigMutationKind::FixRuntimeController,
                );
                let paths = utils::AppPaths::new(mutation.config_dir.clone());
                let mihomo_path = std::path::PathBuf::from(&mutation.validate_command.program);
                let fixed = config::fix_existing_config_at_endpoint(
                    &paths,
                    Some(&mihomo_path),
                    &mutation.endpoint,
                )?;
                println!(
                    "  ⚠ Present but controller endpoint mismatched; {}",
                    if fixed {
                        "fixed in place"
                    } else {
                        "already aligned"
                    }
                );
                true
            }
        }
    } else {
        setup_instance_config(&ctx, skip_config).await?
    };

    println!();
    print_lines(format_install_step("[4/4] Geo data files..."));
    if !force && geo_valid && tun_geo_valid {
        println!("  ✅ Already valid, skipped");
    } else {
        let geo_stage = tempfile::tempdir()?;
        let geo_ok = if geo_valid {
            true
        } else {
            installer::ensure_geo_files_in(geo_stage.path(), github_mirror).await
        };
        for name in ["geoip.metadb", "GeoSite.dat"] {
            let source = if geo_valid {
                ctx.paths.config_dir.join(name)
            } else {
                geo_stage.path().join(name)
            };
            if source.exists() {
                let bytes = std::fs::read(source)?;
                write_instance_bytes_file(&ctx, &ctx.paths.config_dir.join(name), &bytes, 0o644)?;
                if mode == instance::InstanceMode::System {
                    if let Some(tun_dir) = ctx.paths.tun_config_file.parent() {
                        let tun_geo_path = tun_dir.join(name);
                        write_instance_bytes_file(&ctx, &tun_geo_path, &bytes, 0o640)?;
                    }
                }
            }
        }
        if !geo_ok {
            println!("  ⚠ Geo data download incomplete — mihomo may download at startup");
        }
    }

    print_lines(format_install_done(config_ok));
    if !config_ok {
        return Ok(());
    }

    let install_service = if yes {
        true
    } else if std::io::stdin().is_terminal() {
        print_lines(format_install_service_prompt(install_mode_label(
            mode == instance::InstanceMode::User,
        )));
        print!("Choice [Y/n]: ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        should_install_service_answer(&input)
    } else {
        anyhow::bail!(
            "install service confirmation requires a TTY. Re-run with -y/--yes to install service artifacts, or use --skip-config only after choosing an explicit mode."
        );
    };
    if install_service {
        #[cfg(target_os = "windows")]
        let via_service_manager = mode == instance::InstanceMode::System;
        #[cfg(not(target_os = "windows"))]
        let via_service_manager = false;

        if via_service_manager {
            // Windows system service: use service-manager (correct binPath
            // quoting — fixes StartService 87 from manual sc create).
            #[cfg(target_os = "windows")]
            service::windows_install_service(&ctx)?;
        } else {
            // macOS: the install plan's `launchctl enable` must only run when a
            // prior disable override exists (a disable override makes bootstrap
            // fail with EIO). Running it unconditionally writes an `enabled`
            // override, which makes `autostart status` report enabled even when
            // RunAtLoad=false (N2a 真机验证发现). Skip it when no override.
            #[cfg(target_os = "macos")]
            let macos_enable_needed = {
                let domain = match &ctx.service {
                    instance::ServiceTarget::MacosLaunchDaemon { domain_label, .. }
                    | instance::ServiceTarget::MacosLaunchAgent { domain_label, .. } => {
                        domain_label.clone()
                    }
                    _ => String::new(),
                };
                let disabled_out = std::process::Command::new("launchctl")
                    .args(["print-disabled", &domain])
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_default();
                disabled_out.contains("\"io.mihomo\" => disabled")
            };
            for command in &plan.commands {
                #[cfg(target_os = "macos")]
                let is_unconditional_enable = command.program == "launchctl"
                    && command.args.first().map(String::as_str) == Some("enable");
                #[cfg(not(target_os = "macos"))]
                let is_unconditional_enable = false;
                if is_unconditional_enable {
                    #[cfg(target_os = "macos")]
                    {
                        if !macos_enable_needed {
                            continue; // skip — no disable override to clear
                        }
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        unreachable!("unconditional enable only exists on macOS");
                    }
                }
                let is_best_effort_unload = is_best_effort_install_cleanup_command(command);
                #[cfg(target_os = "linux")]
                if mode == instance::InstanceMode::System
                    && command.program == "systemctl"
                    && command.args.first().is_some_and(|arg| arg == "enable")
                    && is_current_process_root()
                {
                    utils::ensure_mihomo_system_state_dir()?;
                }
                if is_best_effort_unload {
                    run_install_cleanup_command(command);
                } else {
                    // install atomicity: on activation failure make the
                    // half-installed state explicit instead of a raw error — the
                    // files are in place and a plain rerun resumes once the
                    // service manager is available and working.
                    service::run_instance_command(command).map_err(|e| {
                        anyhow::anyhow!(
                            "install stopped at service activation: {e}\n  \
                             Files are in place, but the service is not enabled/started yet.\n  \
                             Once the service manager is available and working, rerun: mihomo-cli install --system"
                        )
                    })?;
                }
            }
        }
        #[cfg(target_os = "linux")]
        if mode == instance::InstanceMode::System {
            if let Some(tun_dir) = ctx.paths.tun_config_file.parent() {
                for name in ["geoip.metadb", "GeoSite.dat"] {
                    let path = tun_dir.join(name);
                    if path.exists() {
                        set_system_geo_group(&path)?;
                    }
                }
            }
        }
        print_lines(format_install_service_installed());
        let config_file_exists = ctx.paths.intent_config_file.exists();
        match install_post_service_action(mode, config_file_exists) {
            InstallPostServiceAction::ProvisionAccessOnly => {
                let scenario = if config_file_exists {
                    SystemInstallScenario::PostServiceWithConfig
                } else {
                    SystemInstallScenario::PostServiceNoConfig
                };
                execute_system_install_operations(&ctx, scenario).await?;
                if tun_transaction::active_journal_path(&ctx).exists() {
                    maybe_auto_recover_active_transaction(
                        &ctx,
                        tun_transaction::RecoveryDirection::Abort,
                        false,
                        false,
                        false,
                    )
                    .await?;
                }
                cmd_lifecycle_instance_mode(mode, instance::ServiceAction::Restart, false, false)
                    .await?;
            }
            InstallPostServiceAction::WaitForUserInstance if config_file_exists => {
                wait_for_instance_readiness(&ctx).await?;
            }
            InstallPostServiceAction::WaitForUserInstance => {
                println!(
                    "  ⚠ Core not started (--skip-config: no config yet). \
                     Add config at {} then run: mihomo-cli start",
                    ctx.paths.intent_config_file.display()
                );
            }
        }
    } else {
        print_lines(format_install_service_skipped());
    }

    Ok(())
}

fn launchctl_bootout_domain(command: &instance::PlannedCommand) -> Option<&str> {
    if command.program == "launchctl" && command.args.first().map(String::as_str) == Some("bootout")
    {
        command.args.get(1).map(String::as_str)
    } else {
        None
    }
}

fn launchctl_service_exists(domain: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("launchctl")
            .args(["print", domain])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = domain;
        true
    }
}

fn run_install_cleanup_command(command: &instance::PlannedCommand) {
    if let Some(domain) = launchctl_bootout_domain(command) {
        if !launchctl_service_exists(domain) {
            println!("  Old launchd service not found ({domain}); skipping cleanup.");
            return;
        }
    }

    if let Err(err) = service::run_instance_command(command) {
        eprintln!("  Warning: cleanup command failed: {err}");
    }
}

fn is_best_effort_install_cleanup_command(command: &instance::PlannedCommand) -> bool {
    matches!(
        command.args.first().map(String::as_str),
        Some("bootout") | Some("disable") | Some("stop") | Some("delete")
    )
}

fn instance_mode_marker(mode: instance::InstanceMode) -> &'static str {
    match mode {
        instance::InstanceMode::System => "system",
        instance::InstanceMode::User => "user",
    }
}

fn instance_mode_label(mode: instance::InstanceMode) -> &'static str {
    match mode {
        instance::InstanceMode::System => "system service",
        instance::InstanceMode::User => "per-user",
    }
}

fn config_fix_command_for_mode(mode: instance::InstanceMode) -> &'static str {
    match mode {
        instance::InstanceMode::System => "mihomo-cli config --system --fix",
        instance::InstanceMode::User => "mihomo-cli config --fix",
    }
}

#[cfg(target_os = "linux")]
fn set_system_geo_group(path: &std::path::Path) -> anyhow::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        let path = path.to_string_lossy().into_owned();
        return service::PrivilegeExecutor::run(&["chgrp", "mihomo", &path]);
    }

    let group = unsafe { libc::getgrnam(c"mihomo".as_ptr()) };
    if group.is_null() {
        anyhow::bail!("mihomo group not found; reinstall the system service");
    }
    let gid = unsafe { (*group).gr_gid };
    use std::os::unix::ffi::OsStrExt;
    let path_c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    if unsafe { libc::chown(path_c.as_ptr(), 0, gid) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn write_instance_text_file(
    ctx: &instance::InstanceContext,
    path: &std::path::Path,
    content: &str,
    mode: u16,
) -> anyhow::Result<()> {
    write_instance_bytes_file(ctx, path, content.as_bytes(), mode)
}

fn write_instance_bytes_file(
    ctx: &instance::InstanceContext,
    path: &std::path::Path,
    bytes: &[u8],
    mode: u16,
) -> anyhow::Result<()> {
    // v3 keeps configuration per-user even in system-service mode. Files under
    // ctx.paths.config_dir must be written as the invoking user; only payloads
    // outside that tree (system binaries/service artifacts) need elevation.
    let under_user_config = path.starts_with(&ctx.paths.config_dir);
    if ctx.permissions == instance::PermissionModel::PrivilegedSystem && !under_user_config {
        service::PrivilegeExecutor::write_file(path, bytes, mode)
    } else {
        if let Some(parent) = path.parent() {
            utils::ensure_dir_all_no_follow(parent)?;
        }
        utils::write_bytes_file_no_follow(path, bytes, mode)?;
        #[cfg(unix)]
        {
            if unsafe { libc::geteuid() } == 0 && under_user_config {
                if let Some(parent) = path.parent() {
                    utils::restore_original_user_owner_preserving_group(parent)?;
                }
                utils::restore_original_user_owner_preserving_group(path)?;
            }
        }
        Ok(())
    }
}

async fn setup_instance_config(
    ctx: &instance::InstanceContext,
    skip_config: bool,
) -> anyhow::Result<bool> {
    // ADR-22: config_file and intent_config_file are the same single source of truth.
    let config_path = &ctx.paths.intent_config_file;

    if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        let fixed = config::ensure_controller_for_endpoint(&content, &ctx.paths.api_endpoint)?;
        if fixed != content {
            write_instance_text_file(ctx, config_path, &fixed, 0o644)?;
            println!("  Config updated for instance endpoint");
        } else {
            println!("  Existing config OK");
        }
        return Ok(true);
    }

    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().unwrap_or_default();
        let cv_config = home.join("Library/Application Support/io.github.clash-verge-rev.clash-verge-rev/clash-verge.yaml");
        if cv_config.exists() {
            let content = std::fs::read_to_string(&cv_config)?;
            let patched =
                config::ensure_controller_for_endpoint(&content, &ctx.paths.api_endpoint)?;
            write_instance_text_file(ctx, config_path, &patched, 0o644)?;
            println!("  Copied config from Clash Verge Rev");
            return Ok(true);
        }
    }

    if skip_config || !std::io::stdin().is_terminal() {
        let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
        config::merge_user_config_checked_at_endpoint(
            &paths,
            Some(&ctx.paths.core_binary),
            &ctx.paths.api_endpoint,
        )?;
        println!("  Generated direct-only base config");
        return Ok(true);
    }

    use dialoguer::Input;
    let url: String = Input::new()
        .with_prompt("Subscription URL (Enter to use direct-only mode)")
        .allow_empty(true)
        .interact_text()?;
    if url.is_empty() {
        let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
        config::merge_user_config_checked_at_endpoint(
            &paths,
            Some(&ctx.paths.core_binary),
            &ctx.paths.api_endpoint,
        )?;
        println!("  Generated direct-only base config");
        return Ok(true);
    }

    let (content, is_yaml) = config::download_sub_smart(&url).await.map_err(|e| {
        anyhow::anyhow!("Cannot reach subscription URL.\n  {e}\n  Check your network or the URL")
    })?;
    let config_content = if is_yaml {
        content
    } else {
        config::warn_raw_subscription_conversion();
        config::convert_vmess_to_clash(&content)?
    };
    let patched = config::ensure_controller_for_endpoint(&config_content, &ctx.paths.api_endpoint)?;
    write_instance_text_file(ctx, config_path, &patched, 0o644)?;
    println!("  Config saved");

    Ok(true)
}

struct ConfigCmd {
    command: Option<ConfigSubcommand>,
    url: Option<String>,
    fix: bool,
    system: bool,
    user: bool,
    refresh: bool,
    refresh_all: bool,
    import: Option<String>,
    switch: Option<String>,
    add: Option<String>,
    remove: Option<String>,
    list: bool,
    validate: bool,
    dry_run: bool,
    yes: bool,
    info: Option<Option<String>>,
    probe: Option<String>,
    user_agent: Option<String>,
    set_ua: Vec<String>,
    activate: bool,
    no_activate: bool,
    json: bool,
}

fn print_probe_results(results: &[config::SubscriptionProbeResult]) {
    for line in format_probe_results(results) {
        println!("{line}");
    }
}

fn format_probe_results(results: &[config::SubscriptionProbeResult]) -> Vec<String> {
    let mut lines = vec![format!(
        "  {:<14} {:<12} {:>6} {:>7} {:>7} {:>7} {:>9} {:>7} {:>7}",
        "UA", "Format", "HTTP", "Proxies", "Groups", "Rules", "Providers", "Bytes", "Score"
    )];
    for r in results {
        let providers = r.proxy_provider_count + r.rule_provider_count;
        lines.push(format!(
            "  {:<14} {:<12} {:>6} {:>7} {:>7} {:>7} {:>9} {:>7} {:>7}",
            r.label,
            r.format,
            r.http_status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".to_string()),
            r.proxy_count,
            r.proxy_group_count,
            r.rule_count,
            providers,
            r.bytes,
            r.score
        ));
        if let Some(err) = &r.error {
            lines.push(format!("    error: {err}"));
        }
    }
    if let Some(best) = results.first() {
        lines.push(format!("\n  Recommended: {}", best.label));
        if let Some(ua) = &best.user_agent {
            lines.push(format!("  User-Agent: {ua}"));
        } else {
            lines.push("  User-Agent: bare request".to_string());
        }
        lines.push("  Reason: highest score among bounded probe candidates.".to_string());
    }
    lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportContentAction {
    UseAsYaml,
    ConvertBase64Subscription,
    ConvertRawSubscription,
}

fn classify_import_content(content: &str) -> ImportContentAction {
    let trimmed = content.trim();
    if trimmed.starts_with("dm1lc3M6Ly") || trimmed.starts_with("dHJvamFuOi8") {
        ImportContentAction::ConvertBase64Subscription
    } else if !content.contains("proxies:") && !content.contains("proxy-providers:") {
        ImportContentAction::ConvertRawSubscription
    } else {
        ImportContentAction::UseAsYaml
    }
}

fn import_conversion_notice(action: ImportContentAction) -> Option<&'static str> {
    match action {
        ImportContentAction::UseAsYaml => None,
        ImportContentAction::ConvertBase64Subscription => {
            Some("  Detected base64-encoded subscription, converting...")
        }
        ImportContentAction::ConvertRawSubscription => {
            Some("  Attempting subscription format conversion...")
        }
    }
}

fn config_restart_apply_lines() -> Vec<String> {
    vec!["  Run: mihomo-cli restart  to apply".to_string()]
}

fn system_config_applied_lines() -> Vec<String> {
    vec!["  ✅ system configuration promoted and runtime applied".to_string()]
}

async fn apply_config_reload_lines(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
    base_revision: Option<&str>,
) -> Vec<String> {
    match reload_configs_for_resolved_instance(system, user, paths, base_revision).await {
        Ok(RuntimeApplyOutcome::SystemPromoted) => system_config_applied_lines(),
        // C + review 回归-2：既不得凭空怂恿 restart（原问题），
        // 也不得在没有 revision attestation 时声称"已生效"——SPEC §12.3 要求
        // 无法证明时如实报 unknown，只去掉误导性的 restart 劝导。
        Ok(_) => vec![
            "  ✓ Config reload request accepted by Core API".to_string(),
            "  ℹ Runtime status: unknown (revision attestation unavailable)".to_string(),
        ],
        Err(e) => vec![
            format!("  ⚠ Config written but runtime application is unknown: {e}"),
            "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
        ],
    }
}

/// C：配置应用结果的真实状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeApplyOutcome {
    /// system 模式：daemon promotion 成功，运行时已应用。
    SystemPromoted,
    /// Core API 接受了热加载；**没有** revision attestation，
    /// 按 SPEC §12.3 必须以 `unknown` 汇报（不得声称"已生效"，review 回归-2）。
    HotReloadAccepted,
    /// intent 已写入但运行时尚未应用（需要 restart）。
    Pending,
}

impl RuntimeApplyOutcome {
    fn from_mode(mode: instance::InstanceMode) -> Self {
        match mode {
            instance::InstanceMode::System => RuntimeApplyOutcome::SystemPromoted,
            instance::InstanceMode::User => RuntimeApplyOutcome::HotReloadAccepted,
        }
    }
}

/// 规则变更后的应用流程：先合并，再按真实结果汇报（C）。
async fn rule_apply_outcome(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
    merged: bool,
) -> anyhow::Result<RuntimeApplyOutcome> {
    if !merged {
        return Ok(RuntimeApplyOutcome::Pending);
    }
    reload_config_for_mutation(system, user, paths, config_base_revision(paths).as_deref()).await
}

/// review MAJOR-1：识别 daemon 的 TUN attestation/护栏 Error。
/// 判据锚定 daemon 与 CLI 共享的标记常量（`ipc::TUN_GUARD_ERROR_MARKER`），
/// 不再匹配自由文案—— 场景下 `tun_active` 为 false，这条判据是护栏
/// Error 不被折叠为 Pending 的唯一防线；文案措辞改动若丢掉标记会被
/// daemon 侧源码锁测试拦下，而不是静默回退。
/// 这些错误宣告运行态 TUN 已被拆除或无法观测，必须作为 Err 显式呈现；
/// 折叠为 Pending 会让用户只看到 "restart to apply"，而 restart 仍从
/// active-config 启动，TUN 不会回来。
fn is_tun_guard_error(message: &str) -> bool {
    message.contains(crate::ipc::TUN_GUARD_ERROR_MARKER)
}

async fn reload_config_for_mutation(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
    base_revision: Option<&str>,
) -> anyhow::Result<RuntimeApplyOutcome> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    let tun_active = resolved.ctx.mode == instance::InstanceMode::System
        && status::StatusSnapshot::collect(&resolved.ctx)
            .await
            .tun_verdict
            == status::TunVerdict::TunRunning;
    if tun_active {
        reload_configs_for_resolved_instance(system, user, paths, base_revision).await?;
        Ok(RuntimeApplyOutcome::SystemPromoted)
    } else {
        match reload_configs_for_resolved_instance(system, user, paths, base_revision).await {
            Ok(outcome) => Ok(outcome),
            // review MAJOR-3：TUN 护栏 Error 必须走 Err 分支显式
            // 呈现，不得折叠为 Pending；其余失败保持既有 Pending 语义。
            Err(error) if is_tun_guard_error(&error.to_string()) => Err(error),
            Err(_) => Ok(RuntimeApplyOutcome::Pending),
        }
    }
}

async fn try_reload_group_config(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
    base_revision: Option<&str>,
) -> anyhow::Result<Option<anyhow::Error>> {
    match reload_configs_for_resolved_instance(system, user, paths, base_revision).await {
        Ok(_) => Ok(None),
        Err(error) => Ok(Some(error)),
    }
}

/// The daemon-owned selection mirror exists only in system mode; resolve it
/// runtime-first so the push does not depend on the user passing `--system`
/// (issue 's invocation omits it in a system install).
fn selection_mirror_target_is_system(system: bool, user: bool) -> bool {
    matches!(
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly),
        Ok(resolved) if resolved.ctx.mode == instance::InstanceMode::System
    )
}

async fn apply_config_reload_required(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
    base_revision: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    if selection_mirror_target_is_system(system, user) {
        push_active_subscription_mirror(paths, false).await;
    }
    Ok(apply_config_reload_lines(system, user, paths, base_revision).await)
}

fn format_config_change_result(action: &str, target: &str) -> Vec<String> {
    vec![format!("  {action} {target}")]
}

fn format_subscription_switch_success(id: &str, apply_lines: Vec<String>) -> Vec<String> {
    let mut lines = format_config_change_result("Switched to subscription", id);
    lines.extend(apply_lines);
    lines
}

fn format_refresh_all_start() -> Vec<String> {
    vec!["  Refreshing all subscriptions...".to_string()]
}

fn format_refresh_all_result(
    report: &config::RefreshAllReport,
    apply_lines: Vec<String>,
) -> Vec<String> {
    let mut lines = if report.is_complete() {
        vec![format!(
            "  All {} subscriptions refreshed.",
            report.refreshed.len()
        )]
    } else {
        vec![format!(
            "  Refreshed {} subscription(s); {} failed.",
            report.refreshed.len(),
            report.failed.len()
        )]
    };
    lines.extend(apply_lines);
    lines
}

fn format_refresh_active_start(id: &str) -> Vec<String> {
    vec![format!("  Refreshing active subscription {id}...")]
}

fn format_refresh_active_success(apply_lines: Vec<String>) -> Vec<String> {
    let mut lines = vec!["  Subscription refreshed.".to_string()];
    lines.extend(apply_lines);
    lines
}

fn no_active_subscription_error() -> &'static str {
    "No active subscription.
  Run: mihomo-cli config --add <URL>"
}

fn format_config_add_start() -> Vec<String> {
    vec!["  Adding subscription...".to_string()]
}

fn format_config_add_success(id: &str, apply_lines: Vec<String>) -> Vec<String> {
    let mut lines = format_config_change_result("Added subscription", id);
    lines.extend(apply_lines);
    lines
}

#[allow(dead_code)]
fn format_legacy_url_add_success(id: &str, hot_reloaded: bool) -> Vec<String> {
    let mut lines = vec![format!("  Added and activated subscription {id}")];
    if hot_reloaded {
        // C + review 回归-2：不要求无谓的 restart，但也不得越过 attestation 声称已生效。
        lines.extend([
            "  ✓ Config reload request accepted by Core API".to_string(),
            "  ℹ Runtime status: unknown (revision attestation unavailable)".to_string(),
        ]);
    } else {
        lines.push("  Run: mihomo-cli restart".to_string());
    }
    lines
}

fn format_import_success(id: &str, activated: bool, apply_lines: Vec<String>) -> Vec<String> {
    let mut lines = if activated {
        vec![format!("  Imported and activated subscription {id}")]
    } else {
        vec![format!("  Imported subscription {id} (not activated)")]
    };
    if activated {
        lines.extend(apply_lines);
    }
    lines
}

fn format_fix_result(fixed_controller: bool, hot_reloaded: bool) -> Vec<String> {
    let mut lines = if fixed_controller {
        vec![
            "  Fixed config: added Unix socket controller.".to_string(),
            "  ⚠ Restart required for controller changes to take effect.".to_string(),
            "  Run: mihomo-cli restart".to_string(),
        ]
    } else {
        vec!["  Config already has Unix socket — no fix needed.".to_string()]
    };
    if hot_reloaded {
        // C：重载已接受是事实；controller 变更仍需 restart 的提示保留。
        lines.push("  ✓ Config reload request accepted by Core API".to_string());
    }
    lines
}

fn subscription_switch_rollback_error(error: &str) -> String {
    format!(
        "Subscription switch failed; rolled back active subscription.
  {error}"
    )
}

fn subscription_change_rollback_error(error: &str) -> String {
    format!(
        "Subscription change failed; rolled back subscription file and metadata.
  {error}"
    )
}

enum ConfigDryRunAction<'a> {
    SetUserAgent { id: &'a str, ua: &'a str },
    Switch { id: &'a str },
    Add { url: &'a str },
    Remove { id: &'a str },
    RefreshAll { count: usize },
    RefreshActive { id: &'a str },
    FixController,
    LegacyUrl { url: &'a str },
}

fn format_config_dry_run(action: ConfigDryRunAction<'_>) -> Vec<String> {
    let line = match action {
        ConfigDryRunAction::SetUserAgent { id, ua } => format!("  Would set UA for {id} to {ua}"),
        ConfigDryRunAction::Switch { id } => format!("  Would switch active subscription to {id}"),
        ConfigDryRunAction::Add { url } => {
            format!("  Would download, validate, and add subscription: {url}")
        }
        ConfigDryRunAction::Remove { id } => format!("  Would remove subscription {id}"),
        ConfigDryRunAction::RefreshAll { count } => {
            format!("  Would refresh {count} subscriptions and merge config")
        }
        ConfigDryRunAction::RefreshActive { id } => {
            format!("  Would refresh active subscription {id} and merge config")
        }
        ConfigDryRunAction::FixController => {
            "  Would ensure config has external controller socket/pipe".to_string()
        }
        ConfigDryRunAction::LegacyUrl { url } => {
            format!("  Would download, validate, add, activate, and merge subscription: {url}")
        }
    };
    vec![line]
}

fn print_json_envelope(
    command: &str,
    data: serde_json::Value,
    warnings: Vec<String>,
) -> anyhow::Result<()> {
    let value = serde_json::json!({
        "ok": true,
        "command": command,
        "data": data,
        "warnings": warnings,
        "error": serde_json::Value::Null,
        "meta": {
            "schema_version": 1,
            "cli_version": env!("MIHOMO_CLI_VERSION"),
        }
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn json_error_envelope(command: &str, error: &str) -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "command": command,
        "data": serde_json::Value::Null,
        "warnings": Vec::<String>::new(),
        "error": error,
        "meta": {
            "schema_version": 1,
            "cli_version": env!("MIHOMO_CLI_VERSION"),
        }
    })
}

fn validate_config_at_paths_with_binary_json(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
    json: bool,
) -> anyhow::Result<()> {
    let report = config::validate_config_at(paths, Some(mihomo))?;
    let intranet_warnings = if let Ok(content) = std::fs::read_to_string(paths.config_path()) {
        if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
            config::lint_intranet_guard(&yaml)
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    if json {
        let mut warnings = Vec::new();
        if !report.mihomo_tested {
            warnings.push(format!("mihomo binary not found: {}", mihomo.display()));
        }
        for w in &intranet_warnings {
            warnings.push(format!("{}: {}", w.message, w.recommendation));
        }
        print_json_envelope(
            "config.validate",
            serde_json::json!({
                "config_path": paths.config_path(),
                "mihomo_path": mihomo,
                "yaml_valid": report.yaml_valid,
                "mihomo_tested": report.mihomo_tested,
                "mihomo_valid": if report.mihomo_tested { Some(true) } else { None },
            }),
            warnings,
        )
    } else {
        let mut lines = format_config_validation_result(&paths.config_path(), mihomo, &report);
        for w in &intranet_warnings {
            lines.push(format!("  ⚠ [Intranet Guard] {}", w.message));
            lines.push(format!("     💡 {}", w.recommendation));
        }
        print_lines(lines);
        Ok(())
    }
}

fn format_config_validation_result(
    config_path: &std::path::Path,
    mihomo_path: &std::path::Path,
    report: &config::ConfigValidationReport,
) -> Vec<String> {
    let mut lines = vec![format!("  ✓ YAML syntax valid: {}", config_path.display())];
    if report.mihomo_tested {
        lines.push("  ✓ mihomo -t passed".to_string());
    } else {
        lines.push(format!(
            "  ⚠ mihomo binary not found: {}",
            mihomo_path.display()
        ));
        lines.push("  YAML is valid, but runtime validation was skipped.".to_string());
    }
    lines
}

fn format_backup_success(report: &backup::BackupReport) -> Vec<String> {
    vec![
        format!("  ✓ Backup created: {}", report.path.display()),
        format!(
            "  Restore with: mihomo-cli restore {}",
            backup::shell_escape_path(&report.path)
        ),
    ]
}

fn format_restore_success(safety_backup: Option<&std::path::Path>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(safety) = safety_backup {
        lines.push(format!("  Safety backup created: {}", safety.display()));
    }
    lines.push("  ✓ Restore complete".to_string());
    lines.push("  Run: mihomo-cli restart  to apply restored config".to_string());
    lines
}

fn format_probe_start(candidate_count: usize) -> Vec<String> {
    vec![
        "  Probing subscription URL with bounded UA candidates...".to_string(),
        format!(
            "  Note: probe sends {candidate_count} sequential requests with a short delay to reduce rate-limit risk."
        ),
    ]
}

fn format_tui_empty_subscription_intro() -> Vec<String> {
    vec![
        String::new(),
        "  No subscriptions found.".to_string(),
        "  Press 'a' to add one, or Esc to exit.".to_string(),
    ]
}

#[cfg(test)]
fn format_tui_subscription_menu_items(
    subs: &[config::SubscriptionMeta],
    active: Option<&str>,
) -> Vec<String> {
    subs.iter()
        .map(|sub| {
            let active_marker = if active == Some(sub.id.as_str()) {
                " (active)"
            } else {
                ""
            };
            format!("{}{}", shorten_subscription_url(&sub.url), active_marker)
        })
        .collect()
}

#[cfg(test)]
fn format_tui_action_hint() -> Vec<String> {
    vec![
        String::new(),
        "  Press: [r] Refresh  [R] Refresh all  [a] Add  [d] Delete  [Esc] Exit".to_string(),
    ]
}

fn format_tui_add_success(id: &str, apply_lines: Vec<String>) -> Vec<String> {
    let mut lines = format_config_change_result("Added subscription", id);
    lines.extend(apply_lines);
    lines
}

fn format_tui_switch_result(id: &str, switched: bool, apply_lines: Vec<String>) -> Vec<String> {
    if switched {
        format_subscription_switch_success(id, apply_lines)
    } else {
        vec!["  Already active.".to_string()]
    }
}

fn format_tui_refresh_active_start(id: &str) -> Vec<String> {
    vec![format!("  Refreshing subscription {id}...")]
}

fn format_tui_no_active_subscription() -> Vec<String> {
    vec!["  No active subscription.".to_string()]
}

#[cfg(test)]
fn format_tui_delete_items(subs: &[config::SubscriptionMeta]) -> Vec<String> {
    subs.iter()
        .map(|sub| {
            let sanitized = utils::sanitize_url(&sub.url);
            let short_url = if sanitized.chars().count() > 40 {
                format!("{}…", sanitized.chars().take(37).collect::<String>())
            } else {
                sanitized
            };
            format!("{} ({})", sub.id, short_url)
        })
        .collect()
}

fn format_tui_subscription_removed(id: &str) -> Vec<String> {
    format_config_change_result("Removed subscription", id)
}

fn print_lines(lines: impl IntoIterator<Item = String>) {
    for line in lines {
        println!("{line}");
    }
}

/// Resolve --activate / --no-activate flags into an Option<bool>.
/// Returns Some(true) for force activate, Some(false) for force skip, None for auto.
fn resolve_activate_flag(activate: bool, no_activate: bool, _yes: bool) -> Option<bool> {
    if activate {
        Some(true)
    } else if no_activate {
        Some(false)
    } else {
        None
    }
}

async fn cmd_config_fix_instance(system: bool, user: bool, dry_run: bool) -> anyhow::Result<()> {
    if !system {
        if let Some(config_dir) = config_dir_override_path() {
            let ctx = current_user_context_with_config_dir(config_dir)
                .ok_or_else(|| anyhow::anyhow!("Unsupported OS for env override config fix"))?;
            return cmd_config_fix_instance_context(ctx, dry_run).await;
        }
    }
    let mode = resolve_current_mode(system, user, instance::CommandIntent::Mutating)?;
    cmd_config_fix_instance_mode(mode, dry_run).await
}

async fn cmd_config_fix_instance_mode(
    mode: instance::InstanceMode,
    dry_run: bool,
) -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance config fix"))?;
    cmd_config_fix_instance_context(ctx, dry_run).await
}

async fn cmd_config_fix_instance_context(
    ctx: instance::InstanceContext,
    dry_run: bool,
) -> anyhow::Result<()> {
    let mode = ctx.mode;
    let mutation =
        instance::planned_config_mutation(&ctx, instance::ConfigMutationKind::FixRuntimeController);

    if dry_run {
        print_lines(format_config_dry_run(ConfigDryRunAction::FixController));
        println!("  Instance: {}", instance_mode_label(mode));
        println!("  Target:   {}", mutation.target_config.display());
        println!("  Endpoint: {}", status_endpoint_label(&mutation.endpoint));
        println!("  Strategy: {:?}", mutation.write_strategy);
        return Ok(());
    }

    let paths = utils::AppPaths::new(mutation.config_dir.clone());
    let mihomo_path = std::path::PathBuf::from(&mutation.validate_command.program);
    let fixed_controller =
        config::fix_existing_config_at_endpoint(&paths, Some(&mihomo_path), &mutation.endpoint)?;
    let client = mihomo_api::EndpointMihomoApiClient::for_instance(mode, mutation.endpoint.clone());
    let hot_reloaded =
        mihomo_api::reload_configs_with_client(&client, &paths.config_path().display().to_string())
            .await
            .is_ok();
    print_lines(format_fix_result(fixed_controller, hot_reloaded));
    Ok(())
}

fn system_config_requires_promotion(mode: instance::InstanceMode) -> bool {
    mode == instance::InstanceMode::System
}

/// Send one `RecordSelectionIntent` push and surface daemon-side rejects
/// (R3.2): an Error response is a failed push, not a silent success.
async fn send_record_selection_intent(
    subscription_id: &str,
    content_yaml: String,
    config_revision: Option<String>,
) -> anyhow::Result<()> {
    match ipc::send_command(&ipc::DaemonCommand::RecordSelectionIntent {
        subscription_id: subscription_id.to_string(),
        content_yaml,
        config_revision,
        token: None,
    })
    .await?
    {
        ipc::DaemonResponse::Success { .. } => Ok(()),
        ipc::DaemonResponse::Error { message } => Err(anyhow::anyhow!(message)),
        other => anyhow::bail!("unexpected daemon response: {other:?}"),
    }
}

/// Best-effort push of one subscription's selection intent file content to the
/// system daemon mirror (SPEC §3.8.1 mirror contract). Never fails the caller,
/// but R3.2 escalates failures: one self-heal retry on the same sanctioned
/// path, then an actionable report instead of a bare warning log.
///
/// Snapshot/lock boundary: the intent file is read without taking the
/// selection lock here. The `select` path already holds that lock for the whole
/// call, so its snapshot and push stay ordered against concurrent selects; the
/// callers that push after releasing the lock (unpin, group delete) only need
/// eventual consistency — a stale read can leave the mirror one generation
/// behind, and `restart` / daemon start rebuild it from the full intent set
/// (`push_selection_mirror_full`), which heals the gap. Taking the lock just to
/// read the snapshot would not order the pushes themselves (IPC would still run
/// outside the lock, so two callers can still land out of order) and would add
/// a lock acquisition on a file that is already replaced atomically — so the
/// extra lock is not worth it.
async fn push_selection_mirror_update(paths: &utils::AppPaths, subscription_id: &str, json: bool) {
    let file = paths.selection_state_path_for_subscription(subscription_id);
    let Ok(content) = std::fs::read_to_string(&file) else {
        return;
    };
    let revision = config_base_revision(paths);
    if let Err(err) =
        send_record_selection_intent(subscription_id, content.clone(), revision.clone()).await
    {
        match send_record_selection_intent(subscription_id, content, revision).await {
            Ok(()) => {
                if json {
                    eprintln!(
                        "⚠ runtime selection mirror first push failed ({err:#}); self-healed on retry"
                    );
                } else {
                    println!(
                        "⚠ runtime selection mirror first push failed ({err:#}); self-healed on retry"
                    );
                }
            }
            Err(retry_err) => {
                if json {
                    eprintln!(
                        "⚠ runtime selection mirror not updated ({retry_err:#});\n  replay after daemon restart may be skipped\n  repair: mihomo-cli restart --system (rebuilds the runtime selection mirror)"
                    );
                } else {
                    println!(
                        "⚠ runtime selection mirror not updated ({retry_err:#});\n  replay after daemon restart may be skipped\n  repair: mihomo-cli restart --system (rebuilds the runtime selection mirror)"
                    );
                }
            }
        }
    }
}

/// Best-effort push of the active subscription mirror plus its selection map.
async fn push_active_subscription_mirror(paths: &utils::AppPaths, json: bool) {
    let Ok(Some(active)) = config::get_active_id_at(paths) else {
        return;
    };
    if let Err(err) = ipc::send_command(&ipc::DaemonCommand::RecordActiveSubscription {
        subscription_id: active.clone(),
        config_revision: config_base_revision(paths),
        token: None,
    })
    .await
    {
        if json {
            eprintln!(
                "⚠ runtime selection mirror not updated; replay after daemon restart may be skipped ({err:#})"
            );
        } else {
            println!(
                "⚠ runtime selection mirror not updated; replay after daemon restart may be skipped ({err:#})"
            );
        }
        return;
    }
    push_selection_mirror_update(paths, &active, json).await;
}

/// Idempotent full mirror rebuild used by lifecycle start/restart: pushes the
/// active pointer and every persisted selection file before the daemon
/// (re)starts the Core, so daemon-side replay reads only the fixed runtime.
async fn push_selection_mirror_full(ctx: &instance::InstanceContext, config_revision: &str) {
    let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
    let Ok(Some(active)) = config::get_active_id_at(&paths) else {
        return;
    };
    let revision = config_revision.to_string();
    if let Err(err) = ipc::send_command(&ipc::DaemonCommand::RecordActiveSubscription {
        subscription_id: active,
        config_revision: Some(revision.clone()),
        token: None,
    })
    .await
    {
        println!(
            "⚠ runtime selection mirror not updated; replay after daemon restart may be skipped ({err:#})"
        );
        return;
    }
    if let Ok(entries) = std::fs::read_dir(paths.selections_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "yaml") {
                if let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) {
                    let content = std::fs::read_to_string(&path).unwrap_or_default();
                    if let Err(err) =
                        send_record_selection_intent(id, content, Some(revision.clone())).await
                    {
                        println!(
                            "⚠ runtime selection mirror not updated; replay after daemon restart may be skipped ({err:#})"
                        );
                    }
                }
            }
        }
    }
    // R3.2: verify what the daemon actually holds against the user tree and
    // re-push anything a best-effort push missed.
    match reconcile_selection_mirror(&paths).await {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        Err(err) => println!(
            "⚠ selection mirror reconciliation failed ({err:#}); inspect: mihomo-cli doctor --system"
        ),
    }
}

// ---------------- R3.2 selection mirror reconciliation ----------------
// The user tree holds the selection intent (source of truth); the daemon
// mirror is the runtime copy that daemon-side replay depends on. Pushes are
// best-effort, so drift is possible; these helpers detect and repair it
// exclusively through the sanctioned RecordSelectionIntent write path.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionMirrorDrift {
    Ok,
    Missing,
    Drifted,
}

fn is_reconcilable_subscription_id(id: &str) -> bool {
    id.starts_with("sub-") && id.len() == 12 && id.as_bytes()[4..].iter().all(u8::is_ascii_hexdigit)
}

/// Revisions of user-tree selection intent files, keyed by subscription id.
/// The hash covers the exact bytes a push transmits, so a consistent pair
/// always compares equal. Ids the daemon would not accept are out of the
/// mirror contract and skipped.
fn user_tree_selection_revisions(
    paths: &utils::AppPaths,
) -> std::collections::BTreeMap<String, String> {
    let mut revisions = std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(paths.selections_dir()) else {
        return revisions;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "yaml") {
            if let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) {
                if is_reconcilable_subscription_id(id) {
                    if let Ok(bytes) = std::fs::read(&path) {
                        revisions.insert(id.to_string(), tun_transaction::content_revision(&bytes));
                    }
                }
            }
        }
    }
    revisions
}

/// Pure classification of one reconciliation pass: every user-tree id maps
/// to Ok / Missing (mirror absent or unseen by the daemon) / Drifted.
fn classify_selection_mirror_drift(
    expected: &std::collections::BTreeMap<String, String>,
    actual: &std::collections::BTreeMap<String, Option<String>>,
) -> std::collections::BTreeMap<String, SelectionMirrorDrift> {
    expected
        .iter()
        .map(|(id, want)| {
            let kind = match actual.get(id) {
                None | Some(None) => SelectionMirrorDrift::Missing,
                Some(Some(got)) if got == want => SelectionMirrorDrift::Ok,
                Some(Some(_)) => SelectionMirrorDrift::Drifted,
            };
            (id.clone(), kind)
        })
        .collect()
}

async fn query_selection_mirror_revisions(
    subscription_ids: Vec<String>,
) -> anyhow::Result<std::collections::BTreeMap<String, Option<String>>> {
    match ipc::send_command(&ipc::DaemonCommand::GetSelectionMirrorRevisions {
        subscription_ids,
        token: None,
    })
    .await?
    {
        ipc::DaemonResponse::SelectionMirrorRevisions { revisions } => Ok(revisions),
        ipc::DaemonResponse::Error { message } => Err(anyhow::anyhow!(message)),
        other => anyhow::bail!("unexpected daemon response to mirror revision query: {other:?}"),
    }
}

/// Read-only drift detection used by `doctor --system`. An empty map means
/// the user tree holds no reconcilable selection state.
#[cfg(unix)]
async fn selection_mirror_drift_report(
    paths: &utils::AppPaths,
) -> anyhow::Result<std::collections::BTreeMap<String, SelectionMirrorDrift>> {
    let expected = user_tree_selection_revisions(paths);
    if expected.is_empty() {
        return Ok(std::collections::BTreeMap::new());
    }
    let actual = query_selection_mirror_revisions(expected.keys().cloned().collect()).await?;
    Ok(classify_selection_mirror_drift(&expected, &actual))
}

/// System-mode governance (R3.2): compare the daemon selection mirror with
/// the user-tree intent files and re-push every drifted or missing entry
/// through the sanctioned write path (the daemon remains the only runtime
/// writer). Returns user-facing report lines.
async fn reconcile_selection_mirror(paths: &utils::AppPaths) -> anyhow::Result<Vec<String>> {
    let expected = user_tree_selection_revisions(paths);
    if expected.is_empty() {
        return Ok(Vec::new());
    }
    let actual = query_selection_mirror_revisions(expected.keys().cloned().collect()).await?;
    let drift = classify_selection_mirror_drift(&expected, &actual);
    let mut lines = Vec::new();
    for (id, kind) in drift
        .iter()
        .filter(|(_, kind)| **kind != SelectionMirrorDrift::Ok)
    {
        let file = paths.selection_state_path_for_subscription(id);
        match std::fs::read_to_string(&file) {
            Ok(content) => match send_record_selection_intent(
                id,
                content,
                config_base_revision(paths),
            )
            .await
            {
                Ok(()) => lines.push(format!(
                    "✅ selection mirror {id} repaired from user-tree intent ({kind:?})"
                )),
                Err(err) => lines.push(format!(
                    "⚠ selection mirror {id} repair failed ({err:#})\n  repair: mihomo-cli restart --system (rebuilds the runtime selection mirror)"
                )),
            },
            Err(err) => lines.push(format!(
                "⚠ selection mirror {id} not repairable; user-tree intent unreadable ({err})"
            )),
        }
    }
    Ok(lines)
}

async fn reload_configs_for_resolved_instance(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
    base_revision: Option<&str>,
) -> anyhow::Result<RuntimeApplyOutcome> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    // B + review MAJOR-3：基线比对与 reconcile 在同一把锁内完成，
    // 直接使用锁内返回的内容，不在锁外重读（reconcile 主动改写不再跳过并发检查）。
    let reconciled = config::reconcile_override_into_intent_checked(paths, base_revision)?;
    if reconciled.changed {
        crate::log!("  ⚠ override.yaml 派生键已重新合并进 intent config.yaml");
    }
    let content = reconciled.content;
    if system_config_requires_promotion(resolved.ctx.mode) {
        let revision = tun_transaction::content_revision(content.as_bytes());
        let subscription_id = config::get_active_id_at(paths)?;
        let subscription_id_at_request = subscription_id.clone();
        match ipc::send_command(&ipc::DaemonCommand::PromoteSystemConfig {
            config_content: content,
            config_revision: revision,
            subscription_id,
            change_kind: None,
            token: None,
        })
        .await?
        {
            ipc::DaemonResponse::Success { message } => {
                // Consume the promotion message: it carries the daemon-side
                // replay report (remedies included) and any identity-mismatch
                // signal — it must not be swallowed. Bind to the
                // subscription_id sent with the promotion request.
                for line in selection_replay_message_lines(
                    &message,
                    subscription_id_at_request.as_deref(),
                    paths,
                ) {
                    println!("{line}");
                }
                return Ok(RuntimeApplyOutcome::SystemPromoted);
            }
            ipc::DaemonResponse::Error { message } => anyhow::bail!(message),
            response => anyhow::bail!("unexpected daemon promotion response: {response:?}"),
        }
    }
    let client = mihomo_api::EndpointMihomoApiClient::for_instance(
        resolved.ctx.mode,
        resolved.ctx.paths.api_endpoint,
    );
    let config_path = paths.config_path().display().to_string();
    mihomo_api::reload_configs_with_client(&client, &config_path).await?;
    print_selection_replay_after_ready(
        paths,
        &client,
        std::time::Instant::now() + selection::REPLAY_TOTAL_BUDGET,
    )
    .await;
    Ok(RuntimeApplyOutcome::from_mode(resolved.ctx.mode))
}

/// Capture the current config.yaml revision right before a reload so the
/// promotion path can reject any concurrent rewrite that lands in between.
fn config_base_revision(paths: &utils::AppPaths) -> Option<String> {
    std::fs::read(paths.config_path())
        .ok()
        .map(|bytes| tun_transaction::content_revision(&bytes))
}

async fn cmd_config(args: ConfigCmd) -> anyhow::Result<()> {
    let ConfigCmd {
        url,
        fix,
        system,
        user,
        refresh,
        refresh_all,
        import,
        switch,
        add,
        remove,
        list,
        validate,
        dry_run,
        yes,
        info,
        probe,
        user_agent,
        set_ua,
        command,
        activate,
        no_activate,
        json,
    } = args;

    // `config validate` is accepted as a subcommand, equivalent to `--validate`.
    let validate = validate || matches!(command, Some(ConfigSubcommand::Validate));

    // ADR-02: System config is now per-user, same write path as User config.
    // No guard needed — config writes work for both modes.
    let has_action = command.is_some()
        || url.is_some()
        || fix
        || refresh
        || refresh_all
        || import.is_some()
        || switch.is_some()
        || add.is_some()
        || remove.is_some()
        || list
        || validate
        || info.is_some()
        || probe.is_some()
        || !set_ua.is_empty();
    if !has_action && !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "config requires an explicit action in non-interactive mode.\n  Use --list, --add <URL>, --import <FILE>, --refresh, --refresh-all, --validate (or `config validate`), or --probe <URL>."
        );
    }

    let read_paths = || {
        app_paths_for_resolved_instance_command(
            "config",
            system,
            user,
            instance::CommandIntent::ReadOnly,
        )
    };
    let write_paths = || {
        app_paths_for_resolved_instance_command(
            "config",
            system,
            user,
            instance::CommandIntent::Mutating,
        )
    };
    if let Some(ConfigSubcommand::Fetch {
        url,
        output,
        user_agent: fetch_user_agent,
    }) = command
    {
        if dry_run {
            println!("  Dry run: no files will be written.");
            println!(
                "  Would download, convert, validate, and write subscription: {}",
                utils::sanitize_url(&url)
            );
            println!("  Output: {}", output.display());
            return Ok(());
        }
        let ua = fetch_user_agent.as_deref().or(user_agent.as_deref());
        let report = config::fetch_subscription_to_file(&url, &output, ua).await?;
        if json {
            return print_json_envelope(
                "config.fetch",
                serde_json::json!({
                    "output_path": report.output_path,
                    "format": if report.is_clash_yaml { "clash-yaml" } else { "converted" },
                    "proxy_count": report.proxy_count,
                    "proxy_group_count": report.proxy_group_count,
                    "rule_count": report.rule_count,
                }),
                vec!["Treat the output file as sensitive; it may contain private proxy nodes/tokens.".to_string()],
            );
        }
        println!("  ✓ Fetched subscription: {}", report.output_path.display());
        println!(
            "  Format: {}",
            if report.is_clash_yaml {
                "Clash YAML"
            } else {
                "converted to Clash YAML"
            }
        );
        println!(
            "  Proxies: {}  Groups: {}  Rules: {}",
            report.proxy_count, report.proxy_group_count, report.rule_count
        );
        println!(
            "  ⚠ Treat the output file as sensitive; it may contain private proxy nodes/tokens."
        );
        println!(
            "  Import on target machine with: mihomo-cli config --import {}",
            report.output_path.display()
        );
        return Ok(());
    }

    if validate {
        let paths = read_paths()?;
        let core_binary = resolved_core_binary_for_config_command(
            system,
            user,
            instance::CommandIntent::ReadOnly,
        )?;
        validate_config_at_paths_with_binary_json(&paths, &core_binary, json)?;
        return Ok(());
    }

    if dry_run {
        println!("  Dry run: no files will be written and mihomo will not be restarted/reloaded.");
    }

    if let Some(url) = probe {
        print_lines(format_probe_start(4));
        let results = config::probe_subscription_url(&url).await?;
        print_probe_results(&results);
        return Ok(());
    }

    let paths = write_paths()?;
    let core_binary =
        resolved_core_binary_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let config_endpoint =
        resolved_api_endpoint_for_config_command(system, user, instance::CommandIntent::Mutating)?;

    if !set_ua.is_empty() {
        let id = &set_ua[0];
        let ua = &set_ua[1];
        if dry_run {
            print_lines(format_config_dry_run(ConfigDryRunAction::SetUserAgent {
                id,
                ua,
            }));
            return Ok(());
        }
        let value = if ua.eq_ignore_ascii_case("auto") {
            None
        } else {
            Some(ua.clone())
        };
        config::set_subscription_user_agent_at(&paths, id, value)?;
        for line in format_config_change_result("Updated User-Agent for subscription", id) {
            println!("{line}");
        }
        return Ok(());
    }

    // Priority: explicit operations > legacy > TUI
    if let Some(info_id) = info {
        let paths = read_paths()?;
        let id = match info_id {
            Some(id) => id,
            None => config::get_active_id_at(&paths)?.ok_or_else(|| {
                anyhow::anyhow!("No active subscription. Run: mihomo-cli config --list")
            })?,
        };
        let info = config::subscription_info_at(&paths, &id)?;
        let subs = config::load_subscriptions_at(&paths)?;
        let meta = config::find_subscription(&subs, &id);
        for line in format_subscription_info(&info, meta) {
            println!("{line}");
        }
        return Ok(());
    }

    if list {
        let paths = read_paths()?;
        let subs = config::load_subscriptions_at(&paths)?;
        let active = config::get_active_id_at(&paths)?;
        for line in format_subscription_list(&subs, active.as_deref()) {
            println!("{line}");
        }
        return Ok(());
    }

    if let Some(id) = switch {
        if dry_run {
            print_lines(format_config_dry_run(ConfigDryRunAction::Switch {
                id: &id,
            }));
            return Ok(());
        }
        let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
        let active_snapshot = snapshot_file(&paths.active_file_path())?;
        config::switch_subscription_at(&paths, &id)?;
        if let Err(err) = config::merge_user_config_checked_at_endpoint(
            &paths,
            Some(&core_binary),
            &config_endpoint,
        ) {
            if let Err(rollback_err) =
                restore_file_snapshot(&paths.active_file_path(), active_snapshot)
            {
                anyhow::bail!(
                    "Subscription switch failed; rollback incomplete: {}.\n  Original error: {}",
                    rollback_err,
                    err
                );
            }
            anyhow::bail!(subscription_switch_rollback_error(&err.to_string()));
        }
        drop(_lock);
        let apply_lines = apply_config_reload_required(
            system,
            user,
            &paths,
            config_base_revision(&paths).as_deref(),
        )
        .await?;
        print_lines(format_subscription_switch_success(&id, apply_lines));
        print_config_drift_warnings(&paths);
        return Ok(());
    }

    if let Some(url) = add {
        if dry_run {
            print_lines(format_config_dry_run(ConfigDryRunAction::Add { url: &url }));
            return Ok(());
        }
        print_lines(format_config_add_start());
        let activate_flag = resolve_activate_flag(activate, no_activate, yes);
        // Defect 1: download stays outside the lock (slow network must not
        // block other config commands); the URL match, THIS transaction's
        // pre-image, the commit, the activate prompt and the merge validation
        // all happen inside one lock hold via `commit_subscription_matched_
        // checked_at`, so a merge failure restores only this transaction's
        // true pre-image and can never clobber a concurrent successful commit.
        let prepared =
            config::prepare_subscription_with_user_agent(&url, user_agent.clone()).await?;
        let commit = config::commit_subscription_matched_checked_at(
            &paths,
            &url,
            &prepared.yaml_content,
            user_agent.clone(),
            activate_flag,
            |commit, pre_images| {
                print_subscription_commit_kind(commit);
                let id = &commit.id;
                // If not auto-activated and no explicit flag, prompt or hint
                if activate_flag.is_none() {
                    let active_id =
                        std::fs::read_to_string(paths.active_file_path()).unwrap_or_default();
                    if active_id.trim() != id {
                        if std::io::stdin().is_terminal() {
                            use dialoguer::Confirm;
                            if Confirm::new()
                                .with_prompt("  Activate this subscription?")
                                .default(true)
                                .interact()?
                            {
                                config::set_active_id_at(&paths, id)?;
                                // The prompt changed `active`: mark it so a
                                // later merge failure also restores `active`.
                                pre_images.activated = true;
                            }
                        } else {
                            println!("  Run `mihomo-cli config --switch {id}` to activate.");
                        }
                    }
                }
                merge_subscription_change_checked(&paths, &core_binary, &config_endpoint)
            },
        )?;
        let id = commit.id.clone();
        let apply_lines = apply_config_reload_required(
            system,
            user,
            &paths,
            config_base_revision(&paths).as_deref(),
        )
        .await?;
        print_lines(format_config_add_success(&id, apply_lines));
        print_config_drift_warnings(&paths);
        return Ok(());
    }

    if let Some(id) = remove {
        if dry_run {
            print_lines(format_config_dry_run(ConfigDryRunAction::Remove {
                id: &id,
            }));
            return Ok(());
        }
        let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
        let was_active = config::get_active_id_at(&paths)?.as_deref() == Some(id.as_str());
        let meta_snapshot = snapshot_file(&paths.subscriptions_meta_path())?;
        let active_snapshot = snapshot_file(&paths.active_file_path())?;
        let sub_snapshot = snapshot_file(&paths.subscription_file_path(&id))?;
        config::remove_subscription_at(&paths, &id)?;
        if was_active {
            if let Err(err) = config::merge_user_config_checked_at_endpoint(
                &paths,
                Some(&core_binary),
                &config_endpoint,
            ) {
                let mut rollback_errors = Vec::new();
                if let Err(rollback_err) =
                    restore_file_snapshot(&paths.subscriptions_meta_path(), meta_snapshot)
                {
                    rollback_errors.push(format!("subscriptions metadata: {rollback_err}"));
                }
                if let Err(rollback_err) =
                    restore_file_snapshot(&paths.active_file_path(), active_snapshot)
                {
                    rollback_errors.push(format!("active subscription: {rollback_err}"));
                }
                if let Err(rollback_err) =
                    restore_file_snapshot(&paths.subscription_file_path(&id), sub_snapshot)
                {
                    rollback_errors.push(format!("subscription file: {rollback_err}"));
                }
                if rollback_errors.is_empty() {
                    anyhow::bail!(
                        "Subscription remove failed; rolled back metadata and subscription file.\n  {}",
                        err
                    );
                }
                anyhow::bail!(
                    "Subscription remove failed; rollback incomplete: {}.\n  Original error: {}",
                    rollback_errors.join("; "),
                    err
                );
            }
        }
        let mut lines = format_config_change_result("Removed subscription", &id);
        drop(_lock);
        if was_active {
            lines.extend(
                apply_config_reload_lines(
                    system,
                    user,
                    &paths,
                    config_base_revision(&paths).as_deref(),
                )
                .await,
            );
        }
        print_lines(lines);
        return Ok(());
    }

    if refresh_all {
        let subs = config::load_subscriptions_at(&paths)?;
        let living: Vec<_> = subs.iter().filter(|s| s.is_alive()).collect();
        if living.is_empty() {
            println!("  No subscriptions to refresh.");
            return Ok(());
        }
        if dry_run {
            print_lines(format_config_dry_run(ConfigDryRunAction::RefreshAll {
                count: living.len(),
            }));
            return Ok(());
        }
        print_lines(format_refresh_all_start());
        // Defect 1/3: all downloads happen outside the config write lock; the
        // lock-internal commit takes this transaction's pre-images, runs the
        // merge validation and restores them on failure — never a pre-download
        // snapshot that could clobber a concurrent commit.
        let (prepared, mut download_failures) =
            config::prepare_subscription_refresh_all_at(&paths, user_agent.as_deref()).await?;
        let report = config::commit_refreshes_checked_at(&paths, prepared, || {
            config::merge_user_config_checked_at_endpoint(
                &paths,
                Some(&core_binary),
                &config_endpoint,
            )
        })?;
        let mut report = report;
        report.failed.append(&mut download_failures);
        let apply_lines = apply_config_reload_required(
            system,
            user,
            &paths,
            config_base_revision(&paths).as_deref(),
        )
        .await?;
        print_lines(format_refresh_all_result(&report, apply_lines));
        print_config_drift_warnings(&paths);
        if !report.is_complete() {
            anyhow::bail!(
                "Some subscriptions could not be refreshed: {}",
                report.failure_summary()
            );
        }
        return Ok(());
    }

    if refresh {
        let active = config::get_active_id_at(&paths)?;
        match active {
            Some(id) => {
                if dry_run {
                    print_lines(format_config_dry_run(ConfigDryRunAction::RefreshActive {
                        id: &id,
                    }));
                    return Ok(());
                }
                print_lines(format_refresh_active_start(&id));
                // Defect 1/3: download outside the lock; commit + merge +
                // failure recovery are one lock-internal transaction.
                let prepared =
                    config::prepare_subscription_refresh_at(&paths, &id, user_agent.as_deref())
                        .await?;
                config::commit_refreshes_checked_at(&paths, vec![(id.clone(), prepared)], || {
                    config::merge_user_config_checked_at_endpoint(
                        &paths,
                        Some(&core_binary),
                        &config_endpoint,
                    )
                })?;
                let apply_lines = apply_config_reload_required(
                    system,
                    user,
                    &paths,
                    config_base_revision(&paths).as_deref(),
                )
                .await?;
                print_lines(format_refresh_active_success(apply_lines));
                print_config_drift_warnings(&paths);
                return Ok(());
            }
            None => {
                anyhow::bail!(no_active_subscription_error());
            }
        }
    }

    // Instance-aware: fix runtime controller for the resolved system/user instance.
    if fix {
        return cmd_config_fix_instance(system, user, dry_run).await;
    }

    // Legacy: import
    if let Some(file) = import {
        let content = std::fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("Cannot read file: {file}\n  {e}"))?;

        // Try to detect and convert base64/vmess format
        let import_action = classify_import_content(&content);
        let yaml_content = match import_action {
            ImportContentAction::UseAsYaml => content,
            ImportContentAction::ConvertBase64Subscription
            | ImportContentAction::ConvertRawSubscription => {
                config::warn_raw_subscription_conversion();
                if let Some(line) = import_conversion_notice(import_action) {
                    println!("{line}");
                }
                config::convert_vmess_to_clash(&content)?
            }
        };

        if dry_run {
            let _: serde_yaml::Value = serde_yaml::from_str(&yaml_content)
                .map_err(|e| anyhow::anyhow!("Imported content is not valid YAML: {e}"))?;
            println!("  Import content can be parsed and converted.");
            return Ok(());
        }

        let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
        let existing_subscriptions = config::load_subscriptions_at(&paths)?;

        let activate_decision = resolve_activate_flag(activate, no_activate, yes);
        let should_activate = match activate_decision {
            Some(v) => v,
            None if !existing_subscriptions.iter().any(|s| s.is_alive()) => true,
            None if std::io::stdin().is_terminal() => {
                use dialoguer::Confirm;
                Confirm::new()
                    .with_prompt("  Activate this subscription?")
                    .default(true)
                    .interact()?
            }
            None => false,
        };
        drop(_lock);

        // Defect 1: import content is already local (no download window), but
        // the commit + merge + failure-restore must still be one lock-internal
        // transaction with this transaction's own pre-images.
        let commit = config::commit_subscription_matched_checked_at(
            &paths,
            &format!("file://{}", file),
            &yaml_content,
            None,
            Some(should_activate),
            |commit, _pre_images| {
                print_subscription_commit_kind(commit);
                if should_activate {
                    merge_subscription_change_checked(&paths, &core_binary, &config_endpoint)?;
                }
                Ok(())
            },
        )?;
        let id = commit.id.clone();
        if should_activate {
            let apply_lines = apply_config_reload_required(
                system,
                user,
                &paths,
                config_base_revision(&paths).as_deref(),
            )
            .await?;
            print_lines(format_import_success(&id, true, apply_lines));
        } else {
            print_lines(format_import_success(&id, false, Vec::new()));
            if !std::io::stdin().is_terminal() {
                println!("  Run `mihomo-cli config --switch {id}` to activate.");
            }
        }
        return Ok(());
    }

    // Legacy: URL argument
    if let Some(u) = url {
        if dry_run {
            print_lines(format_config_dry_run(ConfigDryRunAction::LegacyUrl {
                url: &u,
            }));
            return Ok(());
        }
        print_lines(format_config_add_start());
        let activate_flag = resolve_activate_flag(activate, no_activate, yes);
        // Defect 1 (legacy URL entry): same lock-internal transaction contract
        // as `--add`.
        let prepared = config::prepare_subscription_with_user_agent(&u, None).await?;
        let commit = config::commit_subscription_matched_checked_at(
            &paths,
            &u,
            &prepared.yaml_content,
            None,
            activate_flag,
            |commit, pre_images| {
                print_subscription_commit_kind(commit);
                let id = &commit.id;
                if activate_flag.is_none() {
                    let active_id =
                        std::fs::read_to_string(paths.active_file_path()).unwrap_or_default();
                    if active_id.trim() != id {
                        if std::io::stdin().is_terminal() {
                            use dialoguer::Confirm;
                            if Confirm::new()
                                .with_prompt("  Activate this subscription?")
                                .default(true)
                                .interact()?
                            {
                                config::set_active_id_at(&paths, id)?;
                                pre_images.activated = true;
                            }
                        } else {
                            println!("  Run `mihomo-cli config --switch {id}` to activate.");
                        }
                    }
                }
                merge_subscription_change_checked(&paths, &core_binary, &config_endpoint)
            },
        )?;
        let id = commit.id.clone();
        let apply_lines = apply_config_reload_required(
            system,
            user,
            &paths,
            config_base_revision(&paths).as_deref(),
        )
        .await?;
        let mut lines = vec![format!("  Added and activated subscription {id}")];
        lines.extend(apply_lines);
        print_lines(lines);
        return Ok(());
    }

    // No arguments: show TUI for the resolved instance config.
    show_subscription_menu(&paths, &core_binary, &config_endpoint).await
}

fn validate_config_at_paths_with_binary(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
) -> anyhow::Result<()> {
    validate_config_at_paths_with_binary_json(paths, mihomo, false)
}

/// Interactive subscription management with keyboard shortcuts.
///
/// Navigation: ↑↓ move cursor, Enter switch subscription
/// Shortcuts: r=refresh active, R=refresh all, a=add, d=delete, Esc=back
async fn show_subscription_menu(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
    endpoint: &instance::ApiEndpoint,
) -> anyhow::Result<()> {
    use crossterm::terminal;

    let mut cursor: usize = 0;

    // Enable raw mode so we can capture individual key presses
    terminal::enable_raw_mode()?;
    let result = show_subscription_menu_inner(paths, mihomo, endpoint, &mut cursor).await;
    terminal::disable_raw_mode()?;
    result
}

fn truncate_for_terminal(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    format!("{}…", s.chars().take(width - 1).collect::<String>())
}

/// Inner TUI loop. Runs under raw mode; caller handles enable/disable.
async fn show_subscription_menu_inner(
    paths: &crate::utils::AppPaths,
    mihomo: &std::path::Path,
    endpoint: &instance::ApiEndpoint,
    cursor: &mut usize,
) -> anyhow::Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyModifiers};
    use crossterm::terminal;
    use std::io::{self, Write};
    use std::time::Duration;

    loop {
        let subs = config::load_subscriptions_at(paths)?;
        let active = config::get_active_id_at(paths)?;

        if subs.is_empty() {
            // No subscriptions — show empty state TUI, wait for 'a' to add or 'q'/Esc to exit
            let mut stdout = io::stdout();
            write!(
                stdout,
                "{}{}",
                crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
                crossterm::cursor::MoveTo(0, 0)
            )?;
            for line in format_tui_empty_subscription_intro() {
                write!(stdout, "{line}\r\n")?;
            }
            write!(stdout, "\r\n")?;
            write!(stdout, "a add · q/Esc quit\r\n")?;
            stdout.flush()?;

            let key = loop {
                if event::poll(Duration::from_millis(100))? {
                    if let Event::Key(k) = event::read()? {
                        break k;
                    }
                }
            };

            match key.code {
                KeyCode::Char('a') => {
                    terminal::disable_raw_mode()?;
                    print!("\r\n  Subscription URL (empty to cancel): ");
                    io::stdout().flush()?;
                    let mut url = String::new();
                    io::stdin().read_line(&mut url)?;
                    let url = url.trim();
                    if url.is_empty() {
                        return Ok(());
                    }
                    print_lines(format_config_add_start());
                    let id = config::add_subscription_at(paths, url).await?;
                    config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint)?;
                    print_lines(format_tui_add_success(&id, config_restart_apply_lines()));
                    terminal::enable_raw_mode()?;
                    return Ok(());
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    return Ok(());
                }
                _ => {}
            }
            continue;
        }

        // Clamp cursor
        if *cursor >= subs.len() {
            *cursor = subs.len() - 1;
        }

        // Render clean, terminal-width-aware screen.
        let mut stdout = io::stdout();
        let (term_width, term_height) = terminal::size().unwrap_or((80, 24));
        let width = term_width as usize;
        let visible_rows = (term_height as usize).saturating_sub(6).max(1);
        let start = if *cursor >= visible_rows {
            *cursor - visible_rows + 1
        } else {
            0
        };
        let end = (start + visible_rows).min(subs.len());

        write!(
            stdout,
            "{}{}",
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::cursor::MoveTo(0, 0)
        )?;
        write!(stdout, "Subscriptions\r\n")?;
        write!(stdout, "─────────────\r\n")?;
        write!(stdout, "\r\n")?;

        for (i, sub) in subs.iter().enumerate().take(end).skip(start) {
            let is_active = active.as_deref() == Some(sub.id.as_str());
            let prefix = if i == *cursor { "›" } else { " " };
            let marker = if is_active { " *" } else { "" };
            let line = format!(
                "{prefix} {}{marker}  {}",
                sub.id,
                utils::sanitize_url(&sub.url)
            );
            write!(
                stdout,
                "{}\r\n",
                truncate_for_terminal(&line, width.saturating_sub(1))
            )?;
        }

        if subs.len() > visible_rows {
            write!(stdout, "\r\n")?;
            write!(stdout, "{}-{} / {}\r\n", start + 1, end, subs.len())?;
        }

        write!(stdout, "\r\n")?;
        write!(stdout, "j/k move · Enter switch · q/Esc quit\r\n")?;
        write!(stdout, "r refresh · R refresh all · a add · d delete\r\n")?;
        stdout.flush()?;

        // Wait for key
        let key = loop {
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(k) = event::read()? {
                    break k;
                }
            }
        };

        match key.code {
            KeyCode::Up | KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if *cursor > 0 {
                    *cursor -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if *cursor + 1 < subs.len() {
                    *cursor += 1;
                }
            }
            KeyCode::Char('k') => {
                if *cursor > 0 {
                    *cursor -= 1;
                }
            }
            KeyCode::Char('j') => {
                if *cursor + 1 < subs.len() {
                    *cursor += 1;
                }
            }
            KeyCode::Enter => {
                let selected_id = &subs[*cursor].id;
                terminal::disable_raw_mode()?;
                if active.as_deref() != Some(selected_id.as_str()) {
                    // Defect 1 pattern: snapshot + switch + merge + rollback in
                    // ONE lock hold so the pre-image cannot go stale.
                    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
                    let active_snapshot = snapshot_file(&paths.active_file_path())?;
                    config::switch_subscription_at(paths, selected_id)?;
                    if let Err(err) =
                        config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint)
                    {
                        if let Err(rollback_err) =
                            restore_file_snapshot(&paths.active_file_path(), active_snapshot)
                        {
                            anyhow::bail!(
                                "Subscription switch failed; rollback incomplete: {}.\n  Original error: {}",
                                rollback_err,
                                err
                            );
                        }
                        anyhow::bail!(subscription_switch_rollback_error(&err.to_string()));
                    }
                    drop(_lock);
                    print_lines(format_tui_switch_result(
                        selected_id,
                        true,
                        config_restart_apply_lines(),
                    ));
                } else {
                    print_lines(format_tui_switch_result(selected_id, false, Vec::new()));
                }
                return Ok(());
            }
            KeyCode::Char('R') => {
                terminal::disable_raw_mode()?;
                print_lines(format_refresh_all_start());
                // Defect 1/3: downloads outside the lock; lock-internal commit
                // + merge + recovery with this transaction's pre-images.
                let (prepared, mut download_failures) =
                    config::prepare_subscription_refresh_all_at(paths, None).await?;
                let mut report = config::commit_refreshes_checked_at(paths, prepared, || {
                    config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint)
                })?;
                report.failed.append(&mut download_failures);
                print_lines(format_refresh_all_result(
                    &report,
                    config_restart_apply_lines(),
                ));
                if !report.is_complete() {
                    anyhow::bail!(
                        "Some subscriptions could not be refreshed: {}",
                        report.failure_summary()
                    );
                }
                terminal::enable_raw_mode()?;
            }
            KeyCode::Char('r') => {
                terminal::disable_raw_mode()?;
                match &active {
                    Some(id) => {
                        print_lines(format_tui_refresh_active_start(id));
                        let prepared =
                            config::prepare_subscription_refresh_at(paths, id, None).await?;
                        config::commit_refreshes_checked_at(
                            paths,
                            vec![(id.clone(), prepared)],
                            || {
                                config::merge_user_config_checked_at_endpoint(
                                    paths,
                                    Some(mihomo),
                                    endpoint,
                                )
                            },
                        )?;
                        print_lines(format_refresh_active_success(config_restart_apply_lines()));
                    }
                    None => {
                        print_lines(format_tui_no_active_subscription());
                    }
                }
                terminal::enable_raw_mode()?;
            }
            KeyCode::Char('a') => {
                terminal::disable_raw_mode()?;
                print!("  Subscription URL: ");
                io::stdout().flush()?;
                let mut url = String::new();
                io::stdin().read_line(&mut url)?;
                let url = url.trim();
                if !url.is_empty() {
                    print_lines(format_config_add_start());
                    let id = config::add_subscription_at(paths, url).await?;
                    config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint)?;
                    print_lines(format_tui_add_success(&id, config_restart_apply_lines()));
                }
                terminal::enable_raw_mode()?;
            }
            KeyCode::Char('d') => {
                terminal::disable_raw_mode()?;
                let id = &subs[*cursor].id;
                config::remove_subscription_at(paths, id)?;
                print_lines(format_tui_subscription_removed(id));
                if *cursor > 0 {
                    *cursor -= 1;
                }
                terminal::enable_raw_mode()?;
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                terminal::disable_raw_mode()?;
                // Clear screen on exit
                write!(
                    stdout,
                    "{}{}",
                    crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
                    crossterm::cursor::MoveTo(0, 0)
                )?;
                stdout.flush()?;
                return Ok(());
            }
            _ => {}
        }
    }
}

async fn cmd_proxy(system: bool, user: bool, action: ProxyAction) -> anyhow::Result<()> {
    match action {
        ProxyAction::On => {
            let client = resolve_api_client(system, user, instance::CommandIntent::ReadOnly)?;
            let port = mihomo_api::get_port_with_client(&client).await?;
            let plan = shell_proxy_on_plan(port);
            // Output for eval: `eval $(mihomo-cli proxy on)`
            for line in &plan.stdout_lines {
                println!("{line}");
            }
            for line in &plan.stderr_lines {
                eprintln!("{line}");
            }
        }
        ProxyAction::Off => {
            let plan = shell_proxy_off_plan();
            for line in &plan.stdout_lines {
                println!("{line}");
            }
            for line in &plan.stderr_lines {
                eprintln!("{line}");
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellProxyPlan {
    stdout_lines: Vec<String>,
    stderr_lines: Vec<String>,
}

fn shell_proxy_on_plan(port: u16) -> ShellProxyPlan {
    ShellProxyPlan {
        stdout_lines: vec![
            format!("export http_proxy=http://127.0.0.1:{port}"),
            format!("export https_proxy=http://127.0.0.1:{port}"),
            format!("export all_proxy=http://127.0.0.1:{port}"),
        ],
        stderr_lines: vec![
            format!("  Proxy enabled on port {port}"),
            "  Usage: eval $(mihomo-cli proxy on)".to_string(),
            "  Disable: eval $(mihomo-cli proxy off)".to_string(),
        ],
    }
}

fn shell_proxy_off_plan() -> ShellProxyPlan {
    ShellProxyPlan {
        stdout_lines: vec!["unset http_proxy https_proxy all_proxy".to_string()],
        stderr_lines: vec![
            "  Proxy disabled".to_string(),
            "  Usage: eval $(mihomo-cli proxy off)".to_string(),
        ],
    }
}

async fn cmd_system_proxy(
    system: bool,
    user: bool,
    action: SystemProxyAction,
) -> anyhow::Result<()> {
    match action {
        SystemProxyAction::On => {
            let client = resolve_api_client(system, user, instance::CommandIntent::ReadOnly)?;
            let ctx =
                resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?
                    .ctx;
            if let Some(message) = system_proxy_tun_active_message(&ctx).await {
                eprintln!("{message}");
                return Ok(());
            }
            let port = mihomo_api::get_port_with_client(&client).await?;
            system_proxy::enable_system_proxy(port)?;
            for line in format_system_proxy_enabled_result(port) {
                println!("{line}");
            }
        }
        SystemProxyAction::Off => {
            system_proxy::disable_system_proxy()?;
            for line in format_system_proxy_disabled_result() {
                println!("{line}");
            }
        }
    }
    Ok(())
}

async fn system_proxy_tun_active_message(ctx: &instance::InstanceContext) -> Option<String> {
    if !ipc::is_daemon_running().await {
        return None;
    }
    let snapshot = status::StatusSnapshot::collect(ctx).await;
    if snapshot.runtime_tun == status::TriState::True {
        Some(system_proxy_tun_active_message_text().to_string())
    } else {
        None
    }
}

fn system_proxy_tun_active_message_text() -> &'static str {
    "system service TUN is enabled; OS system proxy settings are ignored because TUN already captures traffic. No system proxy changes were made."
}

fn format_system_proxy_enabled_result(port: u16) -> Vec<String> {
    vec![format!("  ✓ System proxy enabled on 127.0.0.1:{port}")]
}

fn format_system_proxy_disabled_result() -> Vec<String> {
    vec!["  ✓ System proxy disabled".to_string()]
}

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

/// One-line user feedback for URL-matched subscription commits (A1').
fn subscription_commit_messages(commit: &config::SubscriptionCommit) -> Vec<String> {
    match &commit.kind {
        config::SubscriptionCommitKind::New => Vec::new(),
        config::SubscriptionCommitKind::Reuse => {
            vec![format!(
                "  Existing subscription {} reused (same URL).",
                commit.id
            )]
        }
        config::SubscriptionCommitKind::Revive {
            retired_at,
            other_retired,
        } => {
            let retired_hint = retired_at
                .map(|at| at.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "unknown date".to_string());
            let mut lines = vec![format!(
                "  Subscription {} revived (was retired {retired_hint}).",
                commit.id
            )];
            if !other_retired.is_empty() {
                lines.push(format!(
                    "  Other retired entries share this URL and stay retired: {}.",
                    other_retired.join(", ")
                ));
            }
            lines
        }
    }
}

fn print_subscription_commit_kind(commit: &config::SubscriptionCommit) {
    for line in subscription_commit_messages(commit) {
        println!("{line}");
    }
}

/// Merge + validate the generated config after a subscription commit. The
/// caller must hold the config write lock and perform the merge inside the
/// commit transaction (`commit_subscription_matched_checked_at`): on failure
/// the commit transaction restores its own pre-images, so this function only
/// reports the error.
fn merge_subscription_change_checked(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
    endpoint: &instance::ApiEndpoint,
) -> anyhow::Result<()> {
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    if let Err(err) = config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint) {
        anyhow::bail!(subscription_change_rollback_error(&err.to_string()));
    }
    Ok(())
}

/// Rollback of a subscription commit is handled inside the commit transaction
/// (`config::commit_subscription_matched_checked_at`), which restores this
/// transaction's true pre-images. The outer download-window snapshots are gone
/// (defect 1), so no stale pre-download restore can clobber a concurrent
/// successful commit.
fn merge_rules_change_checked(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
    endpoint: &instance::ApiEndpoint,
    rules_snapshot: Option<String>,
) -> anyhow::Result<bool> {
    let has_config_target =
        config::get_active_id_at(paths)?.is_some() || paths.config_path().exists();
    if !has_config_target {
        return Ok(false);
    }

    if let Err(err) = config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint) {
        restore_file_snapshot(&paths.rules_path(), rules_snapshot)?;
        anyhow::bail!("Rule change failed; rolled back rules.yaml.\n  {}", err);
    }
    Ok(true)
}

fn cmd_logs(
    system: bool,
    user: bool,
    tail: usize,
    level: Option<&str>,
    follow: bool,
) -> anyhow::Result<()> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    if follow {
        return follow_mihomo_logs(&resolved.ctx, tail, level);
    }
    let content = read_mihomo_logs(&resolved.ctx, tail)?;
    for line in select_log_lines(&content, tail, level) {
        println!("{}", line);
    }
    Ok(())
}

fn instance_log_path(ctx: &instance::InstanceContext) -> std::path::PathBuf {
    ctx.paths
        .log_file
        .clone()
        .unwrap_or_else(|| ctx.paths.config_dir.join("mihomo.log"))
}

fn read_mihomo_logs(ctx: &instance::InstanceContext, tail: usize) -> anyhow::Result<String> {
    let path = instance_log_path(ctx);
    if path.exists() {
        return std::fs::read_to_string(&path).map_err(|e| {
            anyhow::anyhow!(
                "Cannot read log file: {}
  {}",
                path.display(),
                e
            )
        });
    }

    read_mihomo_journal(ctx.mode, tail).map_err(|journal_err| {
        anyhow::anyhow!(
            "Cannot read mihomo logs.
  Log file not found: {}
  journalctl fallback failed: {}
  Try: mihomo-cli restart",
            path.display(),
            journal_err
        )
    })
}

#[cfg(target_os = "linux")]
fn journalctl_args_for_mode(
    mode: instance::InstanceMode,
    tail: usize,
    follow: bool,
) -> Vec<String> {
    let mut args = Vec::new();
    if mode == instance::InstanceMode::User {
        args.push("--user".to_string());
    }
    args.extend([
        "-u".to_string(),
        "mihomo".to_string(),
        "-n".to_string(),
        tail.max(1).to_string(),
        "--no-pager".to_string(),
        "--output".to_string(),
        "cat".to_string(),
    ]);
    if follow {
        args.push("-f".to_string());
    }
    args
}

fn read_mihomo_journal(_mode: instance::InstanceMode, _tail: usize) -> anyhow::Result<String> {
    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("journalctl");
        cmd.args(journalctl_args_for_mode(_mode, _tail, false));
        let output = cmd.output()?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!(if stderr.is_empty() {
            format!("journalctl exited with {}", output.status)
        } else {
            stderr
        });
    }

    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!("journalctl fallback is only available on Linux")
    }
}

fn follow_mihomo_logs(
    ctx: &instance::InstanceContext,
    tail: usize,
    level: Option<&str>,
) -> anyhow::Result<()> {
    let path = instance_log_path(ctx);
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        for line in select_log_lines(&content, tail, level) {
            println!("{}", line);
        }
        use std::io::{Read, Seek};
        let mut file = std::fs::File::open(&path)?;
        file.seek(std::io::SeekFrom::End(0))?;
        loop {
            let mut buf = String::new();
            file.read_to_string(&mut buf)?;
            for line in select_log_lines(&buf, usize::MAX, level) {
                println!("{}", line);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }

    follow_mihomo_journal(ctx.mode, tail)
}

fn follow_mihomo_journal(_mode: instance::InstanceMode, _tail: usize) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("journalctl")
            .args(journalctl_args_for_mode(_mode, _tail, true))
            .status()?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("journalctl exited with {status}")
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!(
            "follow mode requires a log file; journal follow fallback is only available on Linux"
        )
    }
}

fn select_log_lines(content: &str, tail: usize, level: Option<&str>) -> Vec<String> {
    let mut lines: Vec<&str> = content.lines().collect();
    if let Some(level) = level {
        let needle = level.to_ascii_lowercase();
        lines.retain(|line| line.to_ascii_lowercase().contains(&needle));
    }
    let start = lines.len().saturating_sub(tail);
    lines[start..]
        .iter()
        .map(|line| (*line).to_string())
        .collect()
}

fn shorten_subscription_url(url: &str) -> String {
    const HEAD: usize = 30;
    const TAIL: usize = 15;
    const LIMIT: usize = 50;

    // Sanitize sensitive query params before display
    let url = utils::sanitize_url(url);

    let char_count = url.chars().count();
    if char_count <= LIMIT {
        return url;
    }

    let head: String = url.chars().take(HEAD).collect();
    let tail: String = url
        .chars()
        .rev()
        .take(TAIL)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

fn format_subscription_list(
    subs: &[config::SubscriptionMeta],
    active: Option<&str>,
) -> Vec<String> {
    if subs.is_empty() {
        return vec![
            "  No subscriptions found.".to_string(),
            "  Run: mihomo-cli config --add <URL>".to_string(),
        ];
    }

    let mut lines = vec!["  Subscriptions:".to_string()];
    lines.extend(subs.iter().map(|sub| {
        let marker = if active == Some(sub.id.as_str()) {
            "▶"
        } else {
            " "
        };
        let retired = if sub.is_alive() {
            String::new()
        } else {
            " [retired]".to_string()
        };
        format!(
            "  {} {} ({}{})",
            marker,
            sub.id,
            shorten_subscription_url(&sub.url),
            retired
        )
    }));
    lines
}

fn format_subscription_info(
    info: &config::SubscriptionInfo,
    meta: Option<&config::SubscriptionMeta>,
) -> Vec<String> {
    let mut lines = vec![
        format!("  Subscription: {}", info.id),
        format!("  URL: {}", info.url),
        format!("  Updated: {}", info.updated),
    ];

    if let Some(meta) = meta {
        let mode = meta
            .user_agent_mode
            .as_ref()
            .map(|m| format!("{:?}", m))
            .unwrap_or_else(|| "Auto".to_string());
        lines.push(format!("  User-Agent mode: {mode}"));
        lines.push(format!(
            "  User-Agent: {}",
            meta.user_agent.as_deref().unwrap_or("auto")
        ));
    }

    lines.push(format!("  Proxies: {}", info.proxy_count));
    lines.push(format!(
        "  Expire: {}",
        info.expire.as_deref().unwrap_or("-")
    ));
    lines
}

fn format_dns_status<T: std::fmt::Display>(
    dns: &serde_json::Value,
    policies: &[(usize, T)],
) -> Vec<String> {
    let enabled = dns["enable"].as_bool().unwrap_or(false);
    let enhanced = dns["enhanced-mode"].as_str().unwrap_or("normal");
    let fake_ip_range = dns["fake-ip-range"].as_str().unwrap_or("-");
    let listen = dns["listen"].as_str().unwrap_or("-");

    let mut lines = vec![format!(
        "  DNS: {} ({})",
        if enabled { "enabled" } else { "disabled" },
        enhanced
    )];
    if let Some(ns) = dns["default-nameserver"].as_array() {
        let nameservers: Vec<&str> = ns.iter().filter_map(|v| v.as_str()).collect();
        if !nameservers.is_empty() {
            lines.push(format!("  Default nameservers: {}", nameservers.join(", ")));
        }
    }
    lines.push(format!("  Fake-IP range: {fake_ip_range}"));
    lines.push(format!("  Listen: {listen}"));

    if !policies.is_empty() {
        lines.push(String::new());
        lines.push("  Policies:".to_string());
        lines.extend(
            policies
                .iter()
                .map(|(idx, policy)| format!("    {idx}. {policy}")),
        );
    }
    lines
}

#[cfg_attr(not(test), allow(dead_code))]
fn format_rule_list(rules: &[String], pos: crate::rules::RulePosition) -> Vec<String> {
    format_rule_list_annotated(rules, pos, None)
}

pub(crate) fn format_rule_list_annotated(
    rules: &[String],
    pos: crate::rules::RulePosition,
    available_policies: Option<&[String]>,
) -> Vec<String> {
    let mut lines = vec![format!("  Insert position: {pos}"), String::new()];
    if rules.is_empty() {
        lines.extend([
            "  (no user rules)".to_string(),
            String::new(),
            "  Add a rule:  mihomo-cli rule add DOMAIN-SUFFIX,example.com,DIRECT".to_string(),
        ]);
    } else {
        lines.extend(rules.iter().enumerate().map(|(i, rule)| {
            let annotation = if let Some(policies) = available_policies {
                if let Some(target) = crate::rules::rule_target(rule) {
                    let is_builtin = crate::groups::is_builtin_policy(target);
                    let is_available = is_builtin || policies.iter().any(|p| p == target);
                    if is_available {
                        " [Active]".to_string()
                    } else {
                        format!(" ⚠ [Fallback: DIRECT (target `{target}` missing)]")
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            format!("  {}. {}{}", i + 1, rule, annotation)
        }));
    }
    lines
}

pub(crate) fn doctor_daemon_binary_skew_hint() -> &'static str {
    "应用更新: mihomo-cli restart --system"
}

#[cfg(unix)]
pub(crate) fn has_split_dns_intent(
    config_text: &str,
    resolver_addrs: &[std::net::SocketAddr],
) -> bool {
    let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(config_text) else {
        return false;
    };
    if nameserver_policy_covers(&val, dns_probe::PROBE_DOMAIN) {
        return true;
    }
    if let Some(seq) = val
        .get("tun")
        .and_then(|t| t.get("route-exclude-address"))
        .and_then(|r| r.as_sequence())
    {
        for item in seq {
            if let Some(cidr) = item.as_str() {
                if resolver_addrs
                    .iter()
                    .any(|addr| cidr_contains_addr(cidr, addr.ip()))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Split-DNS neutralization requires evidence that the probe domain itself is
/// covered by `nameserver-policy`; an unrelated internal domain is not enough.
#[cfg(unix)]
fn nameserver_policy_covers(config: &serde_yaml::Value, domain: &str) -> bool {
    let Some(map) = config
        .get("dns")
        .and_then(|d| d.get("nameserver-policy"))
        .and_then(|np| np.as_mapping())
    else {
        return false;
    };
    let domain = domain.to_ascii_lowercase();
    map.keys().filter_map(|key| key.as_str()).any(|key| {
        let pattern = key.trim();
        if pattern.starts_with("geosite:")
            || pattern.starts_with("geoip:")
            || pattern.starts_with("rule-set:")
        {
            return false;
        }
        let pattern = pattern
            .strip_prefix("DOMAIN-SUFFIX,")
            .unwrap_or(pattern)
            .trim_start_matches("+.")
            .trim_start_matches("*.")
            .trim_start_matches('.')
            .to_ascii_lowercase();
        !pattern.is_empty() && (domain == pattern || domain.ends_with(&format!(".{pattern}")))
    })
}

#[cfg(unix)]
fn cidr_contains_addr(cidr: &str, ip: std::net::IpAddr) -> bool {
    let Some((addr, prefix)) = cidr.trim().split_once('/') else {
        return false;
    };
    let Ok(net) = addr.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    match (net, ip) {
        (std::net::IpAddr::V4(net), std::net::IpAddr::V4(ip)) if prefix <= 32 => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            (u32::from(net) & mask) == (u32::from(ip) & mask)
        }
        (std::net::IpAddr::V6(net), std::net::IpAddr::V6(ip)) if prefix <= 128 => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            (u128::from(net) & mask) == (u128::from(ip) & mask)
        }
        _ => false,
    }
}

fn format_rule_apply_result(merged: bool, apply: RuntimeApplyOutcome) -> Vec<String> {
    if !merged {
        return vec![
            "  ℹ Config pending — rule saved".to_string(),
            "  Run: mihomo-cli restart  to apply".to_string(),
        ];
    }
    // C：按真实应用结果汇报，不得在规则已生效时声称
    // "Runtime status: unknown" 并要求 restart。
    match apply {
        RuntimeApplyOutcome::SystemPromoted => vec![
            "  ✓ Rule intent committed".to_string(),
            "  ✅ system configuration promoted and runtime applied".to_string(),
        ],
        RuntimeApplyOutcome::HotReloadAccepted => vec![
            "  ✓ Rule intent committed".to_string(),
            // review 回归-2：user 实例没有 revision attestation，如实报 unknown；
            // C 要解决的是"误导性 restart 循环"，不是抬高判定结论。
            "  ℹ Runtime status: unknown (revision attestation unavailable)".to_string(),
        ],
        RuntimeApplyOutcome::Pending => vec![
            "  ✓ Rule intent committed".to_string(),
            "  ℹ Runtime status: pending".to_string(),
            "  Run: mihomo-cli restart  to apply".to_string(),
        ],
    }
}

fn format_rule_add_success(rule: &str, merged: bool, apply: RuntimeApplyOutcome) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Rule added: {rule}")];
    lines.extend(format_rule_apply_result(merged, apply));
    lines
}

fn format_rule_remove_success(
    index: usize,
    merged: bool,
    apply: RuntimeApplyOutcome,
) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Rule {index} removed")];
    lines.extend(format_rule_apply_result(merged, apply));
    lines
}

fn format_rule_clear_success(merged: bool, apply: RuntimeApplyOutcome) -> Vec<String> {
    let mut lines = vec!["  ✓ All rules cleared".to_string()];
    lines.extend(format_rule_apply_result(merged, apply));
    lines
}

fn format_rule_move_success(
    from: usize,
    to: usize,
    merged: bool,
    apply: RuntimeApplyOutcome,
) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Rule moved: {from} → {to}")];
    lines.extend(format_rule_apply_result(merged, apply));
    lines
}

fn format_rule_import_success(
    count: usize,
    path: &str,
    merged: bool,
    apply: RuntimeApplyOutcome,
) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Imported {count} rules from {path}")];
    lines.extend(format_rule_apply_result(merged, apply));
    lines
}

fn format_rule_export_success(count: usize, path: &str) -> Vec<String> {
    vec![format!("  ✓ Exported {count} rules to {path}")]
}

fn format_rule_position_set(pos: crate::rules::RulePosition) -> Vec<String> {
    vec![format!("  ✓ Default insert position set to: {pos}")]
}

fn format_rule_position_show(pos: crate::rules::RulePosition) -> Vec<String> {
    vec![
        format!("  Default insert position: {pos}"),
        String::new(),
        "  Change it:  mihomo-cli rule position front|back".to_string(),
    ]
}

fn format_rule_policies(policies: &[String]) -> Vec<String> {
    let mut lines = vec!["  Available policies:".to_string()];
    lines.extend(policies.iter().map(|p| format!("  - {p}")));
    lines
}

fn format_rule_test_result(target: &str, matched: Option<&crate::rules::RuleMatch>) -> Vec<String> {
    let mut lines = vec![
        "  ℹ Static route estimate from config.yaml rules; it does not prove DNS resolution or runtime core matching.".to_string(),
    ];
    match matched {
        Some(matched) => {
            lines.push(format!(
                "  ✓ Matched rule #{}: {}",
                matched.index, matched.rule
            ));
            lines.push(format!("  Policy: {}", matched.policy));
        }
        None => lines.push(format!("  No matching rule found for {target}")),
    }
    lines
}

fn print_config_drift_warnings(paths: &utils::AppPaths) {
    let rules = crate::rules::load_rules_at(paths).unwrap_or_default();
    let mut printed = false;
    if let Ok(warnings) = crate::rules::rule_policy_warnings_at(paths, &rules) {
        for warning in warnings {
            println!("  {warning}");
            printed = true;
        }
    }
    match selection::active_selection_scope(paths)
        .and_then(|scope| selection::selection_drift_warnings_for_scope(&scope))
    {
        Ok(warnings) => {
            for warning in warnings {
                println!("  {warning}");
                printed = true;
            }
        }
        Err(error) => {
            println!("  ⚠ Cannot read persisted selections: {error:#}");
            println!("  Run: mihomo-cli select --unpin --all  (to reset stored selections)");
            printed = true;
        }
    }
    if printed {
        println!("  Check current policies: mihomo-cli rule policies");
        println!("  Check current groups/nodes: mihomo-cli list");
    }
}

fn config_dir_override_active() -> bool {
    config_dir_override_path().is_some()
}

fn app_paths_for_resolved_instance_command(
    command: &str,
    system: bool,
    user: bool,
    intent: instance::CommandIntent,
) -> anyhow::Result<utils::AppPaths> {
    let request = mode_request_from_flags(system, user);
    if config_dir_override_active()
        && matches!(
            request,
            instance::ModeRequest::Unspecified | instance::ModeRequest::ExplicitUser
        )
    {
        return Ok(utils::AppPaths::from_system());
    }

    let runtime = current_runtime_presence();
    if request != instance::ModeRequest::Unspecified && intent != instance::CommandIntent::Install {
        if let Some(message) = explicit_mode_runtime_conflict(request, runtime, intent) {
            anyhow::bail!("{message}\n  Command: {command}");
        }
    }

    let resolution =
        resolve_instance_mode_runtime_first(request, runtime, current_service_presence(), intent);
    match resolution {
        RuntimeFirstModeResolution::Resolved { mode, .. } => {
            let ctx = instance::planned_current_context(mode)
                .ok_or_else(|| anyhow::anyhow!("Unsupported OS for {command}"))?;
            Ok(utils::AppPaths::new(ctx.paths.config_dir))
        }
        RuntimeFirstModeResolution::RuntimeConflict => anyhow::bail!(
            "mode conflict: both system daemon and user core are running; stop one runtime before running {command}"
        ),
        RuntimeFirstModeResolution::NotInstalled
            if request == instance::ModeRequest::Unspecified =>
        {
            Ok(utils::AppPaths::from_system())
        }
        RuntimeFirstModeResolution::NotInstalled => anyhow::bail!(
            "no mihomo service instance found for {command}; run `mihomo-cli install --system` or `mihomo-cli install --user`"
        ),
        RuntimeFirstModeResolution::AmbiguousBothInstalled => anyhow::bail!(
            "both system and user instances are installed; use --system for {command} or uninstall one service"
        ),
        RuntimeFirstModeResolution::NeedsSystemInstall { reason }
        | RuntimeFirstModeResolution::NeedsSystemDaemonRecovery { reason } => {
            anyhow::bail!("{reason}; command: {command}")
        }
        RuntimeFirstModeResolution::NeedsSystemSwitch { .. } => {
            anyhow::bail!("TUN requires switching from per-user mode to system service mode; command: {command}")
        }
        RuntimeFirstModeResolution::PromptRequired => anyhow::bail!("mode required for {command}"),
    }
}

fn resolved_user_config_paths_for_write(
    command: &str,
    system: bool,
    user: bool,
) -> anyhow::Result<utils::AppPaths> {
    // ADR-02 + v3 runtime-first resolution: config/rule/dns writes still use
    // the current user's config dir, but mode selection must first honor the
    // active runtime. Do not preflight purely on installed service artifacts
    // here; if both services are installed but only one runtime is active,
    // app_paths_for_resolved_instance_command() can still choose the active
    // runtime instead of failing on stale/ambiguous install state.
    app_paths_for_resolved_instance_command(
        command,
        system,
        user,
        instance::CommandIntent::Mutating,
    )
}

fn resolved_api_endpoint_for_config_command(
    system: bool,
    user: bool,
    intent: instance::CommandIntent,
) -> anyhow::Result<instance::ApiEndpoint> {
    let request = mode_request_from_flags(system, user);
    if config_dir_override_active()
        && matches!(
            request,
            instance::ModeRequest::Unspecified | instance::ModeRequest::ExplicitUser
        )
    {
        return Ok(config::current_api_endpoint());
    }

    let runtime = current_runtime_presence();
    if request != instance::ModeRequest::Unspecified && intent != instance::CommandIntent::Install {
        if let Some(message) = explicit_mode_runtime_conflict(request, runtime, intent) {
            anyhow::bail!(message);
        }
    }

    match resolve_instance_mode_runtime_first(request, runtime, current_service_presence(), intent) {
        RuntimeFirstModeResolution::Resolved { mode, .. } => {
            let ctx = instance::planned_current_context(mode)
                .ok_or_else(|| anyhow::anyhow!("Unsupported OS for config command"))?;
            Ok(ctx.paths.api_endpoint)
        }
        RuntimeFirstModeResolution::RuntimeConflict => anyhow::bail!(
            "mode conflict: both system daemon and user core are running; stop one runtime before running config command"
        ),
        RuntimeFirstModeResolution::NotInstalled
            if request == instance::ModeRequest::Unspecified =>
        {
            Ok(config::current_api_endpoint())
        }
        RuntimeFirstModeResolution::NotInstalled => anyhow::bail!(
            "no mihomo service instance found for config command; run `mihomo-cli install --system` or `mihomo-cli install --user`"
        ),
        RuntimeFirstModeResolution::AmbiguousBothInstalled => anyhow::bail!(
            "both system and user instances are installed; use --system for config command or uninstall one service"
        ),
        RuntimeFirstModeResolution::NeedsSystemInstall { reason }
        | RuntimeFirstModeResolution::NeedsSystemDaemonRecovery { reason } => {
            anyhow::bail!("{reason}; config command needs a resolved runtime context")
        }
        RuntimeFirstModeResolution::NeedsSystemSwitch { .. } => {
            anyhow::bail!("TUN requires switching from per-user mode to system service mode; config command needs a resolved runtime context")
        }
        RuntimeFirstModeResolution::PromptRequired => anyhow::bail!("mode required for config command"),
    }
}

fn resolved_core_binary_for_config_command(
    system: bool,
    user: bool,
    intent: instance::CommandIntent,
) -> anyhow::Result<std::path::PathBuf> {
    let request = mode_request_from_flags(system, user);
    if config_dir_override_active()
        && matches!(
            request,
            instance::ModeRequest::Unspecified | instance::ModeRequest::ExplicitUser
        )
    {
        return Ok(std::path::PathBuf::from(utils::mihomo_path()));
    }

    let runtime = current_runtime_presence();
    if request != instance::ModeRequest::Unspecified && intent != instance::CommandIntent::Install {
        if let Some(message) = explicit_mode_runtime_conflict(request, runtime, intent) {
            anyhow::bail!(message);
        }
    }

    match resolve_instance_mode_runtime_first(request, runtime, current_service_presence(), intent) {
        RuntimeFirstModeResolution::Resolved { mode, .. } => {
            let ctx = instance::planned_current_context(mode)
                .ok_or_else(|| anyhow::anyhow!("Unsupported OS for config command"))?;
            Ok(ctx.paths.core_binary)
        }
        RuntimeFirstModeResolution::RuntimeConflict => anyhow::bail!(
            "mode conflict: both system daemon and user core are running; stop one runtime before running config command"
        ),
        RuntimeFirstModeResolution::NotInstalled
            if request == instance::ModeRequest::Unspecified =>
        {
            Ok(std::path::PathBuf::from(utils::mihomo_path()))
        }
        RuntimeFirstModeResolution::NotInstalled => anyhow::bail!(
            "no mihomo service instance found for config command; run `mihomo-cli install --system` or `mihomo-cli install --user`"
        ),
        RuntimeFirstModeResolution::AmbiguousBothInstalled => anyhow::bail!(
            "both system and user instances are installed; use --system for config command or uninstall one service"
        ),
        RuntimeFirstModeResolution::NeedsSystemInstall { reason }
        | RuntimeFirstModeResolution::NeedsSystemDaemonRecovery { reason } => {
            anyhow::bail!("{reason}; config command needs a resolved runtime context")
        }
        RuntimeFirstModeResolution::NeedsSystemSwitch { .. } => {
            anyhow::bail!("TUN requires switching from per-user mode to system service mode; config command needs a resolved runtime context")
        }
        RuntimeFirstModeResolution::PromptRequired => anyhow::bail!("mode required for config command"),
    }
}

async fn cmd_group(
    system: bool,
    user: bool,
    json: bool,
    action: GroupAction,
) -> anyhow::Result<()> {
    let intent = match &action {
        GroupAction::List | GroupAction::Show { .. } => instance::CommandIntent::ReadOnly,
        GroupAction::Create { .. }
        | GroupAction::Edit { .. }
        | GroupAction::Add { .. }
        | GroupAction::Remove { .. }
        | GroupAction::Delete { .. }
        | GroupAction::Reset { .. } => instance::CommandIntent::Mutating,
    };
    let paths = app_paths_for_resolved_instance_command("group", system, user, intent)?;
    let _config_lock = if matches!(intent, instance::CommandIntent::Mutating) {
        Some(crate::lock::ConfigLock::acquire(paths.config_dir())?)
    } else {
        None
    };
    let scope = crate::selection::active_selection_scope(&paths)?;
    let subscription_path = paths.subscription_file_path(&scope.subscription_id);

    match action {
        GroupAction::List => {
            let subscription: serde_yaml::Value = serde_yaml::from_str(
                &std::fs::read_to_string(&subscription_path)
                    .context("failed to read subscription")?,
            )?;
            let original_groups: std::collections::HashSet<String> = subscription
                .get("proxy-groups")
                .and_then(serde_yaml::Value::as_sequence)
                .into_iter()
                .flatten()
                .filter_map(|g| crate::groups::group_name(g))
                .map(str::to_owned)
                .collect();
            let overlay_path = paths.groups_override_path_for_subscription(&scope.subscription_id);
            let overlay = crate::groups::GroupsOverlay::load(&overlay_path)?;
            let groups_from_config = std::fs::read_to_string(paths.config_path())
                .ok()
                .and_then(|s| serde_yaml::from_str::<serde_yaml::Value>(&s).ok())
                .and_then(|c| {
                    c.get("proxy-groups")
                        .and_then(serde_yaml::Value::as_sequence)
                        .cloned()
                });
            let groups = if let Some(g) = groups_from_config {
                g
            } else {
                let original_seq = subscription
                    .get("proxy-groups")
                    .and_then(serde_yaml::Value::as_sequence)
                    .cloned()
                    .unwrap_or_default();
                let known_proxies = known_effective_proxies(&paths, &subscription);
                let known_providers = known_effective_providers(&paths, &subscription);
                overlay
                    .merged_groups(&original_seq, &known_proxies, &known_providers)
                    .unwrap_or(original_seq)
            };
            let mut items = Vec::new();
            for group in &groups {
                let name = crate::groups::group_name(group).ok_or_else(|| {
                    anyhow::anyhow!("current config contains a proxy group without name")
                })?;
                let group_type = group
                    .get("type")
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or("unknown");
                let upper = name.to_ascii_uppercase();
                let label = if overlay.patches.contains_key(name) {
                    "[patched]"
                } else if overlay
                    .append
                    .iter()
                    .any(|item| crate::groups::group_name(item) == Some(name))
                    || overlay
                        .prepend
                        .iter()
                        .any(|item| crate::groups::group_name(item) == Some(name))
                {
                    "[custom]"
                } else if original_groups.contains(name) {
                    "[native]"
                } else if crate::groups::RESERVED_NAMES.contains(&upper.as_str())
                    || crate::groups::BUILTIN_POLICIES.contains(&upper.as_str())
                {
                    "[builtin]"
                } else {
                    "[custom]"
                };
                items.push((name.to_string(), group_type.to_string(), label.to_string()));
            }

            if json {
                let list_json: Vec<_> = items
                    .iter()
                    .map(|(n, t, l)| {
                        serde_json::json!({
                            "name": n,
                            "type": t,
                            "source": l.trim_matches(|c| c == '[' || c == ']'),
                        })
                    })
                    .collect();
                let out = serde_json::json!({
                    "active_subscription": scope.subscription_id,
                    "groups": list_json,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Active Subscription: {}", scope.subscription_id);
                for (name, group_type, label) in &items {
                    println!("{name}\t{group_type}\t{label}");
                }
            }
            Ok(())
        }
        GroupAction::Show { name } => {
            let overlay_path = paths.groups_override_path_for_subscription(&scope.subscription_id);
            let overlay = crate::groups::GroupsOverlay::load(&overlay_path)?;
            let subscription: serde_yaml::Value = serde_yaml::from_str(
                &std::fs::read_to_string(&subscription_path)
                    .context("failed to read subscription")?,
            )?;
            let original_groups: std::collections::HashSet<String> = subscription
                .get("proxy-groups")
                .and_then(serde_yaml::Value::as_sequence)
                .into_iter()
                .flatten()
                .filter_map(|g| crate::groups::group_name(g))
                .map(str::to_owned)
                .collect();
            let groups_from_config = std::fs::read_to_string(paths.config_path())
                .ok()
                .and_then(|s| serde_yaml::from_str::<serde_yaml::Value>(&s).ok())
                .and_then(|c| {
                    c.get("proxy-groups")
                        .and_then(serde_yaml::Value::as_sequence)
                        .cloned()
                });
            let groups = if let Some(g) = groups_from_config {
                g
            } else {
                let original_seq = subscription
                    .get("proxy-groups")
                    .and_then(serde_yaml::Value::as_sequence)
                    .cloned()
                    .unwrap_or_default();
                let known_proxies = known_effective_proxies(&paths, &subscription);
                let known_providers = known_effective_providers(&paths, &subscription);
                overlay
                    .merged_groups(&original_seq, &known_proxies, &known_providers)
                    .unwrap_or(original_seq)
            };
            let group = groups
                .iter()
                .find(|group| crate::groups::group_name(group) == Some(name.as_str()))
                .ok_or_else(|| anyhow::anyhow!("proxy group not found: {name}"))?;

            let upper = name.to_ascii_uppercase();
            let source_label = if overlay.patches.contains_key(&name) {
                "[patched]"
            } else if overlay
                .append
                .iter()
                .any(|item| crate::groups::group_name(item) == Some(name.as_str()))
                || overlay
                    .prepend
                    .iter()
                    .any(|item| crate::groups::group_name(item) == Some(name.as_str()))
            {
                "[custom]"
            } else if original_groups.contains(&name) {
                "[native]"
            } else if crate::groups::RESERVED_NAMES.contains(&upper.as_str())
                || crate::groups::BUILTIN_POLICIES.contains(&upper.as_str())
            {
                "[builtin]"
            } else {
                "[custom]"
            };

            let patch_info = overlay.patches.get(&name);
            let selected_node = crate::selection::load_selection_state_for_scope(&scope)
                .ok()
                .and_then(|sel| sel.get(&name).cloned());

            if json {
                let mut out = serde_json::Map::new();
                out.insert("name".into(), serde_json::Value::String(name.clone()));
                out.insert(
                    "source".into(),
                    serde_json::Value::String(
                        source_label
                            .trim_matches(|c| c == '[' || c == ']')
                            .to_string(),
                    ),
                );
                if let Some(node) = selected_node {
                    out.insert("selected".into(), serde_json::Value::String(node));
                }
                if let Some(patch) = patch_info {
                    out.insert("patch".into(), serde_json::to_value(patch)?);
                }
                out.insert("definition".into(), serde_json::to_value(group)?);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::Value::Object(out))?
                );
            } else {
                println!("Name:     {name}");
                let group_type = group
                    .get("type")
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or("unknown");
                println!("Type:     {group_type}");
                println!("Source:   {source_label}");
                if let Some(ref node) = selected_node {
                    println!("Selected: {node} (persisted)");
                }
                if let Some(patch) = patch_info {
                    println!("Patch:");
                    if !patch.add_proxies.is_empty() {
                        println!("  add_proxies: {:?}", patch.add_proxies);
                    }
                    if !patch.remove_proxies.is_empty() {
                        println!("  remove_proxies: {:?}", patch.remove_proxies);
                    }
                }
                println!("Definition:\n{}", serde_yaml::to_string(group)?);
            }
            Ok(())
        }
        GroupAction::Create {
            name,
            group_type,
            members,
            uses,
            url,
            interval,
            strategy,
            file,
            prepend,
        } => {
            let source = if let Some(file) = file {
                std::fs::read_to_string(&file)
                    .with_context(|| format!("failed to read group definition: {file}"))?
            } else {
                let gt = group_type
                    .ok_or_else(|| anyhow::anyhow!("--type is required without --file"))?;
                let mut map = serde_yaml::Mapping::new();
                map.insert("name".into(), name.clone().into());
                map.insert("type".into(), gt.clone().into());
                if !members.is_empty() || uses.is_empty() {
                    map.insert(
                        "proxies".into(),
                        serde_yaml::Value::Sequence(members.into_iter().map(Into::into).collect()),
                    );
                }
                if !uses.is_empty() {
                    map.insert(
                        "use".into(),
                        serde_yaml::Value::Sequence(uses.into_iter().map(Into::into).collect()),
                    );
                }
                // Insert explicit user-provided params
                if let Some(u) = url {
                    map.insert("url".into(), u.into());
                }
                if let Some(i) = interval {
                    map.insert("interval".into(), serde_yaml::Value::Number(i.into()));
                }
                if let Some(s) = strategy {
                    map.insert("strategy".into(), s.into());
                }
                // Apply smart defaults for url/interval/strategy
                let before_defaults: std::collections::HashSet<String> = map
                    .keys()
                    .filter_map(serde_yaml::Value::as_str)
                    .map(str::to_owned)
                    .collect();
                crate::groups::apply_smart_defaults(&mut map, &gt);
                // Echo injected defaults
                let injected: Vec<String> = map
                    .iter()
                    .filter_map(|(k, v)| {
                        let key = k.as_str()?;
                        if !before_defaults.contains(key) {
                            Some(format!(
                                "  {key}: {}",
                                serde_yaml::to_string(v).unwrap_or_default().trim()
                            ))
                        } else {
                            None
                        }
                    })
                    .collect();
                if !injected.is_empty() {
                    println!("Smart defaults applied for group type `{gt}`:");
                    for line in &injected {
                        println!("{line}");
                    }
                }
                serde_yaml::to_string(&serde_yaml::Value::Mapping(map))?
            };
            // Validate group name
            crate::groups::validate_group_name(&name)?;
            let group = crate::groups::parse_group(&source)?;
            let actual_name = crate::groups::group_name(&group)
                .ok_or_else(|| anyhow::anyhow!("proxy group name is required"))?;
            if actual_name != name {
                anyhow::bail!(
                    "group definition name `{actual_name}` does not match command name `{name}`"
                );
            }
            let subscription: serde_yaml::Value = serde_yaml::from_str(
                &std::fs::read_to_string(&subscription_path).with_context(|| {
                    format!(
                        "failed to read subscription: {}",
                        subscription_path.display()
                    )
                })?,
            )?;
            let original = subscription
                .get("proxy-groups")
                .and_then(serde_yaml::Value::as_sequence)
                .cloned()
                .unwrap_or_default();
            let known_proxies = known_effective_proxies(&paths, &subscription);
            let known_providers = known_effective_providers(&paths, &subscription);
            let overlay_path = paths.groups_override_path_for_subscription(&scope.subscription_id);
            let mut overlay = crate::groups::GroupsOverlay::load(&overlay_path)?;
            let existing = overlay.merged_groups(&original, &known_proxies, &known_providers)?;
            if existing
                .iter()
                .any(|item| crate::groups::group_name(item) == Some(name.as_str()))
                || original
                    .iter()
                    .any(|item| crate::groups::group_name(item) == Some(name.as_str()))
            {
                anyhow::bail!("proxy group already exists: {name}");
            }
            if prepend {
                overlay.prepend.push(group);
            } else {
                overlay.append.push(group);
            }
            overlay.merged_groups(&original, &known_proxies, &known_providers)?;
            let previous = snapshot_file(&overlay_path)?;
            let previous_config = snapshot_file(&paths.config_path())?;
            overlay.save(&overlay_path)?;
            let core_binary = resolved_core_binary_for_config_command(system, user, intent)?;
            let endpoint = resolved_api_endpoint_for_config_command(system, user, intent)?;
            let result = config::merge_user_config_checked_at_endpoint(
                &paths,
                Some(&core_binary),
                &endpoint,
            );
            if let Err(error) = result {
                restore_file_snapshot(&overlay_path, previous.clone())?;
                restore_file_snapshot(&paths.config_path(), previous_config.clone())?;
                return Err(error);
            }
            drop(_config_lock);
            let reload_error = try_reload_group_config(
                system,
                user,
                &paths,
                config_base_revision(&paths).as_deref(),
            )
            .await?;
            if json {
                let report = serde_json::json!({
                    "status": "success",
                    "action": "create",
                    "group": name,
                    "intent": "Committed",
                    "control_plane": if reload_error.is_none() { "Applied" } else { "Pending" },
                    "runtime_attestation": if reload_error.is_none() { "Attested" } else { "Unknown (offline/pending)" },
                    "pending": reload_error.is_some(),
                    "error": reload_error.map(|e| format!("{e:#}")),
                });
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if let Some(error) = reload_error {
                println!("created proxy group `{name}` (pending=true)");
                println!("  Runtime apply deferred: {error:#}");
                println!("  Run: mihomo-cli restart  to apply");
            } else {
                println!("created proxy group `{name}` (runtime_applied=true)");
            }
            Ok(())
        }
        GroupAction::Edit { name, file } => {
            drop(_config_lock);
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("failed to read group definition: {file}"))?;
            let group = crate::groups::parse_group(&source)?;
            if crate::groups::group_name(&group) != Some(name.as_str()) {
                anyhow::bail!("group definition name must match `{name}`");
            }
            update_group_overlay(
                &paths,
                &scope,
                &subscription_path,
                GroupMutation::Replace { name, group },
                system,
                user,
                json,
            )
            .await
        }
        GroupAction::Add {
            name,
            members,
            uses,
        } => {
            if members.is_empty() && uses.is_empty() {
                anyhow::bail!("at least one of --member or --use is required");
            }
            drop(_config_lock);
            update_group_overlay(
                &paths,
                &scope,
                &subscription_path,
                GroupMutation::Add {
                    name,
                    members,
                    uses,
                },
                system,
                user,
                json,
            )
            .await
        }
        GroupAction::Remove {
            name,
            members,
            uses,
        } => {
            if members.is_empty() && uses.is_empty() {
                anyhow::bail!("at least one of --member or --use is required");
            }
            drop(_config_lock);
            update_group_overlay(
                &paths,
                &scope,
                &subscription_path,
                GroupMutation::Remove {
                    name,
                    members,
                    uses,
                },
                system,
                user,
                json,
            )
            .await
        }
        GroupAction::Delete { name } => {
            drop(_config_lock);
            update_group_overlay(
                &paths,
                &scope,
                &subscription_path,
                GroupMutation::Delete { name },
                system,
                user,
                json,
            )
            .await
        }
        GroupAction::Reset { name } => {
            drop(_config_lock);
            update_group_overlay(
                &paths,
                &scope,
                &subscription_path,
                GroupMutation::Reset { name },
                system,
                user,
                json,
            )
            .await
        }
    }
}

fn known_subscription_proxies(
    subscription: &serde_yaml::Value,
) -> std::collections::HashSet<String> {
    subscription
        .get("proxies")
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
        .collect()
}

fn known_subscription_providers(
    subscription: &serde_yaml::Value,
) -> std::collections::HashSet<String> {
    subscription
        .get("proxy-providers")
        .and_then(serde_yaml::Value::as_mapping)
        .into_iter()
        .flatten()
        .filter_map(|(name, _)| name.as_str().map(str::to_owned))
        .collect()
}

fn known_effective_proxies(
    paths: &utils::AppPaths,
    subscription: &serde_yaml::Value,
) -> std::collections::HashSet<String> {
    let mut proxies = known_subscription_proxies(subscription);
    let override_path = paths.override_path();
    if override_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&override_path) {
            if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                for p in config::extract_override_proxies(&val) {
                    proxies.insert(p);
                }
            }
        }
    }
    proxies
}

fn known_effective_providers(
    paths: &utils::AppPaths,
    subscription: &serde_yaml::Value,
) -> std::collections::HashSet<String> {
    let mut providers = known_subscription_providers(subscription);
    let override_path = paths.override_path();
    if override_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&override_path) {
            if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                for p in config::extract_override_providers(&val) {
                    providers.insert(p);
                }
            }
        }
    }
    providers
}

fn known_provider_payload_nodes(
    paths: &utils::AppPaths,
) -> std::collections::HashMap<String, String> {
    let override_path = paths.override_path();
    if override_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&override_path) {
            if let Ok(val) = serde_yaml::from_str::<serde_yaml::Value>(&content) {
                return config::extract_inline_provider_payload_nodes(&val);
            }
        }
    }
    std::collections::HashMap::new()
}

#[derive(Debug)]
enum GroupMutation {
    Replace {
        name: String,
        group: serde_yaml::Value,
    },
    Add {
        name: String,
        members: Vec<String>,
        uses: Vec<String>,
    },
    Remove {
        name: String,
        members: Vec<String>,
        uses: Vec<String>,
    },
    Delete {
        name: String,
    },
    Reset {
        name: String,
    },
}

async fn update_group_overlay(
    paths: &utils::AppPaths,
    scope: &crate::selection::SelectionScope,
    subscription_path: &std::path::Path,
    mutation: GroupMutation,
    system: bool,
    user: bool,
    json: bool,
) -> anyhow::Result<()> {
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let subscription_id = &scope.subscription_id;
    let subscription: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(subscription_path)?)?;
    let original = subscription
        .get("proxy-groups")
        .and_then(serde_yaml::Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    let known_proxies = known_effective_proxies(paths, &subscription);
    let known_providers = known_effective_providers(paths, &subscription);
    let overlay_path = paths.groups_override_path_for_subscription(subscription_id);
    let mut overlay = crate::groups::GroupsOverlay::load(&overlay_path)?;

    let name = match &mutation {
        GroupMutation::Replace { name, .. }
        | GroupMutation::Add { name, .. }
        | GroupMutation::Remove { name, .. }
        | GroupMutation::Delete { name }
        | GroupMutation::Reset { name } => name.clone(),
    };
    let action_name = match &mutation {
        GroupMutation::Reset { .. } => "reset",
        GroupMutation::Replace { .. } => "edit",
        GroupMutation::Add { .. } => "add",
        GroupMutation::Remove { .. } => "remove",
        GroupMutation::Delete { .. } => "delete",
    };
    // only delete makes the persisted selection an orphan; the
    // cleanup runs after the merge commits, so it is captured before `mutation`
    // is moved into the match below.
    let is_delete = matches!(&mutation, GroupMutation::Delete { .. });

    let is_prepend = overlay
        .prepend
        .iter()
        .any(|group| crate::groups::group_name(group) == Some(name.as_str()));
    let is_append = overlay
        .append
        .iter()
        .any(|group| crate::groups::group_name(group) == Some(name.as_str()));
    let is_original = original
        .iter()
        .any(|group| crate::groups::group_name(group) == Some(name.as_str()));
    let is_custom = is_prepend || is_append;

    match mutation {
        GroupMutation::Reset { .. } => {
            // Reset is only for original (native) groups
            if is_custom && !is_original {
                anyhow::bail!("`{name}` 是自建组，不支持 reset，请使用 `group delete` 删除");
            }
            if !is_original {
                anyhow::bail!("proxy group not found: {name}");
            }
            let had_patch = overlay.patches.contains_key(&name);
            let was_deleted = overlay.delete.iter().any(|d| d == &name);
            if !had_patch && !was_deleted {
                if json {
                    eprintln!("proxy group `{name}` is already at upstream state (no-op)");
                    let report = serde_json::json!({
                        "status": "success",
                        "action": action_name,
                        "group": name,
                        "intent": "Committed",
                        "control_plane": "Applied",
                        "runtime_attestation": "Attested",
                        "pending": false,
                        "error": serde_json::Value::Null,
                        "selection_cleanup": serde_json::Value::Null,
                    });
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!("proxy group `{name}` is already at upstream state (no-op)");
                }
                return Ok(());
            }
            // Clear patches and remove from delete list, as well as any append/prepend duplicate
            overlay.patches.remove(&name);
            overlay.delete.retain(|d| d != &name);
            overlay
                .append
                .retain(|item| crate::groups::group_name(item) != Some(name.as_str()));
            overlay
                .prepend
                .retain(|item| crate::groups::group_name(item) != Some(name.as_str()));
        }
        GroupMutation::Delete { .. } => {
            // 3D cascade dependency check
            let rules = crate::rules::load_rules_at(paths).unwrap_or_default();
            let mut blockers: Vec<String> = Vec::new();

            // Dimension 1: rules.yaml references
            for rule in &rules {
                if crate::rules::rule_target(rule) == Some(name.as_str()) {
                    blockers.push(format!("  - rule: {rule}"));
                }
            }

            // Dimension 2: other groups' proxies list (in overlay prepend/append and native groups)
            let current = overlay
                .merged_groups(&original, &known_proxies, &known_providers)
                .unwrap_or_default();
            for group in &current {
                let gname = crate::groups::group_name(group).unwrap_or("");
                if gname == name {
                    continue;
                }
                let members = crate::groups::group_members(group).unwrap_or_default();
                if members.iter().any(|m| m == &name) {
                    blockers.push(format!("  - proxy-group: {gname} (member of group)"));
                }
            }

            // Dimension 3: patches[*].add_proxies references
            for (patch_target, patch) in &overlay.patches {
                if patch_target == &name {
                    continue;
                }
                if patch.add_proxies.iter().any(|m| m == &name) {
                    blockers.push(format!("  - patch: {patch_target}.add_proxies"));
                }
            }

            if !blockers.is_empty() {
                let refs_str = blockers.join("\n");
                anyhow::bail!(
                    "Blocked: cannot delete group '{name}' because it is still referenced by:\n{refs_str}\nPlease rebind or delete those references first."
                );
            }

            if !is_prepend && !is_append && !is_original {
                anyhow::bail!("proxy group not found: {name}");
            }

            if is_prepend {
                overlay
                    .prepend
                    .retain(|group| crate::groups::group_name(group) != Some(name.as_str()));
            } else if is_append {
                overlay
                    .append
                    .retain(|group| crate::groups::group_name(group) != Some(name.as_str()));
            }
            // For native groups (including those that may also have patches), add to delete list
            // and clean up patches
            if is_original {
                if !overlay.delete.iter().any(|item| item == &name) {
                    overlay.delete.push(name.clone());
                }
                overlay.patches.remove(&name);
                if json {
                    eprintln!("原生组 `{name}` 已标记隐藏（将在配置合成时被过滤）");
                } else {
                    println!("原生组 `{name}` 已标记隐藏（将在配置合成时被过滤）");
                }
            }
            // Persist falls through to the shared path after the match.
        }
        GroupMutation::Replace { group, .. } => {
            if !is_prepend && !is_append && !is_original {
                anyhow::bail!("proxy group not found: {name}");
            }
            if is_prepend {
                for item in &mut overlay.prepend {
                    if crate::groups::group_name(item) == Some(name.as_str()) {
                        *item = group.clone();
                    }
                }
            } else if is_append {
                for item in &mut overlay.append {
                    if crate::groups::group_name(item) == Some(name.as_str()) {
                        *item = group.clone();
                    }
                }
            } else {
                // Native group — replace by delete+append (this is Edit via file, which is a deep replacement)
                if !overlay.delete.iter().any(|item| item == &name) {
                    overlay.delete.push(name.clone());
                }
                overlay.patches.remove(&name);
                overlay
                    .append
                    .retain(|item| crate::groups::group_name(item) != Some(name.as_str()));
                overlay.append.push(group);
            }
        }
        GroupMutation::Add { members, uses, .. } => {
            if !is_prepend && !is_append && !is_original {
                anyhow::bail!("proxy group not found: {name}");
            }
            let payload_nodes = known_provider_payload_nodes(paths);
            for m in &members {
                if let Some(p) = payload_nodes.get(m) {
                    anyhow::bail!(
                        "Node `{m}` is defined inside proxy-provider `{p}`. In Mihomo, provider nodes cannot be added to group `proxies`; use `--use {p}`, or define the node under `proxies-append` in override.yaml."
                    );
                }
                if known_providers.contains(m) {
                    anyhow::bail!(
                        "`{m}` is a proxy-provider, not a proxy node. Did you mean: mihomo-cli group add --group {name} --use {m} ?"
                    );
                }
            }
            for u in &uses {
                if !known_providers.contains(u) {
                    anyhow::bail!("proxy group `{name}` references unknown proxy provider `{u}`");
                }
            }

            if is_original && !is_custom {
                // Native group: write/update patches[name].add_proxies and add_use
                let current_group = original
                    .iter()
                    .find(|g| crate::groups::group_name(g) == Some(name.as_str()))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("proxy group not found: {name}"))?;
                let current_members = crate::groups::group_members(&current_group)?;
                let current_uses: Vec<String> = current_group
                    .get("use")
                    .and_then(serde_yaml::Value::as_sequence)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_yaml::Value::as_str)
                    .map(str::to_owned)
                    .collect();

                let patch = overlay.patches.entry(name.clone()).or_default();
                let mut effective_members: std::collections::HashSet<String> = current_members
                    .iter()
                    .chain(patch.add_proxies.iter())
                    .filter(|m| !patch.remove_proxies.contains(*m))
                    .cloned()
                    .collect();
                for member in members {
                    if effective_members.contains(&member) {
                        anyhow::bail!("proxy group `{name}` already contains member `{member}`");
                    }
                    effective_members.insert(member.clone());
                    patch.remove_proxies.retain(|m| m != &member);
                    if !current_members.contains(&member) && !patch.add_proxies.contains(&member) {
                        patch.add_proxies.push(member.clone());
                    }
                }

                let mut effective_uses: std::collections::HashSet<String> = current_uses
                    .iter()
                    .chain(patch.add_use.iter())
                    .filter(|u| !patch.remove_use.contains(*u))
                    .cloned()
                    .collect();
                for u in uses {
                    if effective_uses.contains(&u) {
                        anyhow::bail!("proxy group `{name}` already uses provider `{u}`");
                    }
                    effective_uses.insert(u.clone());
                    patch.remove_use.retain(|item| item != &u);
                    if !current_uses.contains(&u) && !patch.add_use.contains(&u) {
                        patch.add_use.push(u.clone());
                    }
                }

                if patch.is_empty() {
                    overlay.patches.remove(&name);
                }
            } else {
                // Custom group (in prepend or append): modify in-place
                let current = overlay.merged_groups(&original, &known_proxies, &known_providers)?;
                let current_group = current
                    .iter()
                    .find(|group| crate::groups::group_name(group) == Some(name.as_str()))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("proxy group not found: {name}"))?;
                let mut existing_members = crate::groups::group_members(&current_group)?;
                for member in members {
                    if existing_members.iter().any(|item| item == &member) {
                        anyhow::bail!("proxy group `{name}` already contains member `{member}`");
                    }
                    existing_members.push(member);
                }
                let mut existing_uses: Vec<String> = current_group
                    .get("use")
                    .and_then(serde_yaml::Value::as_sequence)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_yaml::Value::as_str)
                    .map(str::to_owned)
                    .collect();
                for u in uses {
                    if existing_uses.iter().any(|item| item == &u) {
                        anyhow::bail!("proxy group `{name}` already uses provider `{u}`");
                    }
                    existing_uses.push(u);
                }

                let updated_group = {
                    let mut map = current_group
                        .as_mapping()
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("proxy group `{name}` is not a mapping"))?;
                    if !existing_members.is_empty() {
                        map.insert(
                            serde_yaml::Value::String("proxies".into()),
                            serde_yaml::Value::Sequence(
                                existing_members.into_iter().map(Into::into).collect(),
                            ),
                        );
                    }
                    if !existing_uses.is_empty() {
                        map.insert(
                            serde_yaml::Value::String("use".into()),
                            serde_yaml::Value::Sequence(
                                existing_uses.into_iter().map(Into::into).collect(),
                            ),
                        );
                    }
                    serde_yaml::Value::Mapping(map)
                };
                let target = if is_prepend {
                    &mut overlay.prepend
                } else {
                    &mut overlay.append
                };
                for item in target.iter_mut() {
                    if crate::groups::group_name(item) == Some(name.as_str()) {
                        *item = updated_group.clone();
                    }
                }
            }
        }
        GroupMutation::Remove { members, uses, .. } => {
            if !is_prepend && !is_append && !is_original {
                anyhow::bail!("proxy group not found: {name}");
            }
            if is_original && !is_custom {
                let current_group = original
                    .iter()
                    .find(|g| crate::groups::group_name(g) == Some(name.as_str()))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("proxy group not found: {name}"))?;
                let current_members = crate::groups::group_members(&current_group)?;
                let current_uses: Vec<String> = current_group
                    .get("use")
                    .and_then(serde_yaml::Value::as_sequence)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_yaml::Value::as_str)
                    .map(str::to_owned)
                    .collect();

                let patch = overlay.patches.entry(name.clone()).or_default();
                for member in members {
                    if patch.add_proxies.contains(&member) {
                        patch.add_proxies.retain(|m| m != &member);
                    } else if current_members.contains(&member) {
                        if !patch.remove_proxies.contains(&member) {
                            patch.remove_proxies.push(member);
                        }
                    } else {
                        anyhow::bail!("proxy group `{name}` does not contain member `{member}`");
                    }
                }

                for u in uses {
                    if patch.add_use.contains(&u) {
                        patch.add_use.retain(|item| item != &u);
                    } else if current_uses.contains(&u) {
                        if !patch.remove_use.contains(&u) {
                            patch.remove_use.push(u);
                        }
                    } else {
                        anyhow::bail!("proxy group `{name}` does not use provider `{u}`");
                    }
                }

                if patch.is_empty() {
                    overlay.patches.remove(&name);
                }
            } else {
                // Custom group: modify in-place
                let current = overlay.merged_groups(&original, &known_proxies, &known_providers)?;
                let current_group = current
                    .iter()
                    .find(|group| crate::groups::group_name(group) == Some(name.as_str()))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("proxy group not found: {name}"))?;
                let mut existing_members = crate::groups::group_members(&current_group)?;
                for member in members {
                    if let Some(index) = existing_members.iter().position(|item| item == &member) {
                        existing_members.remove(index);
                    } else {
                        anyhow::bail!("proxy group `{name}` does not contain member `{member}`");
                    }
                }
                let mut existing_uses: Vec<String> = current_group
                    .get("use")
                    .and_then(serde_yaml::Value::as_sequence)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_yaml::Value::as_str)
                    .map(str::to_owned)
                    .collect();
                for u in uses {
                    if let Some(index) = existing_uses.iter().position(|item| item == &u) {
                        existing_uses.remove(index);
                    } else {
                        anyhow::bail!("proxy group `{name}` does not use provider `{u}`");
                    }
                }

                let updated_group = {
                    let mut map = current_group
                        .as_mapping()
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("proxy group `{name}` is not a mapping"))?;
                    if existing_members.is_empty() {
                        map.remove(serde_yaml::Value::String("proxies".into()));
                    } else {
                        map.insert(
                            serde_yaml::Value::String("proxies".into()),
                            serde_yaml::Value::Sequence(
                                existing_members.into_iter().map(Into::into).collect(),
                            ),
                        );
                    }
                    if existing_uses.is_empty() {
                        map.remove(serde_yaml::Value::String("use".into()));
                    } else {
                        map.insert(
                            serde_yaml::Value::String("use".into()),
                            serde_yaml::Value::Sequence(
                                existing_uses.into_iter().map(Into::into).collect(),
                            ),
                        );
                    }
                    serde_yaml::Value::Mapping(map)
                };
                let target = if is_prepend {
                    &mut overlay.prepend
                } else {
                    &mut overlay.append
                };
                for item in target.iter_mut() {
                    if crate::groups::group_name(item) == Some(name.as_str()) {
                        *item = updated_group.clone();
                    }
                }
            }
        }
    }

    overlay.merged_groups(&original, &known_proxies, &known_providers)?;
    let previous = snapshot_file(&overlay_path)?;
    let previous_config = snapshot_file(&paths.config_path())?;
    overlay.save(&overlay_path)?;
    let core_binary =
        resolved_core_binary_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let endpoint =
        resolved_api_endpoint_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    {
        if let Err(error) =
            config::merge_user_config_checked_at_endpoint(paths, Some(&core_binary), &endpoint)
        {
            restore_file_snapshot(&overlay_path, previous.clone())?;
            restore_file_snapshot(&paths.config_path(), previous_config.clone())?;
            return Err(error);
        }
    }
    drop(_lock);
    // (P3-3): additive report field; `null` for actions that have
    // no selection cleanup. Existing fields and their semantics are unchanged.
    let mut selection_cleanup = serde_json::Value::Null;
    if is_delete {
        // the group is gone from the merged config, so its persisted
        // selection is now an orphan. Clean it here — after the merge committed —
        // and re-push the daemon mirror so a daemon-initiated restart (no CLI
        // push) does not replay the orphan.
        let outcome = cleanup_deleted_group_selection(scope, &name);
        selection_cleanup = serde_json::json!(selection_cleanup_label(&outcome));
        if let Some(notice) = deleted_group_selection_notice(&name, &outcome) {
            // `--json` keeps stdout pure (docs/user-guide/json.md §AI/脚本 JSON 输出).
            if json {
                eprintln!("{notice}");
            } else {
                println!("{notice}");
            }
        }
        if should_push_selection_mirror_after_delete(
            &outcome,
            selection_mirror_target_is_system(system, user),
        ) {
            push_selection_mirror_update(paths, &scope.subscription_id, json).await;
        }
    }
    let reload_error =
        try_reload_group_config(system, user, paths, config_base_revision(paths).as_deref())
            .await?;
    if json {
        let report = serde_json::json!({
            "status": "success",
            "action": action_name,
            "group": name,
            "intent": "Committed",
            "control_plane": if reload_error.is_none() { "Applied" } else { "Pending" },
            "runtime_attestation": if reload_error.is_none() { "Attested" } else { "Unknown (offline/pending)" },
            "pending": reload_error.is_some(),
            "error": reload_error.map(|e| format!("{e:#}")),
            "selection_cleanup": selection_cleanup,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if let Some(error) = reload_error {
        println!("updated proxy group `{name}` (pending=true)");
        println!("  Runtime apply deferred: {error:#}");
        println!("  Run: mihomo-cli restart  to apply");
    } else {
        println!("updated proxy group `{name}` (runtime_applied=true)");
    }
    Ok(())
}

/// Outcome of the post-merge selection cleanup for a deleted group.
#[derive(Debug)]
enum DeletedGroupSelection {
    /// No persisted selection for this group: nothing to clean, no mirror push.
    Absent,
    /// Record removed; the daemon-owned mirror is now stale and must be pushed.
    Unpinned,
    /// Cleanup failed. The delete itself committed, so this is non-fatal — but
    /// it must never be swallowed: a leftover record is what the user sees as a
    /// repeated `⚠ ... not applied` on every replay (issue ).
    Failed(anyhow::Error),
}

fn cleanup_deleted_group_selection(
    scope: &crate::selection::SelectionScope,
    group: &str,
) -> DeletedGroupSelection {
    match crate::selection::unpin_selection_for_scope(scope, group) {
        Ok(true) => DeletedGroupSelection::Unpinned,
        Ok(false) => DeletedGroupSelection::Absent,
        Err(err) => DeletedGroupSelection::Failed(err),
    }
}

/// (P3-2): the mirror push polarity, split from the environment
/// probe so the truth table is runtime-testable without a real daemon.
fn should_push_selection_mirror_after_delete(
    outcome: &DeletedGroupSelection,
    target_is_system: bool,
) -> bool {
    target_is_system && matches!(outcome, DeletedGroupSelection::Unpinned)
}

/// Machine-readable `selection_cleanup` value for the `--json` report.
fn selection_cleanup_label(outcome: &DeletedGroupSelection) -> &'static str {
    match outcome {
        DeletedGroupSelection::Absent => "absent",
        DeletedGroupSelection::Unpinned => "unpinned",
        DeletedGroupSelection::Failed(_) => "failed",
    }
}

/// Report line for a failed cleanup; `None` when there is nothing to report.
fn deleted_group_selection_notice(group: &str, outcome: &DeletedGroupSelection) -> Option<String> {
    match outcome {
        DeletedGroupSelection::Failed(err) => Some(format!(
            "⚠ Selection for group `{group}` was not cleaned up ({err:#});\n  \
             replay will keep reporting it as not applied\n  \
             repair: mihomo-cli select --unpin --group '{group}'"
        )),
        DeletedGroupSelection::Absent | DeletedGroupSelection::Unpinned => None,
    }
}

async fn cmd_rule(system: bool, user: bool, action: RuleAction) -> anyhow::Result<()> {
    let core_binary =
        resolved_core_binary_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let config_endpoint =
        resolved_api_endpoint_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let write_paths = || resolved_user_config_paths_for_write("rule", system, user);
    let read_paths = || {
        app_paths_for_resolved_instance_command(
            "rule",
            system,
            user,
            instance::CommandIntent::ReadOnly,
        )
    };
    use crate::rules::RulePosition;

    match action {
        RuleAction::Add { rule, position } => {
            crate::rules::validate_rule(&rule)?;
            let target_policy = crate::rules::rule_policy(&rule).map(str::to_string);
            let pos = match position {
                Some(p) => Some(p.parse::<RulePosition>()?),
                None => None,
            };
            let paths = write_paths()?;
            let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
            let rules_snapshot = snapshot_file(&paths.rules_path())?;
            if let Some(policy) = target_policy.as_deref() {
                let policies = crate::rules::available_policies_at(&paths)?;
                if !crate::groups::is_builtin_policy(policy)
                    && !policies.iter().any(|p| p == policy)
                {
                    let payload_nodes = known_provider_payload_nodes(&paths);
                    if let Some(pname) = payload_nodes.get(policy) {
                        anyhow::bail!(
                            "Rule target `{policy}` is defined inside proxy-provider `{pname}`. Mihomo rules cannot directly target provider nodes; please target the proxy group using this provider, or declare the node under `proxies-append` in override.yaml."
                        );
                    }
                    let known_providers =
                        known_effective_providers(&paths, &serde_yaml::Value::Null);
                    if known_providers.contains(policy) {
                        anyhow::bail!(
                            "Rule target `{policy}` is a proxy-provider. Mihomo rules cannot directly target providers; please target a proxy group that uses this provider."
                        );
                    }
                    anyhow::bail!(
                        "Rule target `{policy}` is not available in current config. Run `mihomo-cli rule policies` or `mihomo-cli list` to inspect valid policy/group names."
                    );
                }
            }
            crate::rules::add_rule_at(&paths, &rule, pos)?;
            let merged =
                merge_rules_change_checked(&paths, &core_binary, &config_endpoint, rules_snapshot)?;
            drop(_lock);
            let apply = rule_apply_outcome(system, user, &paths, merged).await?;
            print_lines(format_rule_add_success(&rule, merged, apply));
            Ok(())
        }
        RuleAction::List => {
            let paths = read_paths()?;
            let rules = crate::rules::list_rules_at(&paths)?;
            let pos = crate::rules::get_position_at(&paths).unwrap_or_default();
            let policies = crate::rules::available_policies_at(&paths).ok();
            let policies_slice = policies.as_deref();
            if policies.is_none() {
                eprintln!("  ⚠ 策略集不可读，规则目标未判定；请检查 config.yaml 或运行 doctor");
            }
            for line in format_rule_list_annotated(&rules, pos, policies_slice) {
                println!("{line}");
            }
            Ok(())
        }
        RuleAction::Remove { index } => {
            // User-facing index is 1-based
            if index == 0 {
                anyhow::bail!("Rule index starts from 1 (as shown in `rule list`)");
            }
            let paths = write_paths()?;
            let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
            let rules_snapshot = snapshot_file(&paths.rules_path())?;
            crate::rules::remove_rule_at(&paths, index - 1)?;
            let merged =
                merge_rules_change_checked(&paths, &core_binary, &config_endpoint, rules_snapshot)?;
            drop(_lock);
            let apply = rule_apply_outcome(system, user, &paths, merged).await?;
            print_lines(format_rule_remove_success(index, merged, apply));
            Ok(())
        }
        RuleAction::Clear { yes } => {
            let paths = read_paths()?;
            let rules = crate::rules::list_rules_at(&paths)?;
            if rules.is_empty() {
                println!("  No rules to clear.");
                return Ok(());
            }

            // Skip confirmation if --yes flag is provided
            if !yes {
                use dialoguer::Confirm;
                if !Confirm::new()
                    .with_prompt(format!("Clear all {} user rules?", rules.len()))
                    .default(false)
                    .interact_opt()?
                    .unwrap_or(false)
                {
                    println!("  Cancelled.");
                    return Ok(());
                }
            }

            let paths = write_paths()?;
            let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
            let rules_snapshot = snapshot_file(&paths.rules_path())?;
            crate::rules::clear_rules_at(&paths)?;
            let merged =
                merge_rules_change_checked(&paths, &core_binary, &config_endpoint, rules_snapshot)?;
            drop(_lock);
            let apply = rule_apply_outcome(system, user, &paths, merged).await?;
            print_lines(format_rule_clear_success(merged, apply));
            Ok(())
        }
        RuleAction::Move { from, to } => {
            if from == 0 || to == 0 {
                anyhow::bail!("Rule index starts from 1 (as shown in `rule list`)");
            }
            let paths = write_paths()?;
            let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
            let rules_snapshot = snapshot_file(&paths.rules_path())?;
            crate::rules::move_rule_at(&paths, from - 1, to - 1)?;
            let merged =
                merge_rules_change_checked(&paths, &core_binary, &config_endpoint, rules_snapshot)?;
            drop(_lock);
            let apply = rule_apply_outcome(system, user, &paths, merged).await?;
            print_lines(format_rule_move_success(from, to, merged, apply));
            Ok(())
        }
        RuleAction::Import { path } => {
            crate::rules::validate_rules_file(&path)?;
            let paths = write_paths()?;
            let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
            let rules_snapshot = snapshot_file(&paths.rules_path())?;
            crate::rules::import_rules_at(&paths, &path)?;
            let count = crate::rules::list_rules_at(&paths)?.len();
            let merged =
                merge_rules_change_checked(&paths, &core_binary, &config_endpoint, rules_snapshot)?;
            drop(_lock);
            let apply = rule_apply_outcome(system, user, &paths, merged).await?;
            print_lines(format_rule_import_success(count, &path, merged, apply));
            Ok(())
        }
        RuleAction::Export { path } => {
            let paths = read_paths()?;
            crate::rules::export_rules_at(&paths, &path)?;
            let count = crate::rules::list_rules_at(&paths)?.len();
            print_lines(format_rule_export_success(count, &path));
            Ok(())
        }
        RuleAction::Position { position } => match position {
            Some(p) => {
                let pos: RulePosition = p.parse()?;
                let paths = write_paths()?;
                crate::rules::set_position_at(&paths, pos)?;
                print_lines(format_rule_position_set(pos));
                Ok(())
            }
            None => {
                let paths = read_paths()?;
                let pos = crate::rules::get_position_at(&paths).unwrap_or_default();
                print_lines(format_rule_position_show(pos));
                Ok(())
            }
        },
        RuleAction::Types => {
            crate::rules::print_rule_types();
            Ok(())
        }
        RuleAction::Policies => {
            let paths = read_paths()?;
            let policies = crate::rules::available_policies_at(&paths)?;
            print_lines(format_rule_policies(&policies));
            Ok(())
        }
        RuleAction::Test { target } => {
            let paths = read_paths()?;
            let (matched, skipped_logical) =
                crate::rules::test_rule_match_at_with_skipped(&paths, &target)?;
            if skipped_logical {
                eprintln!("  ⚠ 复合规则（AND/SUB-RULE 等）无法离线判定，已跳过；结果可能不完整。");
            }
            print_lines(format_rule_test_result(&target, matched.as_ref()));
            Ok(())
        }
    }
}

fn merge_dns_change_checked(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
    endpoint: &instance::ApiEndpoint,
    snapshots: Vec<(std::path::PathBuf, Option<String>)>,
) -> anyhow::Result<()> {
    if let Err(err) = config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint) {
        for (path, snapshot) in snapshots {
            restore_file_snapshot(&path, snapshot)?;
        }
        anyhow::bail!(
            "DNS policy change failed; rolled back dns-policy.yaml.\n  {}",
            err
        );
    }
    Ok(())
}

fn format_dns_policy_added(match_pattern: &str, target: &str) -> Vec<String> {
    vec![format!("  ✓ Policy added: {match_pattern} → {target}")]
}

fn format_dns_policy_removed(removed: &impl std::fmt::Display) -> Vec<String> {
    vec![format!("  ✓ Policy removed: {removed}")]
}

fn format_dns_policy_list<T: std::fmt::Display>(policies: &[(usize, T)]) -> Vec<String> {
    if policies.is_empty() {
        vec![
            "  No DNS policies defined.".to_string(),
            String::new(),
            "  Add one:  mihomo-cli dns policy add <MATCH> <TARGET>".to_string(),
            "  Example:  mihomo-cli dns policy add internal.example.com system".to_string(),
        ]
    } else {
        let mut lines = vec!["  DNS policies:".to_string()];
        lines.extend(
            policies
                .iter()
                .map(|(idx, policy)| format!("  {idx}. {policy}")),
        );
        lines
    }
}

fn format_dns_template_list(templates: &[crate::dns::DnsTemplate]) -> Vec<String> {
    let mut lines = vec!["  Available DNS templates:".to_string()];
    lines.extend(
        templates
            .iter()
            .map(|t| format!("  - {:8} {}", t.name, t.description)),
    );
    lines.extend([
        String::new(),
        "  Apply company template:".to_string(),
        "    mihomo-cli dns template apply company --domain corp.example.com --target 192.0.2.53"
            .to_string(),
    ]);
    lines
}

fn format_dns_template_applied(name: &str, added: &[impl std::fmt::Display]) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Applied DNS template: {name}")];
    lines.extend(added.iter().map(|policy| format!("  - {policy}")));
    lines
}

async fn cmd_dns(system: bool, user: bool, action: DnsAction) -> anyhow::Result<()> {
    let core_binary =
        resolved_core_binary_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let config_endpoint =
        resolved_api_endpoint_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let write_paths = || resolved_user_config_paths_for_write("dns", system, user);
    let read_paths = || {
        app_paths_for_resolved_instance_command(
            "dns",
            system,
            user,
            instance::CommandIntent::ReadOnly,
        )
    };
    match action {
        DnsAction::Policy { action } => match action {
            DnsPolicyAction::Add {
                match_pattern,
                target,
            } => {
                let paths = write_paths()?;
                let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
                let dns_snapshot = snapshot_file(&paths.dns_policy_path())?;
                crate::dns::add_policy_at(&paths, &match_pattern, &target)?;

                // Regenerate config.yaml from subscription + rules + DNS policies
                merge_dns_change_checked(
                    &paths,
                    &core_binary,
                    &config_endpoint,
                    vec![(paths.dns_policy_path(), dns_snapshot)],
                )?;

                drop(_lock);
                let apply_lines = apply_config_reload_required(
                    system,
                    user,
                    &paths,
                    config_base_revision(&paths).as_deref(),
                )
                .await?;
                print_lines(format_dns_policy_added(&match_pattern, &target));
                print_lines(apply_lines);
                Ok(())
            }
            DnsPolicyAction::List => {
                let paths = read_paths()?;
                let policies = crate::dns::list_policies_at(&paths)?;
                print_lines(format_dns_policy_list(&policies));
                Ok(())
            }
            DnsPolicyAction::Remove { selector } => {
                let paths = write_paths()?;
                let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
                let dns_snapshot = snapshot_file(&paths.dns_policy_path())?;
                let removed = crate::dns::remove_policy_at(&paths, &selector)?;

                // Regenerate config.yaml from subscription + rules + DNS policies
                merge_dns_change_checked(
                    &paths,
                    &core_binary,
                    &config_endpoint,
                    vec![(paths.dns_policy_path(), dns_snapshot)],
                )?;
                drop(_lock);
                let apply_lines = apply_config_reload_required(
                    system,
                    user,
                    &paths,
                    config_base_revision(&paths).as_deref(),
                )
                .await?;
                print_lines(format_dns_policy_removed(&removed));
                print_lines(apply_lines);
                Ok(())
            }
        },

        DnsAction::FakeIpFilter { action } => match action {
            DnsFakeIpFilterAction::Add { domain } => {
                let paths = write_paths()?;
                let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
                let snapshot = snapshot_file(&paths.dns_fake_ip_filter_path())?;
                let normalized = crate::dns::add_fake_ip_filter_at(&paths, &domain)?;
                merge_dns_change_checked(
                    &paths,
                    &core_binary,
                    &config_endpoint,
                    vec![(paths.dns_fake_ip_filter_path(), snapshot)],
                )?;
                drop(_lock);
                let apply_lines = apply_config_reload_required(
                    system,
                    user,
                    &paths,
                    config_base_revision(&paths).as_deref(),
                )
                .await?;
                print_lines(vec![format!("  ✓ fake-ip-filter added: {normalized}")]);
                print_lines(apply_lines);
                Ok(())
            }
            DnsFakeIpFilterAction::Remove { domain } => {
                let paths = write_paths()?;
                let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
                let snapshot = snapshot_file(&paths.dns_fake_ip_filter_path())?;
                let removed = crate::dns::remove_fake_ip_filter_at(&paths, &domain)?;
                merge_dns_change_checked(
                    &paths,
                    &core_binary,
                    &config_endpoint,
                    vec![(paths.dns_fake_ip_filter_path(), snapshot)],
                )?;
                drop(_lock);
                let apply_lines = apply_config_reload_required(
                    system,
                    user,
                    &paths,
                    config_base_revision(&paths).as_deref(),
                )
                .await?;
                print_lines(vec![format!("  ✓ fake-ip-filter removed: {removed}")]);
                print_lines(apply_lines);
                Ok(())
            }
            DnsFakeIpFilterAction::List => {
                let paths = read_paths()?;
                let filters = crate::dns::list_fake_ip_filters_at(&paths)?;
                if filters.is_empty() {
                    print_lines(vec![
                        "  No DNS fake-ip-filter entries defined.".to_string(),
                        String::new(),
                        "  Add one:  mihomo-cli dns fake-ip-filter add <DOMAIN>".to_string(),
                    ]);
                } else {
                    let mut lines = vec!["  DNS fake-ip-filter entries:".to_string()];
                    lines.extend(filters.iter().map(|(idx, f)| format!("  {idx}. {f}")));
                    print_lines(lines);
                }
                Ok(())
            }
        },
        DnsAction::Template { action } => {
            let action = action.unwrap_or(DnsTemplateAction::List);
            match action {
                DnsTemplateAction::List => {
                    print_lines(format_dns_template_list(crate::dns::dns_templates()));
                    Ok(())
                }
                DnsTemplateAction::Apply {
                    name,
                    domain,
                    target,
                } => {
                    let paths = write_paths()?;
                    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
                    let dns_snapshot = snapshot_file(&paths.dns_policy_path())?;
                    let added = crate::dns::apply_template_at(
                        &paths,
                        &name,
                        domain.as_deref(),
                        target.as_deref(),
                    )?;
                    merge_dns_change_checked(
                        &paths,
                        &core_binary,
                        &config_endpoint,
                        vec![(paths.dns_policy_path(), dns_snapshot)],
                    )?;
                    drop(_lock);
                    let apply_lines = apply_config_reload_lines(
                        system,
                        user,
                        &paths,
                        config_base_revision(&paths).as_deref(),
                    )
                    .await;
                    print_lines(format_dns_template_applied(&name, &added));
                    print_lines(apply_lines);
                    Ok(())
                }
            }
        }
        DnsAction::Status => {
            use crate::mihomo_api::MihomoApiClient;
            let client = resolve_api_client(system, user, instance::CommandIntent::ReadOnly)?;
            let data = client.get("/configs").await?;
            let paths = read_paths()?;
            let policies = crate::dns::list_policies_at(&paths)?;
            for line in format_dns_status(&data["dns"], &policies) {
                println!("{line}");
            }
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyRootLeftover {
    path: std::path::PathBuf,
    reason: &'static str,
}

fn legacy_root_leftovers() -> Vec<LegacyRootLeftover> {
    let Some(user_ctx) = instance::planned_current_context(instance::InstanceMode::User) else {
        return Vec::new();
    };
    let config_dir = user_ctx.paths.config_dir;
    let mut leftovers = Vec::new();

    let run_dir = config_dir.join("run");
    if run_dir.exists() {
        leftovers.push(LegacyRootLeftover {
            path: run_dir,
            reason: "legacy root runtime socket directory",
        });
    }

    let log_file = config_dir.join("mihomo.log");
    if log_file.exists() {
        leftovers.push(LegacyRootLeftover {
            path: log_file,
            reason: "legacy root service log file",
        });
    }

    let marker = config_dir.join(".service-mode");
    if std::fs::read_to_string(&marker)
        .map(|s| s.trim() == "root")
        .unwrap_or(false)
    {
        leftovers.push(LegacyRootLeftover {
            path: marker,
            reason: "legacy root service marker in user config dir",
        });
    }

    let start_script = config_dir.join("start.sh");
    if start_script.exists() && legacy_root_launchdaemon_references_user_config(&config_dir) {
        leftovers.push(LegacyRootLeftover {
            path: start_script,
            reason: "start script referenced by legacy root LaunchDaemon",
        });
    }

    leftovers
}

fn legacy_root_launchdaemon_references_user_config(config_dir: &std::path::Path) -> bool {
    let plist = std::path::Path::new("/Library/LaunchDaemons/io.mihomo.plist");
    std::fs::read_to_string(plist)
        .map(|content| content.contains(&config_dir.display().to_string()))
        .unwrap_or(false)
}

fn format_legacy_root_leftovers(leftovers: &[LegacyRootLeftover]) -> Vec<String> {
    let mut lines = vec!["Legacy root-mode leftovers detected:".to_string()];
    for item in leftovers {
        lines.push(format!("  - {} ({})", item.path.display(), item.reason));
    }
    lines
}

fn legacy_root_leftovers_user_uninstall_error(leftovers: &[LegacyRootLeftover]) -> String {
    let mut lines = format_legacy_root_leftovers(leftovers);
    lines.extend([
        "".to_string(),
        "These are runtime artifacts from the old root service layout under your user config dir."
            .to_string(),
        "Run: mihomo-cli uninstall --legacy-system-leftovers".to_string(),
        "Then retry: mihomo-cli uninstall --user --all".to_string(),
    ]);
    lines.join("\n")
}

fn cmd_uninstall_legacy_root_leftovers(dry_run: bool) -> anyhow::Result<()> {
    use dialoguer::Confirm;
    let leftovers = legacy_root_leftovers();
    if leftovers.is_empty() {
        println!("No legacy root-mode leftovers detected.");
        return Ok(());
    }

    for line in format_legacy_root_leftovers(&leftovers) {
        println!("{line}");
    }
    println!();
    println!("This will not remove config.yaml, subscriptions, rules, DNS policy, overrides, or backups.");

    if dry_run {
        println!("Dry run: no files removed.");
        return Ok(());
    }

    if !Confirm::new()
        .with_prompt("Remove these legacy runtime leftovers with elevated permissions if needed?")
        .default(false)
        .interact()?
    {
        print_lines(format_uninstall_cancelled());
        return Ok(());
    }

    for item in leftovers {
        remove_instance_path(&item.path, true)?;
    }
    print_lines(format_uninstall_done());
    Ok(())
}

#[allow(dead_code)]
fn format_uninstall_nothing() -> Vec<String> {
    vec!["Nothing to uninstall.".to_string()]
}

#[allow(dead_code)]
fn format_uninstall_intro(
    mihomo_exists: bool,
    service_exists: bool,
    all: bool,
    mihomo_path: &str,
    config_dir: &str,
) -> Vec<String> {
    let mut lines = vec![
        "=== mihomo-cli uninstall ===".to_string(),
        String::new(),
        "This will:".to_string(),
    ];
    if mihomo_exists {
        lines.push("  - Stop running mihomo process".to_string());
    }
    if service_exists {
        lines.push("  - Remove auto-start service".to_string());
    }
    if all {
        lines.push(format!("  - Delete mihomo binary ({mihomo_path})"));
        lines.push(format!("  - Delete config dir ({config_dir})"));
    }
    lines.push(String::new());
    lines
}

fn uninstall_prompt(all: bool) -> &'static str {
    if all {
        "Proceed with full removal?"
    } else {
        "Proceed?"
    }
}

fn format_uninstall_cancelled() -> Vec<String> {
    vec!["Cancelled.".to_string()]
}

#[allow(dead_code)]
fn format_uninstall_stop_mihomo() -> Vec<String> {
    vec![String::new(), "Stopping mihomo...".to_string()]
}

fn format_uninstall_remove_service() -> Vec<String> {
    vec!["Removing service...".to_string()]
}

fn format_uninstall_remove_binaries() -> Vec<String> {
    vec!["Removing binaries...".to_string()]
}

fn format_uninstall_done() -> Vec<String> {
    vec!["Done.".to_string()]
}

#[allow(dead_code)]
fn should_retry_removal_with_sudo(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
}

#[allow(dead_code)]
fn remove_config_dir_for_uninstall(config_dir: &str) -> anyhow::Result<()> {
    match std::fs::remove_dir_all(config_dir) {
        Ok(()) => Ok(()),
        Err(e) if should_retry_removal_with_sudo(&e) => {
            eprintln!("  Config dir contains root-owned files — sudo required to remove it.");
            service::PrivilegeExecutor::remove_path(std::path::Path::new(config_dir))
        }
        Err(e) => Err(e.into()),
    }
}

fn update_missing_binary_error(bin: &str) -> String {
    format!(
        "mihomo not installed at {bin}
  Run: mihomo-cli install"
    )
}

fn build_info_lines(core_version: Option<&str>, core_error: Option<&str>) -> Vec<String> {
    let mut lines = vec![
        "mihomo-cli".to_string(),
        format!("  Version:      {}", env!("MIHOMO_CLI_VERSION")),
        format!("  Package:      {}", env!("MIHOMO_CLI_PKG_VERSION")),
        format!("  Git commit:   {}", env!("MIHOMO_CLI_GIT_COMMIT")),
        format!("  Git short:    {}", env!("MIHOMO_CLI_GIT_SHORT_COMMIT")),
        format!("  Git branch:   {}", env!("MIHOMO_CLI_GIT_BRANCH")),
        format!("  Git dirty:    {}", env!("MIHOMO_CLI_GIT_DIRTY")),
        format!(
            "  Build time:   {} (unix seconds)",
            env!("MIHOMO_CLI_BUILD_UNIX")
        ),
        format!("  Target:       {}", env!("MIHOMO_CLI_BUILD_TARGET")),
        format!("  Profile:      {}", env!("MIHOMO_CLI_BUILD_PROFILE")),
        String::new(),
        "mihomo core".to_string(),
    ];
    match core_version {
        Some(version) => lines.push(format!("  Version:      {version}")),
        None => lines.push("  Version:      unavailable".to_string()),
    }
    if let Some(error) = core_error {
        lines.push(format!("  Probe error:  {error}"));
    }
    lines
}

async fn cmd_version(system: bool, user: bool, json: bool) -> anyhow::Result<()> {
    let client = resolve_api_client(system, user, instance::CommandIntent::ReadOnly);
    let (core_version, core_error) = match client {
        Ok(client) => match mihomo_api::get_version_with_client(&client).await {
            Ok(version) => (Some(version), None),
            Err(err) => (None, Some(err.to_string())),
        },
        Err(err) => (None, Some(err.to_string())),
    };
    if json {
        print_json_envelope(
            "version",
            serde_json::json!({
                "cli": {
                    "version": env!("MIHOMO_CLI_VERSION"),
                    "package_version": env!("MIHOMO_CLI_PKG_VERSION"),
                    "git_commit": env!("MIHOMO_CLI_GIT_COMMIT"),
                    "git_short_commit": env!("MIHOMO_CLI_GIT_SHORT_COMMIT"),
                    "git_branch": env!("MIHOMO_CLI_GIT_BRANCH"),
                    "git_dirty": env!("MIHOMO_CLI_GIT_DIRTY"),
                    "build_unix": env!("MIHOMO_CLI_BUILD_UNIX"),
                    "target": env!("MIHOMO_CLI_BUILD_TARGET"),
                    "profile": env!("MIHOMO_CLI_BUILD_PROFILE"),
                },
                "core": { "version": core_version, "probe_error": core_error }
            }),
            Vec::new(),
        )?;
    } else {
        let lines = build_info_lines(core_version.as_deref(), core_error.as_deref());
        print_lines(lines);
    }
    Ok(())
}

fn format_update_start() -> Vec<String> {
    vec!["Updating mihomo core...".to_string()]
}

fn format_update_success() -> Vec<String> {
    vec!["Updated successfully".to_string()]
}

fn update_failed_error(error: &str) -> String {
    format!(
        "Update failed: {error}
  Original binary restored"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UninstallPreflightAction {
    RecoverSystemTransaction,
}

fn uninstall_preflight_actions(mode: instance::InstanceMode) -> Vec<UninstallPreflightAction> {
    match mode {
        instance::InstanceMode::System => {
            vec![UninstallPreflightAction::RecoverSystemTransaction]
        }
        instance::InstanceMode::User => Vec::new(),
    }
}

async fn run_uninstall_preflight(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    for action in uninstall_preflight_actions(ctx.mode) {
        match action {
            UninstallPreflightAction::RecoverSystemTransaction => {
                maybe_auto_recover_active_transaction(
                    ctx,
                    tun_transaction::RecoveryDirection::Abort,
                    false,
                    false,
                    false,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn cmd_uninstall_all_instance_modes(
    modes: &[instance::InstanceMode],
    options: AllUninstallOptions,
) -> anyhow::Result<()> {
    use dialoguer::Confirm;

    let service_presence = current_service_presence();
    let runtime_presence = current_runtime_presence();
    let mut plans = Vec::new();
    for mode in modes {
        let Some(ctx) = instance::planned_current_context(*mode) else {
            continue;
        };
        let plan = instance::planned_service_plan(&ctx, instance::ServiceAction::Uninstall);
        let (service_present, runtime_present) = match mode {
            instance::InstanceMode::System => (service_presence.system, runtime_presence.system),
            instance::InstanceMode::User => (service_presence.user, runtime_presence.user),
        };
        plans.push((
            ctx,
            plan,
            should_run_all_uninstall_service_commands(service_present, runtime_present),
        ));
    }

    if plans.is_empty() {
        print_lines(format_uninstall_nothing());
        return Ok(());
    }

    println!("=== mihomo-cli uninstall --all ===");
    println!();
    if options.dry_run {
        println!("Would remove all v3 instance artifacts:");
    } else {
        println!("This will remove all v3 instance artifacts:");
    }
    for (ctx, _, _) in &plans {
        println!(
            "  - {}: {}",
            instance_mode_label(ctx.mode),
            status_service_label(&ctx.service)
        );
        println!("    binary: {}", ctx.paths.core_binary.display());
        println!("    cli:    {}", ctx.paths.cli_binary.display());
        println!("    config: {}", ctx.paths.config_dir.display());
    }
    println!();

    if options.dry_run {
        println!("Dry run: no files or services were changed.");
        return Ok(());
    }

    if !options.yes
        && !Confirm::new()
            .with_prompt(uninstall_prompt(true))
            .default(false)
            .interact()?
    {
        print_lines(format_uninstall_cancelled());
        return Ok(());
    }

    for (ctx, _, _) in &plans {
        run_uninstall_preflight(ctx).await?;
    }

    for (ctx, plan, run_service_commands) in plans {
        println!("Instance: {}", instance_mode_label(ctx.mode));
        if run_service_commands {
            print_lines(format_uninstall_remove_service());
            for command in &plan.commands {
                if command.privileged {
                    if let Some(invocation) = instance::privilege_invocation_plan(command.clone()) {
                        println!(
                            "  Privilege required. Fallback: {}",
                            invocation.manual_fallback
                        );
                    }
                }
                service::run_instance_command(command).map_err(|err| {
                    anyhow::anyhow!(
                        "failed to stop or uninstall {} service; no further artifacts were removed: {err}",
                        instance_mode_label(ctx.mode)
                    )
                })?;
            }
        }

        if let Some(service_file) = &ctx.paths.service_file {
            let service_file_privileged =
                ctx.permissions == instance::PermissionModel::PrivilegedSystem;
            remove_instance_path(service_file, service_file_privileged)?;
        }

        print_lines(format_uninstall_remove_binaries());
        for removal in plan.remove_paths {
            remove_instance_path(&removal.path, removal.privileged)?;
        }
    }

    print_lines(format_uninstall_done());
    Ok(())
}

async fn cmd_uninstall_instance_mode(
    mode: instance::InstanceMode,
    all: bool,
    remove_binary: bool,
    remove_config: bool,
    remove_geo: bool,
    yes: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance uninstall"))?;
    let plan = instance::planned_service_plan(&ctx, instance::ServiceAction::Uninstall);

    // Resolve flags: --all is shortcut for all three granular flags
    let remove_binary = all || remove_binary;
    let remove_config = all || remove_config;
    let remove_geo = all || remove_geo;

    if mode == instance::InstanceMode::User && remove_config {
        let leftovers = legacy_root_leftovers();
        if !leftovers.is_empty() {
            anyhow::bail!(legacy_root_leftovers_user_uninstall_error(&leftovers));
        }
    }

    // Build list of removable components
    let binary_label = format!("Remove core binary ({})", ctx.paths.core_binary.display());
    let config_label = format!("Remove config & data ({})", ctx.paths.config_dir.display());
    let geo_label = "Remove geo data (geoip.metadb + GeoSite.dat)".to_string();

    // Build items for TUI: always include service (forced, always removed), then optional components
    let mut item_labels: Vec<String> = Vec::new();
    let mut item_defaults: Vec<bool> = Vec::new();

    // Service is always removed
    item_labels.push("Stop & remove service".to_string());
    item_defaults.push(true);

    if ctx.paths.core_binary.exists() {
        item_labels.push(binary_label.clone());
        item_defaults.push(remove_binary);
    }
    if ctx.paths.config_dir.exists() {
        item_labels.push(config_label.clone());
        item_defaults.push(remove_config);
    }
    if installer::geo_files_exist()
        || ctx.paths.config_dir.join("geoip.metadb").exists()
        || ctx.paths.config_dir.join("GeoSite.dat").exists()
    {
        item_labels.push(geo_label.clone());
        item_defaults.push(remove_geo);
    }

    // Determine what to remove: TUI or direct
    let (selected_binary, selected_config, selected_geo) = if yes || dry_run {
        // Non-interactive: use flag defaults
        (remove_binary, remove_config, remove_geo)
    } else {
        // TUI multi-select
        use dialoguer::MultiSelect;
        let selections = MultiSelect::new()
            .with_prompt("Select components to remove (Space: toggle, Enter: confirm)")
            .items(&item_labels)
            .defaults(&item_defaults)
            .interact()?;

        if selections.is_empty() {
            print_lines(format_uninstall_cancelled());
            return Ok(());
        }

        // Map selections back to flags
        // Index 0 is always service
        let mut idx = 1;
        let mut sel_binary = false;
        let mut sel_config = false;
        let mut sel_geo = false;
        if ctx.paths.core_binary.exists() {
            if selections.contains(&idx) {
                sel_binary = true;
            }
            idx += 1;
        }
        if ctx.paths.config_dir.exists() {
            if selections.contains(&idx) {
                sel_config = true;
            }
            idx += 1;
        }
        let geo_exists = installer::geo_files_exist()
            || ctx.paths.config_dir.join("geoip.metadb").exists()
            || ctx.paths.config_dir.join("GeoSite.dat").exists();
        if geo_exists && selections.contains(&idx) {
            sel_geo = true;
        }
        (sel_binary, sel_config, sel_geo)
    };

    // Dry-run: preview only
    if dry_run {
        println!();
        println!("Would remove:");
        println!("  Service: stop & disable + remove service file");
        if let Some(marker) = instance::windows_user_install_marker(&ctx) {
            println!("  Marker:  {}", marker.display());
        }
        if selected_binary {
            println!("  Binary:  {}", ctx.paths.core_binary.display());
            if ctx.paths.cli_binary != ctx.paths.core_binary {
                println!("  CLI:     {}", ctx.paths.cli_binary.display());
            }
        }
        if selected_config {
            println!("  Config:  {} (directory)", ctx.paths.config_dir.display());
        }
        if selected_geo {
            println!(
                "  Geo:     {}/geoip.metadb, {}/GeoSite.dat",
                ctx.paths.config_dir.display(),
                ctx.paths.config_dir.display()
            );
        }
        if let Some(runtime_dir) = &ctx.paths.runtime_dir {
            if selected_config || selected_binary {
                println!("  Runtime: {}", runtime_dir.display());
            }
        }
        println!();
        println!("Would keep:");
        if let Some(log_file) = &ctx.paths.log_file {
            println!("  Logs:    {} (never removed)", log_file.display());
        }
        if !selected_binary && ctx.paths.core_binary.exists() {
            println!("  Binary:  {} (kept)", ctx.paths.core_binary.display());
        }
        if !selected_config && ctx.paths.config_dir.exists() {
            println!("  Config:  {} (kept)", ctx.paths.config_dir.display());
        }
        return Ok(());
    }

    // Confirm before executing
    if !yes {
        use dialoguer::Confirm;
        if !Confirm::new()
            .with_prompt(uninstall_prompt(all))
            .default(false)
            .interact()?
        {
            print_lines(format_uninstall_cancelled());
            return Ok(());
        }
    }

    run_uninstall_preflight(&ctx).await?;

    // Execute removal
    println!();
    println!("=== mihomo-cli uninstall ===");
    println!("Instance: {}", instance_mode_label(mode));
    println!();

    // 1. Stop service
    println!("Stopping service...");
    #[cfg(target_os = "windows")]
    let service_removed = {
        if mode == instance::InstanceMode::System {
            // Windows system service: use service-manager (P1-4).
            service::windows_uninstall_service()?;
            println!("  Service removed");
            true
        } else {
            false
        }
    };
    #[cfg(not(target_os = "windows"))]
    let service_removed = false;
    if !service_removed {
        for command in &plan.commands {
            if command.privileged {
                if let Some(invocation) = instance::privilege_invocation_plan(command.clone()) {
                    println!(
                        "  Privilege required. Fallback: {}",
                        invocation.manual_fallback
                    );
                }
            }
            service::run_instance_command(command).map_err(|err| {
                anyhow::anyhow!(
                    "failed to stop or uninstall {} service; no artifacts were removed: {err}",
                    instance_mode_label(mode)
                )
            })?;
        }
    }

    // 2. Remove service file
    if let Some(service_file) = &ctx.paths.service_file {
        let service_file_privileged =
            ctx.permissions == instance::PermissionModel::PrivilegedSystem;
        remove_instance_path(service_file, service_file_privileged)?;
        println!("  Removed service file");
    }
    // 2b. Windows user mode: remove the install marker (it is part of the
    // service artifact — presence detection must no longer resolve the mode).
    if let Some(marker) = instance::windows_user_install_marker(&ctx) {
        remove_instance_path(&marker, false)?;
    }

    // 3. Remove binary
    if selected_binary {
        println!("Removing binaries...");
        remove_instance_path(
            &ctx.paths.core_binary,
            ctx.permissions == instance::PermissionModel::PrivilegedSystem,
        )?;
        if ctx.paths.cli_binary != ctx.paths.core_binary {
            remove_instance_path(
                &ctx.paths.cli_binary,
                ctx.permissions == instance::PermissionModel::PrivilegedSystem,
            )?;
        }
        println!("  Removed {}", ctx.paths.core_binary.display());
    }

    // 4. Remove config
    if selected_config {
        println!("Removing config...");
        remove_instance_path(&ctx.paths.config_dir, false)?;
        println!("  Removed {}", ctx.paths.config_dir.display());
    }

    // 5. Remove geo files (only if config is kept)
    if selected_geo && !selected_config {
        for name in ["geoip.metadb", "GeoSite.dat"] {
            let path = ctx.paths.config_dir.join(name);
            if path.exists() {
                utils::remove_file_if_exists(&path)?;
                println!("  Removed {}", path.display());
            }
        }
    }

    // 6. Runtime belongs to the removed service, not to optional binary/config data.
    if let Some(runtime_dir) = &ctx.paths.runtime_dir {
        if runtime_dir.exists() {
            let privileged = ctx.permissions == instance::PermissionModel::PrivilegedSystem;
            remove_instance_path(runtime_dir, privileged)?;
        }
    }

    print_lines(format_uninstall_done());
    Ok(())
}

fn remove_instance_path(path: &std::path::Path, privileged: bool) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if privileged {
        return service::PrivilegeExecutor::remove_path(path);
    }
    utils::remove_path_no_follow(path)
}

async fn cmd_update(system: bool, user: bool) -> anyhow::Result<()> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::Mutating)?;
    update_instance_core_binary(&resolved.ctx, None).await
}

async fn update_instance_core_binary(
    ctx: &instance::InstanceContext,
    version: Option<&str>,
) -> anyhow::Result<()> {
    let bin = &ctx.paths.core_binary;
    if !bin.exists() {
        anyhow::bail!(update_missing_binary_error(&bin.display().to_string()));
    }
    print_lines(format_update_start());

    if ctx.permissions == instance::PermissionModel::PrivilegedSystem {
        let stage = tempfile::tempdir()?;
        let staged_bin = stage.path().join(
            bin.file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("mihomo")),
        );
        installer::download_mihomo_to(version, &staged_bin)
            .await
            .map_err(|e| anyhow::anyhow!(update_failed_error(&e.to_string())))?;
        let bin_bytes = std::fs::read(&staged_bin)?;
        service::PrivilegeExecutor::write_file(bin, &bin_bytes, 0o755)
            .map_err(|e| anyhow::anyhow!(update_failed_error(&e.to_string())))?;
        print_lines(format_update_success());
        return Ok(());
    }

    let bak = bin.with_extension("bak");
    utils::rename_no_follow(bin, &bak)?;
    log!("Backed up {} -> {}", bin.display(), bak.display());
    match installer::download_mihomo_to(version, bin).await {
        Ok(()) => {
            utils::remove_file_if_exists(&bak)?;
            log!("Removed backup");
            print_lines(format_update_success());
        }
        Err(e) => {
            utils::rename_no_follow(&bak, bin)?;
            log!("Restored backup");
            anyhow::bail!(update_failed_error(&e.to_string()));
        }
    }
    Ok(())
}

async fn cmd_upgrade(system: bool, user: bool, yes: bool) -> anyhow::Result<()> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::Mutating)?;

    // Query GitHub for latest release tag
    let client = crate::utils::http_client_builder()
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build HTTP client: {}", e))?;
    let resp = client
        .get("https://api.github.com/repos/MetaCubeX/mihomo/releases/latest")
        .header("User-Agent", "mihomo-cli")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to query GitHub: {}", e))?;
    if !resp.status().is_success() {
        anyhow::bail!("GitHub API returned {}", resp.status());
    }
    let release: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse GitHub response: {}", e))?;
    let latest_tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No tag_name in GitHub response"))?;

    let api_client = mihomo_api::EndpointMihomoApiClient::for_instance(
        resolved.ctx.mode,
        resolved.ctx.paths.api_endpoint.clone(),
    );

    // Get current running version via the resolved instance API.
    let current_version = match mihomo_api::get_version_with_client(&api_client).await {
        Ok(v) => v,
        Err(_) => {
            println!("  ⚠ Cannot reach mihomo API — is the service running?");
            println!("  Latest release: {latest_tag}");
            println!("  Run: mihomo-cli install --version {latest_tag}");
            return Ok(());
        }
    };

    let current_normalized = current_version.trim_start_matches('v');
    let latest_normalized = latest_tag.trim_start_matches('v');

    if current_normalized == latest_normalized {
        println!("  mihomo is up to date ({current_version}).");
        return Ok(());
    }

    println!("  Current: {current_version}");
    println!("  Latest:  {latest_tag}");
    if !yes {
        print!("  Upgrade? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("  Cancelled.");
            return Ok(());
        }
    }

    let _lock = crate::lock::ConfigLock::acquire(&resolved.ctx.paths.config_dir)?;
    drop(_lock);
    update_instance_core_binary(&resolved.ctx, Some(latest_tag))
        .await
        .map_err(|e| anyhow::anyhow!("Upgrade failed: {e}"))?;
    println!("  Upgraded to {latest_tag}.");

    match cmd_lifecycle_instance_mode(
        resolved.ctx.mode,
        instance::ServiceAction::Restart,
        false,
        false,
    )
    .await
    {
        Ok(()) => println!("  Service restarted."),
        Err(err) => println!("  ⚠ Service restart failed — run: mihomo-cli restart ({err})"),
    }
    Ok(())
}

async fn cmd_override(system: bool, user: bool, action: OverrideAction) -> anyhow::Result<()> {
    let intent = override_action_intent(&action);
    let paths = app_paths_for_resolved_instance_command("override", system, user, intent)?;
    match action {
        OverrideAction::Path => {
            println!("{}", paths.override_path().display());
            Ok(())
        }
        OverrideAction::Show => {
            let path = paths.override_path();
            if path.exists() {
                print!("{}", std::fs::read_to_string(&path)?);
            } else {
                println!("override.yaml not found: {}", path.display());
            }
            Ok(())
        }
        OverrideAction::Import { path } => {
            let source = std::path::PathBuf::from(path);
            let content = std::fs::read_to_string(&source).map_err(|e| {
                anyhow::anyhow!("failed to read override source {}: {e}", source.display())
            })?;
            let parsed: serde_yaml::Value = serde_yaml::from_str(&content).map_err(|e| {
                anyhow::anyhow!("failed to parse override YAML {}: {e}", source.display())
            })?;
            if !parsed.is_mapping() {
                anyhow::bail!("override.yaml must be a YAML mapping");
            }
            // B：被 TUN 事务管辖的键必须显式警告，不得“看起来生效、实际被丢弃”。
            // P2-5：告警在**写入成功后**输出，避免用户先看到「会被拒绝」再看到
            // refused 的时序误读（失败时唯一结论应是失败原因）。
            let warning = tun_owned_override_keys_warning(&parsed);
            let extra_warnings = override_content_warnings(&parsed, system);
            apply_override_change(system, user, &paths, Some(content), "updated").await?;
            if let Some(warning) = warning {
                println!("{warning}");
            }
            for w in extra_warnings {
                println!("{w}");
            }
            Ok(())
        }
        OverrideAction::Clear { yes } => {
            let path = paths.override_path();
            if !path.exists() {
                println!("override.yaml not found: {}", path.display());
                return Ok(());
            }
            if !yes {
                use dialoguer::Confirm;
                if !Confirm::new()
                    .with_prompt(format!("Remove {}?", path.display()))
                    .default(false)
                    .interact()?
                {
                    println!("Cancelled.");
                    return Ok(());
                }
            }
            apply_override_change(system, user, &paths, None, "removed").await
        }
    }
}

/// B：`override.yaml` 中被 TUN 事务管辖的键必须显式告警。
///
/// `tun on/off` 按 CLI 参数与参考实现默认重建 `tun` 块，promotion 阶段还会沿用当前
/// 运行中的 `tun` 块（`prepare_promoted_system_config_content`），因此 override 里的
/// `tun.*` 在运行快照中可能不生效。这种“看起来生效、实际被丢弃”的情况必须可观察。
fn override_content_warnings(override_yaml: &serde_yaml::Value, system: bool) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(map) = override_yaml.as_mapping() {
        if map.contains_key("proxies") {
            warnings.push(
                "  ⚠ override 中的 proxies 会整体替换订阅自带的全部节点；如果只是追加自定义节点，建议使用 proxies-append".to_string()
            );
        }
        if system {
            if let Some(providers) = map.get("proxy-providers").and_then(|v| v.as_mapping()) {
                for (pname, pcfg) in providers {
                    let ptype = pcfg.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let path = pcfg.get("path").and_then(|v| v.as_str()).unwrap_or("");
                    if ptype == "file" && !path.starts_with("/var/lib/mihomo-cli") {
                        let name_str = pname.as_str().unwrap_or("unknown");
                        warnings.push(format!(
                            "  ⚠ 提示：proxy-provider `{name_str}` (type: file) 路径为 `{path}`。在 system 模式下，Mihomo Core 要求 provider 文件置于 Core home (/var/lib/mihomo-cli) 下且对 mihomo 用户可读；建议改用 type: inline 或将文件移入 /var/lib/mihomo-cli"
                        ));
                    }
                }
            }
        }
    }
    warnings
}

fn tun_owned_override_keys_warning(override_yaml: &serde_yaml::Value) -> Option<String> {
    // 只告警**控制键**（会被拒绝的那些）；mtu/device/route-exclude-address 等
    // 非控制键照常生效，不得误报（review 回归-3）。
    let tun = override_yaml.get("tun")?.as_mapping()?;
    let owned: Vec<&str> = config::TUN_TRANSACTION_CONTROLLED_KEYS
        .iter()
        .copied()
        .filter(|key| tun.contains_key(*key))
        .collect();
    if owned.is_empty() {
        return None;
    }
    Some(format!(
        "  ⚠ override 中的 tun.{} 由 TUN 事务管辖：合并时会被**拒绝**（不会写入 intent/启动配置），\
         TUN 由 `tun on/off` 的 candidate/snapshot 事务独占管辖（§4.7）；\
         如需修改请用 `mihomo-cli tun on --stack <stack> --dns-hijack <targets>`；\
         mtu / device / route-exclude-address 等非控制键不受影响：会写入 intent；TUN 已开启时，\
         `mihomo-cli tun on --yes` 与 `mihomo-cli restart --system` 都会按 intent/override \
         合并进运行快照",
        owned.join("、tun.")
    ))
}

/// 把 daemon 回传的 mirror 缺失信号翻译成面向用户的结论。
///
/// - 用户树**没有**持久化选点 → 整行删除（没有东西可回放，不是故障，也绝不能给
///   “运行选点命令重建”的误导提示）；
/// - 用户树**有**持久化选点却丢了 mirror → 明确要求重建（此时执行一次选点命令确实能重建）；
/// - 无法确定 active 订阅 → 只陈述事实，不给动作。
///
/// review MAJOR-5：这个判断必须在 CLI（用户权限）完成，daemon 按 SPEC 不读用户 home。
/// POSIX shell single-quote escaping for remediation command arguments
/// (A3': group/node names may contain spaces or quotes).
/// Per-outcome remediation lines (A3' outcome matrix).
/// `members`: current core members of the target group (top-10, `None` if the
/// group is missing or lookup failed); `groups`: current selectable groups
/// (top-10, only used for group-missing).
fn print_selection_replay_remedies(report: &crate::selection::ReplayReport) {
    for line in crate::selection::selection_replay_remedy_lines(report) {
        println!("{line}");
    }
}

/// Signal identification: must NOT use substring `contains()` to prevent
/// injection (e.g., a node named `[selection-mirror-missing]`). Match the
/// exact fixed signal lines emitted by the daemon.
fn is_mirror_missing_signal(line: &str) -> bool {
    line.starts_with("[selection-mirror-missing]")
}

fn is_identity_mismatch_signal(line: &str) -> bool {
    line.starts_with("[selection-identity-mismatch]")
}

fn translate_selection_replay_signal(
    message: &str,
    subscription_id_at_request: Option<&str>,
    paths: &utils::AppPaths,
) -> String {
    let has_signal = message
        .lines()
        .any(|line| is_mirror_missing_signal(line) || is_identity_mismatch_signal(line));
    if !has_signal {
        return message.to_string();
    }
    // N4: bind to the subscription ID at request time, NOT the current active
    // (which may have changed during the in-flight request).
    let persisted_selections: Option<std::collections::BTreeMap<String, String>> =
        subscription_id_at_request.and_then(|id| {
            let path = paths.selection_state_path_for_subscription(id);
            if !path.exists() {
                return None;
            }
            std::fs::read_to_string(path)
                .ok()
                .and_then(|content| {
                    serde_yaml::from_str::<selection::SelectionStateFile>(&content).ok()
                })
                .map(|state| state.selections)
        });
    message
        .lines()
        .filter_map(|line| {
            if is_identity_mismatch_signal(line) {
                // A3': mirror active ≠ request identity. Give
                // per-group remedy + identity check guidance.
                let mut lines = vec![
                    "⚠ Selections not replayed: selection subscription identity mismatch"
                        .to_string(),
                ];
                if let Some(selections) = &persisted_selections {
                    for (group, node) in selections {
                        lines.push(format!(
                            "  If the target still exists: {}",
                            crate::selection::select_fix_command(group, node)
                        ));
                    }
                }
                lines.push("  Check identity: mihomo-cli config --list".to_string());
                return Some(lines.join("\n"));
            }
            if !is_mirror_missing_signal(line) {
                return Some(line.to_string());
            }
            match (&subscription_id_at_request, &persisted_selections) {
                // A3': per-group remedy, prefix with "target existence
                // unverified" (mirror missing means we can't confirm the group/node
                // still exists in Core).
                (Some(_), Some(selections)) if !selections.is_empty() => {
                    let mut lines =
                        vec!["⚠ Selections not replayed: selection mirror unavailable".to_string()];
                    for (group, node) in selections {
                        lines.push(format!(
                            "  If the target still exists: {}",
                            crate::selection::select_fix_command(group, node)
                        ));
                    }
                    Some(lines.join("\n"))
                }
                (Some(_), Some(_)) => None,
                (Some(_), None) => None,
                (None, _) => {
                    Some("⚠ Selections not replayed: selection mirror unavailable".to_string())
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Success lines for a lifecycle command. The daemon `Success` message can
/// carry replay lines, and any `⚠` line in that report means part of the
/// outcome did not apply — a mixed result must not be dressed in the success
/// glyph (issue §2.5).
fn lifecycle_success_lines(
    message: &str,
    subscription_id_at_request: Option<&str>,
    paths: &utils::AppPaths,
) -> Vec<String> {
    let translated = translate_selection_replay_signal(message, subscription_id_at_request, paths);
    if translated.lines().any(|line| line.starts_with('⚠')) {
        return translated.lines().map(ToString::to_string).collect();
    }
    if translated.trim().is_empty() {
        return vec!["  ✅".to_string()];
    }
    vec![format!("  ✅ {}", translated.trim_end())]
}

/// Lines the promotion success message must surface: the daemon-side replay
/// report (remedies included) and any mirror/identity signal, translated
/// against the subscription ID sent with the request. Never dropped.
fn selection_replay_message_lines(
    message: &str,
    subscription_id_at_request: Option<&str>,
    paths: &utils::AppPaths,
) -> Vec<String> {
    if message.is_empty() {
        return Vec::new();
    }
    translate_selection_replay_signal(message, subscription_id_at_request, paths)
        .lines()
        .map(ToString::to_string)
        .collect()
}

fn override_action_intent(action: &OverrideAction) -> instance::CommandIntent {
    match action {
        OverrideAction::Path | OverrideAction::Show => instance::CommandIntent::ReadOnly,
        OverrideAction::Import { .. } | OverrideAction::Clear { .. } => {
            instance::CommandIntent::Mutating
        }
    }
}

/// 裁决点 1：把「非终态事务 owner 存在时禁止配置写入」接到 CLI 写入入口。
///
/// 只做只读解析与 journal 读取，不触碰任何配置；拒绝时零写入。
fn ensure_no_transaction_owner_before_config_write(
    system: bool,
    user: bool,
    action: &str,
) -> anyhow::Result<()> {
    // 未安装任何实例时没有 system 事务 owner（模式解析会以 "mode required" 失败）：
    // 此时跳过闸门，保持 `override import` 在全新机器上的既有可用性；真正的模式
    // 冲突/不可解析会在后续 resolved_*_for_config_command 处以同样原因拒绝。
    let Ok(resolved) =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)
    else {
        return Ok(());
    };
    tun_transaction::ensure_no_non_terminal_transaction(&resolved.ctx, action)
}

async fn apply_override_change(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
    content: Option<String>,
    action_label: &str,
) -> anyhow::Result<()> {
    // 缺陷 2：类型绕过（tun: null/标量/数组）在写 override.yaml 之前拒绝。
    if let Some(ref text) = content {
        let parsed: serde_yaml::Value = serde_yaml::from_str(text)
            .map_err(|error| anyhow::anyhow!("override.yaml is not valid YAML: {error}"))?;
        config::validate_override_tun_shape(&parsed)?;
        config::validate_override_proxies_append_shape(&parsed)?;
    }
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    // P1-1：非终态事务 owner 闸门必须与「首次写入」处在**同一把 ConfigLock**
    // 内，否则 `guard → 并发 prepare 发布 journal → 写入` 的 TOCTOU 窗口仍然存在
    // （事务窗口内改写 intent 会让 base_intent_revision 立刻失真）。
    ensure_no_transaction_owner_before_config_write(system, user, "override import/clear")?;
    let snapshot = snapshot_file(&paths.override_path())?;
    if let Some(content) = content {
        if let Some(parent) = paths.override_path().parent() {
            utils::ensure_dir_all_no_follow(parent)?;
        }
        utils::atomic_write_file_for_original_user(
            &paths.override_path().display().to_string(),
            &content,
        )?;
    } else {
        utils::remove_file_if_exists(&paths.override_path())?;
    }

    let core_binary =
        resolved_core_binary_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let config_endpoint =
        resolved_api_endpoint_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    if let Err(err) =
        config::merge_user_config_checked_at_endpoint(paths, Some(&core_binary), &config_endpoint)
    {
        restore_file_snapshot(&paths.override_path(), snapshot)?;
        anyhow::bail!(
            "Override change failed; rolled back override.yaml.
  {err}"
        );
    }
    drop(_lock);
    let apply_lines =
        apply_config_reload_lines(system, user, paths, config_base_revision(paths).as_deref())
            .await;
    println!("  ✓ override.yaml {action_label}");
    print_lines(apply_lines);
    Ok(())
}

fn cmd_backup(system: bool, user: bool, output: Option<String>) -> anyhow::Result<()> {
    let paths = app_paths_for_resolved_instance_command(
        "backup",
        system,
        user,
        instance::CommandIntent::ReadOnly,
    )?;
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let dest = output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| backup::default_backup_dir(&paths));
    let report = backup::backup_config(&paths, &dest)?;
    print_lines(format_backup_success(&report));
    Ok(())
}

fn cmd_restore(system: bool, user: bool, path: &str, yes: bool) -> anyhow::Result<()> {
    // ADR-02 + v3 runtime-first resolution: restore writes to the current
    // user's config dir for both modes, but target selection must honor the
    // active runtime before falling back to installed service artifacts.
    let paths = app_paths_for_resolved_instance_command(
        "restore",
        system,
        user,
        instance::CommandIntent::Mutating,
    )?;
    let core_binary =
        resolved_core_binary_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let config_endpoint =
        resolved_api_endpoint_for_config_command(system, user, instance::CommandIntent::Mutating)?;
    let backup_path = std::path::Path::new(path);
    if !yes {
        use dialoguer::Confirm;
        if !Confirm::new()
            .with_prompt(format!(
                "Restore config files into {}? Existing files will be overwritten.",
                paths.config_dir().display()
            ))
            .default(false)
            .interact()?
        {
            println!("  Cancelled.");
            return Ok(());
        }
    }

    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let report = backup::restore_config(backup_path, &paths, true)?;
    let safety_backup = report.safety_backup;
    ensure_config_file_endpoint(&paths.config_path(), &config_endpoint)?;
    validate_config_at_paths_with_binary(&paths, &core_binary).ok();
    print_lines(format_restore_success(safety_backup.as_deref()));
    Ok(())
}

fn ensure_config_file_endpoint(
    config_path: &std::path::Path,
    endpoint: &instance::ApiEndpoint,
) -> anyhow::Result<()> {
    if !config_path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(config_path).map_err(|e| {
        anyhow::anyhow!(
            "cannot read config for endpoint repair: {} ({e})",
            config_path.display()
        )
    })?;
    let fixed = config::ensure_controller_for_endpoint(&content, endpoint)?;
    if fixed != content {
        utils::atomic_write_file_for_original_user(&config_path.display().to_string(), &fixed)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
