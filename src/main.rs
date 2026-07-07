use clap::{Parser, Subcommand, ValueEnum};

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        if crate::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("[DEBUG] {}", format!($($arg)*));
        }
    };
}

pub static VERBOSE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

mod config;
mod installer;
mod mihomo_api;
mod service;
mod ui;
mod utils;

#[derive(Parser)]
#[command(name = "mihomo-cli", version, about = "Mihomo CLI — cross-platform setup & control tool", long_about = None)]
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
        /// Install as user-level service (default: root/system service)
        #[arg(short, long)]
        user: bool,
        /// Force reinstall even if already installed
        #[arg(short, long)]
        force: bool,
    },

    /// Configure subscription URL, fix config, or refresh from saved URL
    #[command(visible_alias = "c")]
    Config {
        /// Subscription URL (for initial setup or update)
        #[arg(short, long)]
        url: Option<String>,
        /// Fix the existing config file: ensure Unix socket is configured
        #[arg(long)]
        fix: bool,
        /// Refresh subscription from the previously saved URL
        #[arg(long)]
        refresh: bool,
    },

    /// Remove service and optionally all files
    #[command(visible_alias = "u")]
    Uninstall {
        /// Also remove mihomo binary
        #[arg(short, long)]
        all: bool,
    },

    /// Update mihomo core binary
    #[command(visible_alias = "up")]
    Update,

    // --- Control commands ---
    /// Start mihomo service
    Start {
        /// Use user-level service instead of root
        #[arg(short, long)]
        user: bool,
    },

    /// Stop mihomo service
    Stop {
        /// Use user-level service instead of root
        #[arg(short, long)]
        user: bool,
    },

    /// Restart mihomo service
    Restart {
        /// Use user-level service instead of root
        #[arg(short, long)]
        user: bool,
    },

    /// Interactive fuzzy-select a proxy node (flat list of all groups by default)
    Select {
        /// Limit to a specific proxy group
        #[arg(short, long)]
        group: Option<String>,
    },

    /// List all proxy groups and current nodes
    List,

    /// Test latency of nodes in a group
    Delay {
        #[arg(short, long, default_value = "节点选择")]
        group: String,
    },

    /// Toggle or check TUN mode
    #[command(name = "tun")]
    Tun {
        action: Option<TunAction>,
    },

    /// View active connections (use --flush to close all)
    #[command(name = "conn")]
    Connections {
        /// Close all active connections
        #[arg(short, long)]
        flush: bool,
    },

    /// Set or unset shell proxy environment variables (use with eval)
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
    },

    /// Show running status overview (includes exit IP)
    Status,

    /// Check current exit IP and location
    Ip,
}

#[derive(ValueEnum, Clone)]
enum TunAction { On, Off }

#[derive(Subcommand, Clone)]
enum ProxyAction {
    /// Output export commands for http_proxy / https_proxy
    On,
    /// Output unset commands for proxy variables
    Off,
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
    match cli.command.unwrap_or(Command::Install { user: false, force: false }) {
        Command::Install { user, force } => cmd_install(user, force).await,
        Command::Config { url, fix, refresh } => cmd_config(url, fix, refresh).await,
        Command::Uninstall { all } => cmd_uninstall(all),
        Command::Update => cmd_update().await,
        Command::Start { user } => service::start_mihomo(user),
        Command::Stop { user } => service::stop_mihomo(user),
        Command::Restart { user } => service::restart_mihomo(user),
        Command::Select { group } => match group {
            Some(g) => ui::select_node(&g).await,
            None => ui::flat_select().await,
        },
        Command::List => mihomo_api::list_proxies().await,
        Command::Delay { group } => mihomo_api::delay_test(&group).await,
        Command::Tun { action } => mihomo_api::tun_toggle(action).await,
        Command::Connections { flush } => mihomo_api::connections(flush).await,
        Command::Proxy { action } => cmd_proxy(action).await,
        Command::Status => mihomo_api::status().await,
        Command::Ip => cmd_ip().await,
    }
}

// ── Command implementations ──

