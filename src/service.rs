use std::process::Command;
use crate::utils;

/// Resolve service mode: explicit --user flag overrides, otherwise auto-detect from marker.
fn resolve_mode(forced: bool) -> String {
    if forced {
        "user".to_string()
    } else {
        utils::read_service_mode()
    }
}

pub fn install_service(user_mode: bool) -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        install_launchdaemon()
    } else if cfg!(target_os = "linux") {
        if user_mode {
            install_systemd_user()
        } else {
            install_systemd_system()
        }
    } else if cfg!(target_os = "windows") {
        install_windows()
    } else {
        anyhow::bail!("Unsupported OS")
    }
}

pub fn uninstall_service() -> anyhow::Result<()> {
    if cfg!(target_os = "macos") {
        uninstall_launchdaemon()
    } else if cfg!(target_os = "linux") {
        uninstall_systemd()
    } else if cfg!(target_os = "windows") {
        uninstall_windows()
    } else {
        anyhow::bail!("Unsupported OS")
    }
}

pub fn start_mihomo(user: bool) -> anyhow::Result<()> {
    // Pre-download geo files to avoid chicken-and-egg deadlock on startup
    println!("  Geo data...");
    tokio::task::block_in_place(|| {
        let handle = tokio::runtime::Handle::current();
        handle.block_on(crate::installer::ensure_geo_files());
    });

    if service_installed() {
        let mode = resolve_mode(user);
        if cfg!(target_os = "linux") && mode != "root" {
            println!("Starting mihomo via user service...");
            let status = Command::new("systemctl")
                .args(["--user", "start", "mihomo"])
                .status()?;
            if !status.success() {
                anyhow::bail!("Failed to start service (exit: {})", status);
            }
        } else {
            println!("Starting mihomo via service...");
            let cmd: &[&str] = if cfg!(target_os = "linux") {
                &["systemctl", "start", "mihomo"]
            } else if cfg!(target_os = "macos") {
                &["launchctl", "start", "io.mihomo"]
            } else {
                &["sc.exe", "start", "mihomo"]
            };
            run_privileged(cmd)?;
        }
    } else {
        println!("No service installed, starting mihomo directly...");
        let mihomo = utils::mihomo_path();
        let config = utils::config_dir();
        if !std::path::Path::new(&mihomo).exists() {
            anyhow::bail!("mihomo binary not found at {mihomo}\n  Run: mihomo-cli install");
        }
        if !std::path::Path::new(&utils::config_path()).exists() {
            anyhow::bail!("No config found.\n  Run: mihomo-cli config");
        }
        Command::new("nohup")
            .args([&mihomo, "-d", &config])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
    }

    std::thread::sleep(std::time::Duration::from_secs(2));

    // Verify mihomo actually started
    let running = if cfg!(target_os = "windows") {
        Command::new("tasklist").args(["/FI", "IMAGENAME eq mihomo.exe"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("mihomo"))
            .unwrap_or(false)
    } else {
        Command::new("pgrep").args(["-x", "mihomo"]).output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };

    if !running {
        let log = utils::log_path();
        anyhow::bail!("mihomo failed to start.\n  Check logs: tail -20 {log}");
    }

    // Verify the API socket is actually reachable (not just the process)
    if !crate::mihomo_api::socket_file_exists() {
        println!("  Socket not ready — restarting to recreate...");
        kill_mihomo();
        std::thread::sleep(std::time::Duration::from_secs(3));

        let still_running = if cfg!(target_os = "windows") {
            Command::new("tasklist").args(["/FI", "IMAGENAME eq mihomo.exe"]).output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("mihomo"))
                .unwrap_or(false)
        } else {
            Command::new("pgrep").args(["-x", "mihomo"]).output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };

        if still_running && crate::mihomo_api::socket_file_exists() {
            if check_api_ready() {
                println!("Done. Run: mihomo-cli status");
            } else {
                println!("  Check: mihomo-cli status");
            }
        } else {
            anyhow::bail!("Failed to recreate socket.\n  Check logs: tail -20 {}", utils::log_path());
        }
    } else {
        if check_api_ready() {
            println!("Done. Run: mihomo-cli status");
        } else {
            println!("  Check: mihomo-cli status");
        }
    }
    Ok(())
}

