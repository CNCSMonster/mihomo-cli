use crate::mihomo_api::MihomoApiClient;
use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::io::IsTerminal;

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        if $crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("[DEBUG] {}", format!($($arg)*));
        }
    };
}

pub static VERBOSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

mod backup;
mod config;
mod daemon;
mod dns;
mod installer;
mod instance;
mod ipc;
mod lock;
mod mihomo_api;
mod rules;
mod service;
mod system_proxy;
mod ui;
mod utils;
mod yaml_editor;

#[derive(Parser)]
#[command(name = "mihomo-cli", version = env!("MIHOMO_CLI_VERSION"), about = "Mihomo CLI — cross-platform setup & control tool", long_about = None)]
struct Cli {
    /// Enable verbose debug output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Install mihomo binary and configure subscription
    #[command(visible_alias = "i")]
    Install {
        /// Install TUN/system service mode (advanced; daily use can run `mihomo-cli tun on`)
        #[arg(long = "system", conflicts_with = "user")]
        system: bool,
        /// Install normal per-user proxy mode (default)
        #[arg(short, long, conflicts_with = "system")]
        user: bool,
        /// Force reinstall even if already installed
        #[arg(short, long)]
        force: bool,
        /// Install a specific mihomo core version (e.g. v1.19.27)
        #[arg(long)]
        version: Option<String>,
        /// GitHub mirror base URL prepended to GitHub downloads (geo data)
        /// e.g. https://ghproxy.com/
        #[arg(long = "github-mirror")]
        github_mirror: Option<String>,
    },

    /// Check for and install the latest mihomo core version
    Upgrade {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Show mihomo-cli build information and current mihomo core version
    Version {
        /// Force the system service instance when probing core version (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Interactive subscription TUI, or use flags to manage subscriptions (add/remove/refresh/validate)
    #[command(visible_alias = "c")]
    Config {
        /// Force the system service instance for validation/reload (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Subscription URL (for initial setup or update)
        #[arg(short, long)]
        url: Option<String>,
        /// Fix the existing config file: ensure Unix socket is configured
        #[arg(long)]
        fix: bool,
        /// Refresh active subscription
        #[arg(long)]
        refresh: bool,
        /// Refresh all subscriptions
        #[arg(long, name = "refresh-all")]
        refresh_all: bool,
        /// Import config from a local file
        #[arg(long)]
        import: Option<String>,
        /// Switch to a specific subscription by ID
        #[arg(long)]
        switch: Option<String>,
        /// Add a new subscription source
        #[arg(long)]
        add: Option<String>,
        /// Remove a subscription by ID
        #[arg(long)]
        remove: Option<String>,
        /// List all subscription sources
        #[arg(long)]
        list: bool,
        /// Validate current config.yaml with YAML parser and mihomo -t
        #[arg(long)]
        validate: bool,
        /// Preview/validate the requested config operation without writing or restarting
        #[arg(long, name = "dry-run")]
        dry_run: bool,
        /// Assume yes for config prompts (currently: activate imported subscription)
        #[arg(short, long)]
        yes: bool,
        /// Show subscription info (node count, update time, expiry). Omit ID for active subscription
        #[arg(long)]
        info: Option<Option<String>>,
        /// Probe a subscription URL with bounded UA candidates without writing files
        #[arg(long)]
        probe: Option<String>,
        /// Use a fixed User-Agent for add/refresh URL fetching
        #[arg(long = "user-agent", alias = "ua")]
        user_agent: Option<String>,
        /// Set subscription User-Agent mode: pass <ID> and <UA|auto> (two args)
        #[arg(long = "set-ua", num_args = 2)]
        set_ua: Vec<String>,
        /// Force activate the added/imported subscription
        #[arg(long, conflicts_with = "no_activate")]
        activate: bool,
        /// Do not activate the added/imported subscription
        #[arg(long = "no-activate")]
        no_activate: bool,
    },

    /// Remove service and optionally all files
    #[command(visible_alias = "u")]
    Uninstall {
        /// Uninstall system service instance (advanced/debugging)
        #[arg(long = "system", conflicts_with = "user")]
        system: bool,
        /// Uninstall user-level instance
        #[arg(short, long, conflicts_with = "system")]
        user: bool,
        /// Also remove mihomo binary, config, and all data files (shortcut for --remove-binary --remove-config --remove-geo)
        #[arg(short, long)]
        all: bool,
        /// Remove mihomo core binary
        #[arg(long = "remove-binary")]
        remove_binary: bool,
        /// Remove config and data directory
        #[arg(long = "remove-config")]
        remove_config: bool,
        /// Remove geo data files (geoip.metadb, GeoSite.dat)
        #[arg(long = "remove-geo")]
        remove_geo: bool,
        /// Skip confirmation / TUI, execute directly
        #[arg(long = "yes", short = 'y')]
        yes: bool,
        /// Show what would be removed without deleting files
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Remove only legacy root-mode runtime leftovers from the user config dir
        #[arg(long = "legacy-system-leftovers", conflicts_with_all = ["system", "user", "all", "remove_binary", "remove_config", "remove_geo", "yes"])]
        legacy_root_leftovers: bool,
    },

    /// Update mihomo core binary
    #[command(visible_alias = "up")]
    Update {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    // --- Control commands ---
    /// Start mihomo service
    Start {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Stop mihomo service
    Stop {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Restart mihomo service
    Restart {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Select a node — interactive TUI (no --node) or non-interactive CLI (with --node)
    Select {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Limit to a specific proxy group
        #[arg(short, long)]
        group: Option<String>,
        /// Switch the group to this node non-interactively (requires --group)
        #[arg(long)]
        node: Option<String>,
    },

    /// List all proxy groups and current nodes
    List {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Test latency of nodes in a group
    Delay {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Proxy group to test [default: 节点选择]
        #[arg(short, long, default_value = "节点选择")]
        group: String,
        /// Re-test nodes even when cached results are still fresh
        #[arg(long)]
        refresh: bool,
        /// Reuse cached delay results newer than this many seconds
        #[arg(long = "cache-ttl", default_value_t = 300)]
        cache_ttl: u64,
        /// Select the fastest node after testing
        #[arg(long)]
        fastest: bool,
    },

    /// Toggle or check TUN mode
    #[command(name = "tun")]
    Tun {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        action: Option<TunAction>,
        /// TUN stack: system, gvisor, or mixed
        #[arg(long)]
        stack: Option<TunStack>,
        /// Enable DNS hijack, optionally with a target such as any:53
        #[arg(long = "dns-hijack", num_args = 0..=1, default_missing_value = "any:53")]
        dns_hijack: Option<String>,
    },

    /// View active connections (use --flush to close all)
    #[command(name = "conn")]
    Connections {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Close all active connections
        #[arg(short, long)]
        flush: bool,
    },

    /// Show current proxy IP probe (deprecated; use exit-ip for node/route exit IP)
    Ip {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Probe exit IP for a node, group, URL route, or direct path
    #[command(name = "exit-ip", group(
        ArgGroup::new("exit_ip_target")
            .required(true)
            .multiple(false)
            .args(["node", "group", "url", "direct"])
    ))]
    ExitIp {
        /// Probe a specific outbound node by name
        #[arg(long)]
        node: Option<String>,
        /// Probe the current effective outbound of a proxy group
        #[arg(long)]
        group: Option<String>,
        /// Resolve a URL/host route, then estimate its selected node/path exit IP
        #[arg(long)]
        url: Option<String>,
        /// Probe system direct exit without mihomo or environment proxies
        #[arg(long)]
        direct: bool,
        /// Skip confirmation when a probe needs temporary selector changes
        #[arg(short, long)]
        yes: bool,
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Set or unset shell proxy environment variables (use with eval)
    Proxy {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        #[command(subcommand)]
        action: ProxyAction,
    },

    /// Set or unset OS system proxy
    #[command(name = "system-proxy", after_help = "\
Limitations:
  Linux: only GNOME (gsettings). Headless/server/KDE/other DE → use HTTP_PROXY env var or TUN mode.
  Only affects apps that read OS system proxy settings (GTK/GNOME apps, some browsers).
  CLI tools (curl, wget, codex) typically need HTTP_PROXY/HTTPS_PROXY env vars instead.
  Redundant when TUN mode is active (TUN already captures all traffic).")]
    SystemProxy {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        #[command(subcommand)]
        action: SystemProxyAction,
    },

    /// Show running status overview (includes proxy probe)
    Status {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Show detailed service/config paths
        #[arg(long)]
        verbose: bool,
    },

    /// View mihomo log file
    Logs {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Only show last N lines
        #[arg(long, default_value_t = 50)]
        tail: usize,
        /// Filter lines by level keyword, e.g. info, warning, error, debug
        #[arg(long)]
        level: Option<String>,
        /// Follow new log lines, like tail -f
        #[arg(short, long)]
        follow: bool,
    },
    /// Manage user-defined routing rules
    Rule {
        /// Force the system service instance for validation/reload (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        #[command(subcommand)]
        action: RuleAction,
    },
    /// Manage DNS routing policies (nameserver-policy)
    Dns {
        /// Force the system service instance for validation/reload (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        #[command(subcommand)]
        action: DnsAction,
    },

    /// Manage override.yaml advanced config overlay
    Override {
        /// Force the system service instance for validation/reload (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        #[command(subcommand)]
        action: OverrideAction,
    },

    /// Backup mihomo-cli configuration files
    Backup {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Output directory. Defaults to <instance config>/backups/<timestamp>
        output: Option<String>,
    },

    /// Restore mihomo-cli configuration files from a backup directory
    Restore {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Backup directory created by `mihomo-cli backup`
        path: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Run as system service daemon (internal, used by systemd/launchd)
    #[command(hide = true)]
    Daemon {
        /// Recover from daemon crash (reattach to running core)
        #[arg(long)]
        recover: bool,
    },

    /// Show real-time status dashboard (TUI)
    #[command(visible_alias = "dash")]
    Dashboard,
}

#[derive(ValueEnum, Clone)]
enum TunAction {
    On,
    Off,
    Status,
}

#[derive(ValueEnum, Clone)]
enum TunStack {
    System,
    Gvisor,
    Mixed,
}

impl std::fmt::Display for TunStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => write!(f, "system"),
            Self::Gvisor => write!(f, "gvisor"),
            Self::Mixed => write!(f, "mixed"),
        }
    }
}

#[derive(Subcommand, Clone)]
enum ProxyAction {
    /// Output export commands for http_proxy / https_proxy
    On,
    /// Output unset commands for proxy variables
    Off,
}

#[derive(Subcommand, Clone)]
enum SystemProxyAction {
    /// Enable OS system proxy to current mihomo port
    On,
    /// Disable OS system proxy
    Off,
}

#[derive(Subcommand, Clone)]
enum RuleAction {
    /// Add a routing rule (e.g. DOMAIN-SUFFIX,example.com,DIRECT)
    Add {
        /// Rule string: TYPE,PARAMETER,POLICY
        rule: String,
        /// Insert at front or back (overrides default position)
        #[arg(short, long)]
        position: Option<String>,
    },
    /// List all user-defined rules
    #[command(visible_alias = "ls")]
    List,
    /// Remove a rule by index (1-based)
    #[command(visible_alias = "rm")]
    Remove {
        /// Rule index (1-based, as shown in `rule list`)
        index: usize,
    },
    /// Clear all user-defined rules
    Clear {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
    /// Move a rule from one position to another (1-based indexes)
    Move {
        /// Source index (1-based, as shown in `rule list`)
        from: usize,
        /// Destination index (1-based, as shown in `rule list`)
        to: usize,
    },
    /// Import rules from a YAML file
    Import {
        /// Path to the YAML file to import
        path: String,
    },
    /// Export current rules to a YAML file
    Export {
        /// Path to write the rules file
        path: String,
    },
    /// Set or show the default rule insertion position
    Position {
        /// Position: front or back (omit to show current)
        position: Option<String>,
    },
    /// List supported rule types with examples
    Types,
    /// List valid policies (built-ins + current proxy groups)
    Policies,
    /// Test which rule matches a domain or IP using current config.yaml
    Test {
        /// Domain or IP to test, e.g. google.com or 8.8.8.8
        target: String,
    },
}

#[derive(Subcommand, Clone)]
enum DnsAction {
    /// Manage DNS routing policies
    Policy {
        #[command(subcommand)]
        action: DnsPolicyAction,
    },
    /// Show current DNS configuration
    Status,
    /// List or apply common DNS policy templates
    Template {
        #[command(subcommand)]
        action: Option<DnsTemplateAction>,
    },
}

#[derive(Subcommand, Clone)]
enum DnsTemplateAction {
    /// List available DNS templates
    List,
    /// Apply a DNS template
    Apply {
        /// Template name, e.g. company or ads
        name: String,
        /// Internal domain for company template, e.g. corp.example.com
        #[arg(long)]
        domain: Option<String>,
        /// DNS target for company template, e.g. 10.10.1.251
        #[arg(long)]
        target: Option<String>,
    },
}

#[derive(Subcommand, Clone)]
enum OverrideAction {
    /// Print override.yaml path
    Path,
    /// Show override.yaml content
    Show,
    /// Import a YAML mapping as override.yaml, then merge and hot-reload if possible
    Import {
        /// YAML file to copy to override.yaml
        path: String,
    },
    /// Remove override.yaml, then merge and hot-reload if possible
    Clear {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand, Clone)]
enum DnsPolicyAction {
    /// Add a DNS policy (domain → DNS target)
    Add {
        /// Domain suffix pattern (e.g. ubtrobot.com)
        #[arg(value_name = "MATCH")]
        match_pattern: String,
        /// DNS target: "system" for system DNS, or IP address (e.g. 10.10.1.251)
        #[arg(value_name = "TARGET")]
        target: String,
    },
    /// List all DNS policies
    #[command(visible_alias = "ls")]
    List,
    /// Remove a DNS policy by index (1-based) or match pattern
    #[command(visible_alias = "rm")]
    Remove {
        /// Policy index (1-based) or match pattern
        #[arg(value_name = "INDEX|MATCH")]
        selector: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if cli.verbose {
        VERBOSE.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if let Err(e) = run(cli).await {
        eprintln!("\n  Error: {e}");
        eprintln!("  Run with -v for more details");
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
    }) {
        Command::Install {
            system,
            user,
            force,
            version,
            github_mirror,
        } => {
            cmd_install_entry(
                system,
                user,
                force,
                version.as_deref(),
                github_mirror.as_deref(),
            )
            .await
        }
        Command::Upgrade { system } => cmd_upgrade(system, false).await,
        Command::Version { system } => cmd_version(system, false).await,
        Command::Config {
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
                cmd_uninstall_resolved(system, user, all, remove_binary, remove_config, remove_geo, yes, dry_run)
            }
        }
        Command::Update { system } => cmd_update(system, false).await,
        Command::Start { system } => {
            cmd_lifecycle_resolved(system, false, instance::ServiceAction::Start).await
        }
        Command::Stop { system } => {
            cmd_lifecycle_resolved(system, false, instance::ServiceAction::Stop).await
        }
        Command::Restart { system } => {
            cmd_lifecycle_resolved(system, false, instance::ServiceAction::Restart).await
        }
        Command::Select { system, group, node } => {
            cmd_select_resolved(system, false, group, node).await
        }
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
        } => cmd_tun_resolved(system, false, action, stack, dns_hijack).await,
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
            cmd_status_resolved(system, false).await
        }
        Command::Logs {
            system,
            tail,
            level,
            follow,
        } => cmd_logs(system, false, tail, level.as_deref(), follow),
        Command::Rule { system, action } => cmd_rule(system, false, action).await,
        Command::Dns { system, action } => cmd_dns(system, false, action).await,
        Command::Override { system, action } => cmd_override(system, false, action).await,
        Command::Backup { system, output } => cmd_backup(system, false, output),
        Command::Restore { system, path, yes } => cmd_restore(system, false, &path, yes),
        Command::Daemon { recover } => {
            let sock_path = ipc::system_service_socket_path();
            if recover {
                daemon::recover_daemon(sock_path).await
            } else {
                daemon::run_daemon(sock_path).await
            }
        }
        Command::Dashboard => cmd_dashboard().await,
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
        return legacy_root_service_from_file(std::path::Path::new(
            "/Library/LaunchDaemons/io.mihomo.plist",
        ));
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
            instance::TargetOs::Windows => instance::windows_user_install_marker(ctx)
                .is_some_and(|marker| marker.exists()),
            _ => ctx
                .paths
                .service_file
                .as_ref()
                .is_some_and(|p| p.exists()),
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
    ctx.paths.config_file = config_dir.join("config.yaml");
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
    cmd_install_entry(false, false, false, None, None).await?;
    Ok(true)
}

async fn cmd_lifecycle_resolved(
    system: bool,
    user: bool,
    action: instance::ServiceAction,
) -> anyhow::Result<()> {
    let intent = match action {
        instance::ServiceAction::Start | instance::ServiceAction::Restart => {
            instance::CommandIntent::StartLike
        }
        instance::ServiceAction::Stop => instance::CommandIntent::StopLike,
        instance::ServiceAction::Uninstall => instance::CommandIntent::UninstallLike,
    };
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
    if matches!(action, instance::ServiceAction::Start | instance::ServiceAction::Restart)
        && !system
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
                                                   mihomo-cli daemon --recover   Restart daemon and reattach to running core\n  \
                                                   mihomo-cli stop --system       Stop the orphan core process\n  \
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
    let result = cmd_lifecycle_instance_mode(mode, action).await;

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

#[allow(clippy::too_many_arguments)]
fn cmd_uninstall_resolved(
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
        return cmd_uninstall_all_instance_modes(&modes);
    }

    let mode = resolve_current_mode(system, user, instance::CommandIntent::UninstallLike)?;
    cmd_uninstall_instance_mode(mode, all, remove_binary, remove_config, remove_geo, yes, dry_run)
}

fn api_requires_running_instance_message(ctx: &instance::InstanceContext) -> String {
    match ctx.mode {
        instance::InstanceMode::System => {
            let recovery = system_service_recovery_command(ctx).unwrap_or_else(|| {
                "restart the system service with your OS service manager".to_string()
            });
            format!(
                "mihomo core API is not running for the system service.
                   Recover/start it, then retry:
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

    if mihomo_api::api_get_at_endpoint(&resolved.ctx.paths.api_endpoint, "/configs")
        .await
        .is_err()
    {
        if resolved.ctx.mode == instance::InstanceMode::System && ipc::is_daemon_running().await {
            println!("Core API is not ready; starting system core...");
            ensure_instance_controller_endpoint(&resolved.ctx)?;
            start_system_core_via_daemon(&resolved.ctx).await?;
            wait_for_instance_readiness(&resolved.ctx).await?;
        } else {
            anyhow::bail!(api_requires_running_instance_message(&resolved.ctx));
        }
    }

    Ok(mihomo_api::EndpointMihomoApiClient::new(
        resolved.ctx.paths.api_endpoint,
    ))
}

/// Real-time status dashboard (TUI)
async fn cmd_dashboard() -> anyhow::Result<()> {
    let resolved = resolve_current_instance_context(
        false,
        false,
        instance::CommandIntent::ReadOnly,
    )?;
    let endpoint = &resolved.ctx.paths.api_endpoint;
    let client = mihomo_api::EndpointMihomoApiClient::new(endpoint.clone());

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
            let mode = instance_mode_label(resolved.ctx.mode);
            let tun = configs
                .as_ref()
                .and_then(|c| c.get("tun"))
                .and_then(|t| t.get("enable"))
                .and_then(|e| e.as_bool())
                .unwrap_or(false);
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
                            v.get("now").and_then(|n| n.as_str()).map(|now| {
                                format!("{} → {}", name, now)
                            })
                        })
                        .collect::<Vec<_>>()
                        .join("\n  ")
                })
                .unwrap_or_else(|| "no active group".to_string());

            // Render
            write!(
                stdout,
                "{}{}",
                Clear(ClearType::All),
                MoveTo(0, 0)
            )?;
            writeln!(stdout, "╔══════════════════════════════════════════╗")?;
            writeln!(stdout, "║        mihomo-cli  Dashboard             ║")?;
            writeln!(stdout, "╠══════════════════════════════════════════╣")?;
            writeln!(stdout, "║  Mode:        {:<25} ║", mode)?;
            writeln!(stdout, "║  TUN:         {:<25} ║", if tun { "✅ enabled" } else { "❌ disabled" })?;
            writeln!(stdout, "║  Mixed port:  {:<25} ║", mixed_port)?;
            writeln!(stdout, "║  Connections: {:<25} ║", conn_count)?;
            writeln!(stdout, "║  Upload:      {:<25} ║", format_bytes(upload_total))?;
            writeln!(stdout, "║  Download:    {:<25} ║", format_bytes(download_total))?;
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
    Ok(mihomo_api::EndpointMihomoApiClient::new(
        resolved.ctx.paths.api_endpoint,
    ))
}

async fn cmd_select_resolved(
    system: bool,
    user: bool,
    group: Option<String>,
    node: Option<String>,
) -> anyhow::Result<()> {
    let client = resolve_ready_api_client(system, user, instance::CommandIntent::Mutating).await?;
    match node {
        // Non-interactive CLI: switch group to a specific node.
        Some(node_name) => {
            let group_name = group.ok_or_else(|| {
                anyhow::anyhow!(
                    "--node requires --group (the proxy group that contains the node)"
                )
            })?;
            mihomo_api::select_proxy_with_client(&client, &group_name, &node_name).await?;
            println!("Switched {group_name} → {node_name}");
            Ok(())
        }
        // Interactive TUI (no --node): existing behavior.
        None => match group {
            Some(g) => ui::select_node_with_client(&client, &g).await,
            None => ui::flat_select_with_client(&client).await,
        },
    }
}

async fn cmd_list_resolved(system: bool, user: bool) -> anyhow::Result<()> {
    let client = resolve_ready_api_client(system, user, instance::CommandIntent::ReadOnly).await?;
    mihomo_api::list_proxies_with_client(&client).await
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
    let content = std::fs::read_to_string(&ctx.paths.config_file).map_err(|e| {
        anyhow::anyhow!(
            "cannot read config for endpoint repair: {} ({e})",
            ctx.paths.config_file.display()
        )
    })?;
    let fixed = config::ensure_controller_for_endpoint(&content, &ctx.paths.api_endpoint)?;
    if fixed != content {
        write_instance_text_file(ctx, &ctx.paths.config_file, &fixed, 0o644)?;
    }
    Ok(())
}

fn set_instance_tun_config(
    ctx: &instance::InstanceContext,
    enable: bool,
    stack: Option<&TunStack>,
    dns_hijack: Option<&str>,
) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(&ctx.paths.config_file).map_err(|e| {
        anyhow::anyhow!(
            "cannot read config for TUN update: {} ({e})",
            ctx.paths.config_file.display()
        )
    })?;
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
    if let Some(dns_hijack) = dns_hijack {
        tun.insert(
            serde_yaml::Value::String("dns-hijack".to_string()),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String(dns_hijack.to_string())]),
        );
    }
    write_instance_text_file(
        ctx,
        &ctx.paths.config_file,
        &serde_yaml::to_string(&doc)?,
        0o644,
    )
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

fn tun_action_uses_daemon_status(action: Option<&TunAction>) -> bool {
    matches!(action, Some(TunAction::Status) | None)
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
) -> anyhow::Result<bool> {
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
    cmd_install_instance(instance::InstanceMode::System, true, None, github_mirror).await?;
    Ok(true)
}

async fn prompt_install_system_service_for_tun(github_mirror: Option<&str>) -> anyhow::Result<bool> {
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
    cmd_install_instance(instance::InstanceMode::System, true, None, github_mirror).await?;
    Ok(true)
}

fn system_service_recovery_command(ctx: &instance::InstanceContext) -> Option<String> {
    let command = instance::planned_service_plan(ctx, instance::ServiceAction::Restart)
        .commands
        .into_iter()
        .next()?;
    instance::privilege_invocation_plan(command).map(|plan| plan.manual_fallback)
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
         Recover the daemon, then retry:
  \
           {recovery}
  \
           mihomo-cli {retry}
  \
         If the system service is not installed yet, run: mihomo-cli install --system"
    )
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

async fn cmd_tun_resolved(
    system: bool,
    user: bool,
    action: Option<TunAction>,
    stack: Option<TunStack>,
    dns_hijack: Option<String>,
) -> anyhow::Result<()> {
    let intent = tun_action_intent(action.as_ref());
    if matches!(action, Some(TunAction::On) | Some(TunAction::Off)) {
        let request = mode_request_from_flags(system, user);
        let env = current_environment_state();
        match resolve_environment_for_intent(request, &env, tun_user_intent(action.as_ref())) {
            RuntimeFirstModeResolution::NeedsSystemSwitch {
                user_running,
                user_installed,
            } => {
                if !prompt_switch_user_to_system_for_tun(user_running, user_installed, None).await?
                {
                    return Ok(());
                }
            }
            RuntimeFirstModeResolution::NeedsSystemInstall { .. } => {
                if matches!(action, Some(TunAction::On)) && !system && !user {
                    if !prompt_install_system_service_for_tun(None).await? {
                        return Ok(());
                    }
                } else {
                    anyhow::bail!(tun_requires_system_service_install_message());
                }
            }
            RuntimeFirstModeResolution::NeedsSystemDaemonRecovery { .. } => {
                let ctx = instance::planned_current_context(instance::InstanceMode::System)
                    .ok_or_else(|| anyhow::anyhow!("Unsupported OS for system daemon recovery"))?;
                if let Some(message) = system_tun_requires_daemon_message(action.as_ref(), &ctx) {
                    anyhow::bail!(message);
                }
            }
            _ => {}
        }
    }
    let resolved = resolve_current_instance_context(system, user, intent)?;
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
                "TUN requires the system service, but the per-user core is running.\n  \
                 v3 uses mutually exclusive modes; enabling TUN would make the user core ineffective.\n  \
                 Suggestions:\n  \
                   mihomo-cli stop\n  \
                   mihomo-cli tun on"
            );
        }
        let presence = current_service_presence();
        if presence.system {
            let runtime = current_runtime_presence();
            if runtime.user {
                // user 模式正在运行，需要先停止才能切换到 system
                anyhow::bail!(
                    "TUN requires the system service.\n  \
                     The system service is installed, but the per-user core is running.\n  \
                     v3 uses mutually exclusive modes.\n  \
                     Stop the user instance first, then start the system service:\n  \
                       mihomo-cli stop\n  \
                       mihomo-cli start\n  \
                       mihomo-cli tun on"
                );
            } else {
                // user 没在跑，可以直接启动 system service
                anyhow::bail!(
                    "TUN requires the system service.\n  \
                     The system service is already installed — use it directly:\n  \
                       mihomo-cli start\n  \
                       mihomo-cli tun on\n  \
                     Per-user service does not have the privileges needed for TUN."
                );
            }
        }
        anyhow::bail!(tun_requires_system_service_install_message());
    }

    let daemon_running = ipc::is_daemon_running().await;

    // Phase 3: 如果 daemon 正在运行，通过 IPC 发送 TUN 命令
    if (action.is_some() || tun_action_uses_daemon_status(action.as_ref())) && daemon_running {
        let tun_config_snapshot = if resolved.ctx.mode == instance::InstanceMode::System
            && matches!(action, Some(TunAction::On) | Some(TunAction::Off))
        {
            Some(snapshot_file(&resolved.ctx.paths.config_file)?)
        } else {
            None
        };

        if resolved.ctx.mode == instance::InstanceMode::System {
            match action {
                Some(TunAction::On) => {
                    ensure_instance_controller_endpoint(&resolved.ctx)?;
                    set_instance_tun_config(
                        &resolved.ctx,
                        true,
                        stack.as_ref(),
                        dns_hijack.as_deref(),
                    )?;
                }
                Some(TunAction::Off) => {
                    set_instance_tun_config(&resolved.ctx, false, None, None)?;
                }
                _ => {}
            }
        }
        let cmd = match action {
            Some(TunAction::On) => ipc::DaemonCommand::EnableTun {
                config_path: resolved.ctx.paths.config_file.clone(),
                stack: stack.as_ref().map(ToString::to_string),
                dns_hijack: dns_hijack.clone(),
            },
            Some(TunAction::Off) => ipc::DaemonCommand::DisableTun,
            Some(TunAction::Status) | None => ipc::DaemonCommand::GetStatus,
        };
        let resp = match ipc::send_command(&cmd).await {
            Ok(resp) => resp,
            Err(err) => {
                if let Some(snapshot) = tun_config_snapshot {
                    restore_file_snapshot(&resolved.ctx.paths.config_file, snapshot)?;
                }
                return Err(err);
            }
        };
        match resp {
            ipc::DaemonResponse::Success { message } => {
                println!("  ✅ {message}");
                return Ok(());
            }
            ipc::DaemonResponse::Error { message } => {
                if let Some(snapshot) = tun_config_snapshot {
                    restore_file_snapshot(&resolved.ctx.paths.config_file, snapshot)?;
                }
                anyhow::bail!("daemon error: {message}");
            }
            ipc::DaemonResponse::Status {
                running,
                tun_enabled,
                core_pid,
                config_path,
            } => {
                println!("System daemon: running");
                println!("Core running: {running}");
                println!("TUN enabled: {tun_enabled}");
                if let Some(pid) = core_pid {
                    println!("Core PID: {pid}");
                }
                if let Some(path) = config_path {
                    println!("Config: {}", path.display());
                }
                return Ok(());
            }
        }
    }

    if resolved.ctx.mode == instance::InstanceMode::System {
        if let Some(message) = system_tun_requires_daemon_message(action.as_ref(), &resolved.ctx) {
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
    client: &mihomo_api::EndpointMihomoApiClient,
) -> anyhow::Result<(String, String, String)> {
    let port = mihomo_api::get_port_with_client(client).await?;
    mihomo_api::fetch_ip_info_fast_with_proxy_port(port, std::time::Duration::from_secs(5)).await
}

async fn probe_with_temporary_selection(
    client: &mihomo_api::EndpointMihomoApiClient,
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

#[allow(dead_code)]
async fn cmd_lifecycle_instance(
    system: bool,
    user: bool,
    action: instance::ServiceAction,
) -> anyhow::Result<()> {
    let mode = match mode_request_from_flags(system, user) {
        instance::ModeRequest::ExplicitSystem => instance::InstanceMode::System,
        instance::ModeRequest::ExplicitUser => instance::InstanceMode::User,
        instance::ModeRequest::Unspecified => unreachable!("caller handles unspecified lifecycle"),
    };
    cmd_lifecycle_instance_mode(mode, action).await
}

async fn cmd_lifecycle_instance_mode(
    mode: instance::InstanceMode,
    action: instance::ServiceAction,
) -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance lifecycle"))?;

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
            anyhow::bail!(system_daemon_unavailable_message(
                lifecycle_command_name(action),
                &ctx,
            ));
        }

        let cmd = match action {
            instance::ServiceAction::Start => ipc::DaemonCommand::StartCore {
                config_path: ctx.paths.config_file.clone(),
            },
            instance::ServiceAction::Stop => ipc::DaemonCommand::StopCore,
            instance::ServiceAction::Restart => ipc::DaemonCommand::RestartCore {
                config_path: ctx.paths.config_file.clone(),
            },
            _ => return Err(anyhow::anyhow!("unsupported lifecycle action via IPC")),
        };
        let resp = ipc::send_command(&cmd).await?;
        match resp {
            ipc::DaemonResponse::Success { message } => {
                println!("  ✅ {message}");
                // Readiness is confirmed by the daemon (it waits for the core
                // API before replying Success). The CLI must not re-poll —
                // aligned with clash-verge-service: client trusts the
                // daemon's Success as the readiness contract.
                return Ok(());
            }
            ipc::DaemonResponse::Error { message } => {
                anyhow::bail!("daemon error: {message}");
            }
            ipc::DaemonResponse::Status { .. } => return Ok(()),
        }
    }

    // Phase 5: 启动前检查 TCP 端口是否被占用
    if matches!(action, instance::ServiceAction::Start) {
        if let Some(port) = read_mixed_port_from_config(&ctx.paths.config_file) {
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
    }

    Ok(())
}