async fn cmd_install(user_mode: bool, force: bool) -> anyhow::Result<()> {
    if !force && config::check_config_exists() && service::service_installed() {
        println!("Already installed. Use --force to reinstall.");
        return Ok(());
    }

    println!("=== mihomo-cli install ({}) ===\n", std::env::consts::OS);

    println!("[1/3] Mihomo core binary...");
    installer::download_mihomo().await
        .map_err(|e| anyhow::anyhow!("Failed to download mihomo: {e}\n  Check network and try --verbose for details"))?;

    println!();
    println!("[2/3] Start script...");
    write_start_script()?;

    println!();
    println!("[3/3] Configuration...");
    if config::check_config_exists() {
        println!("  config.yaml already exists, skipped");
        log!("Config at {}", utils::config_path());
    } else {
        setup_config_interactive().await?;
    }

    println!();
    println!("[4/4] Geo data files...");
    installer::ensure_geo_files().await;

    println!();
    println!("=== Done ===");
    println!("  mihomo-cli restart    start/restart service");
    println!("  mihomo-cli select     select proxy node");
    println!("  mihomo-cli status     check status + exit IP");
    println!("  mihomo-cli tun on     enable TUN mode");

    // Ask if user wants to install and start service
    use dialoguer::Confirm;
    let mode_label = if user_mode { "user-level" } else { "root (system)" };
    println!();
    if Confirm::new()
        .with_prompt(format!("Install and start {mode_label} service?"))
        .default(true)
        .interact()?
    {
        service::install_service(user_mode)
            .map_err(|e| anyhow::anyhow!("Service install failed: {e}"))?;
    } else {
        println!("  Skipped. Run: mihomo-cli restart");
    }

    Ok(())
}

async fn setup_config_interactive() -> anyhow::Result<()> {
    // Try Clash Verge Rev config first (macOS only)
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().unwrap_or_default();
        let cv_config = format!("{}/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev/clash-verge.yaml", home.display());
        if std::path::Path::new(&cv_config).exists() {
            let dest = utils::config_path();
            std::fs::create_dir_all(utils::config_dir())?;
            let content = std::fs::read_to_string(&cv_config)?;
            let patched = config::ensure_controller(&content);
            std::fs::write(&dest, &patched)?;
            println!("  Copied config from Clash Verge Rev");
            log!("Source: {cv_config}");
            log!("Dest: {dest}");
            return Ok(());
        }
    }

    use dialoguer::Input;
    let url: String = Input::new()
        .with_prompt("Subscription URL (Enter to skip)")
        .allow_empty(true)
        .interact_text()?;

    if url.is_empty() {
        println!("  Skipped. Place config manually at {}", utils::config_path());
        return Ok(());
    }

    apply_subscription(&url).await
}

async fn apply_subscription(url: &str) -> anyhow::Result<()> {
    log!("Downloading subscription from: {url}");
    let (content, is_yaml) = config::download_sub_smart(url).await
        .map_err(|e| anyhow::anyhow!("Cannot reach subscription URL.\n  {e}\n  Check your network or the URL"))?;

    if is_yaml {
        log!("Format: Clash YAML (UA negotiation succeeded)");
        config::save_config(&content)?;
        println!("  Config saved ({} lines)", content.lines().count());
        // Pre-download geo files so mihomo doesn't deadlock on startup
        installer::ensure_geo_files().await;
    } else {
        log!("Format: raw subscription — converting");
        println!("  Converting subscription format (vmess/base64 → Clash YAML)...");
        let clash_config = config::convert_vmess_to_clash(&content)
            .map_err(|e| anyhow::anyhow!(
                "Failed to convert subscription.\n  \
                 The server returned a non-Clash format and conversion failed.\n  \
                 Error: {e}\n  \
                 Tip: Try opening the URL in a browser to verify it."
            ))?;
        config::save_config(&clash_config)?;
        let count = clash_config.lines()
            .filter(|l| l.trim().starts_with("- name:") && !l.contains("节点选择") && !l.contains("自动选择"))
            .count();
        println!("  Converted {count} proxies to Clash format");
        // Pre-download geo files so mihomo doesn't deadlock on startup
        installer::ensure_geo_files().await;
    }

    // Validate config
    log!("Validating config with mihomo -t...");
    let mihomo = utils::mihomo_path();
    if std::path::Path::new(&mihomo).exists() {
        let output = std::process::Command::new(&mihomo)
            .args(["-t", "-d", &utils::config_dir()])
            .output();
        match output {
            Ok(o) if o.status.success() => println!("  Config validated OK"),
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                log!("Config test failed:\n{stderr}");
                println!("  Warning: config test failed — saved anyway. Check with -v");
            }
            Err(e) => log!("mihomo -t not available: {e}"),
        }
    }
    Ok(())
}

fn write_start_script() -> anyhow::Result<()> {
    if cfg!(target_os = "windows") {
        std::fs::create_dir_all(utils::config_dir())?;
        log!("Windows: no shell wrapper needed");
        return Ok(());
    }
    let script = service::start_script_content();
    let script_path = utils::start_script_path();
    std::fs::create_dir_all(utils::config_dir())?;
    std::fs::write(&script_path, &script)?;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?; }
    log!("Created {}", script_path);
    Ok(())
}