/// Wait for mihomo API to become responsive (socket exists but HTTP server may still be initializing).
fn check_api_ready() -> bool {
    tokio::task::block_in_place(|| {
        let handle = tokio::runtime::Handle::current();
        let ready = handle.block_on(crate::mihomo_api::wait_for_api_ready(15));
        if !ready {
            eprintln!("  ⚠ API not responding after 15s — mihomo may still be initializing.");
        }
        ready
    })
}

pub fn stop_mihomo(user: bool) -> anyhow::Result<()> {
    if service_installed() {
        let mode = resolve_mode(user);
        if cfg!(target_os = "linux") && mode != "root" {
            println!("Stopping mihomo via user service...");
            let status = Command::new("systemctl")
                .args(["--user", "stop", "mihomo"])
                .status()?;
            if !status.success() {
                anyhow::bail!("Failed to stop service (exit: {})", status);
            }
        } else {
            println!("Stopping mihomo via service...");
            let cmd: &[&str] = if cfg!(target_os = "linux") {
                &["systemctl", "stop", "mihomo"]
            } else if cfg!(target_os = "macos") {
                &["launchctl", "stop", "io.mihomo"]
            } else {
                &["sc.exe", "stop", "mihomo"]
            };
            run_privileged(cmd)?;
        }
    } else {
        println!("Stopping mihomo...");
        kill_mihomo();
    }
    println!("Done.");
    Ok(())
}

pub fn restart_mihomo(user: bool) -> anyhow::Result<()> {
    println!("Restarting mihomo...");

    // Pre-download geo files to avoid chicken-and-egg deadlock on startup
    println!("  Geo data...");
    tokio::task::block_in_place(|| {
        let handle = tokio::runtime::Handle::current();
        handle.block_on(crate::installer::ensure_geo_files());
    });

    if service_installed() {
        crate::log!("service detected, using restart_service");

        let svc_mode = resolve_mode(user);
        println!("  Service: {} ({})",
            if cfg!(target_os = "linux") { "systemd" } else { "launchd" },
            svc_mode
        );

        if restart_service(&svc_mode) {
            // Brief wait then verify socket
            std::thread::sleep(std::time::Duration::from_secs(2));
            let sock_ok = crate::mihomo_api::socket_file_exists();
            if sock_ok {
                println!("  Socket recreated.");
                check_api_ready();
            } else {
                // Try fixing the config: ensure Unix socket line exists
                let fixed = crate::config::fix_existing_config();
                if fixed {
                    println!("  ⚠ Config was missing Unix socket — fixed.");
                    println!("  Restarting again...");
                    if restart_service(&svc_mode) {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                        if crate::mihomo_api::socket_file_exists() {
                            println!("  Socket recreated.");
                            check_api_ready();
                            return Ok(());
                        }
                    }
                }
                println!("  ⚠ mihomo restarted but socket not found.");
                println!("     Check: sudo journalctl -u mihomo -n 20 --no-pager");
            }
            return Ok(());
        }
        anyhow::bail!("Restart command failed.\n  Try manually: sudo systemctl restart mihomo");
    }

    crate::log!("no service detected, using direct restart");
    stop_mihomo(user)?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    start_mihomo(user)?;
    Ok(())
}