fn system_core_start_command(ctx: &instance::InstanceContext) -> ipc::DaemonCommand {
    ipc::DaemonCommand::StartCore {
        config_path: ctx.paths.config_file.clone(),
    }
}

async fn start_system_core_via_daemon(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    let cmd = system_core_start_command(ctx);
    match ipc::send_command(&cmd).await? {
        ipc::DaemonResponse::Success { message } => {
            println!("  ✅ {message}");
            Ok(())
        }
        ipc::DaemonResponse::Error { message } => anyhow::bail!("daemon error: {message}"),
        ipc::DaemonResponse::Status { .. } => Ok(()),
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
    while start.elapsed() < timeout {
        match mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs").await {
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
    let content = std::fs::read_to_string(&ctx.paths.config_file).map_err(|e| {
        anyhow::anyhow!(
            "cannot read config for readiness check: {} ({e})",
            ctx.paths.config_file.display()
        )
    })?;
    let expected_line = ctx.paths.api_endpoint.controller_line();
    if content.contains(&expected_line) {
        return Ok(());
    }
    anyhow::bail!(
        "config endpoint does not match selected instance.\n  Expected: {}\n  Config: {}\n  Fix: {}",
        expected_line,
        ctx.paths.config_file.display(),
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
    }
}

fn lifecycle_command_name(action: instance::ServiceAction) -> &'static str {
    match action {
        instance::ServiceAction::Start => "start",
        instance::ServiceAction::Stop => "stop",
        instance::ServiceAction::Restart => "restart",
        instance::ServiceAction::Uninstall => "uninstall",
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

async fn cmd_status_context_with_source(
    ctx: instance::InstanceContext,
    source: Option<instance::ResolutionSource>,
) -> anyhow::Result<()> {
    let plan = instance::planned_status_diagnostics(&ctx);

    let verbose = crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed);

    println!("=== Mihomo Status ===");
    println!("  Mode:          {}", instance_mode_label(plan.mode));
    if let Some(source) = source {
        println!("  Resolved by:   {}", resolution_source_label(source));
    }
    if verbose {
        println!("  Service:       {}", status_service_label(&plan.service));
        println!(
            "  Service state: {}",
            if service_manager_active(&ctx) {
                "active"
            } else {
                "not active/unknown"
            }
        );
        println!(
            "  mihomo binary: {} {}",
            if plan.binary.exists() {
                "✅"
            } else {
                "NOT FOUND"
            },
            plan.binary.display()
        );
    }
    println!(
        "  Config:        {} {}",
        if plan.config_file.exists() {
            "✅"
        } else {
            "missing"
        },
        plan.config_file.display()
    );
    if verbose {
        println!(
            "  API endpoint:  {}",
            status_endpoint_label(&plan.expected_endpoint)
        );
    }

    if ctx.mode == instance::InstanceMode::System {
        if ipc::is_daemon_running().await {
            println!(
                "  Daemon IPC:    ✅ {}",
                ipc::system_service_socket_path().display()
            );
            match ipc::send_command(&ipc::DaemonCommand::GetStatus).await {
                Ok(ipc::DaemonResponse::Status {
                    running,
                    tun_enabled,
                    core_pid,
                    config_path,
                }) => {
                    println!(
                        "  Core running:  {}",
                        if running { "✅" } else { "stopped" }
                    );
                    println!("  TUN enabled:   {}", if tun_enabled { "✅" } else { "no" });
                    if let Some(pid) = core_pid {
                        println!("  Core PID:      {pid}");
                    }
                    if let Some(path) = config_path {
                        println!("  Active config: {}", path.display());
                    }
                }
                Ok(ipc::DaemonResponse::Error { message }) => {
                    println!("  Daemon status: error: {message}");
                }
                Ok(ipc::DaemonResponse::Success { message }) => {
                    println!("  Daemon status: {message}");
                }
                Err(e) => {
                    println!("  Daemon status: error: {e}");
                }
            }
        } else {
            println!(
                "  Daemon IPC:    not running ({})",
                ipc::system_service_socket_path().display()
            );
        }
    } else {
        let api_running = mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs")
            .await
            .is_ok();
        println!(
            "  Core API:      {}",
            if api_running {
                "✅ reachable"
            } else {
                "not reachable"
            }
        );
    }

    print_status_proxy_probe(&ctx.paths.api_endpoint).await;

    if let Some(log_file) = &plan.log_file {
        println!("  Logs:          {}", log_file.display());
    }

    if verbose {
        if let Some(legacy) = current_legacy_root_service() {
            println!();
            for line in format_legacy_root_service_diagnostic(&legacy) {
                println!("{line}");
            }
        }
        println!();
        println!("=== Instance Diagnostics Plan ===");
        for probe in plan.probes {
            println!(
                "  - {:?}: {}{}",
                probe.kind,
                probe.target,
                if probe.privileged {
                    " (privileged)"
                } else {
                    ""
                }
            );
        }
    }

    Ok(())
}

async fn print_status_proxy_probe(endpoint: &instance::ApiEndpoint) {
    let client = mihomo_api::EndpointMihomoApiClient::new(endpoint.clone());
    match mihomo_api::get_port_with_client(&client).await {
        Ok(port) => match mihomo_api::fetch_ip_info_fast_with_proxy_port(
            port,
            std::time::Duration::from_secs(2),
        )
        .await
        {
            Ok((ip, country, source)) => {
                println!("  Exit IP:       {ip} ({country}, via {source})");
            }
            Err(e) if crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) => {
                println!("  Exit IP:       unavailable ({e})");
            }
            Err(_) => {
                println!("  Exit IP:       unavailable");
            }
        },
        Err(e) if crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) => {
            println!("  Exit IP:       unavailable ({e})");
        }
        Err(_) => {
            println!("  Exit IP:       unavailable");
        }
    }
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

async fn cmd_install_entry(
    system: bool,
    user: bool,
    force: bool,
    version: Option<&str>,
    github_mirror: Option<&str>,
) -> anyhow::Result<()> {
    let mode = if system {
        instance::InstanceMode::System
    } else if user {
        instance::InstanceMode::User
    } else {
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
    cmd_install_instance(mode, force, version, github_mirror).await
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

async fn cmd_install_instance(
    mode: instance::InstanceMode,
    force: bool,
    version: Option<&str>,
    github_mirror: Option<&str>,
) -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance install"))?;
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
    println!("  Config:   {}", ctx.paths.config_file.display());
    println!(
        "  Endpoint: {}",
        status_endpoint_label(&ctx.paths.api_endpoint)
    );

    // Pre-flight: check each component, skip valid ones (unless --force)
    let binary_valid = installer::validate_binary_at(&ctx.paths.core_binary).is_ok();
    let service_exists = plan.files.iter().all(|f| f.path.exists());
    let config_exists = ctx.paths.config_file.exists();
    let geo_valid = installer::geo_files_are_valid(&ctx.paths.config_dir, &ctx.paths.core_binary);

    if !force && binary_valid && service_exists && config_exists && geo_valid {
        println!("  All components are up to date — nothing to install.");
        println!("  Use --force to reinstall service artifacts.");
        return Ok(());
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
            std::fs::create_dir_all(&dir.path)
                .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", dir.path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    &dir.path,
                    std::fs::Permissions::from_mode(dir.mode as u32),
                )?;
            }
        }
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
            installer::download_mihomo_to(version, &staged_bin)
                .await
                .map_err(|e| anyhow::anyhow!(install_download_error(&e.to_string())))?;
            let bin_bytes = std::fs::read(&staged_bin)?;
            service::PrivilegeExecutor::write_file(&ctx.paths.core_binary, &bin_bytes, 0o755)?;
        } else {
            installer::download_mihomo_to(version, &ctx.paths.core_binary)
                .await
                .map_err(|e| anyhow::anyhow!(install_download_error(&e.to_string())))?;
        }
        println!("  Installed to {}", ctx.paths.core_binary.display());
    }

    if ctx.permissions == instance::PermissionModel::PrivilegedSystem {
        let current_cli = std::env::current_exe()?;
        let cli_bytes = std::fs::read(&current_cli)?;
        service::PrivilegeExecutor::write_file(&ctx.paths.cli_binary, &cli_bytes, 0o755)?;
        println!(
            "  Installed CLI daemon to {}",
            ctx.paths.cli_binary.display()
        );
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
                let mihomo_path =
                    std::path::PathBuf::from(&mutation.validate_command.program);
                let fixed = config::fix_existing_config_at_endpoint(
                    &paths,
                    Some(&mihomo_path),
                    &mutation.endpoint,
                )?;
                println!(
                    "  ⚠ Present but controller endpoint mismatched; {}",
                    if fixed { "fixed in place" } else { "already aligned" }
                );
                true
            }
        }
    } else {
        setup_instance_config(&ctx).await?
    };

    println!();
    print_lines(format_install_step("[4/4] Geo data files..."));
    if !config_ok {
        println!("  Skipped because config is pending");
    } else if !force && geo_valid {
        println!("  ✅ Already valid, skipped");
    } else {
        let geo_stage = tempfile::tempdir()?;
        let geo_ok = installer::ensure_geo_files_in(geo_stage.path(), github_mirror).await;
        for name in ["geoip.metadb", "GeoSite.dat"] {
            let staged = geo_stage.path().join(name);
            if staged.exists() {
                write_instance_bytes_file(
                    &ctx,
                    &ctx.paths.config_dir.join(name),
                    &std::fs::read(staged)?,
                    0o644,
                )?;
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

    print_lines(format_install_service_prompt(install_mode_label(
        mode == instance::InstanceMode::User,
    )));
    print!("Choice [Y/n]: ");
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if should_install_service_answer(&input) {
        for command in &plan.commands {
            let is_best_effort_unload = is_best_effort_install_cleanup_command(command);
            if is_best_effort_unload {
                run_install_cleanup_command(command);
            } else {
                service::run_instance_command(command)?;
            }
        }
        print_lines(format_install_service_installed());
        if mode == instance::InstanceMode::System {
            wait_for_system_daemon_readiness().await?;
            start_system_core_via_daemon(&ctx).await?;
            wait_for_instance_readiness(&ctx).await?;
        } else {
            wait_for_instance_readiness(&ctx).await?;
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
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode as u32))?;
        }
        Ok(())
    }
}