async fn cmd_config(url: Option<String>, fix: bool, refresh: bool) -> anyhow::Result<()> {
    if fix {
        if config::fix_existing_config() {
            println!("  Fixed config: added Unix socket controller.");
        } else {
            println!("  Config already has Unix socket — no fix needed.");
        }
        if mihomo_api::reload_configs().await.is_ok() {
            println!("  Config hot-reloaded.");
        } else {
            println!("  Run: mihomo-cli restart");
        }
        return Ok(());
    }

    // Refresh from saved URL
    if refresh {
        let urls = utils::read_subscription_urls();
        match urls.first() {
            Some(active_url) => {
                println!("  Refreshing active subscription...");
                apply_subscription(active_url).await?;
                if mihomo_api::reload_configs().await.is_ok() {
                    println!("  Config reloaded.");
                } else {
                    println!("  Run: mihomo-cli restart");
                }
                return Ok(());
            }
            None => {
                anyhow::bail!("No saved subscription URL found.\n  Run: mihomo-cli config -u <URL>  to set one first");
            }
        }
    }

    // If no URL arg and no flags, show interactive management menu
    let url = match url {
        Some(u) => Some(u),
        None => {
            show_config_menu().await?
        }
    };

    let url = match url {
        Some(u) => u,
        None => return Ok(()), // already handled by menu (e.g. refresh or cancel)
    };
    apply_subscription(&url).await?;

    // Save URL as active (first in the multi-URL list)
    let mut urls = utils::read_subscription_urls();
    if urls.first().map(|s| s == &url).unwrap_or(false) {
        // Already active — nothing to do
    } else {
        urls.insert(0, url.clone());
        utils::write_subscription_urls(&urls)?;
        println!("  Subscription URL saved.");
    }

    // Try to hot-reload if mihomo is running
    if mihomo_api::reload_configs().await.is_ok() {
        println!("  Config reloaded");
    } else {
        println!();
        println!("  Run: mihomo-cli restart");
    }
    Ok(())
}

/// Interactive subscription management — pick/refresh/add/remove sources.
/// Returns `Some(url)` if caller should apply it, `None` if already handled.
///
/// Navigation: ↑↓/j/k move, Enter select, ESC go back, ESC+ESC exit.
async fn show_config_menu() -> anyhow::Result<Option<String>> {
    use dialoguer::{Input, Select};

    let mut esc_count = 0u8;

    loop {
        let urls = utils::read_subscription_urls();
        println!();

        // Build menu items: URLs + action rows
        let mut items: Vec<String> = urls.iter().enumerate().map(|(i, u)| {
            let short = if u.len() > 55 {
                format!("  {}…{}", &u[..30], &u[u.len()-20..])
            } else {
                u.clone()
            };
            if i == 0 {
                format!("▶ {}", short)
            } else {
                format!("  {}", short)
            }
        }).collect();

        if items.is_empty() {
            items.push("  (no sources saved)".to_string());
        }
        items.push("───".to_string());
        items.push("+ Add new source".to_string());
        if !urls.is_empty() {
            items.push("- Remove a source".to_string());
        }
        items.push("Cancel".to_string());

        let url_count = urls.len();
        let choice = Select::new()
            .with_prompt("Subscription sources (Esc to go back)")
            .items(&items)
            .default(0)
            .interact_opt()?;

        let choice = match choice {
            Some(c) => {
                esc_count = 0;
                c
            }
            None => {
                // ESC pressed
                esc_count += 1;
                if esc_count >= 2 {
                    anyhow::bail!("Cancelled.");
                }
                eprintln!("  Press Esc again to exit.");
                continue;
            }
        };

        let selected = &items[choice];

        // User picked a URL → use it
        if choice < url_count {
            let url = &urls[choice];
            println!("  Downloading from {}...", &url[..url.find('?').unwrap_or(url.len()).min(40)]);
            apply_subscription(url).await?;
            let mut new_urls = urls.clone();
            new_urls.remove(choice);
            new_urls.insert(0, url.clone());
            utils::write_subscription_urls(&new_urls)?;
            if mihomo_api::reload_configs().await.is_ok() {
                println!("  Config reloaded.");
            } else {
                println!("  Run: mihomo-cli restart");
            }
            return Ok(None);
        }

        if selected == "───" { continue; }

        if selected == "+ Add new source" || selected.contains("(no sources saved)") {
            let new_url: String = Input::new()
                .with_prompt("Subscription URL")
                .allow_empty(false)
                .interact_text()?;
            utils::add_subscription_url(&new_url)?;
            println!("  Saved.");
        } else if selected == "- Remove a source" {
            let remove_items: Vec<String> = urls.iter().enumerate().map(|(i, u)| {
                format!("{}: {}", i+1, &u[..u.find('?').unwrap_or(u.len()).min(50)])
            }).collect();
            let rem = Select::new()
                .with_prompt("Remove which source? (Esc to go back)")
                .items(&remove_items)
                .interact_opt()?;
            match rem {
                Some(idx) => {
                    utils::remove_subscription_url(idx)?;
                    println!("  Removed.");
                }
                None => {
                    // ESC → back to main menu
                    continue;
                }
            }
        } else {
            anyhow::bail!("Cancelled.");
        }
    }
}