/// Run `sudo systemctl restart mihomo` (or `--user` variant) with a 30-second timeout.
/// Returns true if the command succeeded within the timeout.
pub fn restart_service(mode: &str) -> bool {
    if cfg!(target_os = "linux") {
        if mode != "root" {
            run_userctl(&["restart", "mihomo"], std::time::Duration::from_secs(30))
        } else {
            run_sudo_with_timeout(
                &["systemctl", "restart", "mihomo"],
                std::time::Duration::from_secs(30),
            )
        }
    } else if cfg!(target_os = "macos") {
        run_sudo_with_timeout(
            &["launchctl", "stop", "io.mihomo"],
            std::time::Duration::from_secs(30),
        ) && {
            std::thread::sleep(std::time::Duration::from_secs(1));
            run_sudo_with_timeout(
                &["launchctl", "start", "io.mihomo"],
                std::time::Duration::from_secs(30),
            )
        }
    } else if cfg!(target_os = "windows") {
        Command::new("sc.exe")
            .args(["stop", "mihomo"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            && {
                std::thread::sleep(std::time::Duration::from_secs(1));
                Command::new("sc.exe")
                    .args(["start", "mihomo"])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
            }
    } else {
        false
    }
}

/// Run a user-level systemctl command with timeout.
fn run_userctl(args: &[&str], timeout: std::time::Duration) -> bool {
    let mut full_args = vec!["--user"];
    full_args.extend_from_slice(args);
    let status = Command::new("systemctl")
        .args(&full_args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn();
    match status {
        Ok(child) => wait_child(child, timeout),
        Err(_) => false,
    }
}

/// Run a privileged command with smart dispatch (root/cached/prompt).
/// Returns `Ok(())` on success, or a descriptive error on failure.
fn run_privileged(args: &[&str]) -> anyhow::Result<()> {
    if run_sudo_with_timeout(args, std::time::Duration::from_secs(30)) {
        Ok(())
    } else {
        anyhow::bail!("Command failed. Try manually: sudo {}", args.join(" "))
    }
}

/// Run a command that may need root privileges.
///
/// Smart dispatch:
///   1. Already root → run directly, no sudo, no prompt.
///   2. Sudo credentials cached (e.g. `sudo mihomo-cli ...`) → `sudo -n`, no prompt.
///   3. Otherwise → prompt for password via dialoguer, pipe to `sudo -S`.
///
/// The user never interacts with sudo directly — only with mihomo-cli.
fn run_sudo_with_timeout(args: &[&str], timeout: std::time::Duration) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // ── Case 1: Already root ──
    if is_root() {
        crate::log!("already root, running directly");
        let status = Command::new(args[0])
            .args(&args[1..])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        return match status {
            Ok(s) => s.success(),
            Err(_) => false,
        };
    }

    // ── Case 2: Sudo credentials already cached ──
    if sudo_credentials_cached() {
        crate::log!("sudo credentials cached, using sudo -n");
        let status = Command::new("sudo")
            .arg("-n")
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        return match status {
            Ok(s) => s.success(),
            Err(_) => false,
        };
    }

    // ── Case 3: Need password ──
    eprintln!("  The mihomo service runs as root.");
    eprintln!("  Restarting it requires admin privileges.");

    let password = dialoguer::Password::new()
        .with_prompt("sudo password")
        .allow_empty_password(false)
        .interact();

    let password = match password {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprintln!("  No password provided — aborting.");
            eprintln!("  Run manually: sudo systemctl restart mihomo");
            return false;
        }
    };

    // Spawn sudo -S (reads password from stdin, no TTY dependency)
    let mut child = match Command::new("sudo")
        .arg("-S")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  Failed to run sudo: {e}");
            eprintln!("  Try: sudo systemctl restart mihomo");
            return false;
        }
    };

    // Pipe password to sudo's stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(format!("{}\n", password).as_bytes());
    }

    wait_child(child, timeout)
}

/// Check if we're already running as root.
fn is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|o| o.stdout.starts_with(b"0"))
        .unwrap_or(false)
}