async fn setup_instance_config(ctx: &instance::InstanceContext) -> anyhow::Result<bool> {
    if ctx.paths.config_file.exists() {
        let content = std::fs::read_to_string(&ctx.paths.config_file)?;
        let fixed = config::ensure_controller_for_endpoint(&content, &ctx.paths.api_endpoint)?;
        if fixed != content {
            write_instance_text_file(ctx, &ctx.paths.config_file, &fixed, 0o644)?;
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
            write_instance_text_file(ctx, &ctx.paths.config_file, &patched, 0o644)?;
            println!("  Copied config from Clash Verge Rev");
            return Ok(true);
        }
    }

    use dialoguer::Input;
    let url: String = Input::new()
        .with_prompt("Subscription URL (Enter to skip)")
        .allow_empty(true)
        .interact_text()?;
    if url.is_empty() {
        println!(
            "  Skipped. Place config manually at {}",
            ctx.paths.config_file.display()
        );
        return Ok(false);
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
    write_instance_text_file(ctx, &ctx.paths.config_file, &patched, 0o644)?;
    println!("  Config saved");

    Ok(true)
}

struct ConfigCmd {
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

fn format_config_change_result(action: &str, target: &str) -> Vec<String> {
    vec![format!("  {action} {target}")]
}

fn format_subscription_switch_success(id: &str) -> Vec<String> {
    let mut lines = format_config_change_result("Switched to subscription", id);
    lines.extend(config_restart_apply_lines());
    lines
}

fn format_refresh_all_start() -> Vec<String> {
    vec!["  Refreshing all subscriptions...".to_string()]
}

fn format_refresh_all_success() -> Vec<String> {
    let mut lines = vec!["  All subscriptions refreshed.".to_string()];
    lines.extend(config_restart_apply_lines());
    lines
}

fn format_refresh_active_start(id: &str) -> Vec<String> {
    vec![format!("  Refreshing active subscription {id}...")]
}

fn format_refresh_active_success() -> Vec<String> {
    let mut lines = vec!["  Subscription refreshed.".to_string()];
    lines.extend(config_restart_apply_lines());
    lines
}

fn no_active_subscription_error() -> &'static str {
    "No active subscription.
  Run: mihomo-cli config --add <URL>"
}

fn format_config_add_start() -> Vec<String> {
    vec!["  Adding subscription...".to_string()]
}

fn format_config_add_success(id: &str) -> Vec<String> {
    let mut lines = format_config_change_result("Added subscription", id);
    lines.extend(config_restart_apply_lines());
    lines
}

fn format_legacy_url_add_success(id: &str, hot_reloaded: bool) -> Vec<String> {
    let mut lines = vec![format!("  Added and activated subscription {id}")];
    if hot_reloaded {
        lines.push("  Config reloaded".to_string());
    } else {
        lines.push("  Run: mihomo-cli restart".to_string());
    }
    lines
}

fn format_import_success(id: &str, activated: bool) -> Vec<String> {
    let mut lines = if activated {
        vec![format!("  Imported and activated subscription {id}")]
    } else {
        vec![format!("  Imported subscription {id} (not activated)")]
    };
    lines.extend(config_restart_apply_lines());
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
        lines.push("  Config hot-reloaded (other changes).".to_string());
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

fn format_tui_add_success(id: &str) -> Vec<String> {
    let mut lines = format_config_change_result("Added subscription", id);
    lines.extend(config_restart_apply_lines());
    lines
}

fn format_tui_switch_result(id: &str, switched: bool) -> Vec<String> {
    if switched {
        format_subscription_switch_success(id)
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
            let short_url = if sub.url.chars().count() > 40 {
                format!("{}…", sub.url.chars().take(37).collect::<String>())
            } else {
                sub.url.clone()
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

/// Resolve --activate / --no-activate / --yes flags into an Option<bool>.
/// Returns Some(true) for force activate, Some(false) for force skip, None for auto.
fn resolve_activate_flag(activate: bool, no_activate: bool, yes: bool) -> Option<bool> {
    if activate {
        Some(true)
    } else if no_activate {
        Some(false)
    } else if yes {
        Some(true)
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
    let client = mihomo_api::EndpointMihomoApiClient::new(mutation.endpoint.clone());
    let hot_reloaded =
        mihomo_api::reload_configs_with_client(&client, &paths.config_path().display().to_string())
            .await
            .is_ok();
    print_lines(format_fix_result(fixed_controller, hot_reloaded));
    Ok(())
}

async fn reload_configs_for_resolved_instance(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
) -> anyhow::Result<()> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    let client = mihomo_api::EndpointMihomoApiClient::new(resolved.ctx.paths.api_endpoint);
    let config_path = paths.config_path().display().to_string();
    mihomo_api::reload_configs_with_client(&client, &config_path).await
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
        activate,
        no_activate,
    } = args;

    // ADR-02: System config is now per-user, same write path as User config.
    // No guard needed — config writes work for both modes.

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
    if validate {
        let paths = read_paths()?;
        let core_binary = resolved_core_binary_for_config_command(
            system,
            user,
            instance::CommandIntent::ReadOnly,
        )?;
        validate_config_at_paths_with_binary(&paths, &core_binary)?;
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
            restore_file_snapshot(&paths.active_file_path(), active_snapshot)?;
            anyhow::bail!(subscription_switch_rollback_error(&err.to_string()));
        }
        print_lines(format_subscription_switch_success(&id));
        return Ok(());
    }

    if let Some(url) = add {
        if dry_run {
            print_lines(format_config_dry_run(ConfigDryRunAction::Add { url: &url }));
            return Ok(());
        }
        print_lines(format_config_add_start());
        let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
        let meta_snapshot = snapshot_file(&paths.subscriptions_meta_path())?;
        let active_snapshot = snapshot_file(&paths.active_file_path())?;
        let activate_flag = resolve_activate_flag(activate, no_activate, yes);
        let id = config::add_subscription_at_with_user_agent(
            &paths,
            &url,
            user_agent.clone(),
            activate_flag,
        )
        .await?;
        // If not auto-activated and no explicit flag, prompt or hint
        if activate_flag.is_none() {
            let active_id = std::fs::read_to_string(paths.active_file_path()).unwrap_or_default();
            if active_id.trim() != id {
                if std::io::stdin().is_terminal() {
                    use dialoguer::Confirm;
                    if Confirm::new()
                        .with_prompt("  Activate this subscription?")
                        .default(true)
                        .interact()?
                    {
                        config::set_active_id_at(&paths, &id)?;
                    }
                } else {
                    println!("  Run `mihomo-cli config --switch {id}` to activate.");
                }
            }
        }
        merge_subscription_change_checked(
            &paths,
            &core_binary,
            &config_endpoint,
            &id,
            meta_snapshot,
            active_snapshot,
        )?;
        print_lines(format_config_add_success(&id));
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
        config::remove_subscription_at(&paths, &id)?;
        for line in format_config_change_result("Removed subscription", &id) {
            println!("{line}");
        }
        return Ok(());
    }

    if refresh_all {
        let subs = config::load_subscriptions_at(&paths)?;
        if subs.is_empty() {
            println!("  No subscriptions to refresh.");
            return Ok(());
        }
        if dry_run {
            print_lines(format_config_dry_run(ConfigDryRunAction::RefreshAll {
                count: subs.len(),
            }));
            return Ok(());
        }
        print_lines(format_refresh_all_start());
        let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
        config::refresh_all_at(&paths).await?;
        config::merge_user_config_checked_at_endpoint(
            &paths,
            Some(&core_binary),
            &config_endpoint,
        )?;
        print_lines(format_refresh_all_success());
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
                let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
                config::refresh_subscription_at_with_user_agent(&paths, &id, user_agent.as_deref())
                    .await?;
                config::merge_user_config_checked_at_endpoint(
                    &paths,
                    Some(&core_binary),
                    &config_endpoint,
                )?;
                print_lines(format_refresh_active_success());
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
        let meta_snapshot = snapshot_file(&paths.subscriptions_meta_path())?;
        let active_snapshot = snapshot_file(&paths.active_file_path())?;

        // Save as subscription file
        let id = config::generate_subscription_id();
        std::fs::create_dir_all(paths.subscriptions_dir())?;
        utils::atomic_write_file(
            &paths.subscription_file_path(&id).display().to_string(),
            &yaml_content,
        )?;

        // Update metadata
        let mut subs = config::load_subscriptions_at(&paths)?;
        subs.push(config::SubscriptionMeta {
            id: id.clone(),
            url: format!("file://{}", file),
            updated: chrono::Utc::now(),
            user_agent: None,
            user_agent_mode: Some(config::UserAgentMode::Auto),
        });
        config::save_subscriptions_at(&paths, &subs)?;

        // Ask if user wants to activate
        let activate_decision = resolve_activate_flag(activate, no_activate, yes);
        let should_activate = match activate_decision {
            Some(v) => v,
            None => {
                // Auto-activate if first subscription, otherwise prompt in TTY
                if subs.len() == 1 {
                    true
                } else if std::io::stdin().is_terminal() {
                    use dialoguer::Confirm;
                    Confirm::new()
                        .with_prompt("  Activate this subscription?")
                        .default(true)
                        .interact()?
                } else {
                    false
                }
            }
        };

        if should_activate {
            config::set_active_id_at(&paths, &id)?;
            merge_subscription_change_checked(
                &paths,
                &core_binary,
                &config_endpoint,
                &id,
                meta_snapshot,
                active_snapshot,
            )?;
            print_lines(format_import_success(&id, true));
        } else {
            print_lines(format_import_success(&id, false));
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
        let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
        let meta_snapshot = snapshot_file(&paths.subscriptions_meta_path())?;
        let active_snapshot = snapshot_file(&paths.active_file_path())?;
        let activate_flag = resolve_activate_flag(activate, no_activate, yes);
        let id =
            config::add_subscription_at_with_user_agent(&paths, &u, None, activate_flag).await?;
        // If not auto-activated and no explicit flag, prompt or hint
        if activate_flag.is_none() {
            let active_id = std::fs::read_to_string(paths.active_file_path()).unwrap_or_default();
            if active_id.trim() != id {
                if std::io::stdin().is_terminal() {
                    use dialoguer::Confirm;
                    if Confirm::new()
                        .with_prompt("  Activate this subscription?")
                        .default(true)
                        .interact()?
                    {
                        config::set_active_id_at(&paths, &id)?;
                    }
                } else {
                    println!("  Run `mihomo-cli config --switch {id}` to activate.");
                }
            }
        }
        merge_subscription_change_checked(
            &paths,
            &core_binary,
            &config_endpoint,
            &id,
            meta_snapshot,
            active_snapshot,
        )?;
        let hot_reloaded = reload_configs_for_resolved_instance(system, user, &paths)
            .await
            .is_ok();
        print_lines(format_legacy_url_add_success(&id, hot_reloaded));
        return Ok(());
    }

    // No arguments: show TUI for the resolved instance config.
    show_subscription_menu(&paths, &core_binary, &config_endpoint).await
}

fn validate_config_at_paths_with_binary(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
) -> anyhow::Result<()> {
    let report = config::validate_config_at(paths, Some(mihomo))?;
    print_lines(format_config_validation_result(
        &paths.config_path(),
        mihomo,
        &report,
    ));
    Ok(())
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
                    print_lines(format_tui_add_success(&id));
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
            let line = format!("{prefix} {}{marker}  {}", sub.id, sub.url);
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
                    config::switch_subscription_at(paths, selected_id)?;
                    print_lines(format_tui_switch_result(selected_id, true));
                } else {
                    print_lines(format_tui_switch_result(selected_id, false));
                }
                return Ok(());
            }
            KeyCode::Char('R') => {
                terminal::disable_raw_mode()?;
                print_lines(format_refresh_all_start());
                config::refresh_all_at(paths).await?;
                config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint)?;
                print_lines(format_refresh_all_success());
                terminal::enable_raw_mode()?;
            }
            KeyCode::Char('r') => {
                terminal::disable_raw_mode()?;
                match &active {
                    Some(id) => {
                        print_lines(format_tui_refresh_active_start(id));
                        config::refresh_subscription_at(paths, id).await?;
                        config::merge_user_config_checked_at_endpoint(
                            paths,
                            Some(mihomo),
                            endpoint,
                        )?;
                        print_lines(format_refresh_active_success());
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
                    print_lines(format_tui_add_success(&id));
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
            if let Some(message) = system_proxy_tun_active_message().await {
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

async fn system_proxy_tun_active_message() -> Option<String> {
    if !ipc::is_daemon_running().await {
        return None;
    }
    match ipc::send_command(&ipc::DaemonCommand::GetStatus).await {
        Ok(ipc::DaemonResponse::Status {
            tun_enabled: true, ..
        }) => Some(system_proxy_tun_active_message_text().to_string()),
        _ => None,
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
                std::fs::create_dir_all(parent)?;
            }
            utils::atomic_write_file(&path.display().to_string(), &content)?;
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

fn rollback_subscription_change(
    paths: &utils::AppPaths,
    new_subscription_id: &str,
    meta_snapshot: Option<String>,
    active_snapshot: Option<String>,
) -> anyhow::Result<()> {
    restore_file_snapshot(&paths.subscriptions_meta_path(), meta_snapshot)?;
    restore_file_snapshot(&paths.active_file_path(), active_snapshot)?;
    let _ = std::fs::remove_file(paths.subscription_file_path(new_subscription_id));
    Ok(())
}

fn merge_subscription_change_checked(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
    endpoint: &instance::ApiEndpoint,
    new_subscription_id: &str,
    meta_snapshot: Option<String>,
    active_snapshot: Option<String>,
) -> anyhow::Result<()> {
    if let Err(err) = config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint) {
        rollback_subscription_change(paths, new_subscription_id, meta_snapshot, active_snapshot)?;
        anyhow::bail!(subscription_change_rollback_error(&err.to_string()));
    }
    Ok(())
}

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

    let char_count = url.chars().count();
    if char_count <= LIMIT {
        return url.to_string();
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
        format!(
            "  {} {} ({})",
            marker,
            sub.id,
            shorten_subscription_url(&sub.url)
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

fn format_rule_list(rules: &[String], pos: crate::rules::RulePosition) -> Vec<String> {
    let mut lines = vec![format!("  Insert position: {pos}"), String::new()];
    if rules.is_empty() {
        lines.extend([
            "  (no user rules)".to_string(),
            String::new(),
            "  Add a rule:  mihomo-cli rule add DOMAIN-SUFFIX,example.com,DIRECT".to_string(),
        ]);
    } else {
        lines.extend(
            rules
                .iter()
                .enumerate()
                .map(|(i, rule)| format!("  {}. {}", i + 1, rule)),
        );
    }
    lines
}

fn format_rule_apply_result(merged: bool, hot_reloaded: bool, new_rule: bool) -> Vec<String> {
    if merged && hot_reloaded {
        if new_rule {
            vec!["  ✓ Config reloaded — rule is now active".to_string()]
        } else {
            vec!["  ✓ Config reloaded".to_string()]
        }
    } else if merged {
        if new_rule {
            vec!["  ℹ Run: mihomo-cli restart  (to apply the new rule)".to_string()]
        } else {
            vec!["  ℹ Run: mihomo-cli restart  (to apply)".to_string()]
        }
    } else {
        vec!["  ℹ Config pending — rule saved, run `mihomo-cli config` first".to_string()]
    }
}

fn format_rule_add_success(rule: &str, merged: bool, hot_reloaded: bool) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Rule added: {rule}")];
    lines.extend(format_rule_apply_result(merged, hot_reloaded, true));
    lines
}

fn format_rule_remove_success(index: usize, merged: bool, hot_reloaded: bool) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Rule {index} removed")];
    lines.extend(format_rule_apply_result(merged, hot_reloaded, false));
    lines
}

fn format_rule_clear_success(merged: bool, hot_reloaded: bool) -> Vec<String> {
    let mut lines = vec!["  ✓ All rules cleared".to_string()];
    lines.extend(format_rule_apply_result(merged, hot_reloaded, false));
    lines
}

fn format_rule_move_success(
    from: usize,
    to: usize,
    merged: bool,
    hot_reloaded: bool,
) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Rule moved: {from} → {to}")];
    lines.extend(format_rule_apply_result(merged, hot_reloaded, false));
    lines
}

fn format_rule_import_success(
    count: usize,
    path: &str,
    merged: bool,
    hot_reloaded: bool,
) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Imported {count} rules from {path}")];
    lines.extend(format_rule_apply_result(merged, hot_reloaded, false));
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
    match matched {
        Some(matched) => vec![
            format!("  ✓ Matched rule #{}: {}", matched.index, matched.rule),
            format!("  Policy: {}", matched.policy),
        ],
        None => vec![format!("  No matching rule found for {target}")],
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
            let pos = match position {
                Some(p) => Some(p.parse::<RulePosition>()?),
                None => None,
            };
            let paths = write_paths()?;
            let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
            let rules_snapshot = snapshot_file(&paths.rules_path())?;
            crate::rules::add_rule_at(&paths, &rule, pos)?;
            let merged =
                merge_rules_change_checked(&paths, &core_binary, &config_endpoint, rules_snapshot)?;
            let hot_reloaded = merged
                && reload_configs_for_resolved_instance(system, user, &paths)
                    .await
                    .is_ok();
            print_lines(format_rule_add_success(&rule, merged, hot_reloaded));
            Ok(())
        }
        RuleAction::List => {
            let paths = read_paths()?;
            let rules = crate::rules::list_rules_at(&paths)?;
            let pos = crate::rules::get_position_at(&paths).unwrap_or_default();
            for line in format_rule_list(&rules, pos) {
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
            let hot_reloaded = merged
                && reload_configs_for_resolved_instance(system, user, &paths)
                    .await
                    .is_ok();
            print_lines(format_rule_remove_success(index, merged, hot_reloaded));
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
            let hot_reloaded = merged
                && reload_configs_for_resolved_instance(system, user, &paths)
                    .await
                    .is_ok();
            print_lines(format_rule_clear_success(merged, hot_reloaded));
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
            let hot_reloaded = merged
                && reload_configs_for_resolved_instance(system, user, &paths)
                    .await
                    .is_ok();
            print_lines(format_rule_move_success(from, to, merged, hot_reloaded));
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
            let hot_reloaded = merged
                && reload_configs_for_resolved_instance(system, user, &paths)
                    .await
                    .is_ok();
            print_lines(format_rule_import_success(
                count,
                &path,
                merged,
                hot_reloaded,
            ));
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
            let policies = crate::rules::available_policies()?;
            print_lines(format_rule_policies(&policies));
            Ok(())
        }
        RuleAction::Test { target } => {
            let paths = read_paths()?;
            let matched = crate::rules::test_rule_match_at(&paths, &target)?;
            print_lines(format_rule_test_result(&target, matched.as_ref()));
            Ok(())
        }
    }
}

fn merge_dns_change_checked(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
    endpoint: &instance::ApiEndpoint,
    dns_snapshot: Option<String>,
) -> anyhow::Result<()> {
    if let Err(err) = config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint) {
        restore_file_snapshot(&paths.dns_policy_path(), dns_snapshot)?;
        anyhow::bail!(
            "DNS policy change failed; rolled back dns-policy.yaml.\n  {}",
            err
        );
    }
    Ok(())
}

fn format_dns_config_updated() -> Vec<String> {
    vec!["  ✓ Config updated — restart mihomo to apply DNS changes".to_string()]
}

fn format_dns_policy_added(match_pattern: &str, target: &str) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Policy added: {match_pattern} → {target}")];
    lines.extend(format_dns_config_updated());
    lines
}

fn format_dns_policy_removed(removed: &impl std::fmt::Display) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Policy removed: {removed}")];
    lines.extend(format_dns_config_updated());
    lines
}

fn format_dns_policy_list<T: std::fmt::Display>(policies: &[(usize, T)]) -> Vec<String> {
    if policies.is_empty() {
        vec![
            "  No DNS policies defined.".to_string(),
            String::new(),
            "  Add one:  mihomo-cli dns policy add <MATCH> <TARGET>".to_string(),
            "  Example:  mihomo-cli dns policy add ubtrobot.com system".to_string(),
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
        "    mihomo-cli dns template apply company --domain corp.example.com --target 10.10.1.251"
            .to_string(),
    ]);
    lines
}

fn format_dns_template_applied(name: &str, added: &[impl std::fmt::Display]) -> Vec<String> {
    let mut lines = vec![format!("  ✓ Applied DNS template: {name}")];
    lines.extend(added.iter().map(|policy| format!("  - {policy}")));
    lines.extend(format_dns_config_updated());
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
                merge_dns_change_checked(&paths, &core_binary, &config_endpoint, dns_snapshot)?;

                // Also try PATCH for hot-reload (may need restart)
                let patch = serde_json::json!({
                    "dns": {"nameserver-policy": {match_pattern.clone(): target.clone()}}
                });
                use crate::mihomo_api::MihomoApiClient;
                let client = resolve_api_client(system, user, instance::CommandIntent::Mutating)?;
                let _ = client.patch("/configs", patch).await;
                print_lines(format_dns_policy_added(&match_pattern, &target));
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
                merge_dns_change_checked(&paths, &core_binary, &config_endpoint, dns_snapshot)?;
                print_lines(format_dns_policy_removed(&removed));
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
                    merge_dns_change_checked(&paths, &core_binary, &config_endpoint, dns_snapshot)?;
                    print_lines(format_dns_template_applied(&name, &added));
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

async fn cmd_version(system: bool, user: bool) -> anyhow::Result<()> {
    let client = resolve_api_client(system, user, instance::CommandIntent::ReadOnly);
    let (core_version, core_error) = match client {
        Ok(client) => match mihomo_api::get_version_with_client(&client).await {
            Ok(version) => (Some(version), None),
            Err(err) => (None, Some(err.to_string())),
        },
        Err(err) => (None, Some(err.to_string())),
    };
    let lines = build_info_lines(core_version.as_deref(), core_error.as_deref());
    print_lines(lines);
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

fn cmd_uninstall_all_instance_modes(modes: &[instance::InstanceMode]) -> anyhow::Result<()> {
    use dialoguer::Confirm;

    let mut plans = Vec::new();
    for mode in modes {
        let Some(ctx) = instance::planned_current_context(*mode) else {
            continue;
        };
        let plan = instance::planned_service_plan(&ctx, instance::ServiceAction::Uninstall);
        plans.push((ctx, plan));
    }

    if plans.is_empty() {
        print_lines(format_uninstall_nothing());
        return Ok(());
    }

    println!("=== mihomo-cli uninstall --all ===");
    println!();
    println!("This will remove all v3 instance artifacts:");
    for (ctx, _) in &plans {
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

    if !Confirm::new()
        .with_prompt(uninstall_prompt(true))
        .default(false)
        .interact()?
    {
        print_lines(format_uninstall_cancelled());
        return Ok(());
    }

    for (ctx, plan) in plans {
        println!("Instance: {}", instance_mode_label(ctx.mode));
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
            if let Err(err) = service::run_instance_command(command) {
                eprintln!("  Warning: service command failed: {err}");
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

fn cmd_uninstall_instance_mode(
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
        if geo_exists
            && selections.contains(&idx) {
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

    // Execute removal
    println!();
    println!("=== mihomo-cli uninstall ===");
    println!("Instance: {}", instance_mode_label(mode));
    println!();

    // 1. Stop service
    println!("Stopping service...");
    for command in &plan.commands {
        if command.privileged {
            if let Some(invocation) = instance::privilege_invocation_plan(command.clone()) {
                println!(
                    "  Privilege required. Fallback: {}",
                    invocation.manual_fallback
                );
            }
        }
        if let Err(err) = service::run_instance_command(command) {
            eprintln!("  Warning: service command failed: {err}");
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
        remove_instance_path(&ctx.paths.core_binary, ctx.permissions == instance::PermissionModel::PrivilegedSystem)?;
        if ctx.paths.cli_binary != ctx.paths.core_binary {
            remove_instance_path(&ctx.paths.cli_binary, ctx.permissions == instance::PermissionModel::PrivilegedSystem)?;
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
                std::fs::remove_file(&path)?;
                println!("  Removed {}", path.display());
            }
        }
    }

    // 6. Remove runtime dir if binary or config was removed
    if let Some(runtime_dir) = &ctx.paths.runtime_dir {
        if (selected_binary || selected_config) && runtime_dir.exists() {
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
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
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
    std::fs::rename(bin, &bak)?;
    log!("Backed up {} -> {}", bin.display(), bak.display());
    match installer::download_mihomo_to(version, bin).await {
        Ok(()) => {
            std::fs::remove_file(&bak)?;
            log!("Removed backup");
            print_lines(format_update_success());
        }
        Err(e) => {
            std::fs::rename(&bak, bin)?;
            log!("Restored backup");
            anyhow::bail!(update_failed_error(&e.to_string()));
        }
    }
    Ok(())
}

async fn cmd_upgrade(system: bool, user: bool) -> anyhow::Result<()> {
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

    let api_client =
        mihomo_api::EndpointMihomoApiClient::new(resolved.ctx.paths.api_endpoint.clone());

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
    print!("  Upgrade? [y/N] ");
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if !input.trim().eq_ignore_ascii_case("y") {
        println!("  Cancelled.");
        return Ok(());
    }

    let _lock = crate::lock::ConfigLock::acquire(&resolved.ctx.paths.config_dir)?;
    update_instance_core_binary(&resolved.ctx, Some(latest_tag))
        .await
        .map_err(|e| anyhow::anyhow!("Upgrade failed: {e}"))?;
    println!("  Upgraded to {latest_tag}.");

    match cmd_lifecycle_instance_mode(resolved.ctx.mode, instance::ServiceAction::Restart).await {
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
            apply_override_change(system, user, &paths, Some(content), "updated").await
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

fn override_action_intent(action: &OverrideAction) -> instance::CommandIntent {
    match action {
        OverrideAction::Path | OverrideAction::Show => instance::CommandIntent::ReadOnly,
        OverrideAction::Import { .. } | OverrideAction::Clear { .. } => {
            instance::CommandIntent::Mutating
        }
    }
}

async fn apply_override_change(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
    content: Option<String>,
    action_label: &str,
) -> anyhow::Result<()> {
    let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
    let snapshot = snapshot_file(&paths.override_path())?;
    if let Some(content) = content {
        if let Some(parent) = paths.override_path().parent() {
            std::fs::create_dir_all(parent)?;
        }
        utils::atomic_write_file(&paths.override_path().display().to_string(), &content)?;
    } else {
        let _ = std::fs::remove_file(paths.override_path());
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
    let hot_reloaded = reload_configs_for_resolved_instance(system, user, paths)
        .await
        .is_ok();
    println!("  ✓ override.yaml {action_label}");
    if hot_reloaded {
        println!("  ✓ Runtime config reloaded");
    } else {
        println!("  ⚠ Runtime reload skipped or failed; restart mihomo to apply");
    }
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
        utils::atomic_write_file(&config_path.display().to_string(), &fixed)?;
    }
    Ok(())
}

#[cfg(test)]
mod cli_parse_tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        let mut argv = vec!["mihomo-cli"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv).expect("CLI arguments should parse")
    }

    #[test]
    fn read_mixed_port_rejects_out_of_range_values() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.yaml");
        std::fs::write(&config, "mixed-port: 70000\n").unwrap();
        assert_eq!(read_mixed_port_from_config(&config), None);

        std::fs::write(&config, "mixed-port: 7897\n").unwrap();
        assert_eq!(read_mixed_port_from_config(&config), Some(7897));
    }

    #[test]
    fn restored_config_endpoint_is_repaired_for_resolved_instance() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config.yaml");
        std::fs::write(
            &config,
            "mixed-port: 7897\nexternal-controller-unix: /tmp/old-mihomo.sock\n",
        )
        .unwrap();

        ensure_config_file_endpoint(
            &config,
            &instance::ApiEndpoint::UnixSocket(std::path::PathBuf::from(
                "/var/run/mihomo/mihomo.sock",
            )),
        )
        .unwrap();

        let fixed = std::fs::read_to_string(&config).unwrap();
        assert!(fixed.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!fixed.contains("/tmp/old-mihomo.sock"));
    }

    #[test]
    fn public_help_exposes_system_override_on_user_facing_commands() {
        let mut root = Cli::command();
        let subcommands: Vec<String> = root
            .get_subcommands()
            .filter(|cmd| !cmd.is_hide_set())
            .map(|cmd| cmd.get_name().to_string())
            .filter(|name| name != "help" && name != "dashboard")
            .collect();

        for name in subcommands {
            let help = root
                .find_subcommand_mut(&name)
                .expect("subcommand from iterator should exist")
                .render_help()
                .to_string();
            assert!(
                help.contains("--system"),
                "public command `{name}` should expose the v3 explicit system override:
{help}"
            );
        }
    }

    #[test]
    fn public_help_exposes_user_flag_only_for_install_and_uninstall() {
        let mut root = Cli::command();
        let subcommands: Vec<String> = root
            .get_subcommands()
            .map(|cmd| cmd.get_name().to_string())
            .filter(|name| name != "dashboard")
            .collect();

        for name in subcommands {
            let help = root
                .find_subcommand_mut(&name)
                .expect("subcommand from iterator should exist")
                .render_help()
                .to_string();
            let may_expose_user = matches!(name.as_str(), "install" | "uninstall");
            let exposes_user_flag = help.contains("-u, --user ") || help.contains("    --user ");
            assert_eq!(
                exposes_user_flag, may_expose_user,
                "unexpected --user flag visibility in `{name}` help:
{help}"
            );
        }
    }

    #[test]
    fn legacy_root_leftovers_uninstall_flag_parses() {
        let cli = parse(&["uninstall", "--legacy-system-leftovers", "--dry-run"]);
        match cli.command {
            Some(Command::Uninstall {
                legacy_root_leftovers,
                dry_run,
                ..
            }) => {
                assert!(legacy_root_leftovers);
                assert!(dry_run);
            }
            _ => panic!("expected uninstall --legacy-system-leftovers"),
        }
    }

    #[test]
    fn legacy_root_leftover_messages_preserve_user_payload_boundary() {
        let leftovers = vec![LegacyRootLeftover {
            path: std::path::PathBuf::from("/Users/alice/.config/mihomo/run"),
            reason: "legacy root runtime socket directory",
        }];
        let lines = format_legacy_root_leftovers(&leftovers);
        assert_eq!(lines[0], "Legacy root-mode leftovers detected:");
        assert!(lines[1].contains("/Users/alice/.config/mihomo/run"));

        let err = legacy_root_leftovers_user_uninstall_error(&leftovers);
        assert!(err.contains("mihomo-cli uninstall --legacy-system-leftovers"));
        assert!(err.contains("mihomo-cli uninstall --user --all"));
    }
    static ENV_OVERRIDE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_config_dir_override<T>(dir: &std::path::Path, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_OVERRIDE_TEST_LOCK.lock().unwrap();
        let old = std::env::var("MIHOMO_CLI_CONFIG_DIR").ok();
        std::env::set_var("MIHOMO_CLI_CONFIG_DIR", dir);
        let result = f();
        match old {
            Some(value) => std::env::set_var("MIHOMO_CLI_CONFIG_DIR", value),
            None => std::env::remove_var("MIHOMO_CLI_CONFIG_DIR"),
        }
        result
    }

    #[cfg(unix)]
    #[test]
    fn legacy_service_definition_detection_catches_user_home_paths() {
        let legacy = r#"<plist><dict>
<key>ProgramArguments</key><array><string>/Users/kuku/.config/mihomo/start.sh</string></array>
<key>WorkingDirectory</key><string>/Users/kuku/.config/mihomo</string>
</dict></plist>"#;
        let refs = service_definition_user_home_references(
            legacy,
            "/Users/kuku/.config/mihomo",
            "/Users/kuku",
        );
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().any(|p| p.ends_with("start.sh")));

        let v2 = r#"ExecStart=/usr/local/lib/mihomo/mihomo -d /etc/mihomo
RuntimeDirectory=mihomo"#;
        assert!(service_definition_user_home_references(
            v2,
            "/home/kuku/.config/mihomo",
            "/home/kuku",
        )
        .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn legacy_root_detection_ignores_v3_per_user_working_directory() {
        let temp = tempfile::tempdir().unwrap();
        let plist = temp.path().join("io.mihomo.plist");
        std::fs::write(
            &plist,
            r#"<plist><dict>
<key>ProgramArguments</key><array><string>/Library/Application Support/mihomo/start.sh</string></array>
<key>WorkingDirectory</key><string>/Users/kuku/.config/mihomo</string>
</dict></plist>"#,
        )
        .unwrap();

        assert!(legacy_root_service_from_file(&plist).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn legacy_root_detection_catches_user_home_executable_references() {
        assert!(is_legacy_user_home_service_executable_ref(
            std::path::Path::new("/Users/kuku/.config/mihomo/start.sh")
        ));
        assert!(is_legacy_user_home_service_executable_ref(
            std::path::Path::new("/home/kuku/.local/bin/mihomo")
        ));
        assert!(!is_legacy_user_home_service_executable_ref(
            std::path::Path::new("/Users/kuku/.config/mihomo")
        ));
    }

    #[test]
    fn legacy_root_service_diagnostic_shows_structured_evidence() {
        let legacy = instance::LegacyRootService {
            service_file: std::path::PathBuf::from("/Library/LaunchDaemons/io.mihomo.plist"),
            referenced_paths: vec![std::path::PathBuf::from(
                "/Users/kuku/.config/mihomo/start.sh",
            )],
            referenced_home: Some(std::path::PathBuf::from("/Users/kuku")),
            referenced_current_user_home: true,
        };
        let lines = format_legacy_root_service_diagnostic(&legacy);
        let text = lines.join("\n");
        assert!(text.contains("Legacy Root Layout Detected"));
        assert!(text.contains("/Library/LaunchDaemons/io.mihomo.plist"));
        assert!(text.contains("/Users/kuku/.config/mihomo/start.sh"));
        assert!(text.contains("mihomo-cli uninstall --all"));
    }

    #[test]
    fn resolution_source_labels_are_user_facing() {
        assert_eq!(
            resolution_source_label(instance::ResolutionSource::ExplicitFlag),
            "explicit mode flag"
        );
        assert_eq!(
            resolution_source_label(instance::ResolutionSource::ServicePresence),
            "installed service detection"
        );
        assert_eq!(
            resolution_source_label(instance::ResolutionSource::EnvOverride),
            "MIHOMO_CLI_CONFIG_DIR"
        );
    }

    #[test]
    fn env_override_status_resolution_reports_override_source_and_paths() {
        let isolated =
            std::env::temp_dir().join(format!("mihomo-cli-status-test-{}", std::process::id()));
        with_config_dir_override(&isolated, || {
            let resolved =
                resolve_current_instance_context(false, false, instance::CommandIntent::ReadOnly)
                    .unwrap();
            assert_eq!(resolved.source, instance::ResolutionSource::EnvOverride);
            assert_eq!(resolved.ctx.mode, instance::InstanceMode::User);
            assert_eq!(resolved.ctx.paths.config_dir, isolated);
            assert_eq!(resolved.ctx.paths.config_file, isolated.join("config.yaml"));
        });
    }

    #[test]
    fn instance_mode_flag_suffix_uses_v3_system_name() {
        assert_eq!(
            instance_mode_marker(instance::InstanceMode::System),
            "system"
        );
        assert_eq!(instance_mode_marker(instance::InstanceMode::User), "user");
        assert_eq!(
            instance_mode_label(instance::InstanceMode::System),
            "system service"
        );
        assert_eq!(
            instance_mode_label(instance::InstanceMode::User),
            "per-user"
        );
        assert_eq!(
            config_fix_command_for_mode(instance::InstanceMode::System),
            "mihomo-cli config --system --fix"
        );
        assert_eq!(
            config_fix_command_for_mode(instance::InstanceMode::User),
            "mihomo-cli config --fix"
        );
    }

    #[test]
    fn config_dir_override_wins_for_unspecified_and_user_read_paths() {
        let isolated = std::env::temp_dir().join(format!("mihomo-cli-test-{}", std::process::id()));
        with_config_dir_override(&isolated, || {
            let unspecified = app_paths_for_resolved_instance_command(
                "config",
                false,
                false,
                instance::CommandIntent::ReadOnly,
            )
            .unwrap();
            assert_eq!(unspecified.config_dir(), isolated.as_path());

            let explicit_user = app_paths_for_resolved_instance_command(
                "config",
                false,
                true,
                instance::CommandIntent::ReadOnly,
            )
            .unwrap();
            assert_eq!(explicit_user.config_dir(), isolated.as_path());
        });
    }

    #[test]
    fn config_dir_override_does_not_redirect_explicit_system() {
        let isolated =
            std::env::temp_dir().join(format!("mihomo-cli-test-system-{}", std::process::id()));
        with_config_dir_override(&isolated, || {
            let explicit_system = app_paths_for_resolved_instance_command(
                "config",
                true,
                false,
                instance::CommandIntent::ReadOnly,
            );
            match explicit_system {
                Ok(paths) => assert_ne!(paths.config_dir(), isolated.as_path()),
                Err(err) => assert!(
                    err.to_string().contains("per-user core")
                        || err.to_string().contains("both system daemon"),
                    "explicit system should either resolve outside MIHOMO_CLI_CONFIG_DIR or fail on active runtime conflict: {err}"
                ),
            }
        });
    }

    #[test]
    fn runtime_first_resolution_prefers_active_runtime_over_ambiguous_installs() {
        let both_installed = instance::ServicePresence {
            system: true,
            user: true,
        };
        let system_runtime = instance::ServicePresence {
            system: true,
            user: false,
        };
        let user_runtime = instance::ServicePresence {
            system: false,
            user: true,
        };

        assert_eq!(
            resolve_instance_mode_runtime_first(
                instance::ModeRequest::Unspecified,
                system_runtime,
                both_installed,
                instance::CommandIntent::Mutating,
            ),
            RuntimeFirstModeResolution::Resolved {
                mode: instance::InstanceMode::System,
                source: instance::ResolutionSource::RuntimePresence,
            }
        );
        assert_eq!(
            resolve_instance_mode_runtime_first(
                instance::ModeRequest::Unspecified,
                user_runtime,
                both_installed,
                instance::CommandIntent::Mutating,
            ),
            RuntimeFirstModeResolution::Resolved {
                mode: instance::InstanceMode::User,
                source: instance::ResolutionSource::RuntimePresence,
            }
        );
    }

    #[test]
    fn runtime_first_resolution_fails_fast_on_active_runtime_conflict() {
        let no_services = instance::ServicePresence {
            system: false,
            user: false,
        };
        let both_runtime = instance::ServicePresence {
            system: true,
            user: true,
        };

        assert_eq!(
            resolve_instance_mode_runtime_first(
                instance::ModeRequest::Unspecified,
                both_runtime,
                no_services,
                instance::CommandIntent::ReadOnly,
            ),
            RuntimeFirstModeResolution::RuntimeConflict
        );
    }

    #[test]
    fn environment_resolution_models_tun_install_and_daemon_recovery() {
        let none = instance::ServicePresence {
            system: false,
            user: false,
        };
        let system_installed = instance::ServicePresence {
            system: true,
            user: false,
        };
        let user_installed = instance::ServicePresence {
            system: false,
            user: true,
        };

        assert_eq!(
            resolve_environment_for_intent(
                instance::ModeRequest::Unspecified,
                &EnvironmentState {
                    runtime: none,
                    installed: none,
                    legacy_root: None,
                },
                UserIntent::TunOn,
            ),
            RuntimeFirstModeResolution::NeedsSystemInstall {
                reason: "TUN requires the privileged system service".to_string(),
            }
        );
        assert_eq!(
            resolve_environment_for_intent(
                instance::ModeRequest::Unspecified,
                &EnvironmentState {
                    runtime: none,
                    installed: user_installed,
                    legacy_root: None,
                },
                UserIntent::TunOn,
            ),
            RuntimeFirstModeResolution::NeedsSystemSwitch {
                user_running: false,
                user_installed: true,
            }
        );
        assert_eq!(
            resolve_environment_for_intent(
                instance::ModeRequest::Unspecified,
                &EnvironmentState {
                    runtime: none,
                    installed: system_installed,
                    legacy_root: None,
                },
                UserIntent::TunOn,
            ),
            RuntimeFirstModeResolution::NeedsSystemDaemonRecovery {
                reason: "system service is installed but daemon IPC is unavailable".to_string(),
            }
        );
    }

    #[test]
    fn runtime_first_resolution_falls_back_to_service_artifacts_when_idle() {
        let no_runtime = instance::ServicePresence {
            system: false,
            user: false,
        };
        let system_installed = instance::ServicePresence {
            system: true,
            user: false,
        };

        assert_eq!(
            resolve_instance_mode_runtime_first(
                instance::ModeRequest::Unspecified,
                no_runtime,
                system_installed,
                instance::CommandIntent::ReadOnly,
            ),
            RuntimeFirstModeResolution::Resolved {
                mode: instance::InstanceMode::System,
                source: instance::ResolutionSource::ServicePresence,
            }
        );
    }

    #[test]
    fn default_command_is_deferred_to_install() {
        let cli = parse(&[]);
        assert!(cli.command.is_none());
        assert!(!cli.verbose);
    }

    #[test]
    fn global_verbose_parses_before_subcommand() {
        let cli = parse(&["--verbose", "config", "--list"]);
        assert!(cli.verbose);
        match cli.command {
            Some(Command::Config { list, .. }) => assert!(list),
            _ => panic!("expected config --list"),
        }
    }

    #[test]
    fn version_command_formats_build_metadata() {
        let lines = build_info_lines(Some("v1.2.3"), None);
        let text = lines.join(
            "
",
        );
        assert!(text.contains("mihomo-cli"));
        assert!(text.contains("Version:"));
        assert!(text.contains("Git commit:"));
        assert!(text.contains("mihomo core"));
        assert!(text.contains("v1.2.3"));

        let lines = build_info_lines(None, Some("not running"));
        let text = lines.join(
            "
",
        );
        assert!(text.contains("unavailable"));
        assert!(text.contains("not running"));
    }

    #[test]
    fn version_command_supports_system_override_without_user_flag() {
        match parse(&["version", "--system"]).command {
            Some(Command::Version { system }) => assert!(system),
            _ => panic!("expected version --system"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "version", "--user"]).is_err());
    }

    #[test]
    fn update_and_upgrade_support_system_override_without_user_flag() {
        match parse(&["update", "--system"]).command {
            Some(Command::Update { system }) => assert!(system),
            _ => panic!("expected update --system"),
        }
        match parse(&["upgrade", "--system"]).command {
            Some(Command::Upgrade { system }) => assert!(system),
            _ => panic!("expected upgrade --system"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "update", "--user"]).is_err());
        assert!(Cli::try_parse_from(["mihomo-cli", "upgrade", "--user"]).is_err());
    }

    #[test]
    fn config_ua_options_parse_as_public_contract() {
        let cli = parse(&[
            "config",
            "--add",
            "https://example.test/sub",
            "--user-agent",
            "clash-verge/v2.0.4",
        ]);
        match cli.command {
            Some(Command::Config {
                add, user_agent, ..
            }) => {
                assert_eq!(add.as_deref(), Some("https://example.test/sub"));
                assert_eq!(user_agent.as_deref(), Some("clash-verge/v2.0.4"));
            }
            _ => panic!("expected config --add with user-agent"),
        }

        let cli = parse(&["config", "--set-ua", "sub-a", "auto"]);
        match cli.command {
            Some(Command::Config { set_ua, .. }) => {
                assert_eq!(set_ua, vec!["sub-a".to_string(), "auto".to_string()]);
            }
            _ => panic!("expected config --set-ua"),
        }

        let cli = parse(&["config", "--system", "--validate"]);
        match cli.command {
            Some(Command::Config {
                system, validate, ..
            }) => {
                assert!(system);
                assert!(validate);
            }
            _ => panic!("expected config --system --validate"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "config", "--user"]).is_err());
    }

    #[test]
    fn config_set_ua_requires_exactly_two_values() {
        let result = Cli::try_parse_from(["mihomo-cli", "config", "--set-ua", "sub-a"]);
        let err = match result {
            Ok(_) => panic!("--set-ua must reject missing UA value"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::WrongNumberOfValues);
    }

    #[test]
    fn rule_and_dns_subcommands_parse() {
        match parse(&["rule", "list"]).command {
            Some(Command::Rule { system, action }) => {
                assert!(!system);
                assert!(matches!(action, RuleAction::List));
            }
            _ => panic!("expected rule list"),
        }
        match parse(&["rule", "--system", "list"]).command {
            Some(Command::Rule { system, action }) => {
                assert!(system);
                assert!(matches!(action, RuleAction::List));
            }
            _ => panic!("expected rule --system list"),
        }

        match parse(&["dns", "policy", "list"]).command {
            Some(Command::Dns { system, action }) => {
                assert!(!system);
                assert!(matches!(
                    action,
                    DnsAction::Policy {
                        action: DnsPolicyAction::List
                    }
                ));
            }
            _ => panic!("expected dns policy list"),
        }
    }

    #[test]
    fn service_active_probe_plans_match_supported_service_managers() {
        let inputs = instance::PathInputs::for_tests();

        let linux_root = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        let plan = service_active_probe_plan(&linux_root).unwrap();
        assert_eq!(plan.program, "systemctl");
        assert_eq!(plan.args, vec!["is-active", "--quiet", "mihomo"]);
        assert_eq!(plan.output_contains, None);

        let linux_user = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::User,
            &inputs,
        );
        let plan = service_active_probe_plan(&linux_user).unwrap();
        assert_eq!(plan.args, vec!["--user", "is-active", "--quiet", "mihomo"]);

        let mac_user = instance::InstanceContext::planned(
            instance::TargetOs::Macos,
            instance::InstanceMode::User,
            &inputs,
        );
        let plan = service_active_probe_plan(&mac_user).unwrap();
        assert_eq!(plan.program, "launchctl");
        assert_eq!(plan.args, vec!["print", "gui/501/io.mihomo"]);

        let win_root = instance::InstanceContext::planned(
            instance::TargetOs::Windows,
            instance::InstanceMode::System,
            &inputs,
        );
        let plan = service_active_probe_plan(&win_root).unwrap();
        assert_eq!(plan.program, "sc.exe");
        assert_eq!(plan.args, vec!["query", "mihomo"]);
        assert_eq!(plan.output_contains.as_deref(), Some("RUNNING"));
    }

    #[test]
    fn service_active_probe_runner_captures_output_instead_of_inheriting_terminal() {
        let plan = ServiceActiveProbePlan {
            program: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf stdout-noise; printf stderr-noise >&2".to_string(),
            ],
            output_contains: None,
        };
        let out = run_service_active_probe(&plan).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "stdout-noise");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "stderr-noise");
    }

    #[test]
    fn service_active_probe_success_handles_status_and_windows_output() {
        let quiet = ServiceActiveProbePlan {
            program: "systemctl".to_string(),
            args: vec![],
            output_contains: None,
        };
        assert!(service_active_probe_success(&quiet, true, ""));
        assert!(!service_active_probe_success(&quiet, false, ""));

        let windows = ServiceActiveProbePlan {
            program: "sc.exe".to_string(),
            args: vec![],
            output_contains: Some("RUNNING".to_string()),
        };
        assert!(service_active_probe_success(
            &windows,
            true,
            "STATE : 4 RUNNING"
        ));
        assert!(!service_active_probe_success(
            &windows,
            true,
            "STATE : 1 STOPPED"
        ));
        assert!(!service_active_probe_success(
            &windows,
            false,
            "STATE : 4 RUNNING"
        ));
    }

    #[test]
    fn v3_instance_flags_parse_for_control_and_api_commands() {
        match parse(&["start", "--system"]).command {
            Some(Command::Start { system }) => {
                assert!(system);
            }
            _ => panic!("expected start --system"),
        }

        match parse(&["select", "--system", "--group", "Proxy"]).command {
            Some(Command::Select {
                system,
                group,
                node,
            }) => {
                assert!(system);
                assert_eq!(group.as_deref(), Some("Proxy"));
                assert!(node.is_none());
            }
            _ => panic!("expected select --system --group"),
        }

        assert!(Cli::try_parse_from(["mihomo-cli", "delay", "--user", "--fastest"]).is_err());

        match parse(&["conn", "--system", "--flush"]).command {
            Some(Command::Connections { system, flush }) => {
                assert!(system);
                assert!(flush);
            }
            _ => panic!("expected conn --system --flush"),
        }

        match parse(&["ip", "--system"]).command {
            Some(Command::Ip { system }) => assert!(system),
            _ => panic!("expected ip --system"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "ip", "--user"]).is_err());

        match parse(&["logs", "--system", "--level", "error"]).command {
            Some(Command::Logs {
                system,
                level,
                follow,
                ..
            }) => {
                assert!(system);
                assert_eq!(level.as_deref(), Some("error"));
                assert!(!follow);
            }
            _ => panic!("expected logs --system --level error"),
        }
        match parse(&["logs", "-f"]).command {
            Some(Command::Logs { follow, .. }) => assert!(follow),
            _ => panic!("expected logs -f"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "logs", "--user"]).is_err());

        match parse(&["tun", "status"]).command {
            Some(Command::Tun { system, action, .. }) => {
                assert!(!system);
                assert!(matches!(action, Some(TunAction::Status)));
            }
            _ => panic!("expected tun status"),
        }
        match parse(&["tun", "--system", "status"]).command {
            Some(Command::Tun { system, action, .. }) => {
                assert!(system);
                assert!(matches!(action, Some(TunAction::Status)));
            }
            _ => panic!("expected tun --system status"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "tun", "--user", "status"]).is_err());

        assert!(Cli::try_parse_from(["mihomo-cli", "status", "--verbose", "--user"]).is_err());
    }

    #[test]
    fn exit_ip_requires_exactly_one_target_mode() {
        match parse(&["exit-ip", "--node", "Korea 01"]).command {
            Some(Command::ExitIp {
                node,
                group,
                url,
                direct,
                ..
            }) => {
                assert_eq!(node.as_deref(), Some("Korea 01"));
                assert!(group.is_none());
                assert!(url.is_none());
                assert!(!direct);
            }
            _ => panic!("expected exit-ip --node"),
        }
        match parse(&["exit-ip", "--url", "https://github.com"]).command {
            Some(Command::ExitIp { url, .. }) => {
                assert_eq!(url.as_deref(), Some("https://github.com"))
            }
            _ => panic!("expected exit-ip --url"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "exit-ip"]).is_err());
        assert!(Cli::try_parse_from([
            "mihomo-cli",
            "exit-ip",
            "--node",
            "Korea 01",
            "--group",
            "节点选择",
        ])
        .is_err());
        assert!(
            Cli::try_parse_from(["mihomo-cli", "exit-ip", "--direct", "--url", "github.com",])
                .is_err()
        );
        assert!(Cli::try_parse_from(["mihomo-cli", "exit-ip", "--yes"]).is_err());
    }

    #[test]
    fn exit_ip_helpers_normalize_url_and_select_probe_group() {
        assert_eq!(
            normalize_url_host("https://github.com/CNCSMonster/mihomo-cli"),
            "github.com"
        );
        assert_eq!(normalize_url_host("github.com/path"), "github.com");
        let groups = vec![
            ProxyGroupInfo {
                name: "节点选择".to_string(),
                kind: "Selector".to_string(),
                now: Some("HK 01".to_string()),
                all: vec!["HK 01".to_string(), "Korea 01".to_string()],
            },
            ProxyGroupInfo {
                name: "GLOBAL".to_string(),
                kind: "Selector".to_string(),
                now: Some("DIRECT".to_string()),
                all: vec!["DIRECT".to_string(), "Korea 01".to_string()],
            },
        ];
        let proxy_names = std::collections::BTreeSet::from([
            "DIRECT".to_string(),
            "HK 01".to_string(),
            "Korea 01".to_string(),
        ]);
        assert_eq!(
            select_probe_group_for_node(&groups, "Korea 01").unwrap(),
            "GLOBAL"
        );
        assert_eq!(
            resolve_effective_outbound("节点选择", &groups, &proxy_names).unwrap(),
            "HK 01"
        );
        assert_eq!(
            resolve_effective_outbound("GLOBAL", &groups, &proxy_names).unwrap(),
            "DIRECT"
        );
        assert!(select_probe_group_for_node(&groups, "missing").is_err());
    }

    #[test]
    fn api_commands_parse_without_mode_flags() {
        match parse(&["list"]).command {
            Some(Command::List { .. }) => {}
            _ => panic!("expected list"),
        }

        match parse(&["select", "--group", "Proxy"]).command {
            Some(Command::Select { group, .. }) => {
                assert_eq!(group.as_deref(), Some("Proxy"));
            }
            _ => panic!("expected select --group"),
        }

        match parse(&["delay", "--fastest"]).command {
            Some(Command::Delay { fastest, .. }) => {
                assert!(fastest);
            }
            _ => panic!("expected delay --fastest"),
        }

        match parse(&["tun", "on"]).command {
            Some(Command::Tun { action, .. }) => {
                assert!(matches!(action, Some(TunAction::On)));
            }
            _ => panic!("expected tun on"),
        }

        match parse(&["conn", "--flush"]).command {
            Some(Command::Connections { flush, .. }) => {
                assert!(flush);
            }
            _ => panic!("expected conn --flush"),
        }
    }

    #[test]
    fn service_mode_flags_parse_for_install_and_uninstall() {
        match parse(&["install", "--user", "--force"]).command {
            Some(Command::Install { user, force, .. }) => {
                assert!(user);
                assert!(force);
            }
            _ => panic!("expected install command"),
        }

        match parse(&["install", "--system"]).command {
            Some(Command::Install { system, user, .. }) => {
                assert!(system);
                assert!(!user);
                assert_eq!(
                    mode_request_from_flags(system, user),
                    instance::ModeRequest::ExplicitSystem
                );
            }
            _ => panic!("expected install --system command"),
        }

        match parse(&["uninstall", "--system", "--all"]).command {
            Some(Command::Uninstall {
                system, user, all, ..
            }) => {
                assert!(system);
                assert!(!user);
                assert!(all);
            }
            _ => panic!("expected uninstall --system --all"),
        }
    }

    #[test]
    fn uninstall_granular_flags_parse_correctly() {
        // --all is shortcut for all three granular flags
        let cli = parse(&["uninstall", "--all", "--yes"]);
        match cli.command {
            Some(Command::Uninstall { all, yes, .. }) => {
                assert!(all);
                assert!(yes);
            }
            _ => panic!("expected uninstall --all --yes"),
        }

        // Individual flags
        let cli = parse(&["uninstall", "--remove-binary", "--remove-config"]);
        match cli.command {
            Some(Command::Uninstall { remove_binary, remove_config, remove_geo, .. }) => {
                assert!(remove_binary);
                assert!(remove_config);
                assert!(!remove_geo);
            }
            _ => panic!("expected uninstall --remove-binary --remove-config"),
        }

        // --yes + granular flags should work
        let cli = parse(&["uninstall", "--remove-geo", "--yes"]);
        match cli.command {
            Some(Command::Uninstall { remove_geo, yes, .. }) => {
                assert!(remove_geo);
                assert!(yes);
            }
            _ => panic!("expected uninstall --remove-geo --yes"),
        }

        // --dry-run alone
        let cli = parse(&["uninstall", "--dry-run"]);
        match cli.command {
            Some(Command::Uninstall { dry_run, .. }) => {
                assert!(dry_run);
            }
            _ => panic!("expected uninstall --dry-run"),
        }
    }

    #[test]
    fn system_and_user_service_flags_conflict_on_install_uninstall() {
        for args in [
            ["install", "--system", "--user"].as_slice(),
            ["uninstall", "--system", "--user"].as_slice(),
        ] {
            let err = match Cli::try_parse_from(
                std::iter::once("mihomo-cli").chain(args.iter().copied()),
            ) {
                Ok(_) => panic!("--system and --user must conflict"),
                Err(err) => err,
            };
            assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
        }
    }

    #[test]
    fn root_flag_is_not_public_cli_surface() {
        for args in [
            ["install", "--root"].as_slice(),
            ["uninstall", "--root"].as_slice(),
            ["start", "--root"].as_slice(),
            ["status", "--root"].as_slice(),
            ["tun", "--root", "on"].as_slice(),
            ["config", "--root", "--fix"].as_slice(),
            ["select", "--root"].as_slice(),
            ["system-proxy", "--root", "on"].as_slice(),
        ] {
            let err = match Cli::try_parse_from(
                std::iter::once("mihomo-cli").chain(args.iter().copied()),
            ) {
                Ok(_) => panic!("--root must not be accepted for {args:?}"),
                Err(err) => err,
            };
            assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
        }
    }

    #[test]
    fn proxy_and_system_proxy_subcommands_parse_without_side_effects() {
        match parse(&["proxy", "off"]).command {
            Some(Command::Proxy {
                system,
                action: ProxyAction::Off,
            }) => assert!(!system),
            _ => panic!("expected proxy off"),
        }
        match parse(&["proxy", "--system", "on"]).command {
            Some(Command::Proxy {
                system,
                action: ProxyAction::On,
            }) => assert!(system),
            _ => panic!("expected proxy --system on"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "proxy", "--user", "on"]).is_err());

        match parse(&["system-proxy", "on"]).command {
            Some(Command::SystemProxy {
                system,
                action: SystemProxyAction::On,
            }) => assert!(!system),
            _ => panic!("expected system-proxy on"),
        }
        match parse(&["system-proxy", "--system", "on"]).command {
            Some(Command::SystemProxy {
                system,
                action: SystemProxyAction::On,
            }) => assert!(system),
            _ => panic!("expected system-proxy --system on"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "system-proxy", "--user", "on"]).is_err());
    }

    #[test]
    fn shell_proxy_plans_are_eval_safe_contract() {
        assert_eq!(
            shell_proxy_on_plan(7890),
            ShellProxyPlan {
                stdout_lines: vec![
                    "export http_proxy=http://127.0.0.1:7890".to_string(),
                    "export https_proxy=http://127.0.0.1:7890".to_string(),
                    "export all_proxy=http://127.0.0.1:7890".to_string(),
                ],
                stderr_lines: vec![
                    "  Proxy enabled on port 7890".to_string(),
                    "  Usage: eval $(mihomo-cli proxy on)".to_string(),
                    "  Disable: eval $(mihomo-cli proxy off)".to_string(),
                ],
            }
        );
        assert_eq!(
            shell_proxy_off_plan(),
            ShellProxyPlan {
                stdout_lines: vec!["unset http_proxy https_proxy all_proxy".to_string()],
                stderr_lines: vec![
                    "  Proxy disabled".to_string(),
                    "  Usage: eval $(mihomo-cli proxy off)".to_string(),
                ],
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn journalctl_args_follow_resolved_instance_mode() {
        assert_eq!(
            journalctl_args_for_mode(instance::InstanceMode::User, 25, false),
            vec![
                "--user",
                "-u",
                "mihomo",
                "-n",
                "25",
                "--no-pager",
                "--output",
                "cat",
            ]
        );
        assert_eq!(
            journalctl_args_for_mode(instance::InstanceMode::System, 0, false),
            vec!["-u", "mihomo", "-n", "1", "--no-pager", "--output", "cat"]
        );
        assert_eq!(
            journalctl_args_for_mode(instance::InstanceMode::System, 10, true),
            vec![
                "-u",
                "mihomo",
                "-n",
                "10",
                "--no-pager",
                "--output",
                "cat",
                "-f"
            ]
        );
    }

    #[test]
    fn select_log_lines_filters_before_tail_case_insensitively() {
        let content = "INFO one\nDEBUG two\nERROR three\ninfo four\nWARN five\n";

        assert_eq!(
            select_log_lines(content, 2, Some("info")),
            vec!["INFO one".to_string(), "info four".to_string()],
            "level filter is applied before tail to show the last N matching log lines"
        );
        assert_eq!(
            select_log_lines(content, 2, None),
            vec!["info four".to_string(), "WARN five".to_string()]
        );
        assert!(select_log_lines(content, 0, None).is_empty());
        assert_eq!(
            select_log_lines(content, 10, Some("error")),
            vec!["ERROR three".to_string()]
        );
    }

    #[test]
    fn config_dry_run_messages_are_centralized() {
        assert_eq!(
            format_config_dry_run(ConfigDryRunAction::SetUserAgent {
                id: "sub-a",
                ua: "auto",
            }),
            vec!["  Would set UA for sub-a to auto".to_string()]
        );
        assert_eq!(
            format_config_dry_run(ConfigDryRunAction::Switch { id: "sub-a" }),
            vec!["  Would switch active subscription to sub-a".to_string()]
        );
        assert_eq!(
            format_config_dry_run(ConfigDryRunAction::Add {
                url: "https://example.test/sub",
            }),
            vec![
                "  Would download, validate, and add subscription: https://example.test/sub"
                    .to_string()
            ]
        );
        assert_eq!(
            format_config_dry_run(ConfigDryRunAction::Remove { id: "sub-a" }),
            vec!["  Would remove subscription sub-a".to_string()]
        );
        assert_eq!(
            format_config_dry_run(ConfigDryRunAction::RefreshAll { count: 2 }),
            vec!["  Would refresh 2 subscriptions and merge config".to_string()]
        );
        assert_eq!(
            format_config_dry_run(ConfigDryRunAction::RefreshActive { id: "sub-a" }),
            vec!["  Would refresh active subscription sub-a and merge config".to_string()]
        );
        assert_eq!(
            format_config_dry_run(ConfigDryRunAction::FixController),
            vec!["  Would ensure config has external controller socket/pipe".to_string()]
        );
        assert_eq!(
            format_config_dry_run(ConfigDryRunAction::LegacyUrl { url: "u" }),
            vec![
                "  Would download, validate, add, activate, and merge subscription: u".to_string()
            ]
        );
    }

    #[test]
    fn format_system_proxy_result_reports_enabled_port_and_disabled_state() {
        assert_eq!(
            format_system_proxy_enabled_result(7897),
            vec!["  ✓ System proxy enabled on 127.0.0.1:7897".to_string()]
        );
        assert_eq!(
            format_system_proxy_disabled_result(),
            vec!["  ✓ System proxy disabled".to_string()]
        );
        assert_eq!(
            system_proxy_tun_active_message_text(),
            "system service TUN is enabled; OS system proxy settings are ignored because TUN already captures traffic. No system proxy changes were made."
        );
    }

    #[test]
    fn import_content_classification_preserves_yaml_and_flags_raw_formats() {
        assert_eq!(
            classify_import_content(
                "proxies:
  - name: a
"
            ),
            ImportContentAction::UseAsYaml
        );
        assert_eq!(
            classify_import_content(
                "proxy-providers:
  provider-a: {}
"
            ),
            ImportContentAction::UseAsYaml
        );
        assert_eq!(
            classify_import_content("  dm1lc3M6Ly9leGFtcGxl"),
            ImportContentAction::ConvertBase64Subscription
        );
        assert_eq!(
            classify_import_content(
                "trojan://example
vmess://example"
            ),
            ImportContentAction::ConvertRawSubscription
        );
        assert_eq!(
            import_conversion_notice(ImportContentAction::UseAsYaml),
            None
        );
        assert_eq!(
            import_conversion_notice(ImportContentAction::ConvertRawSubscription),
            Some("  Attempting subscription format conversion...")
        );
    }

    #[test]
    fn tun_without_any_instance_points_to_system_service_install() {
        let message = tun_requires_system_service_install_message();
        assert!(message.contains("TUN requires the privileged system service"));
        assert!(message.contains("mihomo-cli tun on"));
        assert!(message.contains("mihomo-cli install --system"));
        assert!(message.contains("Per-user service does not have the privileges needed for TUN"));
    }

    #[test]
    fn refresh_messages_are_planned() {
        assert_eq!(
            format_refresh_all_success(),
            vec![
                "  All subscriptions refreshed.".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_refresh_active_start("sub-a"),
            vec!["  Refreshing active subscription sub-a...".to_string()]
        );
        assert_eq!(
            format_refresh_active_success(),
            vec![
                "  Subscription refreshed.".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            no_active_subscription_error(),
            "No active subscription.
  Run: mihomo-cli config --add <URL>"
        );
    }

    #[test]
    fn launchctl_bootout_domain_detects_launchd_cleanup_commands() {
        let command = instance::PlannedCommand {
            program: "launchctl".to_string(),
            args: vec!["bootout".to_string(), "system/io.mihomo".to_string()],
            privileged: true,
        };
        assert_eq!(launchctl_bootout_domain(&command), Some("system/io.mihomo"));

        let other = instance::PlannedCommand {
            program: "launchctl".to_string(),
            args: vec!["bootstrap".to_string(), "system".to_string()],
            privileged: true,
        };
        assert_eq!(launchctl_bootout_domain(&other), None);
    }

    #[test]
    fn install_cleanup_commands_are_best_effort() {
        for first_arg in ["bootout", "disable", "stop", "delete"] {
            assert!(is_best_effort_install_cleanup_command(
                &instance::PlannedCommand {
                    program: "svc".to_string(),
                    args: vec![first_arg.to_string()],
                    privileged: true,
                }
            ));
        }
        assert!(!is_best_effort_install_cleanup_command(
            &instance::PlannedCommand {
                program: "svc".to_string(),
                args: vec!["create".to_string()],
                privileged: true,
            }
        ));
    }

    #[test]
    fn system_core_start_command_uses_v3_per_user_config() {
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        match system_core_start_command(&ctx) {
            ipc::DaemonCommand::StartCore { config_path } => {
                assert_eq!(
                    config_path,
                    std::path::PathBuf::from("/Users/alice/.config/mihomo/config.yaml")
                );
            }
            other => panic!("expected StartCore command, got {other:?}"),
        }
    }

    #[test]
    fn install_messages_are_planned() {
        assert_eq!(
            format_install_already_installed(),
            vec!["Already installed. Use --force to reinstall.".to_string()]
        );
        let install_prompt = format_install_mode_prompt().join("\n");
        assert!(install_prompt.contains("How do you want mihomo-cli to run?"));
        assert!(install_prompt.contains("[1] Normal proxy mode"));
        assert!(install_prompt.contains("No admin password"));
        assert!(install_prompt.contains("[2] TUN mode / all-traffic mode"));
        assert!(install_prompt.contains("Clash Verge Rev-like TUN"));
        assert_eq!(
            format_install_mode_selected(true),
            vec!["Selected: Normal proxy mode".to_string(), String::new()]
        );
        assert_eq!(
            format_install_mode_selected(false),
            vec![
                "Selected: TUN/system service mode".to_string(),
                String::new()
            ]
        );
        assert_eq!(
            format_install_header("linux"),
            vec![
                "=== mihomo-cli install (linux) ===".to_string(),
                String::new()
            ]
        );
        assert_eq!(
            format_install_instance_header(
                instance::InstanceMode::System,
                instance::TargetOs::Linux
            ),
            "=== mihomo-cli install --system (Linux) ==="
        );
        assert_eq!(
            format_install_instance_header(instance::InstanceMode::User, instance::TargetOs::Macos),
            "=== mihomo-cli install --user (Macos) ==="
        );
        assert_eq!(
            install_download_error("timeout"),
            "Failed to download mihomo: timeout
  Check network and try --verbose for details"
        );
        assert_eq!(
            format_install_config_setup_failed("Config setup skipped", "bad url"),
            vec![
                "  ⚠ Config setup skipped: bad url".to_string(),
                "  You can configure later with: mihomo-cli config".to_string(),
            ]
        );
        assert_eq!(
            format_install_done(false),
            vec![
                String::new(),
                "=== Done ===".to_string(),
                "  ✅ Binary installed".to_string(),
                "  ⚠ Config pending — run: mihomo-cli config".to_string(),
            ]
        );
        assert_eq!(
            format_install_done(true),
            vec![
                String::new(),
                "=== Done ===".to_string(),
                "  ✅ Binary installed".to_string(),
                "  ✅ Config ready".to_string(),
                String::new(),
                "  Next steps:".to_string(),
                "    mihomo-cli restart    start/restart service".to_string(),
                "    mihomo-cli select     select proxy node".to_string(),
                "    mihomo-cli status     check service/core status".to_string(),
                "    mihomo-cli ip         check current exit IP".to_string(),
                "    mihomo-cli tun on     enable TUN mode".to_string(),
            ]
        );
        assert_eq!(install_mode_label(true), "user-level");
        assert_eq!(install_mode_label(false), "system");
        assert_eq!(
            format_install_pending_service_notice(),
            vec![
                String::new(),
                "Config is pending. Service will not be started yet.".to_string(),
                "After configuring, run: mihomo-cli restart".to_string(),
            ]
        );
        assert_eq!(
            format_install_service_prompt("user-level"),
            vec![
                String::new(),
                "Install and start user-level service?".to_string(),
                "  [y] Yes, install and start".to_string(),
                "  [n] No, skip (you can run 'mihomo-cli restart' later)".to_string(),
            ]
        );
        assert!(should_install_service_answer(""));
        assert!(should_install_service_answer(" YES "));
        assert!(!should_install_service_answer("n"));
    }

    #[test]
    fn uninstall_and_update_messages_are_planned() {
        assert_eq!(
            format_uninstall_nothing(),
            vec!["Nothing to uninstall.".to_string()]
        );
        assert_eq!(
            format_uninstall_intro(
                true,
                true,
                true,
                "/usr/local/bin/mihomo",
                "/home/me/.config/mihomo"
            ),
            vec![
                "=== mihomo-cli uninstall ===".to_string(),
                String::new(),
                "This will:".to_string(),
                "  - Stop running mihomo process".to_string(),
                "  - Remove auto-start service".to_string(),
                "  - Delete mihomo binary (/usr/local/bin/mihomo)".to_string(),
                "  - Delete config dir (/home/me/.config/mihomo)".to_string(),
                String::new(),
            ]
        );
        assert_eq!(
            format_uninstall_intro(false, true, false, "/bin/mihomo", "/cfg"),
            vec![
                "=== mihomo-cli uninstall ===".to_string(),
                String::new(),
                "This will:".to_string(),
                "  - Remove auto-start service".to_string(),
                String::new(),
            ]
        );
        assert_eq!(uninstall_prompt(true), "Proceed with full removal?");
        assert_eq!(uninstall_prompt(false), "Proceed?");
        assert_eq!(format_uninstall_cancelled(), vec!["Cancelled.".to_string()]);
        assert_eq!(
            format_uninstall_stop_mihomo(),
            vec![String::new(), "Stopping mihomo...".to_string()]
        );
        assert_eq!(
            format_uninstall_remove_service(),
            vec!["Removing service...".to_string()]
        );
        assert_eq!(
            format_uninstall_remove_binaries(),
            vec!["Removing binaries...".to_string()]
        );
        assert_eq!(format_uninstall_done(), vec!["Done.".to_string()]);
        assert!(should_retry_removal_with_sudo(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
        assert!(!should_retry_removal_with_sudo(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));

        assert_eq!(
            update_missing_binary_error("/bin/mihomo"),
            "mihomo not installed at /bin/mihomo
  Run: mihomo-cli install"
        );
        assert_eq!(
            format_update_start(),
            vec!["Updating mihomo core...".to_string()]
        );
        assert_eq!(
            format_update_success(),
            vec!["Updated successfully".to_string()]
        );
        assert_eq!(
            update_failed_error("network down"),
            "Update failed: network down
  Original binary restored"
        );
    }

    #[test]
    fn probe_and_tui_subscription_messages_are_planned() {
        use chrono::{TimeZone, Utc};

        assert_eq!(
            format_probe_start(4),
            vec![
                "  Probing subscription URL with bounded UA candidates...".to_string(),
                "  Note: probe sends 4 sequential requests with a short delay to reduce rate-limit risk."
                    .to_string(),
            ]
        );
        assert_eq!(
            format_tui_empty_subscription_intro(),
            vec![
                String::new(),
                "  No subscriptions found.".to_string(),
                "  Press 'a' to add one, or Esc to exit.".to_string(),
            ]
        );
        assert_eq!(
            format_tui_action_hint(),
            vec![
                String::new(),
                "  Press: [r] Refresh  [R] Refresh all  [a] Add  [d] Delete  [Esc] Exit"
                    .to_string(),
            ]
        );
        assert_eq!(
            format_refresh_all_start(),
            vec!["  Refreshing all subscriptions...".to_string()]
        );

        let updated = Utc.with_ymd_and_hms(2026, 7, 17, 0, 0, 0).unwrap();
        let subs = vec![
            config::SubscriptionMeta {
                id: "sub-a".to_string(),
                url: "https://example.test/short".to_string(),
                updated,
                user_agent: None,
                user_agent_mode: None,
            },
            config::SubscriptionMeta {
                id: "sub-b".to_string(),
                url: "https://订阅.example.test/路径/with/a/very/very/very/long/token/abcdef1234567890".to_string(),
                updated,
                user_agent: None,
                user_agent_mode: None,
            },
        ];
        let menu_items = format_tui_subscription_menu_items(&subs, Some("sub-b"));
        assert_eq!(menu_items[0], "https://example.test/short");
        assert!(menu_items[1].contains('…'), "item was: {}", menu_items[1]);
        assert!(menu_items[1].ends_with("bcdef1234567890 (active)"));

        let delete_items = format_tui_delete_items(&subs);
        assert_eq!(delete_items[0], "sub-a (https://example.test/short)");
        assert!(delete_items[1].starts_with("sub-b (https://订阅.example.test/路径/with/a/"));
        assert!(delete_items[1].ends_with("…)"));

        assert_eq!(
            format_tui_add_success("sub-a"),
            vec![
                "  Added subscription sub-a".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_tui_switch_result("sub-a", false),
            vec!["  Already active.".to_string()]
        );
        assert_eq!(
            format_tui_refresh_active_start("sub-a"),
            vec!["  Refreshing subscription sub-a...".to_string()]
        );
        assert_eq!(
            format_tui_no_active_subscription(),
            vec!["  No active subscription.".to_string()]
        );
        assert_eq!(
            format_tui_subscription_removed("sub-a"),
            vec!["  Removed subscription sub-a".to_string()]
        );
    }

    #[test]
    fn dns_command_messages_are_planned() {
        let policy = crate::dns::DnsPolicy {
            match_pattern: "corp.example.com".to_string(),
            target: "10.0.0.2".to_string(),
        };
        assert_eq!(
            format_dns_policy_added("corp.example.com", "10.0.0.2"),
            vec![
                "  ✓ Policy added: corp.example.com → 10.0.0.2".to_string(),
                "  ✓ Config updated — restart mihomo to apply DNS changes".to_string(),
            ]
        );
        assert_eq!(
            format_dns_policy_removed(&policy),
            vec![
                "  ✓ Policy removed: corp.example.com → 10.0.0.2".to_string(),
                "  ✓ Config updated — restart mihomo to apply DNS changes".to_string(),
            ]
        );
        assert_eq!(
            format_dns_policy_list::<crate::dns::DnsPolicy>(&[]),
            vec![
                "  No DNS policies defined.".to_string(),
                String::new(),
                "  Add one:  mihomo-cli dns policy add <MATCH> <TARGET>".to_string(),
                "  Example:  mihomo-cli dns policy add ubtrobot.com system".to_string(),
            ]
        );
        assert_eq!(
            format_dns_policy_list(&[(1, policy.clone())]),
            vec![
                "  DNS policies:".to_string(),
                "  1. corp.example.com → 10.0.0.2".to_string(),
            ]
        );
        assert_eq!(
            format_dns_template_list(crate::dns::dns_templates()),
            vec![
                "  Available DNS templates:".to_string(),
                "  - company  route one internal domain suffix to a company DNS server".to_string(),
                "  - ads      route common ad/tracker DNS suffixes to a filtering DNS server"
                    .to_string(),
                String::new(),
                "  Apply company template:".to_string(),
                "    mihomo-cli dns template apply company --domain corp.example.com --target 10.10.1.251".to_string(),
            ]
        );
        assert_eq!(
            format_dns_template_applied("company", &[policy]),
            vec![
                "  ✓ Applied DNS template: company".to_string(),
                "  - corp.example.com → 10.0.0.2".to_string(),
                "  ✓ Config updated — restart mihomo to apply DNS changes".to_string(),
            ]
        );
    }

    #[test]
    fn rule_action_messages_are_planned() {
        assert_eq!(
            format_rule_add_success("DOMAIN,example.com,DIRECT", true, true),
            vec![
                "  ✓ Rule added: DOMAIN,example.com,DIRECT".to_string(),
                "  ✓ Config reloaded — rule is now active".to_string(),
            ]
        );
        assert_eq!(
            format_rule_add_success("DOMAIN,example.com,DIRECT", true, false),
            vec![
                "  ✓ Rule added: DOMAIN,example.com,DIRECT".to_string(),
                "  ℹ Run: mihomo-cli restart  (to apply the new rule)".to_string(),
            ]
        );
        assert_eq!(
            format_rule_remove_success(2, false, false),
            vec![
                "  ✓ Rule 2 removed".to_string(),
                "  ℹ Config pending — rule saved, run `mihomo-cli config` first".to_string(),
            ]
        );
        assert_eq!(
            format_rule_clear_success(true, true),
            vec![
                "  ✓ All rules cleared".to_string(),
                "  ✓ Config reloaded".to_string(),
            ]
        );
        assert_eq!(
            format_rule_move_success(1, 3, true, false),
            vec![
                "  ✓ Rule moved: 1 → 3".to_string(),
                "  ℹ Run: mihomo-cli restart  (to apply)".to_string(),
            ]
        );
        assert_eq!(
            format_rule_import_success(4, "rules.txt", false, false),
            vec![
                "  ✓ Imported 4 rules from rules.txt".to_string(),
                "  ℹ Config pending — rule saved, run `mihomo-cli config` first".to_string(),
            ]
        );
        assert_eq!(
            format_rule_export_success(4, "rules.txt"),
            vec!["  ✓ Exported 4 rules to rules.txt".to_string()]
        );
    }

    #[test]
    fn rule_query_messages_are_planned() {
        assert_eq!(
            format_rule_position_set(crate::rules::RulePosition::Front),
            vec!["  ✓ Default insert position set to: front".to_string()]
        );
        assert_eq!(
            format_rule_position_show(crate::rules::RulePosition::Back),
            vec![
                "  Default insert position: back".to_string(),
                String::new(),
                "  Change it:  mihomo-cli rule position front|back".to_string(),
            ]
        );
        assert_eq!(
            format_rule_policies(&["DIRECT".to_string(), "Proxy".to_string()]),
            vec![
                "  Available policies:".to_string(),
                "  - DIRECT".to_string(),
                "  - Proxy".to_string(),
            ]
        );
        let matched = crate::rules::RuleMatch {
            index: 2,
            rule: "DOMAIN,example.com,DIRECT".to_string(),
            policy: "DIRECT".to_string(),
        };
        assert_eq!(
            format_rule_test_result("example.com", Some(&matched)),
            vec![
                "  ✓ Matched rule #2: DOMAIN,example.com,DIRECT".to_string(),
                "  Policy: DIRECT".to_string(),
            ]
        );
        assert_eq!(
            format_rule_test_result("none.test", None),
            vec!["  No matching rule found for none.test".to_string()]
        );
    }

    #[test]
    fn override_action_intent_matches_readonly_and_mutating_actions() {
        assert_eq!(
            override_action_intent(&OverrideAction::Path),
            instance::CommandIntent::ReadOnly
        );
        assert_eq!(
            override_action_intent(&OverrideAction::Show),
            instance::CommandIntent::ReadOnly
        );
        assert_eq!(
            override_action_intent(&OverrideAction::Import {
                path: "/tmp/o.yaml".to_string(),
            }),
            instance::CommandIntent::Mutating
        );
        assert_eq!(
            override_action_intent(&OverrideAction::Clear { yes: true }),
            instance::CommandIntent::Mutating
        );
    }

    #[test]
    fn override_subcommands_parse_with_system_override() {
        match parse(&["override", "--system", "path"]).command {
            Some(Command::Override { system, action }) => {
                assert!(system);
                assert!(matches!(action, OverrideAction::Path));
            }
            _ => panic!("expected override --system path"),
        }
        match parse(&["override", "import", "/tmp/override.yaml"]).command {
            Some(Command::Override { system, action }) => {
                assert!(!system);
                assert!(matches!(action, OverrideAction::Import { .. }));
            }
            _ => panic!("expected override import"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "override", "--user", "path"]).is_err());
    }

    #[test]
    fn backup_and_restore_parse_without_mode_flags() {
        match parse(&["backup", "/tmp/out"]).command {
            Some(Command::Backup { system, output }) => {
                assert!(!system);
                assert_eq!(output.as_deref(), Some("/tmp/out"));
            }
            _ => panic!("expected backup /tmp/out"),
        }
        match parse(&["backup", "--system", "/tmp/out"]).command {
            Some(Command::Backup { system, output }) => {
                assert!(system);
                assert_eq!(output.as_deref(), Some("/tmp/out"));
            }
            _ => panic!("expected backup --system /tmp/out"),
        }

        match parse(&["restore", "/tmp/backup", "--yes"]).command {
            Some(Command::Restore { system, path, yes }) => {
                assert!(!system);
                assert_eq!(path, "/tmp/backup");
                assert!(yes);
            }
            _ => panic!("expected restore /tmp/backup"),
        }
        match parse(&["restore", "--system", "/tmp/backup", "--yes"]).command {
            Some(Command::Restore { system, path, yes }) => {
                assert!(system);
                assert_eq!(path, "/tmp/backup");
                assert!(yes);
            }
            _ => panic!("expected restore --system /tmp/backup"),
        }
    }

    #[test]
    fn validation_backup_and_restore_messages_are_planned() {
        let config_path = std::path::Path::new("/tmp/mihomo config/config.yaml");
        let mihomo_path = std::path::Path::new("/opt/mihomo");
        assert_eq!(
            format_config_validation_result(
                config_path,
                mihomo_path,
                &config::ConfigValidationReport {
                    yaml_valid: true,
                    mihomo_tested: true,
                },
            ),
            vec![
                "  ✓ YAML syntax valid: /tmp/mihomo config/config.yaml".to_string(),
                "  ✓ mihomo -t passed".to_string(),
            ]
        );
        assert_eq!(
            format_config_validation_result(
                config_path,
                mihomo_path,
                &config::ConfigValidationReport {
                    yaml_valid: true,
                    mihomo_tested: false,
                },
            ),
            vec![
                "  ✓ YAML syntax valid: /tmp/mihomo config/config.yaml".to_string(),
                "  ⚠ mihomo binary not found: /opt/mihomo".to_string(),
                "  YAML is valid, but runtime validation was skipped.".to_string(),
            ]
        );

        let backup_report = backup::BackupReport {
            path: std::path::PathBuf::from("/tmp/mihomo backups/backup one"),
            copied_items: vec!["config.yaml".to_string()],
        };
        assert_eq!(
            format_backup_success(&backup_report),
            vec![
                "  ✓ Backup created: /tmp/mihomo backups/backup one".to_string(),
                "  Restore with: mihomo-cli restore '/tmp/mihomo backups/backup one'".to_string(),
            ]
        );
        assert_eq!(
            format_restore_success(Some(std::path::Path::new("/tmp/safety backup"))),
            vec![
                "  Safety backup created: /tmp/safety backup".to_string(),
                "  ✓ Restore complete".to_string(),
                "  Run: mihomo-cli restart  to apply restored config".to_string(),
            ]
        );
        assert_eq!(
            format_restore_success(None),
            vec![
                "  ✓ Restore complete".to_string(),
                "  Run: mihomo-cli restart  to apply restored config".to_string(),
            ]
        );
    }

    #[test]
    fn config_mutation_messages_are_planned() {
        assert_eq!(
            format_config_add_start(),
            vec!["  Adding subscription...".to_string()]
        );
        assert_eq!(
            format_config_add_success("sub-a"),
            vec![
                "  Added subscription sub-a".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_legacy_url_add_success("sub-a", true),
            vec![
                "  Added and activated subscription sub-a".to_string(),
                "  Config reloaded".to_string(),
            ]
        );
        assert_eq!(
            format_legacy_url_add_success("sub-a", false),
            vec![
                "  Added and activated subscription sub-a".to_string(),
                "  Run: mihomo-cli restart".to_string(),
            ]
        );
        assert_eq!(
            format_import_success("sub-a", true),
            vec![
                "  Imported and activated subscription sub-a".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_import_success("sub-a", false),
            vec![
                "  Imported subscription sub-a (not activated)".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_fix_result(true, true),
            vec![
                "  Fixed config: added Unix socket controller.".to_string(),
                "  ⚠ Restart required for controller changes to take effect.".to_string(),
                "  Run: mihomo-cli restart".to_string(),
                "  Config hot-reloaded (other changes).".to_string(),
            ]
        );
        assert_eq!(
            format_fix_result(false, false),
            vec!["  Config already has Unix socket — no fix needed.".to_string()]
        );
    }

    #[test]
    fn subscription_switch_and_rollback_messages_are_planned() {
        assert_eq!(
            format_subscription_switch_success("sub-a"),
            vec![
                "  Switched to subscription sub-a".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            subscription_switch_rollback_error("invalid yaml"),
            "Subscription switch failed; rolled back active subscription.
  invalid yaml"
        );
        assert_eq!(
            subscription_change_rollback_error("mihomo -t failed"),
            "Subscription change failed; rolled back subscription file and metadata.
  mihomo -t failed"
        );
    }

    #[test]
    fn config_change_result_helpers_keep_restart_hint_consistent() {
        assert_eq!(
            format_config_change_result("Switched to subscription", "sub-a"),
            vec!["  Switched to subscription sub-a".to_string()]
        );
        assert_eq!(
            config_restart_apply_lines(),
            vec!["  Run: mihomo-cli restart  to apply".to_string()]
        );
    }

    #[test]
    fn format_probe_results_shows_scores_errors_and_recommendation() {
        let results = vec![
            config::SubscriptionProbeResult {
                label: "clash-verge".to_string(),
                user_agent: Some("clash-verge/v2.0.4".to_string()),
                format: "clash-yaml".to_string(),
                http_status: Some(200),
                proxy_count: 10,
                proxy_group_count: 3,
                rule_count: 42,
                proxy_provider_count: 1,
                rule_provider_count: 2,
                bytes: 4096,
                score: 765,
                error: None,
            },
            config::SubscriptionProbeResult {
                label: "bare".to_string(),
                user_agent: None,
                format: "error".to_string(),
                http_status: None,
                proxy_count: 0,
                proxy_group_count: 0,
                rule_count: 0,
                proxy_provider_count: 0,
                rule_provider_count: 0,
                bytes: 0,
                score: -100,
                error: Some("timeout".to_string()),
            },
        ];

        let lines = format_probe_results(&results);

        assert!(lines[0].contains("UA"));
        assert!(lines[0].contains("Providers"));
        assert!(lines[1].contains("clash-verge"));
        assert!(lines[1].contains("clash-yaml"));
        assert!(
            lines[1].contains("3"),
            "providers should sum proxy + rule providers: {}",
            lines[1]
        );
        assert!(lines.iter().any(|line| line == "    error: timeout"));
        assert!(lines
            .iter()
            .any(|line| line == "\n  Recommended: clash-verge"));
        assert!(lines
            .iter()
            .any(|line| line == "  User-Agent: clash-verge/v2.0.4"));
    }

    #[test]
    fn format_subscription_list_marks_active_and_shortens_urls_safely() {
        use chrono::{TimeZone, Utc};

        let updated = Utc.with_ymd_and_hms(2026, 7, 17, 0, 0, 0).unwrap();
        let subs = vec![
            config::SubscriptionMeta {
                id: "sub-a".to_string(),
                url: "https://example.test/short".to_string(),
                updated,
                user_agent: None,
                user_agent_mode: None,
            },
            config::SubscriptionMeta {
                id: "sub-b".to_string(),
                url: "https://订阅.example.test/路径/with/a/very/very/very/long/token/abcdef1234567890"
                    .to_string(),
                updated,
                user_agent: None,
                user_agent_mode: None,
            },
        ];

        assert_eq!(
            format_subscription_list(&[], None),
            vec![
                "  No subscriptions found.".to_string(),
                "  Run: mihomo-cli config --add <URL>".to_string(),
            ]
        );

        let lines = format_subscription_list(&subs, Some("sub-b"));

        assert_eq!(lines[0], "  Subscriptions:");
        assert_eq!(lines[1], "    sub-a (https://example.test/short)");
        assert!(lines[2].starts_with("  ▶ sub-b (https://订阅.example.test/路径/wit"));
        assert!(lines[2].contains('…'), "line was: {}", lines[2]);
        assert!(
            lines[2].ends_with("bcdef1234567890)"),
            "line was: {}",
            lines[2]
        );
    }

    #[test]
    fn format_subscription_info_shows_metadata_and_expire_fallback() {
        use chrono::{TimeZone, Utc};

        let updated = Utc.with_ymd_and_hms(2026, 7, 17, 8, 30, 0).unwrap();
        let info = config::SubscriptionInfo {
            id: "sub-fixed".to_string(),
            url: "https://example.test/sub".to_string(),
            updated,
            proxy_count: 12,
            expire: Some("2026-12-31".to_string()),
        };
        let meta = config::SubscriptionMeta {
            id: "sub-fixed".to_string(),
            url: info.url.clone(),
            updated,
            user_agent: Some("clash-verge/v2.0.4".to_string()),
            user_agent_mode: Some(config::UserAgentMode::Fixed),
        };

        assert_eq!(
            format_subscription_info(&info, Some(&meta)),
            vec![
                "  Subscription: sub-fixed".to_string(),
                "  URL: https://example.test/sub".to_string(),
                format!("  Updated: {updated}"),
                "  User-Agent mode: Fixed".to_string(),
                "  User-Agent: clash-verge/v2.0.4".to_string(),
                "  Proxies: 12".to_string(),
                "  Expire: 2026-12-31".to_string(),
            ]
        );

        let info_without_expire = config::SubscriptionInfo {
            expire: None,
            ..info
        };
        assert_eq!(
            format_subscription_info(&info_without_expire, None),
            vec![
                "  Subscription: sub-fixed".to_string(),
                "  URL: https://example.test/sub".to_string(),
                format!("  Updated: {updated}"),
                "  Proxies: 12".to_string(),
                "  Expire: -".to_string(),
            ]
        );
    }

    #[test]
    fn format_dns_status_shows_defaults_and_policies() {
        let dns = serde_json::json!({
            "enable": true,
            "enhanced-mode": "fake-ip",
            "fake-ip-range": "198.18.0.1/16",
            "listen": "127.0.0.1:1053",
            "default-nameserver": ["223.5.5.5", 1, "1.1.1.1"],
        });
        let policies = vec![(1, "nameserver-policy: +.corp -> 10.0.0.1".to_string())];

        assert_eq!(
            format_dns_status(&dns, &policies),
            vec![
                "  DNS: enabled (fake-ip)".to_string(),
                "  Default nameservers: 223.5.5.5, 1.1.1.1".to_string(),
                "  Fake-IP range: 198.18.0.1/16".to_string(),
                "  Listen: 127.0.0.1:1053".to_string(),
                String::new(),
                "  Policies:".to_string(),
                "    1. nameserver-policy: +.corp -> 10.0.0.1".to_string(),
            ]
        );

        assert_eq!(
            format_dns_status::<String>(&serde_json::json!({}), &[]),
            vec![
                "  DNS: disabled (normal)".to_string(),
                "  Fake-IP range: -".to_string(),
                "  Listen: -".to_string(),
            ]
        );
    }

    #[test]
    fn format_rule_list_shows_position_empty_hint_and_numbered_rules() {
        assert_eq!(
            format_rule_list(&[], crate::rules::RulePosition::Front),
            vec![
                "  Insert position: front".to_string(),
                String::new(),
                "  (no user rules)".to_string(),
                String::new(),
                "  Add a rule:  mihomo-cli rule add DOMAIN-SUFFIX,example.com,DIRECT".to_string(),
            ]
        );

        let rules = vec![
            "DOMAIN-SUFFIX,example.com,DIRECT".to_string(),
            "DOMAIN-KEYWORD,openai,Proxy".to_string(),
        ];
        assert_eq!(
            format_rule_list(&rules, crate::rules::RulePosition::Back),
            vec![
                "  Insert position: back".to_string(),
                String::new(),
                "  1. DOMAIN-SUFFIX,example.com,DIRECT".to_string(),
                "  2. DOMAIN-KEYWORD,openai,Proxy".to_string(),
            ]
        );
    }

    #[test]
    fn tun_system_install_prompt_is_task_oriented() {
        let lines = format_tun_system_install_prompt();
        let text = lines.join(
            "
",
        );
        assert!(text.contains("TUN requires the privileged mihomo system service"));
        assert!(text.contains("Install system service now"));
        assert!(text.contains("Password is required once"));
        assert!(should_install_system_for_tun_answer(""));
        assert!(should_install_system_for_tun_answer(" yes "));
        assert!(!should_install_system_for_tun_answer("n"));
    }

    #[test]
    fn tun_user_to_system_switch_prompt_is_conservative() {
        let text = format_tun_user_to_system_switch_prompt(true, true).join("\n");
        assert!(text.contains("per-user mihomo core is currently running"));
        assert!(text.contains("Switch to TUN/system service mode"));
        assert!(text.contains("stop/remove the per-user service"));
        assert!(text.contains("keep your user config"));
        assert!(!should_switch_user_to_system_for_tun_answer(""));
        assert!(should_switch_user_to_system_for_tun_answer("y"));
        assert!(should_switch_user_to_system_for_tun_answer(" yes "));
        assert!(!should_switch_user_to_system_for_tun_answer("n"));
    }

    #[test]
    fn system_lifecycle_daemon_unavailable_message_gives_recovery_command() {
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        let message = system_daemon_unavailable_message("start", &ctx);
        assert!(message.contains("system daemon IPC is not running"));
        assert!(message.contains("cannot start the system core"));
        assert!(message.contains("Recover the daemon"));
        assert!(message.contains("sudo systemctl restart mihomo"));
        assert!(message.contains("mihomo-cli start"));
        assert!(message.contains("mihomo-cli install --system"));
    }

    #[test]
    fn system_tun_mutation_requires_running_daemon_with_task_retry() {
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Macos,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        let on = system_tun_requires_daemon_message(Some(&TunAction::On), &ctx)
            .expect("tun on should require daemon");
        assert!(on.contains("system daemon IPC"));
        assert!(on.contains("sudo launchctl kickstart -k system/io.mihomo"));
        assert!(on.contains("mihomo-cli tun on"));
        assert!(on.contains("mihomo-cli install --system"));
        assert!(system_tun_requires_daemon_message(Some(&TunAction::Off), &ctx).is_some());
        assert!(system_tun_requires_daemon_message(Some(&TunAction::Status), &ctx).is_none());
        assert!(system_tun_requires_daemon_message(None, &ctx).is_none());
    }

    #[test]
    fn tun_status_is_readonly_and_uses_daemon_status() {
        assert_eq!(
            tun_action_intent(Some(&TunAction::On)),
            instance::CommandIntent::Mutating
        );
        assert_eq!(
            tun_action_intent(Some(&TunAction::Off)),
            instance::CommandIntent::Mutating
        );
        assert_eq!(
            tun_action_intent(Some(&TunAction::Status)),
            instance::CommandIntent::ReadOnly
        );
        assert_eq!(tun_action_intent(None), instance::CommandIntent::ReadOnly);
        assert!(tun_action_uses_daemon_status(Some(&TunAction::Status)));
        assert!(tun_action_uses_daemon_status(None));
        assert!(!tun_action_uses_daemon_status(Some(&TunAction::On)));
    }

    #[test]
    fn non_windows_pipe_probe_is_false_on_this_target() {
        #[cfg(not(windows))]
        assert!(!windows_pipe_connectable(r"\\.\pipe\mihomo-alice"));
    }

    #[test]
    fn status_detects_no_instance_only_without_explicit_mode_or_presence() {
        let none = instance::ServicePresence {
            system: false,
            user: false,
        };
        let user_running = instance::ServicePresence {
            system: false,
            user: true,
        };

        assert!(status_has_no_instance(false, false, none, none));
        assert!(!status_has_no_instance(true, false, none, none));
        assert!(!status_has_no_instance(false, false, none, user_running));
    }

    #[test]
    fn api_not_running_message_is_task_oriented() {
        let user = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::User,
            &instance::PathInputs::for_tests(),
        );
        let system = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        let user_message = api_requires_running_instance_message(&user);
        assert!(user_message.contains("mihomo core API is not running"));
        assert!(user_message.contains("mihomo-cli start"));
        assert!(!user_message.contains("--system"));
        let system_message = api_requires_running_instance_message(&system);
        assert!(system_message.contains("system service"));
        assert!(system_message.contains("sudo systemctl restart mihomo"));
        assert!(system_message.contains("mihomo-cli start"));
    }

    #[test]
    fn stop_without_instance_is_clear_noop() {
        assert_eq!(
            format_stop_no_instance(),
            vec![
                "No running mihomo instance detected.".to_string(),
                "Nothing to stop.".to_string(),
            ]
        );
    }

    #[test]
    fn start_without_instance_points_to_install_or_tun_task() {
        let message = start_requires_install_message();
        assert!(message.contains("No mihomo service is installed yet"));
        assert!(message.contains("mihomo-cli install --user"));
        assert!(message.contains("mihomo-cli tun on"));
        assert!(!message.contains("--system"));
    }

    #[test]
    fn no_instance_status_is_task_oriented_and_not_planned_user_status() {
        let output = format_no_instance_status().join("\n");
        assert!(output.contains("No running mihomo instance detected."));
        assert!(output.contains("No service is installed."));
        assert!(output.contains("mihomo-cli tun on"));
        assert!(!output.contains("Instance:"));
        assert!(!output.contains("Resolved by:"));
        assert!(!output.contains("Service:"));
    }

    #[test]
    fn uninstall_all_without_explicit_mode_targets_both_v3_modes() {
        assert_eq!(
            uninstall_modes_for_request(false, false, true),
            Some(vec![
                instance::InstanceMode::System,
                instance::InstanceMode::User,
            ])
        );
        assert_eq!(uninstall_modes_for_request(true, false, true), None);
        assert_eq!(uninstall_modes_for_request(false, true, true), None);
        assert_eq!(uninstall_modes_for_request(false, false, false), None);
    }

    #[test]
    fn install_mode_conflict_rejects_opposite_installed_service() {
        let user_installed = instance::ServicePresence {
            system: false,
            user: true,
        };
        let system_installed = instance::ServicePresence {
            system: true,
            user: false,
        };
        let none_installed = instance::ServicePresence {
            system: false,
            user: false,
        };

        let system_err =
            install_mode_conflict_message(instance::InstanceMode::System, user_installed)
                .expect("system install should reject installed user service");
        assert!(system_err.contains("per-user service is installed"));
        assert!(system_err.contains("mihomo-cli uninstall --user"));

        let user_err =
            install_mode_conflict_message(instance::InstanceMode::User, system_installed)
                .expect("user install should reject installed system service");
        assert!(user_err.contains("system service is installed"));
        assert!(user_err.contains("mihomo-cli uninstall --system"));

        assert!(
            install_mode_conflict_message(instance::InstanceMode::User, none_installed,).is_none()
        );
    }

    #[test]
    fn set_instance_tun_config_updates_user_owned_config_before_daemon_ipc() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        ctx.paths.config_dir = temp.path().to_path_buf();
        ctx.paths.config_file = temp.path().join("config.yaml");
        std::fs::write(
            &ctx.paths.config_file,
            "mixed-port: 7897\ntun:\n  enable: false\n  stack: system\n",
        )
        .unwrap();

        set_instance_tun_config(&ctx, true, Some(&TunStack::Gvisor), Some("any:53")).unwrap();

        let doc: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&ctx.paths.config_file).unwrap())
                .unwrap();
        assert_eq!(doc["mixed-port"].as_i64(), Some(7897));
        assert_eq!(doc["tun"]["enable"].as_bool(), Some(true));
        assert_eq!(doc["tun"]["stack"].as_str(), Some("gvisor"));
        assert_eq!(doc["tun"]["dns-hijack"][0].as_str(), Some("any:53"));
    }

    #[test]
    fn ensure_instance_controller_endpoint_repairs_config_for_selected_instance() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        ctx.paths.config_dir = temp.path().to_path_buf();
        ctx.paths.config_file = temp.path().join("config.yaml");
        ctx.paths.backup_dir = temp.path().join("backups");
        std::fs::write(
            &ctx.paths.config_file,
            "mixed-port: 7897\nexternal-controller-unix: /tmp/old.sock\n",
        )
        .unwrap();

        ensure_instance_controller_endpoint(&ctx).unwrap();
        let fixed = std::fs::read_to_string(&ctx.paths.config_file).unwrap();
        assert!(fixed.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!fixed.contains("/tmp/old.sock"));
    }

    #[test]
    fn explicit_mode_request_rejects_opposite_active_runtime() {
        let user_running = instance::ServicePresence {
            system: false,
            user: true,
        };
        let system_running = instance::ServicePresence {
            system: true,
            user: false,
        };
        let both_running = instance::ServicePresence {
            system: true,
            user: true,
        };

        let system_err = explicit_mode_runtime_conflict(
            instance::ModeRequest::ExplicitSystem,
            user_running,
            instance::CommandIntent::ReadOnly,
        )
        .expect("explicit system should reject an active user runtime");
        assert!(system_err.contains("only the per-user core"));

        let user_err = explicit_mode_runtime_conflict(
            instance::ModeRequest::ExplicitUser,
            system_running,
            instance::CommandIntent::ReadOnly,
        )
        .expect("explicit user should reject an active system runtime");
        assert!(user_err.contains("system daemon appears to be running"));

        let both_read_err = explicit_mode_runtime_conflict(
            instance::ModeRequest::ExplicitSystem,
            both_running,
            instance::CommandIntent::ReadOnly,
        )
        .expect("explicit read should reject conflicting runtimes");
        assert!(both_read_err.contains("both system daemon and per-user core are running"));

        assert!(explicit_mode_runtime_conflict(
            instance::ModeRequest::ExplicitSystem,
            both_running,
            instance::CommandIntent::StopLike,
        )
        .is_none());
        assert!(explicit_mode_runtime_conflict(
            instance::ModeRequest::ExplicitSystem,
            user_running,
            instance::CommandIntent::UninstallLike,
        )
        .is_none());
        assert!(explicit_mode_runtime_conflict(
            instance::ModeRequest::ExplicitUser,
            system_running,
            instance::CommandIntent::UninstallLike,
        )
        .is_none());
        assert!(explicit_mode_runtime_conflict(
            instance::ModeRequest::ExplicitSystem,
            both_running,
            instance::CommandIntent::UninstallLike,
        )
        .is_none());
    }

    #[test]
    fn v3_mutual_exclusion_blocks_starting_opposite_runtime() {
        let user_running = instance::ServicePresence {
            system: false,
            user: true,
        };
        let system_running = instance::ServicePresence {
            system: true,
            user: false,
        };
        let both_running = instance::ServicePresence {
            system: true,
            user: true,
        };

        let system_err =
            v3_mutual_exclusion_violation(instance::InstanceMode::System, user_running, "start")
                .expect("system start should be blocked while user runtime is active");
        assert!(system_err.contains("per-user core is running"));
        assert!(system_err.contains("mihomo-cli stop"));
        assert!(!system_err.contains("mihomo-cli stop --user"));

        let user_err =
            v3_mutual_exclusion_violation(instance::InstanceMode::User, system_running, "start")
                .expect("user start should be blocked while system runtime is active");
        assert!(user_err.contains("system daemon is running"));
        assert!(user_err.contains("mihomo-cli stop --system"));

        let both_start_err =
            v3_mutual_exclusion_violation(instance::InstanceMode::System, both_running, "start")
                .expect("start should be blocked while both runtimes are active");
        assert!(both_start_err.contains("both system daemon and per-user core are running"));
        assert!(both_start_err.contains("mihomo-cli stop --system"));

        assert!(v3_mutual_exclusion_violation(
            instance::InstanceMode::System,
            both_running,
            "stop",
        )
        .is_none());
    }

    #[test]
    fn rule_and_dns_nested_subcommands_parse() {
        match parse(&[
            "rule",
            "add",
            "DOMAIN-SUFFIX,example.com,DIRECT",
            "--position",
            "front",
        ])
        .command
        {
            Some(Command::Rule {
                action:
                    RuleAction::Add {
                        rule,
                        position: Some(position),
                    },
                ..
            }) => {
                assert_eq!(rule, "DOMAIN-SUFFIX,example.com,DIRECT");
                assert_eq!(position, "front");
            }
            _ => panic!("expected rule add"),
        }

        match parse(&[
            "dns",
            "template",
            "apply",
            "company",
            "--domain",
            "corp.example",
            "--target",
            "10.0.0.1",
        ])
        .command
        {
            Some(Command::Dns {
                action:
                    DnsAction::Template {
                        action:
                            Some(DnsTemplateAction::Apply {
                                name,
                                domain: Some(domain),
                                target: Some(target),
                            }),
                    },
                ..
            }) => {
                assert_eq!(name, "company");
                assert_eq!(domain, "corp.example");
                assert_eq!(target, "10.0.0.1");
            }
            _ => panic!("expected dns template apply"),
        }
    }
}

// ── Gate 5: Resolution source 测试 ──────────────────────────────────

#[cfg(test)]
mod g5_resolution_source_tests {
    use super::*;
    use crate::instance::{ModeRequest, ServicePresence};

    fn presence(system: bool, user: bool) -> ServicePresence {
        ServicePresence { system, user }
    }

    fn env(runtime_sys: bool, runtime_usr: bool, installed_sys: bool, installed_usr: bool) -> EnvironmentState {
        EnvironmentState {
            runtime: presence(runtime_sys, runtime_usr),
            installed: presence(installed_sys, installed_usr),
            legacy_root: None,
        }
    }

    #[test]
    fn g5_runtime_user_only_resolves_to_user() {
        // 仅 user socket 存活 → user 模式
        let result = resolve_environment_for_intent(
            ModeRequest::Unspecified,
            &env(false, true, false, true),
            UserIntent::ApiRead,
        );
        match result {
            RuntimeFirstModeResolution::Resolved { mode, source } => {
                assert_eq!(mode, instance::InstanceMode::User);
                assert_eq!(source, instance::ResolutionSource::RuntimePresence);
            }
            other => panic!("expected Resolved(User), got {:?}", other),
        }
    }

    #[test]
    fn g5_runtime_system_only_resolves_to_system() {
        // 仅 system daemon 运行 → system 模式
        let result = resolve_environment_for_intent(
            ModeRequest::Unspecified,
            &env(true, false, true, false),
            UserIntent::ApiRead,
        );
        match result {
            RuntimeFirstModeResolution::Resolved { mode, source } => {
                assert_eq!(mode, instance::InstanceMode::System);
                assert_eq!(source, instance::ResolutionSource::RuntimePresence);
            }
            other => panic!("expected Resolved(System), got {:?}", other),
        }
    }

    #[test]
    fn g5_both_runtime_conflict_errors() {
        // 两者都运行 → 报错（互斥冲突）
        let result = resolve_environment_for_intent(
            ModeRequest::Unspecified,
            &env(true, true, true, true),
            UserIntent::ApiRead,
        );
        assert_eq!(result, RuntimeFirstModeResolution::RuntimeConflict);
    }

    #[test]
    fn g5_nothing_installed_errors() {
        // 两者都不存在 → NotInstalled
        let result = resolve_environment_for_intent(
            ModeRequest::Unspecified,
            &env(false, false, false, false),
            UserIntent::ApiRead,
        );
        assert_eq!(result, RuntimeFirstModeResolution::NotInstalled);
    }

    #[test]
    fn g5_system_installed_not_running_resolves_to_system() {
        // 仅 system service 已装（未运行）→ system 模式
        let result = resolve_environment_for_intent(
            ModeRequest::Unspecified,
            &env(false, false, true, false),
            UserIntent::Start,
        );
        match result {
            RuntimeFirstModeResolution::Resolved { mode, .. } => {
                assert_eq!(mode, instance::InstanceMode::System);
            }
            other => panic!("expected Resolved(System), got {:?}", other),
        }
    }

    #[test]
    fn g5_explicit_system_overrides_auto_detection() {
        // --system 显式指定 → system 模式（覆盖自动检测）
        let result = resolve_environment_for_intent(
            ModeRequest::ExplicitSystem,
            &env(false, true, false, true), // user 在跑
            UserIntent::ApiRead,
        );
        match result {
            RuntimeFirstModeResolution::Resolved { mode, .. } => {
                assert_eq!(mode, instance::InstanceMode::System);
            }
            other => panic!("expected Resolved(System) with explicit flag, got {:?}", other),
        }
    }

    #[test]
    fn g5_explicit_user_overrides_auto_detection() {
        // --user 显式指定 → user 模式（覆盖自动检测）
        let result = resolve_environment_for_intent(
            ModeRequest::ExplicitUser,
            &env(true, false, true, false), // system 在跑
            UserIntent::ApiRead,
        );
        match result {
            RuntimeFirstModeResolution::Resolved { mode, .. } => {
                assert_eq!(mode, instance::InstanceMode::User);
            }
            other => panic!("expected Resolved(User) with explicit flag, got {:?}", other),
        }
    }

    #[test]
    fn g5_both_installed_but_not_running_errors() {
        // 两者都装了但都没跑 → AmbiguousBothInstalled
        let result = resolve_environment_for_intent(
            ModeRequest::Unspecified,
            &env(false, false, true, true),
            UserIntent::ApiRead,
        );
        assert_eq!(result, RuntimeFirstModeResolution::AmbiguousBothInstalled);
    }
}
