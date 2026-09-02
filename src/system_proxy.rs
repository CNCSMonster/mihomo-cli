use anyhow::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl PlannedCommand {
    fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

pub fn enable_system_proxy(port: u16) -> Result<()> {
    for cmd in enable_commands(port)? {
        run_planned(&cmd)?;
    }
    #[cfg(target_os = "windows")]
    notify_windows_settings_changed();
    Ok(())
}

pub fn disable_system_proxy() -> Result<()> {
    for cmd in disable_commands()? {
        run_planned(&cmd)?;
    }
    #[cfg(target_os = "windows")]
    notify_windows_settings_changed();
    Ok(())
}

/// Broadcast WM_SETTINGCHANGE so running applications pick up the new proxy settings.
/// Without this, many WinINet/WinHTTP-based apps cache the old settings until restart.
#[cfg(target_os = "windows")]
fn notify_windows_settings_changed() {
    use std::ptr;
    #[link(name = "wininet")]
    extern "system" {
        fn InternetSetOptionW(
            hinternet: *mut std::ffi::c_void,
            dwoption: u32,
            lpbuffer: *mut std::ffi::c_void,
            dwbufferlength: u32,
        ) -> i32;
    }
    const INTERNET_OPTION_SETTINGS_CHANGED: u32 = 39;
    unsafe {
        InternetSetOptionW(
            ptr::null_mut(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            ptr::null_mut(),
            0,
        );
    }
}

/// Enumerate all enabled network services on macOS.
/// Skips the first header line and services prefixed with `*` (disabled).
fn macos_enabled_network_services() -> Result<Vec<String>> {
    let output = std::process::Command::new("networksetup")
        .arg("-listallnetworkservices")
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run networksetup: {}", e))?;
    if !output.status.success() {
        anyhow::bail!("networksetup -listallnetworkservices failed");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let services: Vec<String> = stdout
        .lines()
        .skip(1) // skip header "An asterisk (*) denotes that a network service is disabled."
        .filter(|line| !line.starts_with('*'))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(services)
}

pub fn enable_commands(port: u16) -> Result<Vec<PlannedCommand>> {
    if cfg!(target_os = "windows") {
        let server = format!("127.0.0.1:{}", port);
        Ok(vec![
            PlannedCommand::new(
                "reg",
                &[
                    "add",
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                    "/v",
                    "ProxyServer",
                    "/t",
                    "REG_SZ",
                    "/d",
                    &server,
                    "/f",
                ],
            ),
            PlannedCommand::new(
                "reg",
                &[
                    "add",
                    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                    "/v",
                    "ProxyEnable",
                    "/t",
                    "REG_DWORD",
                    "/d",
                    "1",
                    "/f",
                ],
            ),
        ])
    } else if cfg!(target_os = "macos") {
        let services = macos_enabled_network_services()?;
        let mut cmds = Vec::new();
        for svc in &services {
            cmds.push(PlannedCommand::new(
                "networksetup",
                &["-setwebproxy", svc, "127.0.0.1", &port.to_string()],
            ));
            cmds.push(PlannedCommand::new(
                "networksetup",
                &["-setsecurewebproxy", svc, "127.0.0.1", &port.to_string()],
            ));
            cmds.push(PlannedCommand::new(
                "networksetup",
                &[
                    "-setsocksfirewallproxy",
                    svc,
                    "127.0.0.1",
                    &port.to_string(),
                ],
            ));
        }
        Ok(cmds)
    } else if cfg!(target_os = "linux") {
        Ok(vec![
            PlannedCommand::new(
                "gsettings",
                &["set", "org.gnome.system.proxy", "mode", "manual"],
            ),
            PlannedCommand::new(
                "gsettings",
                &["set", "org.gnome.system.proxy.http", "host", "127.0.0.1"],
            ),
            PlannedCommand::new(
                "gsettings",
                &[
                    "set",
                    "org.gnome.system.proxy.http",
                    "port",
                    &port.to_string(),
                ],
            ),
            PlannedCommand::new(
                "gsettings",
                &["set", "org.gnome.system.proxy.https", "host", "127.0.0.1"],
            ),
            PlannedCommand::new(
                "gsettings",
                &[
                    "set",
                    "org.gnome.system.proxy.https",
                    "port",
                    &port.to_string(),
                ],
            ),
            PlannedCommand::new(
                "gsettings",
                &["set", "org.gnome.system.proxy.socks", "host", "127.0.0.1"],
            ),
            PlannedCommand::new(
                "gsettings",
                &[
                    "set",
                    "org.gnome.system.proxy.socks",
                    "port",
                    &port.to_string(),
                ],
            ),
        ])
    } else {
        anyhow::bail!("system-proxy is only supported on macOS, Linux GNOME, and Windows")
    }
}

pub fn disable_commands() -> Result<Vec<PlannedCommand>> {
    if cfg!(target_os = "windows") {
        Ok(vec![PlannedCommand::new(
            "reg",
            &[
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
                "/v",
                "ProxyEnable",
                "/t",
                "REG_DWORD",
                "/d",
                "0",
                "/f",
            ],
        )])
    } else if cfg!(target_os = "macos") {
        let services = macos_enabled_network_services()?;
        let mut cmds = Vec::new();
        for svc in &services {
            cmds.push(PlannedCommand::new(
                "networksetup",
                &["-setwebproxystate", svc, "off"],
            ));
            cmds.push(PlannedCommand::new(
                "networksetup",
                &["-setsecurewebproxystate", svc, "off"],
            ));
            cmds.push(PlannedCommand::new(
                "networksetup",
                &["-setsocksfirewallproxystate", svc, "off"],
            ));
        }
        Ok(cmds)
    } else if cfg!(target_os = "linux") {
        Ok(vec![PlannedCommand::new(
            "gsettings",
            &["set", "org.gnome.system.proxy", "mode", "none"],
        )])
    } else {
        anyhow::bail!("system-proxy is only supported on macOS, Linux GNOME, and Windows")
    }
}

fn run_planned(cmd: &PlannedCommand) -> Result<()> {
    let status = std::process::Command::new(&cmd.program)
        .args(&cmd.args)
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run {}: {}", cmd.program, e))?;
    if !status.success() {
        anyhow::bail!("Command failed: {} {}", cmd.program, cmd.args.join(" "));
    }
    Ok(())
}

/// 系统代理状态查询结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SystemProxyState {
    /// 系统代理已启用
    Enabled,
    /// 系统代理已禁用
    Disabled,
    /// 当前平台或桌面环境不支持查询
    Unsupported,
    /// 查询失败或无法解析
    Unknown,
}

/// 查询当前系统代理状态（只读，不修改任何设置）
pub fn query_system_proxy() -> SystemProxyState {
    #[cfg(target_os = "linux")]
    {
        query_linux_system_proxy()
    }
    #[cfg(target_os = "macos")]
    {
        query_macos_system_proxy()
    }
    #[cfg(target_os = "windows")]
    {
        query_windows_system_proxy()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        SystemProxyState::Unsupported
    }
}

#[cfg(target_os = "linux")]
fn query_linux_system_proxy() -> SystemProxyState {
    // 查询 GNOME 系统代理模式
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.system.proxy", "mode"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let mode = String::from_utf8_lossy(&output.stdout);
            let mode = mode.trim().trim_matches('\'');
            match mode {
                "manual" => SystemProxyState::Enabled,
                "none" => SystemProxyState::Disabled,
                _ => SystemProxyState::Unknown,
            }
        }
        Ok(_) => SystemProxyState::Unknown,
        Err(_) => SystemProxyState::Unsupported, // gsettings 不存在，可能不是 GNOME
    }
}

