use clap::{ArgGroup, Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub(crate) enum ConfigSubcommand {
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
    /// Validate current config.yaml with YAML parser and mihomo -t (same as `config --validate`)
    Validate,
}

#[derive(Parser)]
#[command(name = "mihomo-cli", version = env!("MIHOMO_CLI_VERSION"), about = "Mihomo CLI — cross-platform setup & control tool", long_about = None)]
pub(crate) struct Cli {
    /// Enable verbose debug output
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,

    /// Output a machine-readable JSON envelope on stdout
    #[arg(long, global = true)]
    pub(crate) json: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand)]
// Parsed once at process startup. Keeping the clap derive command tree readable
// is more valuable here than boxing the largest variant to save a few hundred bytes.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Command {
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
        #[arg(long = "system", conflicts_with = "user")]
        system: bool,
        /// Force the per-user service instance
        #[arg(long = "user", conflicts_with = "system")]
        user: bool,
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

#[derive(Clone, ValueEnum)]
pub(crate) enum AutostartAction {
    /// Enable autostart
    On,
    /// Disable autostart
    Off,
    /// Query autostart state
    Status,
}

#[derive(Clone, ValueEnum)]
pub(crate) enum UseMode {
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
pub(crate) enum TunAction {
    On,
    Off,
    Status,
}

#[derive(ValueEnum, Clone)]
pub(crate) enum TunStack {
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
pub(crate) enum ProxyAction {
    /// Output export commands for http_proxy / https_proxy
    On,
    /// Output unset commands for proxy variables
    Off,
}

#[derive(Subcommand, Clone)]
pub(crate) enum SystemProxyAction {
    /// Enable OS system proxy to current mihomo port
    On,
    /// Disable OS system proxy
    Off,
}

#[derive(Subcommand, Clone)]
pub(crate) enum GroupAction {
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
        url: Option<String>,
        #[arg(long)]
        interval: Option<u64>,
        #[arg(long)]
        strategy: Option<String>,
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
    /// Reset an original group to its upstream state (removes all patches and unhides)
    Reset { name: String },
}

#[derive(Subcommand, Clone)]
pub(crate) enum RuleAction {
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
pub(crate) enum DnsAction {
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
pub(crate) enum DnsTemplateAction {
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
pub(crate) enum OverrideAction {
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
pub(crate) enum DnsFakeIpFilterAction {
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
pub(crate) enum DnsPolicyAction {
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