async fn cmd_proxy(action: ProxyAction) -> anyhow::Result<()> {
    match action {
        ProxyAction::On => {
            let port = mihomo_api::get_port().await?;
            println!("export http_proxy=http://127.0.0.1:{port}");
            println!("export https_proxy=http://127.0.0.1:{port}");
        }
        ProxyAction::Off => {
            println!("unset http_proxy https_proxy all_proxy");
        }
    }
    Ok(())
}

async fn cmd_ip() -> anyhow::Result<()> {
    match mihomo_api::fetch_ip_info().await {
        Ok((ip, country, source)) => {
            println!("  Exit IP: {} ({}) via {}", ip, country, source);
            Ok(())
        }
        Err(_) => {
            anyhow::bail!("Failed to fetch IP info.\n  Is mihomo running? Run: mihomo-cli status");
        }
    }
}

fn cmd_uninstall(all: bool) -> anyhow::Result<()> {
    use dialoguer::Confirm;
    let service_exists = service::service_installed();
    let mihomo_exists = std::path::Path::new(&utils::mihomo_path()).exists();
    if !service_exists && !mihomo_exists {
        println!("Nothing to uninstall.");
        return Ok(());
    }

    println!("=== mihomo-cli uninstall ===\n");
    println!("This will:");
    if mihomo_exists { println!("  - Stop running mihomo process"); }
    if service_exists { println!("  - Remove auto-start service"); }
    if all {
        println!("  - Delete mihomo binary ({})", utils::mihomo_path());
    }
    println!("  - Keep config at {}", utils::config_dir());
    println!();

    let prompt = if all { "Proceed with full removal?" } else { "Proceed?" };
    if !Confirm::new().with_prompt(prompt).default(false).interact()? { println!("Cancelled."); return Ok(()); }

    if mihomo_exists {
        println!("\nStopping mihomo...");
        service::kill_mihomo();
        // Clean up stale socket
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file("/tmp/verge/verge-mihomo.sock");
            log!("Cleaned up socket");
        }
    }
    if service_exists {
        println!("Removing service...");
        service::uninstall_service()?;
        log!("Service uninstalled");
    }
    if all {
        println!("Removing binaries...");
        if std::path::Path::new(&utils::mihomo_path()).exists() {
            std::fs::remove_file(utils::mihomo_path())?;
            log!("Removed {}", utils::mihomo_path());
        }
        let sp = utils::start_script_path();
        if std::path::Path::new(&sp).exists() {
            std::fs::remove_file(&sp)?;
            log!("Removed {sp}");
        }
    }

    // Clean up partial geo downloads
    let dir = utils::config_dir();
    for name in &["geoip.metadb.tmp", "GeoSite.dat.tmp"] {
        let _ = std::fs::remove_file(format!("{dir}/{name}"));
    }

    println!("Done.");
    Ok(())
}

async fn cmd_update() -> anyhow::Result<()> {
    let bin = utils::mihomo_path();
    if !std::path::Path::new(&bin).exists() {
        anyhow::bail!("mihomo not installed at {bin}\n  Run: mihomo-cli install");
    }
    println!("Updating mihomo core...");
    let bak = format!("{bin}.bak");
    std::fs::rename(&bin, &bak)?;
    log!("Backed up {bin} -> {bak}");
    match installer::download_mihomo().await {
        Ok(()) => {
            std::fs::remove_file(bak)?;
            log!("Removed backup");
            println!("Updated successfully");
        }
        Err(e) => {
            std::fs::rename(&bak, &bin)?;
            log!("Restored backup");
            anyhow::bail!("Update failed: {e}\n  Original binary restored");
        }
    }
    Ok(())
}