/// Check if sudo credentials are already cached (non-interactive check).
fn sudo_credentials_cached() -> bool {
    std::process::Command::new("sudo")
        .args(["-n", "true"])  // -n: non-interactive, exits immediately if password needed
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Wait for a child process with timeout.
fn wait_child(mut child: std::process::Child, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => return false,
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("  Command timed out after {}s.", timeout.as_secs());
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

pub fn service_installed() -> bool {
    if cfg!(target_os = "macos") {
        std::path::Path::new("/Library/LaunchDaemons/io.mihomo.plist").exists()
    } else if cfg!(target_os = "linux") {
        // Check actual files on disk — don't rely solely on the service-mode marker
        let sys = std::path::Path::new("/etc/systemd/system/mihomo.service");
        if sys.exists() {
            // Ensure marker reflects reality
            let _ = utils::write_service_mode("root");
            return true;
        }
        let home = dirs::home_dir().unwrap_or_default();
        let user_path = format!("{}/.config/systemd/user/mihomo.service", home.display());
        let user = std::path::Path::new(&user_path);
        if user.exists() {
            let _ = utils::write_service_mode("user");
            return true;
        }
        false
    } else if cfg!(target_os = "windows") {
        let output = Command::new("sc.exe").args(["query", "mihomo"]).output();
        match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains("STATE"),
            Err(_) => false,
        }
    } else {
        false
    }
}

pub fn kill_mihomo() {
    if cfg!(target_os = "windows") {
        let _ = Command::new("taskkill").args(["/F", "/IM", "mihomo.exe"]).status();
        return;
    }

    // Try direct kill first (works if same user)
    let direct_ok = Command::new("pkill")
        .args(["-9", "-x", "mihomo"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if direct_ok {
        return;
    }

    // If direct failed, try with sudo (root-mode service).
    // Print to stderr so the user sees the sudo password prompt.
    eprintln!("  mihomo runs as root — sudo required to restart.");
    let _ = Command::new("sudo")
        .args(["pkill", "-9", "-x", "mihomo"])
        .status();
}

pub fn start_script_content() -> String {
    if cfg!(target_os = "windows") {
        "".to_string()
    } else {
        let home = dirs::home_dir().unwrap_or_default().display().to_string();
        format!(
            "#!/bin/bash\nmkdir -p /tmp/verge && chmod 777 /tmp/verge\nexec \"{home}/.local/bin/mihomo\" -d \"{home}/.config/mihomo\"\n"
        )
    }
}

// --- macOS ---

fn install_launchdaemon() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let plist = format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>io.mihomo</string>
<key>ProgramArguments</key><array><string>{home}/.config/mihomo/start.sh</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>WorkingDirectory</key><string>{home}/.config/mihomo</string>
<key>StandardOutPath</key><string>{home}/.config/mihomo/mihomo.log</string>
<key>StandardErrorPath</key><string>{home}/.config/mihomo/mihomo.log</string>
</dict></plist>
"#
    );
    let plist_path = "/Library/LaunchDaemons/io.mihomo.plist";

    println!("Creating {plist_path} (sudo required)...");
    let mut child = Command::new("sudo")
        .args(["tee", plist_path])
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    use std::io::Write;
    child.stdin.take().unwrap().write_all(plist.as_bytes())?;
    child.wait()?;

    Command::new("sudo").args(["chmod", "644", plist_path]).status()?;
    Command::new("sudo").args(["launchctl", "load", plist_path]).status()?;

    // Write service mode marker
    utils::write_service_mode("root")?;

    // Verify mihomo started
    std::thread::sleep(std::time::Duration::from_secs(3));
    let running = std::process::Command::new("pgrep")
        .args(["-x", "mihomo"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if running {
        println!("LaunchDaemon installed and started — mihomo is running");
    } else {
        println!("LaunchDaemon installed but mihomo may not be running.");
        println!("Check logs: tail -f {home}/.config/mihomo/mihomo.log");
    }
    Ok(())
}

fn uninstall_launchdaemon() -> anyhow::Result<()> {
    let p = "/Library/LaunchDaemons/io.mihomo.plist";
    if !std::path::Path::new(p).exists() { println!("No LaunchDaemon found"); return Ok(()); }
    let _ = Command::new("sudo").args(["launchctl", "bootout", "system", p]).status();
    let _ = Command::new("sudo").args(["rm", p]).status();
    let _ = std::fs::remove_file(utils::service_mode_path());
    println!("LaunchDaemon removed");
    Ok(())
}

// --- Linux ---

fn install_systemd_system() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    
    // Clean up old user service if exists
    let user_unit = format!("{}/.config/systemd/user/mihomo.service", home);
    if std::path::Path::new(&user_unit).exists() {
        println!("Removing old user service...");
        let _ = Command::new("systemctl").args(["--user", "stop", "mihomo"]).status();
        let _ = Command::new("systemctl").args(["--user", "disable", "mihomo"]).status();
        let _ = std::fs::remove_file(&user_unit);
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    }

    let unit = format!(
        "[Unit]\n\
         Description=Mihomo proxy\n\
         After=network.target\n\n\
         [Service]\n\
         Type=simple\n\
         User=root\n\
         ExecStartPre=/bin/mkdir -p /tmp/verge\n\
         ExecStart={home}/.local/bin/mihomo -d {home}/.config/mihomo\n\
         Restart=on-failure\n\n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    );
    let unit_path = "/etc/systemd/system/mihomo.service";

    println!("Creating {unit_path} (sudo required)...");
    let mut child = Command::new("sudo")
        .args(["tee", unit_path])
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    use std::io::Write;
    child.stdin.take().unwrap().write_all(unit.as_bytes())?;
    child.wait()?;

    Command::new("sudo").args(["systemctl", "daemon-reload"]).status()?;
    Command::new("sudo").args(["systemctl", "enable", "--now", "mihomo"]).status()?;

    // Write service mode marker
    utils::write_service_mode("root")?;

    println!("systemd system service installed and started (root mode)");
    Ok(())
}

fn install_systemd_user() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    
    // Clean up old system service if exists
    let sys_unit = "/etc/systemd/system/mihomo.service";
    if std::path::Path::new(sys_unit).exists() {
        println!("Removing old system service...");
        let _ = Command::new("sudo").args(["systemctl", "stop", "mihomo"]).status();
        let _ = Command::new("sudo").args(["systemctl", "disable", "mihomo"]).status();
        let _ = Command::new("sudo").args(["rm", sys_unit]).status();
        let _ = Command::new("sudo").args(["systemctl", "daemon-reload"]).status();
    }

    let sd_dir = format!("{home}/.config/systemd/user");
    std::fs::create_dir_all(&sd_dir)?;
    let unit = format!(
        "[Unit]\n\
         Description=Mihomo proxy\n\
         After=network.target\n\n\
         [Service]\n\
         ExecStartPre=/bin/mkdir -p /tmp/verge\n\
         ExecStart={home}/.local/bin/mihomo -d {home}/.config/mihomo\n\
         Restart=on-failure\n\n\
         [Install]\n\
         WantedBy=default.target\n"
    );
    let unit_path = format!("{sd_dir}/mihomo.service");
    std::fs::write(&unit_path, &unit)?;
    Command::new("systemctl").args(["--user", "daemon-reload"]).status()?;
    Command::new("systemctl").args(["--user", "enable", "--now", "mihomo"]).status()?;

    // Write service mode marker
    utils::write_service_mode("user")?;

    println!("systemd user service installed and started");
    Ok(())
}

fn uninstall_systemd() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default().display().to_string();
    let mode = utils::read_service_mode();
    
    if mode == "root" {
        let unit_path = "/etc/systemd/system/mihomo.service";
        if !std::path::Path::new(unit_path).exists() {
            println!("No system service found");
            return Ok(());
        }
        let _ = Command::new("sudo").args(["systemctl", "stop", "mihomo"]).status();
        let _ = Command::new("sudo").args(["systemctl", "disable", "mihomo"]).status();
        let _ = Command::new("sudo").args(["rm", unit_path]).status();
        let _ = Command::new("sudo").args(["systemctl", "daemon-reload"]).status();
        let _ = std::fs::remove_file(utils::service_mode_path());
        println!("systemd system service removed");
    } else {
        let unit_path = format!("{}/.config/systemd/user/mihomo.service", home);
        if !std::path::Path::new(&unit_path).exists() {
            println!("No user service found");
            return Ok(());
        }
        let _ = Command::new("systemctl").args(["--user", "stop", "mihomo"]).status();
        let _ = Command::new("systemctl").args(["--user", "disable", "mihomo"]).status();
        let _ = std::fs::remove_file(&unit_path);
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
        let _ = std::fs::remove_file(utils::service_mode_path());
        println!("systemd user service removed");
    }
    Ok(())
}

// --- Windows ---

fn mihomo_dirs() -> (String, String, String) {
    let local = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
    let config_dir = format!("{}\\mihomo", local.display());
    let bin_path = format!("{}\\mihomo.exe", config_dir);
    let config_path = format!("{}\\config.yaml", config_dir);
    (config_dir, bin_path, config_path)
}

fn install_windows() -> anyhow::Result<()> {
    let (config_dir, bin_path, _) = mihomo_dirs();
    std::fs::create_dir_all(&config_dir)?;

    Command::new("sc.exe").args([
        "create", "mihomo",
        "binPath=", &format!("\"{bin_path}\" -d \"{config_dir}\""),
        "start=", "auto",
        "DisplayName=", "Mihomo Proxy Service",
    ]).status()?;

    Command::new("sc.exe").args(["start", "mihomo"]).status()?;
    
    // Write service mode marker
    utils::write_service_mode("root")?;
    
    println!("Windows service installed and started");
    Ok(())
}

fn uninstall_windows() -> anyhow::Result<()> {
    let _ = Command::new("sc.exe").args(["stop", "mihomo"]).status();
    let _ = Command::new("sc.exe").args(["delete", "mihomo"]).status();
    let _ = std::fs::remove_file(utils::service_mode_path());
    println!("Windows service removed");
    Ok(())
}
