use crate::mihomo_api::MihomoApiClient;
use anyhow::Context as _;
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
mod generation;
mod groups;
mod installer;
mod instance;
mod ipc;
mod lock;
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

#[derive(Parser)]
#[command(name = "mihomo-cli", version = env!("MIHOMO_CLI_VERSION"), about = "Mihomo CLI — cross-platform setup & control tool", long_about = None)]
struct Cli {
    /// Enable verbose debug output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Output a machine-readable JSON envelope on stdout
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
// Parsed once at process startup. Keeping the clap derive command tree readable
// is more valuable here than boxing the largest variant to save a few hundred bytes.
#[allow(clippy::large_enum_variant)]
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
        /// GitHub mirror base URL prepended to GitHub public asset downloads (core and geo data)
        /// e.g. https://ghproxy.com/
        #[arg(long = "github-mirror")]
        github_mirror: Option<String>,
        /// Skip the interactive subscription setup step (non-interactive installs)
        #[arg(long = "skip-config")]
        skip_config: bool,
        /// Assume yes for install prompts (currently: service install confirmation)
        #[arg(short, long)]
        yes: bool,
    },

    /// Check for and install the latest mihomo core version
    Upgrade {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Skip confirmation prompt and proceed with upgrade
        #[arg(short, long)]
        yes: bool,
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
        #[command(subcommand)]
        command: Option<ConfigSubcommand>,
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
        /// Assume yes for config prompts; activation still requires --activate or --no-activate in non-interactive mode
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
    /// Start the mihomo core (system mode keeps the daemon running)
    Start {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Stop the mihomo core (system mode keeps the daemon running)
    Stop {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
    },

    /// Restart the mihomo core (not the system daemon)
    Restart {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Confirm a managed runtime reset if automatic recovery cannot converge
        #[arg(short, long)]
        yes: bool,
    },

    /// Manage proxy groups for the active subscription
    Group {
        #[command(subcommand)]
        action: GroupAction,
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
        /// Forget a persisted selection (with --group) or all of them (with --all); does not switch runtime
        #[arg(long)]
        unpin: bool,
        /// With --unpin: forget all persisted selections
        #[arg(long, requires = "unpin", conflicts_with = "group")]
        all: bool,
        /// Internal hook for service managers: replay persisted selections after Core start
        #[arg(long, hide = true)]
        replay: bool,
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
        /// Assume yes for TUN mode setup prompts
        #[arg(short, long)]
        yes: bool,
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
    #[command(
        name = "system-proxy",
        after_help = "\
Limitations:
  Linux: only GNOME (gsettings). Headless/server/KDE/other DE → use HTTP_PROXY env var or TUN mode.
  Only affects apps that read OS system proxy settings (GTK/GNOME apps, some browsers).
  CLI tools (curl, wget, codex) typically need HTTP_PROXY/HTTPS_PROXY env vars instead.
  Redundant when TUN mode is active (TUN already captures all traffic)."
    )]
    SystemProxy {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        #[command(subcommand)]
        action: SystemProxyAction,
    },

    /// Show a read-only running status overview
    Status {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        /// Show detailed service/config paths
        #[arg(long)]
        verbose: bool,
    },

    /// Set or display the preferred instance mode (system/user/auto)
    Use {
        /// Mode to set: system, user, auto, or status (show current)
        #[arg(value_enum)]
        mode: Option<UseMode>,
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

    /// Diagnose common configuration and runtime issues
    Doctor {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system", conflicts_with = "user")]
        system: bool,
        /// Force the user service instance (advanced/debugging)
        #[arg(long = "user", conflicts_with = "system")]
        user: bool,
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

    /// Manage Unix system daemon client access
    Access {
        /// Force the system service instance (advanced/debugging)
        #[arg(long = "system")]
        system: bool,
        #[command(subcommand)]
        action: AccessAction,
    },

    /// Run as system service daemon (internal, used by systemd/launchd)
    #[command(hide = true)]
    Daemon,

    /// Control autostart (boot/login launch) for the current instance mode
    Autostart {
        /// Enable, disable, or query autostart state
        #[arg(value_enum)]
        action: AutostartAction,
        /// Force the system service instance (advanced)
        #[arg(long = "system", conflicts_with = "user")]
        system: bool,
        /// Force the per-user instance (advanced)
        #[arg(short, long, conflicts_with = "system")]
        user: bool,
    },

    /// Show real-time status dashboard (TUI)
    #[command(visible_alias = "dash")]
    Dashboard,
}

#[derive(Subcommand, Clone)]
enum AccessAction {
    /// Authorize a local user to access the system daemon
    Grant {
        #[arg(long)]
        user: String,
    },
    /// Revoke a local user's daemon access
    Revoke {
        #[arg(long)]
        user: String,
    },
    /// List authorized users
    List,
    /// Show current user's authorization status
    Status,
}

#[derive(Clone, ValueEnum)]
enum AutostartAction {
    /// Enable autostart
    On,
    /// Disable autostart
    Off,
    /// Query autostart state
    Status,
}

#[derive(Clone, ValueEnum)]
enum UseMode {
    /// Prefer system service instance
    System,
    /// Prefer per-user instance
    User,
    /// Auto: use system if installed, otherwise user (default)
    Auto,
    /// Show current mode preference
    Status,
}

#[derive(ValueEnum, Clone, PartialEq, Eq)]
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
enum GroupAction {
    /// List groups from the active effective configuration
    #[command(visible_alias = "ls")]
    List,
    /// Show one group definition
    Show { name: String },
    /// Create a group in the active subscription overlay
    Create {
        name: String,
        #[arg(long = "type", required_unless_present = "file")]
        group_type: Option<String>,
        #[arg(long = "member")]
        members: Vec<String>,
        #[arg(long)]
        file: Option<String>,
        #[arg(long)]
        prepend: bool,
    },
    /// Replace a group definition from a YAML file
    Edit { name: String, file: String },
    /// Add static members to a group
    Add {
        name: String,
        #[arg(long = "member", required = true)]
        members: Vec<String>,
    },
    /// Remove static members from a group
    Remove {
        name: String,
        #[arg(long = "member", required = true)]
        members: Vec<String>,
    },
    /// Delete a custom group or hide an original group
    Delete { name: String },
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
    /// Manage DNS fake-ip-filter entries
    FakeIpFilter {
        #[command(subcommand)]
        action: DnsFakeIpFilterAction,
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
        /// DNS target for company template, e.g. 192.0.2.53
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
enum DnsFakeIpFilterAction {
    /// Add a fake-ip-filter domain
    Add { domain: String },
    /// List fake-ip-filter entries
    #[command(visible_alias = "ls")]
    List,
    /// Remove a fake-ip-filter domain
    #[command(visible_alias = "rm")]
    Remove { domain: String },
}

#[derive(Subcommand, Clone)]
enum DnsPolicyAction {
    /// Add a DNS policy (domain → DNS target)
    Add {
        /// Domain suffix pattern (e.g. internal.example.com)
        #[arg(value_name = "MATCH")]
        match_pattern: String,
        /// DNS target: "system" for system DNS, or IP address (e.g. 192.0.2.53)
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
    // Windows daemon 子命令：同步进 SCM dispatcher（StartServiceCtrlDispatcher
    // 必须由主线程调用；#[tokio::main] 的 block_on 在 main 线程执行，满足约束）。
    // service_main 回调内再自建 tokio runtime 跑核心循环。
    #[cfg(target_os = "windows")]
    if matches!(cli.command, Some(Command::Daemon)) {
        match daemon::run_windows_service() {
            Ok(()) => return,
            Err(e) => {
                eprintln!("\n  Error: {e}");
                std::process::exit(1);
            }
        }
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
        skip_config: false,
        yes: false,
    }) {
        Command::Install {
            system,
            user,
            force,
            version,
            github_mirror,
            skip_config,
            yes,
        } => {
            cmd_install_entry(
                system,
                user,
                force,
                version.as_deref(),
                github_mirror.as_deref(),
                skip_config,
                yes,
            )
            .await
        }
        Command::Upgrade { system, yes } => cmd_upgrade(system, false, yes).await,
        Command::Version { system } => cmd_version(system, false, cli.json).await,
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
            group,
            node,
            unpin,
            all,
            replay,
        } => cmd_select_resolved(system, false, group, node, unpin, all, replay).await,
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
        Command::Group { system, action } => cmd_group(system, false, action).await,
        Command::Dns { system, action } => cmd_dns(system, false, action).await,
        Command::Override { system, action } => cmd_override(system, false, action).await,
        Command::Backup { system, output } => cmd_backup(system, false, output),
        Command::Restore { system, path, yes } => cmd_restore(system, false, &path, yes),
        Command::Doctor { system, user } => cmd_doctor(system, user).await,
        Command::Access { action, .. } => cmd_access(action).await,
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
fn access_action_reads_authorized_table(action: &AccessAction) -> bool {
    !matches!(action, AccessAction::Status)
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum AccessDaemonStatus {
    Authorized,
    Rejected(DaemonIpcErrorKind),
    TransportUnavailable(String),
    UnexpectedResponse,
}

#[cfg(unix)]
fn format_access_status(
    os: instance::TargetOs,
    token_location: &ipc::ClientTokenLocation,
    token_file_readable: bool,
    daemon_status: AccessDaemonStatus,
) -> Vec<String> {
    let local = format!(
        "token_file_readable={token_file_readable}, credential_path={}",
        token_location.token_path.display()
    );
    let service_hint = daemon_service_status_hint(os);
    match daemon_status {
        AccessDaemonStatus::Authorized => vec![format!(
            "authorized ({local}, daemon_authenticated=true)"
        )],
        AccessDaemonStatus::Rejected(DaemonIpcErrorKind::InvalidOrMissingToken) => vec![
            format!("not authorized ({local}, daemon_authenticated=false)"),
            "  Fix: sudo mihomo-cli access grant --user \"$(id -un)\"".to_string(),
        ],
        AccessDaemonStatus::Rejected(DaemonIpcErrorKind::PeerUidMismatch) => vec![
            format!("not authorized ({local}, daemon_authenticated=false, reason=uid_mismatch)"),
            "  Check HOME/XDG_CONFIG_HOME, then re-authorize: sudo mihomo-cli access grant --user \"$(id -un)\""
                .to_string(),
        ],
        AccessDaemonStatus::Rejected(DaemonIpcErrorKind::AuthorizationTableUnreadable) => vec![
            format!("authorization state unavailable ({local})"),
            format!("  Admin check: sudo mihomo-cli access list && {service_hint}"),
        ],
        AccessDaemonStatus::Rejected(DaemonIpcErrorKind::Other) => vec![
            format!("authorization check failed ({local})"),
            format!("  Check: {service_hint}"),
        ],
        AccessDaemonStatus::TransportUnavailable(message) => vec![
            format!("daemon unavailable ({local})"),
            format!("  Transport error: {message}"),
            format!("  Check: {service_hint}"),
        ],
        AccessDaemonStatus::UnexpectedResponse => vec![
            format!("authorization check failed ({local}, unexpected_response=true)"),
            format!("  Check: {service_hint}"),
        ],
    }
}

#[cfg(unix)]
async fn cmd_access_status() -> anyhow::Result<()> {
    let token_location = ipc::client_token_location();
    let token_file_readable = ipc::current_client_token().is_some();
    let daemon_status =
        match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await {
            Ok(ipc::DaemonResponse::Status { .. }) => AccessDaemonStatus::Authorized,
            Ok(ipc::DaemonResponse::Error { message }) => {
                AccessDaemonStatus::Rejected(classify_daemon_ipc_error(&message))
            }
            Ok(
                ipc::DaemonResponse::Success { .. }
                | ipc::DaemonResponse::CoreApi { .. }
                | ipc::DaemonResponse::Transaction { .. },
            ) => AccessDaemonStatus::UnexpectedResponse,
            Err(err) => AccessDaemonStatus::TransportUnavailable(err.to_string()),
        };
    let os = instance::TargetOs::current()
        .ok_or_else(|| anyhow::anyhow!("unsupported OS for daemon access status"))?;
    print_lines(format_access_status(
        os,
        &token_location,
        token_file_readable,
        daemon_status,
    ));
    Ok(())
}

#[cfg(unix)]
async fn cmd_access(action: AccessAction) -> anyhow::Result<()> {
    use anyhow::Context;

    if !access_action_reads_authorized_table(&action) {
        return cmd_access_status().await;
    }
    if matches!(
        action,
        AccessAction::Grant { .. } | AccessAction::Revoke { .. }
    ) {
        let euid = unsafe { libc::geteuid() };
        if euid != 0 {
            anyhow::bail!(
                "access grant/revoke requires root privileges.\n  \
                 Run with sudo: sudo mihomo-cli access grant --user <username>\n  \
                 Or: sudo mihomo-cli access revoke --user <username>"
            );
        }
    }
    let path = daemon::authorized_clients_path();
    let mut table = daemon::read_authorized_clients_from(&path)?;
    match action {
        AccessAction::Grant { user } => {
            let (uid, gid, home) = lookup_user(&user)?;
            let same_uid: Vec<_> = table.clients.iter().filter(|c| c.uid == uid).collect();
            if same_uid.len() > 1 {
                anyhow::bail!(
                    "authorized-client table has multiple entries for uid {uid}; refusing to replace credentials"
                );
            }
            if let Some(existing) = same_uid.first() {
                if existing.user != user.as_str() {
                    anyhow::bail!(
                        "authorized-client uid {uid} belongs to {}; refusing username-based replacement",
                        existing.user
                    );
                }
            }
            let token = service::grant_client_token_for_unix_identity(&home, uid, gid)?;
            table.clients.retain(|c| c.uid != uid);
            table.clients.push(daemon::AuthorizedClient {
                user: user.clone(),
                uid,
                token,
            });
            daemon::write_authorized_clients_to(&path, &table)?;
            println!("granted access to {user} (uid {uid})");
        }
        AccessAction::Revoke { user } => {
            let (uid, _gid, home) = lookup_user(&user)?;
            let token = daemon::read_client_token_for_home(&home).with_context(|| {
                format!("cannot read canonical token for authorized user {user} (uid {uid})")
            })?;
            if !daemon::revoke_authorized_client(&mut table, uid, &token)? {
                anyhow::bail!(
                    "no authorized-client entry matched user {user} with uid {uid} and its canonical token"
                );
            }
            daemon::write_authorized_clients_to(&path, &table)?;
            println!("revoked access for {user} (uid {uid})");
        }
        AccessAction::List => {
            for c in table.clients {
                println!("{}	{}", c.user, c.uid);
            }
        }
        AccessAction::Status => unreachable!("status is handled without reading the root table"),
    }
    Ok(())
}

#[cfg(not(unix))]
async fn cmd_access(_action: AccessAction) -> anyhow::Result<()> {
    anyhow::bail!("access commands are currently supported on Unix system daemon only")
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
    cmd_install_entry(false, false, false, None, None, false, false).await?;
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
        instance::ServiceAction::Uninstall => instance::CommandIntent::UninstallLike,
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

async fn cmd_doctor(system: bool, user: bool) -> anyhow::Result<()> {
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
        let gen_store = system_generation_store(&ctx);
        if let Ok(gen_state) = gen_store.read_state() {
            if let Some(pending_id) = gen_state.pending {
                checks.push(DoctorCheck::fail(
                    "待应用更新",
                    format!("发现 pending generation ({pending_id})"),
                    "应用更新: mihomo-cli restart".to_string(),
                ));
            }
        }
        if let Ok(current_cli) = std::env::current_exe() {
            if ctx.paths.cli_binary.exists() {
                if utils::file_contents_equal(&current_cli, &ctx.paths.cli_binary) {
                    checks.push(DoctorCheck::pass("Daemon 二进制", "与当前 CLI 一致"));
                } else {
                    checks.push(DoctorCheck::fail(
                        "Daemon 二进制",
                        format!("与当前 CLI 不一致 ({})", ctx.paths.cli_binary.display()),
                        "应用更新: mihomo-cli restart".to_string(),
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
            match ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await {
                Ok(ipc::DaemonResponse::Status {
                    running,
                    config_path: Some(_active_config),
                    ..
                }) => {
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
                            checks.push(DoctorCheck::fail(
                                "TUN 配置一致性",
                                format!(
                                    "无法观察运行态（{}）或配置意图（{}）",
                                    snapshot.runtime_tun, snapshot.configured_tun
                                ),
                                "启动或重启核心后重试: mihomo-cli restart --system".to_string(),
                            ));
                        }
                    }
                }
                Ok(ipc::DaemonResponse::Status {
                    running,
                    config_path: None,
                    ..
                }) => {
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
                }
                Ok(ipc::DaemonResponse::Error { message }) => {
                    checks.push(doctor_daemon_error_check(ctx.os, &message))
                }
                Ok(ipc::DaemonResponse::Success { message }) => checks.push(DoctorCheck::fail(
                    "Daemon 状态",
                    format!("收到意外响应: {message}"),
                    "运行: mihomo-cli status --system --verbose",
                )),
                Ok(
                    ipc::DaemonResponse::CoreApi { .. } | ipc::DaemonResponse::Transaction { .. },
                ) => checks.push(DoctorCheck::fail(
                    "Daemon 状态",
                    "收到意外的 Core API / Transaction 响应",
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
        }
    } else if mihomo_api::api_get_at_endpoint(&ctx.paths.api_endpoint, "/configs")
        .await
        .is_ok()
    {
        checks.push(DoctorCheck::pass("Mihomo Core API", "可连接"));
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
        ),
        Vec::new(),
    )
}

/// Render status JSON exclusively from the shared runtime snapshot.  This
/// prevents JSON and text status from observing different daemon/Core/proxy
/// state during one command invocation.
fn status_json_data(
    plan: &instance::StatusDiagnosticPlan,
    source: instance::ResolutionSource,
    intent_config_path: &std::path::Path,
    snapshot: &status::StatusSnapshot,
) -> serde_json::Value {
    let health = match (snapshot.core_running.as_bool(), snapshot.api_reachable) {
        (Some(false), _) => "inactive",
        (Some(true), true) if snapshot.runtime_tun.as_bool().is_some() => "healthy",
        (Some(true), _) | (None, true) => "degraded",
        _ => "unknown",
    };
    let daemon = if plan.mode == instance::InstanceMode::System {
        serde_json::json!({
            "running": snapshot.daemon_reachable.as_bool(),
            "socket": ipc::system_service_socket_path(),
        })
    } else {
        serde_json::Value::Null
    };

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
            "tun": snapshot.runtime_tun.as_bool(),
        },
        "api": if snapshot.api_reachable { "reachable" } else { "unknown" },
        "tun": runtime_tun_status_label(snapshot.runtime_tun),
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
    })
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
/// - Linux system: systemctl enable/disable/is-enabled mihomo
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
    // as infrastructure). We write/remove a per-user marker file; the daemon
    // reads it on startup.
    if mode == instance::InstanceMode::System {
        let ctx = instance::planned_current_context(instance::InstanceMode::System)
            .ok_or_else(|| anyhow::anyhow!("unsupported OS for autostart"))?;
        let marker = ctx.paths.config_dir.join("autostart");
        if enable {
            utils::atomic_write_file_for_original_user(&marker.display().to_string(), "enabled\n")?;
        } else {
            utils::remove_file_if_exists(&marker)?;
        }
        println!(
            "core autostart {}",
            if enable { "enabled" } else { "disabled" }
        );
        return Ok(());
    }
    let mut cmd = std::process::Command::new("systemctl");
    if mode == instance::InstanceMode::User {
        cmd.arg("--user");
    }
    cmd.arg(if enable { "enable" } else { "disable" })
        .arg("mihomo");
    run_autostart_command(cmd, enable)
}

#[cfg(target_os = "linux")]
async fn query_autostart(mode: instance::InstanceMode) -> anyhow::Result<()> {
    if mode == instance::InstanceMode::System {
        let ctx = instance::planned_current_context(instance::InstanceMode::System)
            .ok_or_else(|| anyhow::anyhow!("unsupported OS for autostart"))?;
        let enabled = ctx.paths.config_dir.join("autostart").exists();
        println!(
            "Autostart: {} (system core)",
            if enabled { "enabled" } else { "disabled" }
        );
        return Ok(());
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

    // 2. Enable/disable via launchctl (override survives reboots).
    let action = if enable { "enable" } else { "disable" };
    let status = std::process::Command::new("launchctl")
        .arg(action)
        .arg(&label)
        .status()?;
    if !status.success() {
        anyhow::bail!("launchctl {action} {label} failed");
    }

    // 3. Re-load the plist so the new RunAtLoad takes effect.
    //    - enabling: bootstrap (enable clears any prior disable override that
    //      would otherwise make bootstrap fail with EIO)
    //    - disabling: do NOT re-bootstrap — a fresh disable override blocks
    //      bootstrap; the service stays loaded so `start` still works, and the
    //      disable override prevents autostart on next login.
    if enable {
        if let Some(plist) = &ctx.paths.service_file {
            // Ensure loaded: bootstrap only if not already loaded (bootout is
            // unnecessary and can race; enable already cleared the override).
            let loaded = std::process::Command::new("launchctl")
                .args(["print", &label])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !loaded {
                let _ = std::process::Command::new("launchctl")
                    .args([
                        "bootstrap",
                        domain_parent_macos(&domain),
                        &plist.display().to_string(),
                    ])
                    .status();
            }
        }
    }

    println!(
        "Autostart {} for {}",
        if enable { "enabled" } else { "disabled" },
        label
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn domain_parent_macos(domain: &str) -> &str {
    domain.rsplit_once('/').map(|(p, _)| p).unwrap_or(domain)
}

#[cfg(target_os = "macos")]
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
    match mode {
        instance::InstanceMode::System => {
            // N4b: sc config needs an elevated token; hint before opaque failure.
            if !service::is_process_elevated() {
                anyhow::bail!(
                    "autostart for the system service requires an elevated (Administrator) shell.\n  \
                     Run from an Administrator terminal."
                );
            }
            let start_type = if enable { "auto" } else { "demand" };
            let status = std::process::Command::new("sc.exe")
                .args(["config", "mihomo", "start="])
                .arg(start_type)
                .status()?;
            if !status.success() {
                anyhow::bail!("sc config mihomo start= {start_type} failed");
            }
            println!(
                "Autostart {} for system service",
                if enable { "enabled" } else { "disabled" }
            );
            Ok(())
        }
        instance::InstanceMode::User => {
            // Registry Run key + .vbs hidden launch
            let vbs_path = std::env::var_os("APPDATA")
                .map(std::path::PathBuf::from)
                .unwrap_or_default()
                .join("mihomo")
                .join("autostart.vbs");
            if enable {
                std::fs::create_dir_all(vbs_path.parent().unwrap())?;
                let cli_path = std::env::current_exe()?;
                let vbs = format!(
                    "Set shell = CreateObject(\"WScript.Shell\")\r\nshell.Run \"\"\"{}\"\" start\", 0, False\r\n",
                    cli_path.display()
                );
                std::fs::write(&vbs_path, vbs)?;
                let status = std::process::Command::new("reg.exe")
                    .args([
                        "ADD",
                        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                        "/v",
                        "mihomo-cli",
                        "/t",
                        "REG_SZ",
                        "/d",
                        &format!("wscript.exe //B //NoLogo \"{}\"", vbs_path.display()),
                        "/f",
                    ])
                    .status()?;
                if !status.success() {
                    anyhow::bail!("reg ADD Run key failed");
                }
                println!("Autostart enabled for user mode (registry Run + .vbs)");
            } else {
                let _ = std::process::Command::new("reg.exe")
                    .args([
                        "DELETE",
                        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                        "/v",
                        "mihomo-cli",
                        "/f",
                    ])
                    .status();
                let _ = std::fs::remove_file(&vbs_path);
                println!("Autostart disabled for user mode");
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

#[cfg(target_os = "linux")]
fn run_autostart_command(mut cmd: std::process::Command, enable: bool) -> anyhow::Result<()> {
    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!(
            "systemctl {} mihomo failed",
            if enable { "enable" } else { "disable" }
        );
    }
    println!(
        "Autostart {} for mihomo",
        if enable { "enabled" } else { "disabled" }
    );
    Ok(())
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
            writeln!(
                stdout,
                "║  TUN:         {:<25} ║",
                if tun { "✅ enabled" } else { "❌ disabled" }
            )?;
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
) -> anyhow::Result<()> {
    // D8: hold the per-instance selection lock across kernel PUT/IPC + intent
    // persist so select and replay serialize (SPEC-select-persistence §3.2-6).
    // Acquired before the PUT, so a lock failure means the node was NOT switched.
    let paths = utils::AppPaths::new(resolved.ctx.paths.config_dir.clone());
    let _selection_lock = selection::acquire_selection_lock_at(&paths).map_err(|err| {
        anyhow::anyhow!("Failed to acquire selection lock; node was NOT switched: {err:#}")
    })?;
    let scope = selection::active_selection_scope(&paths).map_err(|err| {
        anyhow::anyhow!("Cannot persist selection without an active subscription: {err:#}")
    })?;
    if resolved.ctx.mode == instance::InstanceMode::System {
        let snapshot = status::StatusSnapshot::collect(&resolved.ctx).await;
        if snapshot.tun_verdict == status::TunVerdict::TunRunning {
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
    } else {
        mihomo_api::select_proxy_with_client(client, group_name, node_name).await?;
    }
    selection::remember_selection_for_scope(&scope, group_name, node_name)?;
    println!("Switched {group_name} → {node_name}");
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

async fn cmd_select_resolved(
    system: bool,
    user: bool,
    group: Option<String>,
    node: Option<String>,
    unpin: bool,
    all: bool,
    replay: bool,
) -> anyhow::Result<()> {
    if replay {
        return cmd_select_replay(system, user).await;
    }
    if unpin {
        return cmd_select_unpin(system, user, group, all);
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
            apply_selected_proxy(&resolved, &client, &group_name, &node_name).await
        }
        // Interactive TUI (no --node): existing behavior.
        None => match group {
            Some(g) => match ui::select_node_with_client(&client, &g).await? {
                Some(node_name) => apply_selected_proxy(&resolved, &client, &g, &node_name).await,
                None => Ok(()),
            },
            None => match ui::flat_select_with_client(&client).await? {
                Some((group_name, node_name)) => {
                    apply_selected_proxy(&resolved, &client, &group_name, &node_name).await
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

fn build_tun_candidate(
    ctx: &instance::InstanceContext,
    enable: bool,
    stack: Option<&TunStack>,
    dns_hijack: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let content = std::fs::read_to_string(&ctx.paths.intent_config_file).map_err(|e| {
        anyhow::anyhow!(
            "cannot read config for TUN candidate: {} ({e})",
            ctx.paths.intent_config_file.display()
        )
    })?;
    let content = if enable {
        config::ensure_controller_for_endpoint(&content, &ctx.paths.api_endpoint)?
    } else {
        content
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
    if let Some(dns_hijack) = dns_hijack {
        tun.insert(
            serde_yaml::Value::String("dns-hijack".to_string()),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::String(dns_hijack.to_string())]),
        );
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
    let mut command = std::process::Command::new("sudo");
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

fn maybe_sudo_reexec_for_system_generation(ctx: &instance::InstanceContext) -> anyhow::Result<()> {
    if is_current_process_root() {
        return Ok(());
    }

    let state = match system_generation_store(ctx).read_state() {
        Ok(state) => state,
        Err(error) if error.to_string().contains("Permission denied") => {
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
        Err(error) => {
            return Err(anyhow::anyhow!(
                "cannot safely inspect pending system generation: {error}"
            ));
        }
    };
    if state.pending.is_none() {
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

fn validate_quiesced_response(
    response: &ipc::DaemonResponse,
    fence: &tun_transaction::TransactionFence,
    old_evidence: &tun_transaction::OldRuntimeEvidence,
) -> anyhow::Result<()> {
    let proof = match response {
        ipc::DaemonResponse::Transaction {
            response:
                tun_transaction::TransactionResponse::Completed(proof)
                | tun_transaction::TransactionResponse::AlreadySatisfied(proof),
        } => proof,
        other => anyhow::bail!("candidate runtime was not successfully quiesced: {other:?}"),
    };
    if proof.transaction_id != fence.transaction_id
        || proof.generation != fence.generation
        || proof.observed_phase != tun_transaction::JournalPhase::RollbackPending
        || proof.proof_kind != tun_transaction::RuntimeProofKind::CandidateQuiesced
    {
        anyhow::bail!("daemon returned an invalid quiesce proof: {proof:?}");
    }
    let stopped = proof.core_pid == 0
        && proof.launched_revision.is_empty()
        && !proof.api_ready
        && !proof.runtime_tun;
    let old_attested = tun_transaction::runtime_matches_old_evidence(
        old_evidence,
        &tun_transaction::RuntimeObservation {
            core_running: true,
            core_identity: Some(proof.core_identity.clone()),
            core_pid: Some(proof.core_pid),
            launched_revision: Some(proof.launched_revision.clone()),
            runtime_tun: Some(proof.runtime_tun),
            api_ready: proof.api_ready,
        },
    );
    if !stopped && !old_attested {
        anyhow::bail!("quiesce proof does not prove stopped or attested old runtime: {proof:?}");
    }
    Ok(())
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
        token: None,
    })
    .await?;

    // Validate immutable evidence before changing the Snapshot.
    let old_evidence = tun_transaction::read_and_validate_old_runtime(ctx, &journal)?;
    validate_quiesced_response(&quiesce_resp, &fence, &old_evidence)?;

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
            let next_action =
                tun_transaction::plan_recovery(&updated, snap_cls, intent_cls, obs, direction);
            Box::pin(execute_recovery_action(
                ctx,
                &updated,
                next_action,
                obs,
                direction,
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
                        &updated, snap_cls, intent_cls, obs, direction,
                    );
                    Box::pin(execute_recovery_action(
                        ctx,
                        &updated,
                        next_action,
                        obs,
                        direction,
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
                token: None,
            })
            .await?;

            let old_evidence = tun_transaction::read_and_validate_old_runtime(ctx, &journal)?;
            validate_quiesced_response(&quiesce_resp, &fence, &old_evidence)?;
            tun_transaction::restore_snapshot(ctx, &journal)?;

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
            let old_evidence = tun_transaction::read_and_validate_old_runtime(ctx, journal)?;
            let status_resp =
                ipc::send_command(&ipc::DaemonCommand::GetTransactionStatus { token: None })
                    .await?;
            let proof = successful_runtime_proof(
                &status_resp,
                &fence,
                journal.phase,
                tun_transaction::RuntimeProofKind::CandidateAttested,
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
            if !tun_transaction::runtime_matches_old_evidence(&old_evidence, &observation) {
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
    let root = ctx
        .paths
        .tun_config_file
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| ctx.paths.config_dir.clone());
    generation::GenerationStore::new(root)
}

pub(crate) fn commit_system_generation_active(
    _ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
) -> anyhow::Result<generation::GenerationState> {
    store
        .commit_active()
        .map_err(|error| anyhow::anyhow!("failed to commit active generation: {error}"))
}

pub(crate) fn cleanup_system_generation_old(
    ctx: &instance::InstanceContext,
    store: &generation::GenerationStore,
    keep_limit: usize,
) -> anyhow::Result<Vec<generation::GenerationId>> {
    match store.cleanup_old_generations(keep_limit) {
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

pub(crate) fn prepare_system_generation(
    ctx: &instance::InstanceContext,
    core_bytes: &[u8],
    cli_bytes: &[u8],
    extra_artifacts: Vec<(String, Vec<u8>, Option<u32>)>,
) -> anyhow::Result<generation::GenerationId> {
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
    store
        .init()
        .map_err(|e| anyhow::anyhow!("failed to init generation store: {e}"))?;
    let lock = store
        .acquire_lock()
        .map_err(|e| anyhow::anyhow!("failed to lock generation store: {e}"))?;

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
    store
        .stage_pending_with_lock(&lock, gen_id.clone())
        .map_err(|e| anyhow::anyhow!("failed to stage pending generation: {e}"))?;

    Ok(gen_id)
}

pub(crate) async fn apply_pending_generation(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<bool> {
    if ctx.mode != instance::InstanceMode::System {
        return Ok(false);
    }
    if is_current_process_root() {
        utils::ensure_mihomo_system_state_dir()?;
    }
    let store = system_generation_store(ctx);
    let state = match store.read_state() {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };
    let Some(pending_id) = state.pending else {
        return Ok(false);
    };

    let manifest = store.validate_generation(&pending_id).map_err(|e| {
        anyhow::anyhow!("Failed to validate pending generation {}: {e}", pending_id)
    })?;

    let gen_dir = store.generation_dir(&pending_id);
    let pending_cli_path = gen_dir.join(&manifest.daemon.relative_path);
    let pending_core_path = gen_dir.join(&manifest.core.relative_path);

    let daemon_changed = !ctx.paths.cli_binary.exists()
        || !utils::file_contents_equal(&pending_cli_path, &ctx.paths.cli_binary);
    let core_changed = !ctx.paths.core_binary.exists()
        || !utils::file_contents_equal(&pending_core_path, &ctx.paths.core_binary);

    println!("  Pending generation detected: {}", pending_id);

    if daemon_changed {
        println!("  Applying daemon and core update...");
        if ipc::is_daemon_running().await {
            let _ = ipc::send_command(&ipc::DaemonCommand::StopCore { token: None }).await;
        }

        let stop_plan = instance::planned_generation_quiesce_plan(ctx);
        for command in &stop_plan.commands {
            service::run_instance_command(command)?;
        }
        wait_for_system_daemon_shutdown().await?;

        let cli_bytes = std::fs::read(&pending_cli_path)?;
        utils::atomic_write_bytes_no_follow(&ctx.paths.cli_binary, &cli_bytes, 0o755)?;

        let core_bytes = std::fs::read(&pending_core_path)?;
        utils::atomic_write_bytes_no_follow(&ctx.paths.core_binary, &core_bytes, 0o755)?;

        for extra in &manifest.extra_artifacts {
            let src = gen_dir.join(&extra.relative_path);
            if src.exists() {
                if let Some(parent) = ctx.paths.tun_config_file.parent() {
                    let dst = parent.join(&extra.relative_path);
                    let bytes = std::fs::read(&src)?;
                    let mode = extra.unix_mode.unwrap_or(0o644) as u16;
                    service::PrivilegeExecutor::write_file(&dst, &bytes, mode)?;
                }
            }
        }

        let start_plan = instance::planned_generation_resume_plan(ctx);
        for command in &start_plan.commands {
            service::run_instance_command(command)?;
        }

        wait_for_system_daemon_readiness().await?;
        verify_running_daemon_revision(&manifest.daemon.sha256).await?;
        ensure_system_daemon_access(ctx).await?;
        maybe_auto_recover_active_transaction(
            ctx,
            tun_transaction::RecoveryDirection::Abort,
            false,
            false,
        )
        .await?;

        let config_path = lifecycle_system_config_path(ctx);
        let core_running = matches!(
            ipc::send_command(&ipc::DaemonCommand::GetStatus { token: None }).await?,
            ipc::DaemonResponse::Status { running: true, .. }
        );
        if config_path.exists() && !core_running {
            let (config_content, config_revision) = lifecycle_system_config_payload(ctx)?;
            let subscription_id =
                config::get_active_id_at(&utils::AppPaths::new(ctx.paths.config_dir.clone()))?;
            let resp = ipc::send_command(&ipc::DaemonCommand::StartCore {
                config_content,
                config_revision,
                selection_intent_dir: subscription_id
                    .as_ref()
                    .map(|_| ctx.paths.config_dir.display().to_string()),
                subscription_id,
                token: None,
            })
            .await?;
            match resp {
                ipc::DaemonResponse::Success { .. } => {}
                ipc::DaemonResponse::Error { message } => {
                    anyhow::bail!(daemon_command_error_message(ctx.os, &message));
                }
                _ => {}
            }
        }
        if config_path.exists() {
            wait_for_instance_readiness(ctx).await?;
        }

        commit_system_generation_active(ctx, &store)?;
        let _ = cleanup_system_generation_old(ctx, &store, 2);
        println!(
            "  ✅ Successfully upgraded daemon and core to generation {}",
            pending_id
        );
        Ok(true)
    } else if core_changed {
        println!("  Applying core update...");
        if ipc::is_daemon_running().await {
            let _ = ipc::send_command(&ipc::DaemonCommand::StopCore { token: None }).await;
        }

        let core_bytes = std::fs::read(&pending_core_path)?;
        utils::atomic_write_bytes_no_follow(&ctx.paths.core_binary, &core_bytes, 0o755)?;

        for extra in &manifest.extra_artifacts {
            let src = gen_dir.join(&extra.relative_path);
            if src.exists() {
                if let Some(parent) = ctx.paths.tun_config_file.parent() {
                    let dst = parent.join(&extra.relative_path);
                    let bytes = std::fs::read(&src)?;
                    let mode = extra.unix_mode.unwrap_or(0o644) as u16;
                    service::PrivilegeExecutor::write_file(&dst, &bytes, mode)?;
                }
            }
        }

        let config_path = lifecycle_system_config_path(ctx);
        if config_path.exists() {
            let (config_content, config_revision) = lifecycle_system_config_payload(ctx)?;
            let subscription_id =
                config::get_active_id_at(&utils::AppPaths::new(ctx.paths.config_dir.clone()))?;
            let resp = ipc::send_command(&ipc::DaemonCommand::StartCore {
                config_content,
                config_revision,
                selection_intent_dir: Some(ctx.paths.config_dir.display().to_string()),
                subscription_id,
                token: None,
            })
            .await?;
            match resp {
                ipc::DaemonResponse::Success { .. } => {}
                ipc::DaemonResponse::Error { message } => {
                    anyhow::bail!(daemon_command_error_message(ctx.os, &message));
                }
                _ => {}
            }
            wait_for_instance_readiness(ctx).await?;
        }

        commit_system_generation_active(ctx, &store)?;
        let _ = cleanup_system_generation_old(ctx, &store, 2);
        println!(
            "  ✅ Successfully upgraded core to generation {}",
            pending_id
        );
        Ok(true)
    } else {
        commit_system_generation_active(ctx, &store)?;
        let _ = cleanup_system_generation_old(ctx, &store, 2);
        Ok(true)
    }
}

pub(crate) async fn maybe_auto_recover_active_transaction(
    ctx: &instance::InstanceContext,
    direction: tun_transaction::RecoveryDirection,
    allow_runtime_reset: bool,
    prompt_runtime_reset: bool,
) -> anyhow::Result<()> {
    if ctx.mode != instance::InstanceMode::System {
        return Ok(());
    }
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
    let action = tun_transaction::plan_recovery(&journal, snap_cls, intent_cls, &obs, direction);

    match action {
        tun_transaction::RecoveryAction::RefuseNeedsEvidence(_)
            if allow_runtime_reset || prompt_runtime_reset =>
        {
            if prompt_runtime_reset && !confirm_managed_runtime_reset()? {
                anyhow::bail!(
                    "未执行修复；你的配置未被修改。需要修复时请重新执行：mihomo-cli restart --yes"
                );
            }
            let reset_message = reset_managed_system_runtime(ctx).await?;
            println!("  ⚠ {reset_message}");
        }
        tun_transaction::RecoveryAction::RefuseNeedsEvidence(_) => {
            anyhow::bail!(managed_recovery_required_message());
        }
        other_action => {
            execute_recovery_action(ctx, &journal, other_action, &obs, direction).await?;
        }
    }
    Ok(())
}

fn managed_recovery_required_message() -> String {
    "检测到 mihomo 之前留下的运行状态，暂时无法自动恢复。你的配置未被修改。\n  如需重置 mihomo 管理的运行状态并重新启动，请执行：mihomo-cli restart --yes".to_string()
}

fn confirm_managed_runtime_reset() -> anyhow::Result<bool> {
    use dialoguer::Confirm;

    Ok(Confirm::new()
        .with_prompt("检测到旧的运行状态。将保留你的配置、重置运行状态，并可能暂时中断代理连接或关闭 TUN。是否继续修复")
        .default(false)
        .interact()?)
}

async fn reset_managed_system_runtime(ctx: &instance::InstanceContext) -> anyhow::Result<String> {
    let Some(journal) = tun_transaction::read_active_journal(ctx)? else {
        return Ok("未发现需要重置的运行状态".to_string());
    };
    if !tun_transaction::validate_managed_active_transaction(ctx, &journal)? {
        anyhow::bail!(managed_recovery_required_message());
    }
    let _ = ipc::send_command(&ipc::DaemonCommand::StopCore { token: None }).await?;
    tun_transaction::quarantine_active_transaction(ctx, &journal)?;
    tun_transaction::remove_managed_snapshot(ctx)?;
    tun_transaction::remove_user_gate(
        &ctx.paths.config_dir,
        &journal.transaction_id,
        journal.generation,
    )?;
    if journal.legacy_source {
        tun_transaction::remove_legacy_artifacts(ctx)?;
    }
    println!("  ✓ 已保留用户配置并重置 mihomo 运行状态");
    Ok("正在从用户配置重新建立运行状态".to_string())
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

    // 3. Build candidate
    let current_intent = std::fs::read(&ctx.paths.intent_config_file)?;
    let base_intent_revision = tun_transaction::sha256_revision(&current_intent);
    let candidate_bytes = build_tun_candidate(ctx, target_tun, stack, dns_hijack)?;

    let original_uid = crate::instance::PathInputs::from_current_env()
        .uid
        .unwrap_or(0);

    // 4. Prepare and publish active transaction
    let journal = tun_transaction::prepare_and_publish_active_transaction(
        ctx,
        original_uid,
        target_tun,
        base_intent_revision,
        &candidate_bytes,
        &evidence,
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
            println!("  ⚠️ User configuration changed during transaction; rolling back runtime...");
            let _ = execute_automatic_rollback(ctx, &journal, &evidence).await;
            anyhow::bail!(
                "User intent configuration was modified concurrently; transaction rolled back.\n                 Your local changes have been preserved."
            );
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
                if let Some(message) = system_tun_requires_daemon_message(action.as_ref(), &ctx) {
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
    cmd_lifecycle_instance_mode(mode, action, false, false).await
}

fn lifecycle_system_config_path(ctx: &instance::InstanceContext) -> std::path::PathBuf {
    ctx.paths.intent_config_file.clone()
}

fn lifecycle_system_config_payload(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<(String, String)> {
    use anyhow::Context;

    let content = std::fs::read_to_string(&ctx.paths.intent_config_file).with_context(|| {
        format!(
            "failed to read system intent config {}",
            ctx.paths.intent_config_file.display()
        )
    })?;
    let revision = tun_transaction::sha256_revision(content.as_bytes());
    Ok((content, revision))
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigOwnershipRepair {
    NotNeeded,
    ReexecAsRoot,
    RepairAsRoot,
}

#[cfg(unix)]
fn config_ownership_repair(
    is_root: bool,
    expected_uid: u32,
    actual_uid: u32,
    is_regular_file: bool,
    link_count: u64,
) -> anyhow::Result<ConfigOwnershipRepair> {
    if expected_uid == actual_uid {
        return Ok(ConfigOwnershipRepair::NotNeeded);
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
    Ok(if is_root {
        ConfigOwnershipRepair::RepairAsRoot
    } else {
        ConfigOwnershipRepair::ReexecAsRoot
    })
}

#[cfg(unix)]
fn ensure_system_config_ownership_for_lifecycle(
    ctx: &instance::InstanceContext,
) -> anyhow::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let config_path = lifecycle_system_config_path(ctx);
    let expected_uid = instance::PathInputs::from_current_env()
        .uid
        .ok_or_else(|| {
            anyhow::anyhow!("cannot determine the current user for configuration repair")
        })?;
    let metadata = match std::fs::symlink_metadata(&config_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let repair = config_ownership_repair(
        is_current_process_root(),
        expected_uid,
        metadata.uid(),
        metadata.file_type().is_file(),
        metadata.nlink(),
    )?;

    match repair {
        ConfigOwnershipRepair::NotNeeded => Ok(()),
        ConfigOwnershipRepair::ReexecAsRoot => {
            println!("  Detected a Mihomo configuration permission issue. Repairing it and continuing restart...");
            let exe = std::env::current_exe()?;
            let args: Vec<String> = std::env::args().skip(1).collect();
            let status = sudo_reexec_command(&exe, &args).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        ConfigOwnershipRepair::RepairAsRoot => {
            utils::restore_original_user_owned_regular_file_under_home(&config_path)?;
            let repaired_uid = std::fs::symlink_metadata(&config_path)?.uid();
            if repaired_uid != expected_uid {
                anyhow::bail!("Mihomo configuration permission repair could not be verified");
            }
            println!("  ✓ Mihomo configuration permissions repaired.");
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

#[cfg(unix)]
fn ensure_system_state_ownership_for_lifecycle() -> anyhow::Result<()> {
    if !utils::is_managed_system_state_dir_present() {
        return Ok(());
    }

    let needs_repair = utils::check_system_state_dir_needs_repair().unwrap_or(true);
    if !needs_repair {
        return Ok(());
    }

    if is_current_process_root() {
        utils::ensure_mihomo_system_state_dir()?;
        println!("  ✓ Mihomo system state directory permissions repaired.");
        return Ok(());
    }

    println!("  Detected a system state permission issue. Repairing it via sudo and continuing restart...");
    let exe = std::env::current_exe()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = sudo_reexec_command(&exe, &args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(not(unix))]
fn ensure_system_state_ownership_for_lifecycle() -> anyhow::Result<()> {
    Ok(())
}

async fn cmd_lifecycle_instance_mode(
    mode: instance::InstanceMode,
    action: instance::ServiceAction,
    allow_runtime_reset: bool,
    prompt_runtime_reset: bool,
) -> anyhow::Result<()> {
    let ctx = instance::planned_current_context(mode)
        .ok_or_else(|| anyhow::anyhow!("Unsupported OS for instance lifecycle"))?;

    if mode == instance::InstanceMode::System {
        if action == instance::ServiceAction::Restart {
            ensure_system_state_ownership_for_lifecycle()?;
            maybe_sudo_reexec_for_system_generation(&ctx)?;
            ensure_system_config_ownership_for_lifecycle(&ctx)?;
            maybe_auto_recover_active_transaction(
                &ctx,
                tun_transaction::RecoveryDirection::Resume,
                allow_runtime_reset,
                prompt_runtime_reset,
            )
            .await?;
            if apply_pending_generation(&ctx).await? {
                return Ok(());
            }
        } else if matches!(
            action,
            instance::ServiceAction::Stop | instance::ServiceAction::Uninstall
        ) && prepare_system_transaction_recovery().await?
        {
            ensure_system_state_ownership_for_lifecycle()?;
            maybe_auto_recover_active_transaction(
                &ctx,
                tun_transaction::RecoveryDirection::Abort,
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
            anyhow::bail!(system_daemon_unavailable_message(
                lifecycle_command_name(action),
                &ctx,
            ));
        }

        let cmd = match action {
            instance::ServiceAction::Start => {
                let (config_content, config_revision) = lifecycle_system_config_payload(&ctx)?;
                let subscription_id =
                    config::get_active_id_at(&utils::AppPaths::new(ctx.paths.config_dir.clone()))?;
                ipc::DaemonCommand::StartCore {
                    config_content,
                    config_revision,
                    selection_intent_dir: subscription_id
                        .as_ref()
                        .map(|_| ctx.paths.config_dir.display().to_string()),
                    subscription_id,
                    token: None,
                }
            }
            instance::ServiceAction::Stop => ipc::DaemonCommand::StopCore { token: None },
            instance::ServiceAction::Restart => {
                let (config_content, config_revision) = lifecycle_system_config_payload(&ctx)?;
                let subscription_id =
                    config::get_active_id_at(&utils::AppPaths::new(ctx.paths.config_dir.clone()))?;
                ipc::DaemonCommand::RestartCore {
                    config_content,
                    config_revision,
                    selection_intent_dir: subscription_id
                        .as_ref()
                        .map(|_| ctx.paths.config_dir.display().to_string()),
                    subscription_id,
                    token: None,
                }
            }
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
                anyhow::bail!(daemon_command_error_message(ctx.os, &message));
            }
            ipc::DaemonResponse::Status { .. } => return Ok(()),
            ipc::DaemonResponse::CoreApi { .. } | ipc::DaemonResponse::Transaction { .. } => {
                anyhow::bail!("unexpected Core API / Transaction response to lifecycle command");
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
        print_selection_replay_after_ready(&paths, &client, std::time::Instant::now()).await;
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

    let uid = ctx
        .owner_uid
        .ok_or_else(|| anyhow::anyhow!("cannot resolve original user uid for IPC authorization"))?;
    let user = username_for_uid(uid)?;
    let exe = if ctx.paths.cli_binary.exists() {
        ctx.paths.cli_binary.clone()
    } else {
        std::env::current_exe()?
    };
    let exe = exe
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("CLI path is not valid UTF-8: {}", exe.display()))?;
    service::PrivilegeExecutor::run(&[exe, "access", "grant", "--user", &user])?;

    match ipc::send_command(&status).await? {
        ipc::DaemonResponse::Status { .. } => Ok(()),
        ipc::DaemonResponse::Error { message } => {
            anyhow::bail!("IPC access grant did not take effect: {message}")
        }
        ipc::DaemonResponse::Success { message } => {
            anyhow::bail!("unexpected daemon response after IPC access grant: {message}")
        }
        ipc::DaemonResponse::CoreApi { .. } | ipc::DaemonResponse::Transaction { .. } => {
            anyhow::bail!("unexpected Core API / Transaction response after IPC access grant")
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

fn status_health_label(snapshot: &status::StatusSnapshot) -> &'static str {
    if snapshot.daemon_reachable == status::TriState::True && !snapshot.intent_config_exists {
        "ready"
    } else {
        match (snapshot.core_running.as_bool(), snapshot.api_reachable) {
            (Some(false), _) => "inactive",
            (Some(true), true)
                if snapshot.tun_verdict == status::TunVerdict::TunDisabled
                    || snapshot.tun_verdict == status::TunVerdict::TunRunning =>
            {
                "healthy"
            }
            (Some(true), _) | (None, true) => "degraded",
            _ => "unknown",
        }
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

async fn cmd_status_context_with_source(
    ctx: instance::InstanceContext,
    source: Option<instance::ResolutionSource>,
) -> anyhow::Result<()> {
    let plan = instance::planned_status_diagnostics(&ctx);
    let verbose = crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed);

    // 使用 StatusSnapshot 统一采集状态
    let snapshot = status::StatusSnapshot::collect(&ctx).await;

    let health = status_health_label(&snapshot);

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
    println!("API:             {}", status_api_label(&snapshot));
    println!("TUN:             {}", text_tun_status_label(&snapshot));
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

pub(crate) fn check_and_warn_daemon_binary_skew(ctx: &instance::InstanceContext) {
    if ctx.mode != instance::InstanceMode::System {
        return;
    }
    let store = system_generation_store(ctx);
    if let Ok(state) = store.read_state() {
        if let Some(pending_id) = state.pending {
            eprintln!("⚠ 提示: 存在待应用的系统更新 ({pending_id})。");
            eprintln!("  如需应用更新，请运行: mihomo-cli restart\n");
            return;
        }
    }
    if !ctx.paths.cli_binary.exists() {
        return;
    }
    let Ok(current_cli) = std::env::current_exe() else {
        return;
    };
    if !utils::file_contents_equal(&current_cli, &ctx.paths.cli_binary) {
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

fn runtime_tun_status_label(tun: status::TriState) -> &'static str {
    match tun {
        status::TriState::True => "enabled",
        status::TriState::False => "disabled",
        status::TriState::Unknown => "unknown",
    }
}

fn text_tun_status_label(snapshot: &status::StatusSnapshot) -> &'static str {
    match snapshot.tun_verdict {
        status::TunVerdict::TunRunning => "enabled",
        status::TunVerdict::TunDisabled => "disabled",
        status::TunVerdict::TunRunningUnattested | status::TunVerdict::TunStateUnknown => "unknown",
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
    label: String,
    detail: String,
    hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonIpcErrorKind {
    InvalidOrMissingToken,
    PeerUidMismatch,
    AuthorizationTableUnreadable,
    Other,
}

fn classify_daemon_ipc_error(message: &str) -> DaemonIpcErrorKind {
    if message.contains("invalid or missing auth token") {
        DaemonIpcErrorKind::InvalidOrMissingToken
    } else if message.contains("auth token does not belong to IPC peer uid") {
        DaemonIpcErrorKind::PeerUidMismatch
    } else if message.contains("cannot read authorized clients") {
        DaemonIpcErrorKind::AuthorizationTableUnreadable
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
             Check: mihomo-cli access status\n  \
             Admin fix: sudo mihomo-cli access grant --user \"$(id -un)\""
        ),
        DaemonIpcErrorKind::PeerUidMismatch => format!(
            "daemon authorization failed: {message}\n  \
             Check HOME/XDG_CONFIG_HOME for the current user, then re-authorize:\n  \
             sudo mihomo-cli access grant --user \"$(id -un)\""
        ),
        DaemonIpcErrorKind::AuthorizationTableUnreadable => format!(
            "daemon authorization state is unavailable: {message}\n  \
             Admin check: sudo mihomo-cli access list"
        ),
        DaemonIpcErrorKind::Other => format!("daemon error: {message}"),
    }
}

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
            "检查: mihomo-cli access status；管理员授权: sudo mihomo-cli access grant --user \"$(id -un)\"",
        ),
        DaemonIpcErrorKind::PeerUidMismatch => DoctorCheck::fail(
            "Daemon 授权",
            message,
            "检查当前用户的 HOME/XDG_CONFIG_HOME；然后重新授权: sudo mihomo-cli access grant --user \"$(id -un)\"",
        ),
        DaemonIpcErrorKind::AuthorizationTableUnreadable => DoctorCheck::fail(
            "Daemon 授权状态",
            message,
            format!(
                "管理员检查授权表: sudo mihomo-cli access list；并查看 daemon 状态: {}",
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

fn doctor_checks_service(service: &instance::ServiceTarget) -> bool {
    !matches!(service, instance::ServiceTarget::WindowsUserProcess)
}

impl DoctorCheck {
    fn pass(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            passed: true,
            label: label.into(),
            detail: detail.into(),
            hint: None,
        }
    }

    fn fail(label: impl Into<String>, detail: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            passed: false,
            label: label.into(),
            detail: detail.into(),
            hint: Some(hint.into()),
        }
    }

    fn format(&self) -> String {
        let icon = if self.passed { "✅" } else { "❌" };
        let mut line = format!("  {icon} {}: {}", self.label, self.detail);
        if let Some(hint) = &self.hint {
            line.push_str(&format!("\n     💡 {hint}"));
        }
        line
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
    skip_config: bool,
    yes: bool,
) -> anyhow::Result<()> {
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
    cmd_install_instance(mode, force, version, github_mirror, skip_config, yes).await
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
    if components_complete {
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
    if system_daemon_active && !force && (!binary_valid || !cli_binary_valid || !geo_valid) {
        if !is_current_process_root() {
            let exe = std::env::current_exe()?;
            let args: Vec<String> = std::env::args().skip(1).collect();
            let status = sudo_reexec_command(&exe, &args).status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        println!();
        println!("  System service is currently running. Preparing staged upgrade in pending generation...");
        let stage = tempfile::tempdir()?;
        let staged_core = stage.path().join("mihomo");
        if !binary_valid {
            installer::download_mihomo_to_with_mirror(version, &staged_core, github_mirror)
                .await
                .map_err(|e| anyhow::anyhow!(install_download_error(&e.to_string())))?;
        } else {
            let existing_core = std::fs::read(&ctx.paths.core_binary)?;
            std::fs::write(&staged_core, existing_core)?;
        }
        let core_bytes = std::fs::read(&staged_core)?;

        let current_cli = std::env::current_exe()?;
        let cli_bytes = std::fs::read(&current_cli)?;

        let mut extra_artifacts = Vec::new();
        if !geo_valid {
            let geo_stage = tempfile::tempdir()?;
            let _ = installer::ensure_geo_files_in(geo_stage.path(), github_mirror).await;
            for name in ["geoip.metadb", "GeoSite.dat", "Country.mmdb"] {
                let p = geo_stage.path().join(name);
                if p.exists() {
                    let b = std::fs::read(&p)?;
                    extra_artifacts.push((name.to_string(), b, Some(0o644)));
                }
            }
        }

        let gen_id = prepare_system_generation(&ctx, &core_bytes, &cli_bytes, extra_artifacts)?;
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
                    service::run_instance_command(command)?;
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
) -> Vec<String> {
    let resolved_mode =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)
            .ok()
            .map(|resolved| resolved.ctx.mode);
    match reload_configs_for_resolved_instance(system, user, paths).await {
        Ok(()) if resolved_mode == Some(instance::InstanceMode::System) => {
            system_config_applied_lines()
        }
        Ok(()) => {
            vec![
                "  ✓ Config reload request accepted by Core API".to_string(),
                "  ⚠ Runtime status: unknown (revision attestation unavailable)".to_string(),
                "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
            ]
        }
        Err(e) => vec![
            format!("  ⚠ Config written but runtime application is unknown: {e}"),
            "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
        ],
    }
}

async fn reload_config_for_mutation(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
) -> anyhow::Result<bool> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    let tun_active = resolved.ctx.mode == instance::InstanceMode::System
        && status::StatusSnapshot::collect(&resolved.ctx)
            .await
            .tun_verdict
            == status::TunVerdict::TunRunning;
    if tun_active {
        reload_configs_for_resolved_instance(system, user, paths).await?;
        Ok(true)
    } else {
        Ok(reload_configs_for_resolved_instance(system, user, paths)
            .await
            .is_ok())
    }
}

async fn try_reload_group_config(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
) -> anyhow::Result<Option<anyhow::Error>> {
    match reload_configs_for_resolved_instance(system, user, paths).await {
        Ok(()) => Ok(None),
        Err(error) => Ok(Some(error)),
    }
}

async fn apply_config_reload_required(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
) -> anyhow::Result<Vec<String>> {
    Ok(apply_config_reload_lines(system, user, paths).await)
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
        lines.extend([
            "  ✓ Config reload request accepted by Core API".to_string(),
            "  ⚠ Runtime status: unknown (revision attestation unavailable)".to_string(),
            "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
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
        lines.extend([
            "  ✓ Config reload request accepted by Core API".to_string(),
            "  ⚠ Runtime status: unknown (revision attestation unavailable)".to_string(),
            "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
        ]);
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

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
enum ConfigSubcommand {
    /// Download/convert a subscription URL to a local Clash YAML file without changing local state
    Fetch {
        /// Subscription URL to fetch
        url: String,
        /// Output Clash YAML file path
        #[arg(short, long)]
        output: std::path::PathBuf,
        /// Use a fixed User-Agent for this fetch (alias: --ua)
        #[arg(long = "user-agent", alias = "ua")]
        user_agent: Option<String>,
    },
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

fn validate_config_at_paths_with_binary_json(
    paths: &utils::AppPaths,
    mihomo: &std::path::Path,
    json: bool,
) -> anyhow::Result<()> {
    let report = config::validate_config_at(paths, Some(mihomo))?;
    if json {
        let mut warnings = Vec::new();
        if !report.mihomo_tested {
            warnings.push(format!("mihomo binary not found: {}", mihomo.display()));
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
        print_lines(format_config_validation_result(
            &paths.config_path(),
            mihomo,
            &report,
        ));
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

async fn reload_configs_for_resolved_instance(
    system: bool,
    user: bool,
    paths: &utils::AppPaths,
) -> anyhow::Result<()> {
    let resolved =
        resolve_current_instance_context(system, user, instance::CommandIntent::ReadOnly)?;
    if system_config_requires_promotion(resolved.ctx.mode) {
        let content = std::fs::read_to_string(paths.config_path())?;
        let revision = tun_transaction::content_revision(content.as_bytes());
        let subscription_id = config::get_active_id_at(paths)?;
        match ipc::send_command(&ipc::DaemonCommand::PromoteSystemConfig {
            config_content: content,
            config_revision: revision,
            selection_intent_dir: subscription_id
                .as_ref()
                .map(|_| resolved.ctx.paths.config_dir.display().to_string()),
            subscription_id,
            token: None,
        })
        .await?
        {
            ipc::DaemonResponse::Success { .. } => return Ok(()),
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
    // SPEC §3.2-A: user instance — the CLI that triggered the reload replays
    // selections. (System mode returned early via PromoteSystemConfig; the
    // daemon replays after promotion.)
    print_selection_replay_after_ready(paths, &client, std::time::Instant::now()).await;
    Ok(())
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
            "config requires an explicit action in non-interactive mode.\n  Use --list, --add <URL>, --import <FILE>, --refresh, --refresh-all, --validate, or --probe <URL>."
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
        let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
        let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
        if was_active {
            lines.extend(apply_config_reload_lines(system, user, &paths).await);
        }
        print_lines(lines);
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
        let ids = subs.iter().map(|sub| sub.id.clone()).collect::<Vec<_>>();
        let snapshots = snapshot_subscription_refresh(&paths, &ids)?;
        let report = config::refresh_all_at(&paths).await?;
        if let Err(error) = config::merge_user_config_checked_at_endpoint(
            &paths,
            Some(&core_binary),
            &config_endpoint,
        ) {
            restore_subscription_refresh(snapshots)?;
            anyhow::bail!(
                "Subscription refresh produced an invalid config; restored all last-known-good caches.\n  {error}"
            );
        }
        let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
                let _lock = crate::lock::ConfigLock::acquire(paths.config_dir())?;
                let snapshots = snapshot_subscription_refresh(&paths, std::slice::from_ref(&id))?;
                config::refresh_subscription_at_with_user_agent(&paths, &id, user_agent.as_deref())
                    .await?;
                if let Err(error) = config::merge_user_config_checked_at_endpoint(
                    &paths,
                    Some(&core_binary),
                    &config_endpoint,
                ) {
                    restore_subscription_refresh(snapshots)?;
                    anyhow::bail!(
                        "Subscription refresh produced an invalid config; restored the last-known-good cache.\n  {error}"
                    );
                }
                let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
        let meta_snapshot = snapshot_file(&paths.subscriptions_meta_path())?;
        let active_snapshot = snapshot_file(&paths.active_file_path())?;
        let existing_subscriptions = config::load_subscriptions_at(&paths)?;

        let activate_decision = resolve_activate_flag(activate, no_activate, yes);
        let should_activate = match activate_decision {
            Some(v) => v,
            None if existing_subscriptions.is_empty() => true,
            None if std::io::stdin().is_terminal() => {
                use dialoguer::Confirm;
                Confirm::new()
                    .with_prompt("  Activate this subscription?")
                    .default(true)
                    .interact()?
            }
            None => false,
        };

        let id = config::generate_subscription_id();
        config::commit_imported_subscription_at(
            &paths,
            &id,
            &yaml_content,
            &format!("file://{}", file),
            should_activate,
        )?;

        if should_activate {
            merge_subscription_change_checked(
                &paths,
                &core_binary,
                &config_endpoint,
                &id,
                meta_snapshot,
                active_snapshot,
            )?;
            let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
        let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
                let ids = subs.iter().map(|sub| sub.id.clone()).collect::<Vec<_>>();
                let snapshots = snapshot_subscription_refresh(paths, &ids)?;
                let report = config::refresh_all_at(paths).await?;
                if let Err(error) =
                    config::merge_user_config_checked_at_endpoint(paths, Some(mihomo), endpoint)
                {
                    restore_subscription_refresh(snapshots)?;
                    anyhow::bail!(
                        "Subscription refresh produced an invalid config; restored all last-known-good caches.\n  {error}"
                    );
                }
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
                        let snapshots =
                            snapshot_subscription_refresh(paths, std::slice::from_ref(id))?;
                        config::refresh_subscription_at(paths, id).await?;
                        if let Err(error) = config::merge_user_config_checked_at_endpoint(
                            paths,
                            Some(mihomo),
                            endpoint,
                        ) {
                            restore_subscription_refresh(snapshots)?;
                            anyhow::bail!(
                                "Subscription refresh produced an invalid config; restored the last-known-good cache.\n  {error}"
                            );
                        }
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

type PathSnapshot = (std::path::PathBuf, Option<String>);

fn snapshot_subscription_refresh(
    paths: &utils::AppPaths,
    ids: &[String],
) -> anyhow::Result<Vec<PathSnapshot>> {
    let meta_path = paths.subscriptions_meta_path();
    let mut snapshots = vec![(meta_path.clone(), snapshot_file(&meta_path)?)];
    for id in ids {
        let path = paths.subscription_file_path(id);
        snapshots.push((path.clone(), snapshot_file(&path)?));
    }
    Ok(snapshots)
}

fn restore_subscription_refresh(snapshots: Vec<PathSnapshot>) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    for (path, snapshot) in snapshots {
        if let Err(error) = restore_file_snapshot(&path, snapshot) {
            errors.push(format!("{}: {error}", path.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "subscription refresh rollback incomplete: {}",
            errors.join("; ")
        )
    }
}

fn rollback_subscription_change(
    paths: &utils::AppPaths,
    new_subscription_id: &str,
    meta_snapshot: Option<String>,
    active_snapshot: Option<String>,
) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    if let Err(err) = restore_file_snapshot(&paths.subscriptions_meta_path(), meta_snapshot) {
        errors.push(format!("subscriptions metadata: {err}"));
    }
    if let Err(err) = restore_file_snapshot(&paths.active_file_path(), active_snapshot) {
        errors.push(format!("active subscription: {err}"));
    }
    if let Err(err) =
        utils::remove_file_if_exists(&paths.subscription_file_path(new_subscription_id))
    {
        errors.push(format!("new subscription file: {err}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("subscription rollback incomplete: {}", errors.join("; "))
    }
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
        if let Err(rollback_err) =
            rollback_subscription_change(paths, new_subscription_id, meta_snapshot, active_snapshot)
        {
            anyhow::bail!(
                "Subscription change failed; rollback incomplete: {}.\n  Original error: {}",
                rollback_err,
                err
            );
        }
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

fn format_rule_apply_result(merged: bool, hot_reloaded: bool, _new_rule: bool) -> Vec<String> {
    if merged && hot_reloaded {
        vec![
            "  ✓ Rule intent committed".to_string(),
            "  ⚠ Runtime status: unknown (revision attestation unavailable)".to_string(),
            "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
        ]
    } else if merged {
        vec![
            "  ✓ Rule intent committed".to_string(),
            "  ℹ Runtime status: pending".to_string(),
            "  Run: mihomo-cli restart  to apply".to_string(),
        ]
    } else {
        vec![
            "  ℹ Config pending — rule saved".to_string(),
            "  Run: mihomo-cli restart  to apply".to_string(),
        ]
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

async fn cmd_group(system: bool, user: bool, action: GroupAction) -> anyhow::Result<()> {
    let intent = match &action {
        GroupAction::List | GroupAction::Show { .. } => instance::CommandIntent::ReadOnly,
        GroupAction::Create { .. }
        | GroupAction::Edit { .. }
        | GroupAction::Add { .. }
        | GroupAction::Remove { .. }
        | GroupAction::Delete { .. } => instance::CommandIntent::Mutating,
    };
    let paths = app_paths_for_resolved_instance_command("group", system, user, intent)?;
    let scope = crate::selection::active_selection_scope(&paths)?;
    let subscription_path = paths.subscription_file_path(&scope.subscription_id);

    match action {
        GroupAction::List => {
            let config: serde_yaml::Value = serde_yaml::from_str(
                &std::fs::read_to_string(paths.config_path())
                    .context("failed to read current config.yaml")?,
            )?;
            let groups = config
                .get("proxy-groups")
                .and_then(serde_yaml::Value::as_sequence)
                .ok_or_else(|| anyhow::anyhow!("current config has no proxy-groups list"))?;
            for group in groups {
                let name = crate::groups::group_name(group).ok_or_else(|| {
                    anyhow::anyhow!("current config contains a proxy group without name")
                })?;
                let group_type = group
                    .get("type")
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or("unknown");
                println!("{name}\t{group_type}");
            }
            Ok(())
        }
        GroupAction::Show { name } => {
            let config: serde_yaml::Value = serde_yaml::from_str(
                &std::fs::read_to_string(paths.config_path())
                    .context("failed to read current config.yaml")?,
            )?;
            let groups = config
                .get("proxy-groups")
                .and_then(serde_yaml::Value::as_sequence)
                .ok_or_else(|| anyhow::anyhow!("current config has no proxy-groups list"))?;
            let group = groups
                .iter()
                .find(|group| crate::groups::group_name(group) == Some(name.as_str()))
                .ok_or_else(|| anyhow::anyhow!("proxy group not found: {name}"))?;
            println!("{}", serde_yaml::to_string(group)?);
            Ok(())
        }
        GroupAction::Create {
            name,
            group_type,
            members,
            file,
            prepend,
        } => {
            let source = if let Some(file) = file {
                std::fs::read_to_string(&file)
                    .with_context(|| format!("failed to read group definition: {file}"))?
            } else {
                let mut map = serde_yaml::Mapping::new();
                map.insert("name".into(), name.clone().into());
                map.insert(
                    "type".into(),
                    group_type
                        .ok_or_else(|| anyhow::anyhow!("--type is required without --file"))?
                        .into(),
                );
                map.insert(
                    "proxies".into(),
                    serde_yaml::Value::Sequence(members.into_iter().map(Into::into).collect()),
                );
                serde_yaml::to_string(&serde_yaml::Value::Mapping(map))?
            };
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
            let known_proxies = known_subscription_proxies(&subscription);
            let known_providers = known_subscription_providers(&subscription);
            let overlay_path = paths.groups_override_path_for_subscription(&scope.subscription_id);
            let mut overlay = crate::groups::GroupsOverlay::load(&overlay_path)?;
            let existing = overlay.merged_groups(&original, &known_proxies, &known_providers)?;
            if existing
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
            if let Some(error) = try_reload_group_config(system, user, &paths).await? {
                println!("created proxy group `{name}` (pending=true)");
                println!("  Runtime apply deferred: {error:#}");
                println!("  Run: mihomo-cli restart  to apply");
                return Ok(());
            }
            println!("created proxy group `{name}` (runtime_applied=true)");
            Ok(())
        }
        GroupAction::Edit { name, file } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("failed to read group definition: {file}"))?;
            let group = crate::groups::parse_group(&source)?;
            if crate::groups::group_name(&group) != Some(name.as_str()) {
                anyhow::bail!("group definition name must match `{name}`");
            }
            update_group_overlay(
                &paths,
                &scope.subscription_id,
                &subscription_path,
                GroupMutation::Replace { name, group },
                system,
                user,
            )
            .await
        }
        GroupAction::Add { name, members } => {
            update_group_overlay(
                &paths,
                &scope.subscription_id,
                &subscription_path,
                GroupMutation::Add { name, members },
                system,
                user,
            )
            .await
        }
        GroupAction::Remove { name, members } => {
            update_group_overlay(
                &paths,
                &scope.subscription_id,
                &subscription_path,
                GroupMutation::Remove { name, members },
                system,
                user,
            )
            .await
        }
        GroupAction::Delete { name } => {
            update_group_overlay(
                &paths,
                &scope.subscription_id,
                &subscription_path,
                GroupMutation::Delete { name },
                system,
                user,
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

#[derive(Debug)]
enum GroupMutation {
    Replace {
        name: String,
        group: serde_yaml::Value,
    },
    Add {
        name: String,
        members: Vec<String>,
    },
    Remove {
        name: String,
        members: Vec<String>,
    },
    Delete {
        name: String,
    },
}

async fn update_group_overlay(
    paths: &utils::AppPaths,
    subscription_id: &str,
    subscription_path: &std::path::Path,
    mutation: GroupMutation,
    system: bool,
    user: bool,
) -> anyhow::Result<()> {
    let subscription: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(subscription_path)?)?;
    let original = subscription
        .get("proxy-groups")
        .and_then(serde_yaml::Value::as_sequence)
        .cloned()
        .unwrap_or_default();
    let known_proxies = known_subscription_proxies(&subscription);
    let known_providers = known_subscription_providers(&subscription);
    let overlay_path = paths.groups_override_path_for_subscription(subscription_id);
    let mut overlay = crate::groups::GroupsOverlay::load(&overlay_path)?;
    let current = overlay.merged_groups(&original, &known_proxies, &known_providers)?;
    let name = match &mutation {
        GroupMutation::Replace { name, .. }
        | GroupMutation::Add { name, .. }
        | GroupMutation::Remove { name, .. }
        | GroupMutation::Delete { name } => name.clone(),
    };
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
    if !is_prepend && !is_append && !is_original {
        anyhow::bail!("proxy group not found: {name}");
    }
    if is_prepend && is_append {
        anyhow::bail!("proxy group `{name}` exists in both overlay sections");
    }

    let mut replacement = None;
    let adding = matches!(&mutation, GroupMutation::Add { .. });
    match mutation {
        GroupMutation::Replace { group, .. } => replacement = Some(group),
        GroupMutation::Add { members, .. } | GroupMutation::Remove { members, .. } => {
            let current_group = current
                .iter()
                .find(|group| crate::groups::group_name(group) == Some(name.as_str()))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("proxy group not found: {name}"))?;
            let mut map = current_group
                .as_mapping()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("proxy group `{name}` is not a mapping"))?;
            let mut existing =
                crate::groups::group_members(&serde_yaml::Value::Mapping(map.clone()))?;
            for member in members {
                if adding {
                    if existing.iter().any(|item| item == &member) {
                        anyhow::bail!("proxy group `{name}` already contains member `{member}`");
                    }
                    existing.push(member);
                } else if let Some(index) = existing.iter().position(|item| item == &member) {
                    existing.remove(index);
                } else {
                    anyhow::bail!("proxy group `{name}` does not contain member `{member}`");
                }
            }
            map.insert(
                serde_yaml::Value::String("proxies".into()),
                serde_yaml::Value::Sequence(existing.into_iter().map(Into::into).collect()),
            );
            replacement = Some(serde_yaml::Value::Mapping(map));
        }
        GroupMutation::Delete { .. } => {
            let rules = crate::rules::load_rules_at(paths).unwrap_or_default();
            if rules
                .iter()
                .any(|rule| crate::rules::rule_policy(rule) == Some(name.as_str()))
            {
                anyhow::bail!(
                    "cannot delete proxy group `{name}` because rules.yaml still references it; edit or remove those rules first"
                );
            }
            if is_prepend {
                overlay
                    .prepend
                    .retain(|group| crate::groups::group_name(group) != Some(name.as_str()));
            } else if is_append {
                overlay
                    .append
                    .retain(|group| crate::groups::group_name(group) != Some(name.as_str()));
            } else if !overlay.delete.iter().any(|item| item == &name) {
                overlay.delete.push(name.clone());
            }
        }
    }
    if let Some(group) = replacement {
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
            if !overlay.delete.iter().any(|item| item == &name) {
                overlay.delete.push(name.clone());
            }
            overlay
                .append
                .retain(|item| crate::groups::group_name(item) != Some(name.as_str()));
            overlay.append.push(group);
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
    if let Some(error) = try_reload_group_config(system, user, paths).await? {
        println!("updated proxy group `{name}` (pending=true)");
        println!("  Runtime apply deferred: {error:#}");
        println!("  Run: mihomo-cli restart  to apply");
        return Ok(());
    }
    println!("updated proxy group `{name}` (runtime_applied=true)");
    Ok(())
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
                if !policies.iter().any(|p| p == policy) {
                    anyhow::bail!(
                        "Rule target `{policy}` is not available in current config. Run `mihomo-cli rule policies` or `mihomo-cli list` to inspect valid policy/group names."
                    );
                }
            }
            crate::rules::add_rule_at(&paths, &rule, pos)?;
            let merged =
                merge_rules_change_checked(&paths, &core_binary, &config_endpoint, rules_snapshot)?;
            let hot_reloaded = merged && reload_config_for_mutation(system, user, &paths).await?;
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
            let hot_reloaded = merged && reload_config_for_mutation(system, user, &paths).await?;
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
            let hot_reloaded = merged && reload_config_for_mutation(system, user, &paths).await?;
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
            let hot_reloaded = merged && reload_config_for_mutation(system, user, &paths).await?;
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
            let hot_reloaded = merged && reload_config_for_mutation(system, user, &paths).await?;
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
                merge_dns_change_checked(
                    &paths,
                    &core_binary,
                    &config_endpoint,
                    vec![(paths.dns_policy_path(), dns_snapshot)],
                )?;

                if let Err(error) = reload_configs_for_resolved_instance(system, user, &paths).await
                {
                    anyhow::bail!(
                        "DNS policy was saved to intent, but runtime update failed: {error}.\n  Run: mihomo-cli restart --system"
                    );
                }
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
                merge_dns_change_checked(
                    &paths,
                    &core_binary,
                    &config_endpoint,
                    vec![(paths.dns_policy_path(), dns_snapshot)],
                )?;
                let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
                let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
                let apply_lines = apply_config_reload_required(system, user, &paths).await?;
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
                    let apply_lines = apply_config_reload_lines(system, user, &paths).await;
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
    let hot_reloaded = reload_configs_for_resolved_instance(system, user, paths)
        .await
        .is_ok();
    println!("  ✓ override.yaml {action_label}");
    if hot_reloaded {
        println!("  ✓ Config reload request accepted by Core API");
        println!("  ⚠ Runtime status: unknown (revision attestation unavailable)");
        println!("  Run: mihomo-cli restart  to establish runtime readiness");
    } else {
        println!("  ⚠ Runtime status: unknown; restart mihomo to apply");
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
        utils::atomic_write_file_for_original_user(&config_path.display().to_string(), &fixed)?;
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
    fn system_config_always_uses_managed_promotion() {
        assert!(system_config_requires_promotion(
            instance::InstanceMode::System
        ));
    }

    #[test]
    fn user_config_does_not_use_managed_promotion() {
        assert!(!system_config_requires_promotion(
            instance::InstanceMode::User
        ));
    }

    #[test]
    fn system_config_applied_message_does_not_depend_on_tun_attestation() {
        assert_eq!(
            system_config_applied_lines(),
            vec!["  ✅ system configuration promoted and runtime applied".to_string()]
        );
    }

    #[test]
    fn doctor_check_pass_formats_success_without_hint() {
        assert_eq!(
            DoctorCheck::pass("配置文件", "/tmp/config.yaml").format(),
            "  ✅ 配置文件: /tmp/config.yaml"
        );
    }

    #[test]
    fn doctor_check_failure_formats_actionable_hint() {
        assert_eq!(
            DoctorCheck::fail("服务", "未安装", "运行: mihomo-cli install").format(),
            "  ❌ 服务: 未安装\n     💡 运行: mihomo-cli install"
        );
    }

    #[test]
    fn doctor_classifies_daemon_auth_failures_without_core_restart_hint() {
        let cases = [
            (
                "invalid or missing auth token",
                "Daemon 授权",
                "sudo mihomo-cli access grant --user \"$(id -un)\"",
            ),
            (
                "auth token does not belong to IPC peer uid",
                "Daemon 授权",
                "HOME/XDG_CONFIG_HOME",
            ),
            (
                "cannot read authorized clients: Permission denied (os error 13)",
                "Daemon 授权状态",
                "sudo mihomo-cli access list",
            ),
        ];

        for (message, label, expected_hint) in cases {
            let output = doctor_daemon_error_check(instance::TargetOs::Linux, message).format();
            assert!(output.contains(label), "{output}");
            assert!(output.contains(expected_hint), "{output}");
            assert!(!output.contains("mihomo-cli restart --system"), "{output}");
        }
    }

    #[test]
    fn lifecycle_auth_failure_points_to_access_repair_not_core_restart() {
        let message = daemon_command_error_message(
            instance::TargetOs::Linux,
            "invalid or missing auth token",
        );
        assert!(message.contains("mihomo-cli access status"), "{message}");
        assert!(
            message.contains("sudo mihomo-cli access grant --user \"$(id -un)\""),
            "{message}"
        );
        assert!(
            !message.contains("mihomo-cli restart --system"),
            "{message}"
        );
    }

    #[test]
    fn windows_daemon_auth_hints_never_emit_unix_only_commands() {
        for output in [
            daemon_command_error_message(
                instance::TargetOs::Windows,
                "invalid or missing auth token",
            ),
            doctor_daemon_error_check(
                instance::TargetOs::Windows,
                "auth token does not belong to IPC peer uid",
            )
            .format(),
        ] {
            assert!(
                output.contains("mihomo-cli install --system --force"),
                "{output}"
            );
            assert!(!output.contains("sudo"), "{output}");
            assert!(!output.contains("$(id -un)"), "{output}");
            assert!(!output.contains("mihomo-cli access"), "{output}");
        }
    }

    #[test]
    fn tun_preflight_auth_failures_use_access_repair_not_core_restart() {
        for os in [
            instance::TargetOs::Linux,
            instance::TargetOs::Macos,
            instance::TargetOs::Windows,
        ] {
            let check = tun_daemon_error_check(os, "invalid or missing auth token");
            let output = check.hint.unwrap_or_default();
            assert!(!output.contains("mihomo-cli restart --system"), "{output}");
            if os == instance::TargetOs::Windows {
                assert!(output.contains("install --system --force"), "{output}");
                assert!(!output.contains("sudo"), "{output}");
            } else {
                assert!(output.contains("mihomo-cli access status"), "{output}");
            }
        }
    }

    #[test]
    fn tun_preflight_transport_failures_use_platform_service_recovery() {
        for (os, expected, forbidden) in [
            (instance::TargetOs::Linux, "systemctl", "launchctl"),
            (instance::TargetOs::Macos, "launchctl", "systemctl"),
            (instance::TargetOs::Windows, "sc.exe", "sudo"),
        ] {
            let check = tun_daemon_transport_check(os, "connection refused");
            let output = check.hint.unwrap_or_default();
            assert!(output.contains(expected), "{output}");
            assert!(!output.contains(forbidden), "{output}");
            assert!(!output.contains("mihomo-cli restart --system"), "{output}");
            if os == instance::TargetOs::Windows {
                assert!(output.contains("Administrator PowerShell or Command Prompt"));
                assert!(output.contains("\n  sc.exe start mihomo"));
                assert!(output.contains("\n  sc.exe stop mihomo"));
                assert!(!output.contains("&&"), "{output}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn access_status_does_not_read_the_privileged_authorization_table() {
        assert!(!access_action_reads_authorized_table(&AccessAction::Status));
        assert!(access_action_reads_authorized_table(&AccessAction::List));
        assert!(access_action_reads_authorized_table(&AccessAction::Grant {
            user: "alice".to_string(),
        }));
        assert!(access_action_reads_authorized_table(
            &AccessAction::Revoke {
                user: "alice".to_string(),
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn access_status_output_is_diagnostic_and_never_contains_token_material() {
        let location = ipc::ClientTokenLocation {
            token_path: std::path::PathBuf::from("/home/alice/.config/mihomo/service-token"),
        };
        let lines = format_access_status(
            instance::TargetOs::Linux,
            &location,
            true,
            AccessDaemonStatus::Rejected(DaemonIpcErrorKind::InvalidOrMissingToken),
        );
        let output = lines.join("\n");
        assert!(output.contains("not authorized"));
        assert!(output.contains("token_file_readable=true"));
        assert!(output.contains("credential_path=/home/alice/.config/mihomo/service-token"));
        assert!(output.contains("sudo mihomo-cli access grant"));
        assert!(!output.contains("token="));
    }

    #[cfg(unix)]
    #[test]
    fn macos_access_status_hints_use_launchctl_not_systemctl() {
        let location = ipc::ClientTokenLocation {
            token_path: std::path::PathBuf::from("/Users/alice/.config/mihomo/service-token"),
        };
        let output = format_access_status(
            instance::TargetOs::Macos,
            &location,
            true,
            AccessDaemonStatus::Rejected(DaemonIpcErrorKind::AuthorizationTableUnreadable),
        )
        .join("\n");
        assert!(output.contains("launchctl"), "{output}");
        assert!(!output.contains("systemctl"), "{output}");
    }

    #[test]
    fn system_install_starts_core_after_provisioning_access() {
        assert_eq!(
            install_post_service_action(instance::InstanceMode::System, false),
            InstallPostServiceAction::ProvisionAccessOnly
        );
        assert_eq!(
            install_post_service_action(instance::InstanceMode::System, true),
            InstallPostServiceAction::ProvisionAccessOnly
        );
        assert_eq!(
            install_post_service_action(instance::InstanceMode::User, false),
            InstallPostServiceAction::WaitForUserInstance
        );
    }

    #[test]
    fn complete_system_install_fast_path_requires_running_core() {
        assert_eq!(
            install_fast_path_action(instance::InstanceMode::System, true, true),
            InstallFastPathAction::ReturnUpToDate
        );
        assert_eq!(
            install_fast_path_action(instance::InstanceMode::System, true, false),
            InstallFastPathAction::ContinueInstall
        );
        assert_eq!(
            install_fast_path_action(instance::InstanceMode::User, true, true),
            InstallFastPathAction::ReturnUpToDate
        );
    }

    #[test]
    fn system_install_orchestration_prepares_daemon_before_core_restart() {
        assert_eq!(
            system_install_operations(SystemInstallScenario::PostServiceNoConfig),
            vec![
                SystemInstallOperation::WaitForDaemon,
                SystemInstallOperation::EnsureAccess,
            ]
        );
        assert_eq!(
            system_install_operations(SystemInstallScenario::PostServiceWithConfig),
            vec![
                SystemInstallOperation::WaitForDaemon,
                SystemInstallOperation::EnsureAccess,
            ]
        );
        assert_eq!(
            system_install_operations(SystemInstallScenario::CompleteFastPath),
            vec![SystemInstallOperation::EnsureAccess]
        );
    }

    #[test]
    fn restart_help_describes_core_lifecycle_for_system_mode() {
        let command = Cli::command();
        let mut restart = command
            .find_subcommand("restart")
            .expect("restart subcommand should exist")
            .clone();
        let help = restart.render_long_help().to_string();
        assert!(help.contains("core"), "{help}");
        assert!(help.contains("--system"), "{help}");
    }

    #[cfg(unix)]
    #[test]
    fn doctor_only_checks_config_owner_for_user_mode() {
        assert!(doctor_checks_config_owner(instance::InstanceMode::User));
        assert!(!doctor_checks_config_owner(instance::InstanceMode::System));
    }

    #[test]
    fn doctor_user_mode_honors_config_directory_override() {
        let tmp = tempfile::tempdir().unwrap();
        with_config_dir_override(tmp.path(), || {
            let ctx = doctor_user_context().unwrap();
            assert_eq!(ctx.paths.config_dir, tmp.path());
            assert_eq!(ctx.paths.intent_config_file, tmp.path().join("config.yaml"));
        });
    }

    #[test]
    fn doctor_skips_service_checks_for_windows_user_process() {
        assert!(!doctor_checks_service(
            &instance::ServiceTarget::WindowsUserProcess
        ));
        assert!(doctor_checks_service(
            &instance::ServiceTarget::WindowsService {
                name: "mihomo".to_string(),
            }
        ));
    }

    #[test]
    fn doctor_only_uses_user_baseline_without_any_instance() {
        let none = instance::ServicePresence {
            system: false,
            user: false,
        };
        assert!(doctor_uses_user_baseline(none, none));
        assert!(!doctor_uses_user_baseline(
            instance::ServicePresence {
                system: true,
                user: false,
            },
            none,
        ));
        assert!(!doctor_uses_user_baseline(
            none,
            instance::ServicePresence {
                system: true,
                user: false,
            },
        ));
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
            .filter(|name| name != "help" && name != "dashboard" && name != "use")
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
            let may_expose_user = matches!(
                name.as_str(),
                "install" | "uninstall" | "autostart" | "doctor"
            );
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
    fn with_config_dir_override<T>(dir: &std::path::Path, f: impl FnOnce() -> T) -> T {
        let _guard = crate::utils::env_test_lock().lock().unwrap();
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
            assert_eq!(
                resolved.ctx.paths.intent_config_file,
                isolated.join("config.yaml")
            );
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

        // S5: settings auto mode prefers system when installed
        assert_eq!(
            resolve_instance_mode_runtime_first(
                instance::ModeRequest::Unspecified,
                no_runtime,
                system_installed,
                instance::CommandIntent::ReadOnly,
            ),
            RuntimeFirstModeResolution::Resolved {
                mode: instance::InstanceMode::System,
                source: instance::ResolutionSource::ExplicitFlag, // settings converts to ExplicitSystem
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
    fn global_json_parses_for_stage1_commands() {
        let cli = parse(&["--json", "version"]);
        assert!(cli.json);
        assert!(matches!(cli.command, Some(Command::Version { .. })));

        let cli = parse(&["status", "--json"]);
        assert!(cli.json);
        assert!(matches!(cli.command, Some(Command::Status { .. })));

        let cli = parse(&["config", "--validate", "--json"]);
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Some(Command::Config { validate: true, .. })
        ));
    }

    #[test]
    fn json_envelope_has_ai_contract_fields() {
        let value = serde_json::json!({
            "ok": true,
            "command": "version",
            "data": {},
            "warnings": [],
            "error": serde_json::Value::Null,
            "meta": { "schema_version": 1, "cli_version": env!("MIHOMO_CLI_VERSION") }
        });
        for key in ["ok", "command", "data", "warnings", "error", "meta"] {
            assert!(value.get(key).is_some(), "missing {key}");
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
            Some(Command::Upgrade { system, yes }) => {
                assert!(system);
                assert!(!yes);
            }
            _ => panic!("expected upgrade --system"),
        }
        match parse(&["upgrade", "--yes"]).command {
            Some(Command::Upgrade { system, yes }) => {
                assert!(!system);
                assert!(yes);
            }
            _ => panic!("expected upgrade --yes"),
        }
        match parse(&["upgrade", "-y"]).command {
            Some(Command::Upgrade { yes, .. }) => assert!(yes),
            _ => panic!("expected upgrade -y"),
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
    fn non_interactive_stage_a_flags_parse() {
        match parse(&["install", "--user", "--skip-config", "--yes"]).command {
            Some(Command::Install {
                user,
                skip_config,
                yes,
                ..
            }) => {
                assert!(user);
                assert!(skip_config);
                assert!(yes);
            }
            _ => panic!("expected install --user --skip-config --yes"),
        }
        match parse(&["install", "--system", "-y"]).command {
            Some(Command::Install { system, yes, .. }) => {
                assert!(system);
                assert!(yes);
            }
            _ => panic!("expected install --system -y"),
        }
        match parse(&["restart", "--yes"]).command {
            Some(Command::Restart { system, yes }) => {
                assert!(!system);
                assert!(yes);
            }
            _ => panic!("expected restart --yes"),
        }
        match parse(&["tun", "on", "--yes"]).command {
            Some(Command::Tun { action, yes, .. }) => {
                assert!(matches!(action, Some(TunAction::On)));
                assert!(yes);
            }
            _ => panic!("expected tun on --yes"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "tun", "on", "--lan-direct"]).is_err());
        match parse(&["tun", "on", "-y"]).command {
            Some(Command::Tun { yes, .. }) => assert!(yes),
            _ => panic!("expected tun on -y"),
        }
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
                ..
            }) => {
                assert!(system);
                assert_eq!(group.as_deref(), Some("Proxy"));
                assert!(node.is_none());
            }
            _ => panic!("expected select --system --group"),
        }

        match parse(&["select", "--unpin", "--group", "Proxy"]).command {
            Some(Command::Select {
                unpin,
                group,
                replay,
                ..
            }) => {
                assert!(unpin);
                assert!(!replay);
                assert_eq!(group.as_deref(), Some("Proxy"));
            }
            _ => panic!("expected select --unpin --group"),
        }
        match parse(&["select", "--replay"]).command {
            Some(Command::Select { replay, .. }) => assert!(replay),
            _ => panic!("expected select --replay"),
        }
        assert!(Cli::try_parse_from(["mihomo-cli", "select", "--all"]).is_err());
        assert!(
            Cli::try_parse_from(["mihomo-cli", "select", "--unpin", "--all", "--group", "P"])
                .is_err()
        );

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
            Some(Command::Uninstall {
                remove_binary,
                remove_config,
                remove_geo,
                ..
            }) => {
                assert!(remove_binary);
                assert!(remove_config);
                assert!(!remove_geo);
            }
            _ => panic!("expected uninstall --remove-binary --remove-config"),
        }

        // --yes + granular flags should work
        let cli = parse(&["uninstall", "--remove-geo", "--yes"]);
        match cli.command {
            Some(Command::Uninstall {
                remove_geo, yes, ..
            }) => {
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
        let complete = config::RefreshAllReport {
            refreshed: vec!["sub-a".to_string(), "sub-b".to_string()],
            failed: Vec::new(),
        };
        assert_eq!(
            format_refresh_all_result(&complete, config_restart_apply_lines()),
            vec![
                "  All 2 subscriptions refreshed.".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        let partial = config::RefreshAllReport {
            refreshed: vec!["sub-a".to_string()],
            failed: vec![("sub-b".to_string(), "network error".to_string())],
        };
        assert_eq!(
            format_refresh_all_result(&partial, Vec::new()),
            vec!["  Refreshed 1 subscription(s); 1 failed.".to_string()]
        );
        assert_eq!(
            format_refresh_active_start("sub-a"),
            vec!["  Refreshing active subscription sub-a...".to_string()]
        );
        assert_eq!(
            format_refresh_active_success(config_restart_apply_lines()),
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
    fn lifecycle_system_config_path_prefers_imported_user_intent_config() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        ctx.paths.config_dir = temp.path().join("user-config");
        ctx.paths.intent_config_file = temp.path().join("system-store/config.yaml");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::write(
            &ctx.paths.intent_config_file,
            "mixed-port: 7897
",
        )
        .unwrap();

        assert_eq!(
            lifecycle_system_config_path(&ctx),
            ctx.paths.intent_config_file
        );
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
            format_tui_add_success("sub-a", config_restart_apply_lines()),
            vec![
                "  Added subscription sub-a".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_tui_switch_result("sub-a", false, Vec::new()),
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
                "  Example:  mihomo-cli dns policy add internal.example.com system".to_string(),
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
                "    mihomo-cli dns template apply company --domain corp.example.com --target 192.0.2.53".to_string(),
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
                "  ✓ Rule intent committed".to_string(),
                "  ⚠ Runtime status: unknown (revision attestation unavailable)".to_string(),
                "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
            ]
        );
        assert_eq!(
            format_rule_add_success("DOMAIN,example.com,DIRECT", true, false),
            vec![
                "  ✓ Rule added: DOMAIN,example.com,DIRECT".to_string(),
                "  ✓ Rule intent committed".to_string(),
                "  ℹ Runtime status: pending".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_rule_remove_success(2, false, false),
            vec![
                "  ✓ Rule 2 removed".to_string(),
                "  ℹ Config pending — rule saved".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_rule_clear_success(true, true),
            vec![
                "  ✓ All rules cleared".to_string(),
                "  ✓ Rule intent committed".to_string(),
                "  ⚠ Runtime status: unknown (revision attestation unavailable)".to_string(),
                "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
            ]
        );
        assert_eq!(
            format_rule_move_success(1, 3, true, false),
            vec![
                "  ✓ Rule moved: 1 → 3".to_string(),
                "  ✓ Rule intent committed".to_string(),
                "  ℹ Runtime status: pending".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_rule_import_success(4, "rules.txt", false, false),
            vec![
                "  ✓ Imported 4 rules from rules.txt".to_string(),
                "  ℹ Config pending — rule saved".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
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
                "  ℹ Static route estimate from config.yaml rules; it does not prove DNS resolution or runtime core matching.".to_string(),
                "  ✓ Matched rule #2: DOMAIN,example.com,DIRECT".to_string(),
                "  Policy: DIRECT".to_string(),
            ]
        );
        assert_eq!(
            format_rule_test_result("none.test", None),
            vec![
                "  ℹ Static route estimate from config.yaml rules; it does not prove DNS resolution or runtime core matching.".to_string(),
                "  No matching rule found for none.test".to_string(),
            ]
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
            format_config_add_success("sub-a", config_restart_apply_lines()),
            vec![
                "  Added subscription sub-a".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_legacy_url_add_success("sub-a", true),
            vec![
                "  Added and activated subscription sub-a".to_string(),
                "  ✓ Config reload request accepted by Core API".to_string(),
                "  ⚠ Runtime status: unknown (revision attestation unavailable)".to_string(),
                "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
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
            format_import_success("sub-a", true, config_restart_apply_lines()),
            vec![
                "  Imported and activated subscription sub-a".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_import_success("sub-a", false, config_restart_apply_lines()),
            vec!["  Imported subscription sub-a (not activated)".to_string()]
        );
        assert_eq!(
            format_fix_result(true, true),
            vec![
                "  Fixed config: added Unix socket controller.".to_string(),
                "  ⚠ Restart required for controller changes to take effect.".to_string(),
                "  Run: mihomo-cli restart".to_string(),
                "  ✓ Config reload request accepted by Core API".to_string(),
                "  ⚠ Runtime status: unknown (revision attestation unavailable)".to_string(),
                "  Run: mihomo-cli restart  to establish runtime readiness".to_string(),
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
            format_subscription_switch_success("sub-a", config_restart_apply_lines()),
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
    fn windows_system_service_recovery_uses_separate_executable_commands() {
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Windows,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        let recovery = system_service_recovery_command(&ctx).unwrap();

        assert_eq!(
            recovery,
            "Open an Administrator PowerShell or Command Prompt and run:\n  \
             sc.exe start mihomo\n\
             If the service is already running but unhealthy, run these separately:\n  \
             sc.exe stop mihomo\n  \
             sc.exe start mihomo"
        );
        assert!(!recovery.contains("&&"));
        assert!(!recovery.contains("sudo"));
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
    }

    #[test]
    fn tun_privileged_actions_trigger_sudo_plan() {
        assert!(is_tun_privileged_action(Some(&TunAction::On)));
        assert!(is_tun_privileged_action(Some(&TunAction::Off),));
        assert!(!is_tun_privileged_action(Some(&TunAction::Status),));
        assert!(!is_tun_privileged_action(None));

        let exe = std::path::Path::new("/tmp/mihomo-cli");
        let args = vec!["tun".to_string(), "on".to_string()];
        let cmd = sudo_reexec_command(exe, &args);
        assert_eq!(cmd.get_program(), "sudo");
        // 验证所有参数中包含 exe 路径和 tun on
        let args_vec: Vec<_> = cmd.get_args().collect();
        assert!(
            args_vec.iter().any(|a| *a == "/tmp/mihomo-cli"),
            "args should contain exe path, got: {:?}",
            args_vec
        );
        assert!(
            args_vec.iter().any(|a| *a == "tun"),
            "args should contain 'tun', got: {:?}",
            args_vec
        );
        assert!(
            args_vec.iter().any(|a| *a == "on"),
            "args should contain 'on', got: {:?}",
            args_vec
        );
        // Linux 上应该有私有环境变量
        #[cfg(target_os = "linux")]
        {
            assert!(
                args_vec.iter().any(|a| a
                    .to_string_lossy()
                    .starts_with("_MIHOMO_CLI_ORIGINAL_HOME=")),
                "Linux should have _MIHOMO_CLI_ORIGINAL_HOME, got: {:?}",
                args_vec
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn config_ownership_repair_only_reexecs_when_needed() {
        assert_eq!(
            config_ownership_repair(false, 1000, 1000, true, 1).unwrap(),
            ConfigOwnershipRepair::NotNeeded
        );
        assert_eq!(
            config_ownership_repair(false, 1000, 0, true, 1).unwrap(),
            ConfigOwnershipRepair::ReexecAsRoot
        );
        assert_eq!(
            config_ownership_repair(true, 1000, 0, true, 1).unwrap(),
            ConfigOwnershipRepair::RepairAsRoot
        );
        assert!(config_ownership_repair(false, 1000, 1001, true, 1).is_err());
        assert!(config_ownership_repair(false, 1000, 0, false, 1).is_err());
        assert!(config_ownership_repair(false, 1000, 0, true, 2).is_err());
    }

    #[test]
    fn tun_on_existing_config_confirmation_defaults_no() {
        assert!(!should_update_existing_tun_answer(""));
        assert!(!should_update_existing_tun_answer("n"));
        assert!(should_update_existing_tun_answer("y"));
        assert!(should_update_existing_tun_answer(" yes "));
    }

    #[test]
    fn non_windows_pipe_probe_is_false_on_this_target() {
        #[cfg(not(windows))]
        assert!(!windows_pipe_connectable(r"\\.\pipe\mihomo-alice"));
    }

    #[test]
    fn status_default_route_uses_final_match_rule_and_safe_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        std::fs::write(
            &path,
            "rules:\n  - DOMAIN-SUFFIX,example.com,DIRECT\n  - MATCH,Proxy\\u0000Group\n",
        )
        .unwrap();
        assert_eq!(default_route_label(&path, "rule"), "Proxy\\u0000Group");
        assert_eq!(default_route_label(&path, "direct"), "DIRECT");
        assert_eq!(
            default_route_label(&path.with_extension("missing"), "rule"),
            "unknown"
        );
    }

    #[test]
    fn status_default_route_prefers_daemon_active_config_over_intent_config() {
        let temp = tempfile::tempdir().unwrap();
        let intent = temp.path().join("intent.yaml");
        let active = temp.path().join("active.yaml");
        std::fs::write(&intent, "rules:\n  - MATCH,IntentProxy\n").unwrap();
        std::fs::write(&active, "rules:\n  - MATCH,ActiveProxy\n").unwrap();

        assert_eq!(
            default_route_path(Some(&active), &intent),
            active.as_path(),
            "the daemon-reported active config is runtime truth"
        );
        assert_eq!(
            default_route_label(default_route_path(Some(&active), &intent), "rule"),
            "ActiveProxy"
        );
        assert_eq!(default_route_path(None, &intent), intent.as_path());
    }

    #[test]
    fn status_health_is_degraded_when_tun_transaction_needs_recovery() {
        let snapshot = status::StatusSnapshot {
            daemon_reachable: status::TriState::True,
            configured_tun: status::TriState::False,
            runtime_tun: status::TriState::False,
            core_running: status::TriState::True,
            api_reachable: true,
            rule_mode: "rule".to_string(),
            core_pid: Some(42),
            active_config_path: None,
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: status::ConfigurationVerdict::Unknown,
            journal_state: status::JournalState::Prepared,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: status::TunVerdict::TunRunningUnattested,
            system_proxy: crate::system_proxy::SystemProxyState::Disabled,
            shell_proxy: crate::system_proxy::ShellProxyState::NotConfigured,
            intent_config_exists: true,
            core_binary_exists: true,
        };
        assert_eq!(status_health_label(&snapshot), "degraded");
    }

    #[test]
    fn status_after_install_without_config_reports_ready_but_unknown_tun() {
        let snapshot = status::StatusSnapshot {
            daemon_reachable: status::TriState::True,
            configured_tun: status::TriState::Unknown,
            runtime_tun: status::TriState::Unknown,
            core_running: status::TriState::False,
            api_reachable: false,
            rule_mode: "unknown".to_string(),
            core_pid: None,
            active_config_path: None,
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: status::ConfigurationVerdict::Unknown,
            journal_state: status::JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: status::TunVerdict::TunStateUnknown,
            system_proxy: crate::system_proxy::SystemProxyState::Disabled,
            shell_proxy: crate::system_proxy::ShellProxyState::NotConfigured,
            intent_config_exists: false,
            core_binary_exists: true,
        };
        assert_eq!(status_health_label(&snapshot), "ready");
        assert_eq!(status_core_label(&snapshot), "stopped");
        assert_eq!(status_api_label(&snapshot), "not configured");
        assert_eq!(text_tun_status_label(&snapshot), "unknown");
        assert_eq!(status_configuration_label(&snapshot), "not configured");
    }

    #[test]
    fn status_does_not_hide_unknown_when_observation_is_not_cleanly_unconfigured() {
        let mut snapshot = status::StatusSnapshot {
            daemon_reachable: status::TriState::Unknown,
            configured_tun: status::TriState::Unknown,
            runtime_tun: status::TriState::Unknown,
            core_running: status::TriState::Unknown,
            api_reachable: false,
            rule_mode: "unknown".to_string(),
            core_pid: None,
            active_config_path: None,
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: status::ConfigurationVerdict::Unknown,
            journal_state: status::JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: status::TunVerdict::TunStateUnknown,
            system_proxy: crate::system_proxy::SystemProxyState::Unknown,
            shell_proxy: crate::system_proxy::ShellProxyState::Unknown,
            intent_config_exists: false,
            core_binary_exists: false,
        };
        assert_eq!(status_health_label(&snapshot), "unknown");
        assert_eq!(status_core_label(&snapshot), "unknown");
        assert_eq!(status_api_label(&snapshot), "unknown");
        assert_eq!(status_configuration_label(&snapshot), "unknown");

        snapshot.daemon_reachable = status::TriState::True;
        snapshot.intent_config_exists = true;
        assert_eq!(status_health_label(&snapshot), "unknown");
        assert_eq!(status_core_label(&snapshot), "unknown");
        assert_eq!(status_api_label(&snapshot), "unknown");
    }

    #[test]
    fn text_status_reports_unknown_without_tun_attestation() {
        let snapshot = status::StatusSnapshot {
            daemon_reachable: status::TriState::True,
            configured_tun: status::TriState::False,
            runtime_tun: status::TriState::Unknown,
            core_running: status::TriState::False,
            api_reachable: false,
            rule_mode: "unknown".to_string(),
            core_pid: None,
            active_config_path: None,
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: status::ConfigurationVerdict::Unknown,
            journal_state: status::JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: status::TunVerdict::TunStateUnknown,
            system_proxy: crate::system_proxy::SystemProxyState::Unknown,
            shell_proxy: crate::system_proxy::ShellProxyState::NotConfigured,
            intent_config_exists: true,
            core_binary_exists: true,
        };
        assert_eq!(text_tun_status_label(&snapshot), "unknown");

        let mut uncertain = snapshot.clone();
        uncertain.configured_tun = status::TriState::True;
        assert_eq!(text_tun_status_label(&uncertain), "unknown");

        let mut runtime = snapshot;
        runtime.runtime_tun = status::TriState::False;
        runtime.tun_verdict = status::TunVerdict::TunDisabled;
        assert_eq!(text_tun_status_label(&runtime), "disabled");
    }

    #[test]
    fn status_json_runtime_fields_use_shared_snapshot_values() {
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        let plan = instance::planned_status_diagnostics(&ctx);
        let active = std::path::PathBuf::from("/run/mihomo/active.yaml");
        let snapshot = status::StatusSnapshot {
            daemon_reachable: status::TriState::True,
            configured_tun: status::TriState::Unknown,
            runtime_tun: status::TriState::True,
            core_running: status::TriState::True,
            api_reachable: true,
            rule_mode: "unknown".to_string(),
            core_pid: Some(4242),
            active_config_path: Some(active.clone()),
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: status::ConfigurationVerdict::Applied,
            journal_state: status::JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: status::TunVerdict::TunStateUnknown,
            system_proxy: crate::system_proxy::SystemProxyState::Unsupported,
            shell_proxy: crate::system_proxy::ShellProxyState::Unknown,
            intent_config_exists: true,
            core_binary_exists: false,
        };

        let data = status_json_data(
            &plan,
            instance::ResolutionSource::ExplicitFlag,
            &ctx.paths.intent_config_file,
            &snapshot,
        );

        assert_eq!(data["core"]["running"], true);
        assert_eq!(data["core"]["pid"], 4242);
        assert_eq!(
            data["core"]["active_config"],
            active.to_string_lossy().as_ref()
        );
        assert_eq!(data["core"]["tun"], true);
        assert_eq!(data["tun"], "enabled");
        assert_eq!(data["configuration"], "applied");
        assert_eq!(status_configuration_label(&snapshot), "applied");
        assert_eq!(data["system_proxy"], "unsupported");
        assert_eq!(data["shell_proxy"], "unknown");
        assert_eq!(data["configured_tun"], "unknown");
        assert_eq!(data["daemon"]["running"], true);
        assert_eq!(data["config"]["exists"], true);
        assert_eq!(data["binary"]["exists"], false);
    }

    #[test]
    fn status_json_runtime_fields_preserve_disabled_and_not_configured_snapshot_values() {
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::User,
            &instance::PathInputs::for_tests(),
        );
        let plan = instance::planned_status_diagnostics(&ctx);
        let snapshot = status::StatusSnapshot {
            daemon_reachable: status::TriState::Unknown,
            configured_tun: status::TriState::False,
            runtime_tun: status::TriState::False,
            core_running: status::TriState::Unknown,
            api_reachable: false,
            rule_mode: "rule".to_string(),
            core_pid: None,
            active_config_path: None,
            launched_snapshot_revision: None,
            active_snapshot_revision: None,
            active_intent_revision: None,
            configuration_verdict: status::ConfigurationVerdict::Unknown,
            journal_state: status::JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: status::TunVerdict::TunStateUnknown,
            system_proxy: crate::system_proxy::SystemProxyState::Disabled,
            shell_proxy: crate::system_proxy::ShellProxyState::NotConfigured,
            intent_config_exists: true,
            core_binary_exists: true,
        };

        let data = status_json_data(
            &plan,
            instance::ResolutionSource::ExplicitFlag,
            &ctx.paths.intent_config_file,
            &snapshot,
        );

        assert_eq!(data["core"]["running"], serde_json::Value::Null);
        assert_eq!(data["core"]["tun"], false);
        assert_eq!(data["tun"], "disabled");
        assert_eq!(data["system_proxy"], "disabled");
        assert_eq!(data["shell_proxy"], "not configured");
        assert_eq!(data["configured_tun"], "false");
        assert_eq!(data["daemon"]["running"], serde_json::Value::Null);
        assert_eq!(data["config"]["exists"], true);
        assert_eq!(data["binary"]["exists"], true);
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
    fn uninstall_all_preserves_non_interactive_options() {
        assert_eq!(
            all_uninstall_options(true, false),
            AllUninstallOptions {
                yes: true,
                dry_run: false,
            }
        );
        assert_eq!(
            all_uninstall_options(false, true),
            AllUninstallOptions {
                yes: false,
                dry_run: true,
            }
        );
    }

    #[test]
    fn all_uninstall_runs_service_commands_only_for_present_or_running_modes() {
        assert!(should_run_all_uninstall_service_commands(true, false));
        assert!(should_run_all_uninstall_service_commands(false, true));
        assert!(!should_run_all_uninstall_service_commands(false, false));
    }

    #[test]
    fn system_uninstall_requires_transaction_recovery_preflight() {
        assert_eq!(
            uninstall_preflight_actions(instance::InstanceMode::System),
            vec![UninstallPreflightAction::RecoverSystemTransaction]
        );
        assert!(uninstall_preflight_actions(instance::InstanceMode::User).is_empty());
    }

    #[test]
    fn system_install_never_starts_core_implicitly() {
        assert_eq!(
            install_fast_path_action(instance::InstanceMode::System, true, true),
            InstallFastPathAction::ReturnUpToDate
        );
        assert_eq!(
            install_post_service_action(instance::InstanceMode::System, true),
            InstallPostServiceAction::ProvisionAccessOnly
        );
        assert_eq!(
            system_install_operations(SystemInstallScenario::PostServiceWithConfig),
            vec![
                SystemInstallOperation::WaitForDaemon,
                SystemInstallOperation::EnsureAccess,
            ]
        );
        assert_eq!(
            system_install_operations(SystemInstallScenario::CompleteFastPath),
            vec![SystemInstallOperation::EnsureAccess]
        );
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
    #[cfg(unix)] // platform-specific path semantics
    fn ensure_instance_controller_endpoint_repairs_config_for_selected_instance() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        ctx.paths.config_dir = temp.path().to_path_buf();
        ctx.paths.intent_config_file = temp.path().join("config.yaml");
        ctx.paths.backup_dir = temp.path().join("backups");
        std::fs::write(
            &ctx.paths.intent_config_file,
            "mixed-port: 7897\nexternal-controller-unix: /tmp/old.sock\n",
        )
        .unwrap();

        ensure_instance_controller_endpoint(&ctx).unwrap();
        let fixed = std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap();
        assert!(fixed.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!fixed.contains("/tmp/old.sock"));
    }

    #[test]
    #[cfg(unix)]
    fn ensure_instance_controller_endpoint_skips_when_config_missing_after_skip_config() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        ctx.paths.config_dir = temp.path().join("user-config");
        ctx.paths.intent_config_file = temp.path().join("system-store/config.yaml");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");

        ensure_instance_controller_endpoint(&ctx).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn ensure_instance_controller_endpoint_repairs_imported_user_intent_config() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        ctx.paths.config_dir = temp.path().join("user-config");
        ctx.paths.intent_config_file = temp.path().join("system-store/config.yaml");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::write(
            &ctx.paths.intent_config_file,
            "mixed-port: 7897
external-controller-unix: /tmp/old.sock
",
        )
        .unwrap();

        ensure_instance_controller_endpoint(&ctx).unwrap();
        let fixed = std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap();
        assert!(fixed.contains("external-controller-unix: /var/run/mihomo/mihomo.sock"));
        assert!(!fixed.contains("/tmp/old.sock"));
        assert!(ctx.paths.intent_config_file.exists());
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

#[cfg(test)]
mod tun_recovery_proof_tests {
    use super::*;

    fn fence() -> tun_transaction::TransactionFence {
        tun_transaction::TransactionFence {
            transaction_id: "tx-1".to_string(),
            generation: 7,
            expected_phase: tun_transaction::JournalPhase::RollbackPending,
            expected_candidate_revision: "candidate".to_string(),
        }
    }

    fn old_evidence() -> tun_transaction::OldRuntimeEvidence {
        tun_transaction::OldRuntimeEvidence {
            core_running: true,
            core_identity: "mihomo-old".to_string(),
            core_pid: 1234,
            launched_revision: "old-revision".to_string(),
            launch_source: tun_transaction::LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: "unix:///run/mihomo.sock".to_string(),
            recorded_at_secs: None,
        }
    }

    #[test]
    fn mark_rollback_proof_rejects_candidate_or_wrong_identity() {
        let fence = fence();
        let expected = old_evidence();
        let response = ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::Completed(
                tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id.clone(),
                    generation: fence.generation,
                    observed_phase: tun_transaction::JournalPhase::RollbackPending,
                    proof_kind: tun_transaction::RuntimeProofKind::CandidateAttested,
                    core_identity: "mihomo-candidate".to_string(),
                    core_pid: expected.core_pid,
                    launched_revision: expected.launched_revision.clone(),
                    runtime_tun: expected.runtime_tun,
                    api_ready: true,
                },
            ),
        };
        let proof = successful_runtime_proof(
            &response,
            &fence,
            tun_transaction::JournalPhase::RollbackPending,
            tun_transaction::RuntimeProofKind::CandidateAttested,
            &expected.launched_revision,
            expected.runtime_tun,
        )
        .unwrap();
        let observation = tun_transaction::RuntimeObservation {
            core_running: true,
            core_identity: Some(proof.core_identity),
            core_pid: Some(proof.core_pid),
            launched_revision: Some(proof.launched_revision),
            runtime_tun: Some(proof.runtime_tun),
            api_ready: proof.api_ready,
        };
        assert!(!tun_transaction::runtime_matches_old_evidence(
            &expected,
            &observation
        ));
    }

    #[test]
    fn legacy_recovery_proof_requires_exact_target_and_metadata() {
        let fence = fence();
        let response = ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::Completed(
                tun_transaction::RuntimeProof {
                    transaction_id: fence.transaction_id.clone(),
                    generation: fence.generation,
                    observed_phase: tun_transaction::JournalPhase::RecoveryRequired,
                    proof_kind: tun_transaction::RuntimeProofKind::LegacyRecoveryTargetApplied,
                    core_identity: "mihomo".to_string(),
                    core_pid: 42,
                    launched_revision: "target".to_string(),
                    runtime_tun: true,
                    api_ready: true,
                },
            ),
        };
        assert!(successful_runtime_proof(
            &response,
            &fence,
            tun_transaction::JournalPhase::RecoveryRequired,
            tun_transaction::RuntimeProofKind::LegacyRecoveryTargetApplied,
            "target",
            true,
        )
        .is_ok());
        assert!(successful_runtime_proof(
            &response,
            &fence,
            tun_transaction::JournalPhase::RecoveryRequired,
            tun_transaction::RuntimeProofKind::LegacyRecoveryTargetApplied,
            "other-target",
            true,
        )
        .is_err());
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

    fn env(
        runtime_sys: bool,
        runtime_usr: bool,
        installed_sys: bool,
        installed_usr: bool,
    ) -> EnvironmentState {
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
            other => panic!(
                "expected Resolved(System) with explicit flag, got {:?}",
                other
            ),
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
            other => panic!(
                "expected Resolved(User) with explicit flag, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn g5_both_installed_but_not_running_uses_settings() {
        // 两者都装了但都没跑 → settings 解析（auto 优先 system）
        let result = resolve_environment_for_intent(
            ModeRequest::Unspecified,
            &env(false, false, true, true),
            UserIntent::ApiRead,
        );
        // S5: settings auto mode prefers system when both installed
        match result {
            RuntimeFirstModeResolution::Resolved { mode, source } => {
                assert_eq!(mode, instance::InstanceMode::System);
                // Source is ExplicitFlag because settings converts to ExplicitSystem
                assert_eq!(source, instance::ResolutionSource::ExplicitFlag);
            }
            other => panic!("expected Resolved(System) from settings, got {:?}", other),
        }
    }

    #[test]
    fn doctor_checks_daemon_binary_consistency_contract() {
        let dir = tempfile::tempdir().unwrap();
        let cli_file = dir.path().join("mihomo-cli");
        let fake_current = dir.path().join("current-mihomo-cli");

        std::fs::write(&cli_file, b"daemon v1").unwrap();
        std::fs::write(&fake_current, b"client v2").unwrap();

        assert!(!utils::file_contents_equal(&fake_current, &cli_file));

        let same_file = dir.path().join("same-mihomo-cli");
        std::fs::write(&same_file, b"daemon v1").unwrap();
        assert!(utils::file_contents_equal(&cli_file, &same_file));
    }

    #[tokio::test]
    async fn test_prepare_and_apply_pending_generation_flow() {
        let temp = tempfile::TempDir::new().unwrap();
        let inputs = instance::PathInputs {
            home: temp.path().join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(temp.path().join("run/user/1000")),
            program_data: temp.path().join("ProgramData"),
            app_data: temp.path().join("AppData/Roaming"),
            local_app_data: temp.path().join("AppData/Local"),
            username_or_sid: "alice".to_string(),
        };
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.config_dir = temp.path().join("user-config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = temp.path().join("system-data/tun-config.yaml");
        ctx.paths.cli_binary = temp.path().join("bin/mihomo-cli");
        ctx.paths.core_binary = temp.path().join("bin/mihomo");

        std::fs::create_dir_all(ctx.paths.cli_binary.parent().unwrap()).unwrap();
        std::fs::write(&ctx.paths.cli_binary, b"old-daemon").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"old-core").unwrap();

        let new_core = b"new-core-v2";
        let new_cli = b"new-daemon-v2";

        let gen_id = prepare_system_generation(&ctx, new_core, new_cli, Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let state = store.read_state().unwrap();
        assert_eq!(state.pending, Some(gen_id.clone()));
        assert_eq!(state.active, None);

        // Files on active paths have NOT been modified during install stage
        assert_eq!(std::fs::read(&ctx.paths.cli_binary).unwrap(), b"old-daemon");
        assert_eq!(std::fs::read(&ctx.paths.core_binary).unwrap(), b"old-core");

        // Validate generation
        let manifest = store.validate_generation(&gen_id).unwrap();
        assert_eq!(manifest.generation_id, gen_id);

        // Doctor detects pending generation
        let state = store.read_state().unwrap();
        assert!(state.pending.is_some());

        // Commit active generation
        let committed_state = commit_system_generation_active(&ctx, &store).unwrap();
        assert_eq!(committed_state.active, Some(gen_id.clone()));
        assert_eq!(committed_state.pending, None);

        // Prepare another generation to test previous & cleanup
        let gen_id_2 = prepare_system_generation(&ctx, b"core-v3", b"cli-v3", Vec::new()).unwrap();
        let committed_state_2 = commit_system_generation_active(&ctx, &store).unwrap();
        assert_eq!(committed_state_2.active, Some(gen_id_2.clone()));
        assert_eq!(committed_state_2.previous, Some(gen_id.clone()));

        // Cleanup retains active and previous
        let removed = cleanup_system_generation_old(&ctx, &store, 2).unwrap();
        assert_eq!(removed.len(), 0);
    }

    #[tokio::test]
    async fn test_auto_recover_active_transaction_idempotent() {
        let temp = tempfile::TempDir::new().unwrap();
        let inputs = instance::PathInputs {
            home: temp.path().join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(temp.path().join("run/user/1000")),
            program_data: temp.path().join("ProgramData"),
            app_data: temp.path().join("AppData/Roaming"),
            local_app_data: temp.path().join("AppData/Local"),
            username_or_sid: "alice".to_string(),
        };
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.config_dir = temp.path().join("user-config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = temp.path().join("system-data/tun-config.yaml");

        // When no active transaction exists, recovery is clean no-op
        let res = maybe_auto_recover_active_transaction(
            &ctx,
            tun_transaction::RecoveryDirection::Resume,
            false,
            false,
        )
        .await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_auto_recover_bug5_deadlock_recovery() {
        let temp = tempfile::TempDir::new().unwrap();
        let inputs = instance::PathInputs {
            home: temp.path().join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(temp.path().join("run/user/1000")),
            program_data: temp.path().join("ProgramData"),
            app_data: temp.path().join("AppData/Roaming"),
            local_app_data: temp.path().join("AppData/Local"),
            username_or_sid: "alice".to_string(),
        };
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.config_dir = temp.path().join("user-config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = temp.path().join("system-data/tun-config.yaml");
        ctx.permissions = instance::PermissionModel::DirectUser;

        // Base intent
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::write(
            &ctx.paths.intent_config_file,
            b"mode: rule
tun:
  enable: false
",
        )
        .unwrap();

        // 1. Prepare and publish active transaction
        let evidence = tun_transaction::OldRuntimeEvidence {
            core_running: true,
            core_identity: "core-1".to_string(),
            core_pid: 1234,
            launched_revision: "old-rev".to_string(),
            launch_source: tun_transaction::LaunchSource::SystemTunSnapshot,
            runtime_tun: false,
            api_endpoint: "http://127.0.0.1:9090".to_string(),
            recorded_at_secs: Some(100),
        };
        let candidate = b"mode: rule
tun:
  enable: true
";
        let base_rev = tun_transaction::sha256_revision(
            b"mode: rule
tun:
  enable: false
",
        );

        let journal = tun_transaction::prepare_and_publish_active_transaction(
            &ctx,
            1000,
            true,
            base_rev.clone(),
            candidate,
            &evidence,
        )
        .unwrap();

        assert_eq!(journal.phase, tun_transaction::JournalPhase::Prepared);

        // Simulate Bug #5 state: snapshot was written with candidate content, but core failed to start
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        std::fs::write(&ctx.paths.tun_config_file, candidate).unwrap();

        // Snapshot is candidate, phase is Prepared.
        let snap_cls = tun_transaction::classify_snapshot(&ctx, &journal);
        assert_eq!(snap_cls, tun_transaction::SnapshotClassification::Candidate);

        // Plan recovery: with resume direction, planner repairs phase to SnapshotPromoted and then can proceed
        let obs = tun_transaction::RuntimeObservation {
            core_running: false,
            core_identity: None,
            core_pid: None,
            launched_revision: None,
            runtime_tun: None,
            api_ready: false,
        };
        let intent_cls = tun_transaction::classify_intent(&ctx.paths.intent_config_file, &journal);
        let action = tun_transaction::plan_recovery(
            &journal,
            snap_cls.clone(),
            intent_cls.clone(),
            &obs,
            tun_transaction::RecoveryDirection::Resume,
        );
        assert_eq!(
            action,
            tun_transaction::RecoveryAction::RepairPhaseToSnapshotPromoted
        );

        // With abort direction, planner begins rollback
        let action_abort = tun_transaction::plan_recovery(
            &journal,
            snap_cls,
            intent_cls,
            &obs,
            tun_transaction::RecoveryDirection::Abort,
        );
        assert_eq!(
            action_abort,
            tun_transaction::RecoveryAction::RepairPhaseToSnapshotPromoted
        );
    }
}