#[cfg(target_os = "macos")]
fn query_macos_system_proxy() -> SystemProxyState {
    // 查询所有启用的网络服务的代理状态
    let services = match macos_enabled_network_services() {
        Ok(s) => s,
        Err(_) => return SystemProxyState::Unknown,
    };

    if services.is_empty() {
        return SystemProxyState::Disabled;
    }

    // 检查任一服务是否启用了 Web Proxy
    for service in &services {
        let output = std::process::Command::new("networksetup")
            .args(["-getwebproxy", service])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout);
                if content.contains("Enabled: Yes") {
                    return SystemProxyState::Enabled;
                }
            }
        }
    }

    SystemProxyState::Disabled
}

#[cfg(target_os = "windows")]
fn query_windows_system_proxy() -> SystemProxyState {
    use std::process::Command;

    let output = Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "/v",
            "ProxyEnable",
        ])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let content = String::from_utf8_lossy(&output.stdout);
            if content.contains("0x1") {
                SystemProxyState::Enabled
            } else if content.contains("0x0") {
                SystemProxyState::Disabled
            } else {
                SystemProxyState::Unknown
            }
        }
        _ => SystemProxyState::Unknown,
    }
}

/// Shell 代理状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ShellProxyState {
    /// 环境变量中已配置代理
    Configured,
    /// 环境变量中未配置代理
    NotConfigured,
    /// 无法确定
    Unknown,
}

/// 查询当前进程的 shell 代理环境（只读，不修改任何环境变量）
pub fn query_shell_proxy() -> ShellProxyState {
    let proxy_vars = [
        "http_proxy",
        "https_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "all_proxy",
    ];

    for var in &proxy_vars {
        if std::env::var_os(var).is_some() {
            return ShellProxyState::Configured;
        }
    }

    ShellProxyState::NotConfigured
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_enable_commands_set_gnome_proxy() {
        let cmds = enable_commands(7890).unwrap();
        assert!(cmds.iter().any(|c| c.program == "gsettings"));
        assert!(cmds.iter().any(|c| c.args.contains(&"7890".to_string())));
        assert!(cmds.iter().any(|c| c.args.contains(&"manual".to_string())));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_disable_commands_disable_gnome_proxy() {
        let cmds = disable_commands().unwrap();
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].args.contains(&"none".to_string()));
    }
}
