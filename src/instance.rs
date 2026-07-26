#![allow(dead_code)]

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    Macos,
    Linux,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceMode {
    System,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiEndpoint {
    UnixSocket(PathBuf),
    WindowsNamedPipe(String),
}

impl ApiEndpoint {
    pub fn controller_line(&self) -> String {
        match self {
            Self::UnixSocket(path) => {
                format!("external-controller-unix: {}", path.display())
            }
            Self::WindowsNamedPipe(pipe) => {
                format!("external-controller-pipe: {pipe}")
            }
        }
    }

    pub fn controller_key_and_value(&self) -> (&'static str, String) {
        match self {
            Self::UnixSocket(path) => ("external-controller-unix", path.display().to_string()),
            Self::WindowsNamedPipe(pipe) => ("external-controller-pipe", pipe.clone()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionModel {
    DirectUser,
    PrivilegedSystem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceTarget {
    MacosLaunchDaemon {
        domain_label: String,
        plist: PathBuf,
    },
    MacosLaunchAgent {
        domain_label: String,
        plist: PathBuf,
    },
    LinuxSystemdSystem {
        unit: PathBuf,
    },
    LinuxSystemdUser {
        unit: PathBuf,
    },
    WindowsService {
        name: String,
    },
    WindowsUserProcess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePaths {
    pub core_binary: PathBuf,
    pub cli_binary: PathBuf,
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub start_script: Option<PathBuf>,
    pub runtime_dir: Option<PathBuf>,
    pub api_endpoint: ApiEndpoint,
    pub log_file: Option<PathBuf>,
    pub service_file: Option<PathBuf>,
    pub backup_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceContext {
    pub os: TargetOs,
    pub mode: InstanceMode,
    pub paths: InstancePaths,
    pub service: ServiceTarget,
    pub permissions: PermissionModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathInputs {
    pub home: PathBuf,
    pub uid: Option<u32>,
    pub xdg_runtime_dir: Option<PathBuf>,
    pub program_data: PathBuf,
    pub app_data: PathBuf,
    pub local_app_data: PathBuf,
    pub username_or_sid: String,
}

impl PathInputs {
    pub fn for_tests() -> Self {
        Self {
            home: PathBuf::from("/Users/alice"),
            uid: Some(501),
            xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
            program_data: PathBuf::from(r"C:\ProgramData"),
            app_data: PathBuf::from(r"C:\Users\alice\AppData\Roaming"),
            local_app_data: PathBuf::from(r"C:\Users\alice\AppData\Local"),
            username_or_sid: "alice".to_string(),
        }
    }
}

impl TargetOs {
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "macos") {
            Some(Self::Macos)
        } else if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else if cfg!(target_os = "windows") {
            Some(Self::Windows)
        } else {
            None
        }
    }
}

impl PathInputs {
    pub fn from_current_env() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let uid = current_uid();
        let xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let program_data = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
        let app_data = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"));
        let local_app_data = dirs::data_local_dir()
            .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
            .unwrap_or_else(|| home.join("AppData/Local"));
        let username_or_sid = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "user".to_string());

        Self {
            home,
            uid,
            xdg_runtime_dir,
            program_data,
            app_data,
            local_app_data,
            username_or_sid,
        }
    }
}

pub fn planned_current_context(mode: InstanceMode) -> Option<InstanceContext> {
    let os = TargetOs::current()?;
    let inputs = PathInputs::from_current_env();
    Some(InstanceContext::planned(os, mode, &inputs))
}

#[cfg(unix)]
fn current_uid() -> Option<u32> {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
}

#[cfg(not(unix))]
fn current_uid() -> Option<u32> {
    None
}

impl InstanceContext {
    pub fn planned(os: TargetOs, mode: InstanceMode, inputs: &PathInputs) -> Self {
        let paths = planned_paths(os, mode, inputs);
        let service = planned_service(os, mode, inputs, &paths);
        let permissions = match mode {
            InstanceMode::System => PermissionModel::PrivilegedSystem,
            InstanceMode::User => PermissionModel::DirectUser,
        };
        Self {
            os,
            mode,
            paths,
            service,
            permissions,
        }
    }
}

fn planned_paths(os: TargetOs, mode: InstanceMode, inputs: &PathInputs) -> InstancePaths {
    match (os, mode) {
        (TargetOs::Macos, InstanceMode::System) => {
            let app = PathBuf::from("/Library/Application Support/mihomo");
            // ADR-02: 配置始终 per-user，即使系统服务模式
            let config_dir = inputs.home.join(".config/mihomo");
            let runtime_dir = PathBuf::from("/var/run/mihomo");
            InstancePaths {
                core_binary: app.join("bin/mihomo"),
                cli_binary: app.join("bin/mihomo-cli"),
                config_file: config_dir.join("config.yaml"),
                start_script: Some(app.join("start.sh")),
                api_endpoint: ApiEndpoint::UnixSocket(runtime_dir.join("mihomo.sock")),
                log_file: Some(PathBuf::from("/var/log/mihomo/mihomo.log")),
                service_file: Some(PathBuf::from("/Library/LaunchDaemons/io.mihomo.plist")),
                backup_dir: config_dir.join("backups"),
                config_dir,
                runtime_dir: Some(runtime_dir),
            }
        }
        (TargetOs::Macos, InstanceMode::User) => {
            let config_dir = inputs.home.join(".config/mihomo");
            let runtime_dir = PathBuf::from(format!("/tmp/mihomo-{}", inputs.uid.unwrap_or(0)));
            InstancePaths {
                core_binary: inputs.home.join(".local/bin/mihomo"),
                cli_binary: inputs.home.join(".local/bin/mihomo-cli"),
                config_file: config_dir.join("config.yaml"),
                start_script: Some(config_dir.join("start.sh")),
                api_endpoint: ApiEndpoint::UnixSocket(runtime_dir.join("mihomo.sock")),
                log_file: Some(inputs.home.join("Library/Logs/mihomo/mihomo.log")),
                service_file: Some(inputs.home.join("Library/LaunchAgents/io.mihomo.plist")),
                backup_dir: config_dir.join("backups"),
                config_dir,
                runtime_dir: Some(runtime_dir),
            }
        }
        (TargetOs::Linux, InstanceMode::System) => {
            // ADR-02: 配置始终 per-user，即使系统服务模式
            let config_dir = inputs.home.join(".config/mihomo");
            let runtime_dir = PathBuf::from("/var/run/mihomo");
            InstancePaths {
                core_binary: PathBuf::from("/usr/local/lib/mihomo/mihomo"),
                cli_binary: PathBuf::from("/usr/local/bin/mihomo-cli"),
                config_file: config_dir.join("config.yaml"),
                start_script: None,
                api_endpoint: ApiEndpoint::UnixSocket(runtime_dir.join("mihomo.sock")),
                log_file: Some(PathBuf::from("/var/log/mihomo/mihomo.log")),
                service_file: Some(PathBuf::from("/etc/systemd/system/mihomo.service")),
                backup_dir: config_dir.join("backups"),
                config_dir,
                runtime_dir: Some(runtime_dir),
            }
        }
        (TargetOs::Linux, InstanceMode::User) => {
            let config_dir = inputs.home.join(".config/mihomo");
            let runtime_dir = inputs
                .xdg_runtime_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("/run/user/1000"))
                .join("mihomo");
            InstancePaths {
                core_binary: inputs.home.join(".local/bin/mihomo"),
                cli_binary: inputs.home.join(".local/bin/mihomo-cli"),
                config_file: config_dir.join("config.yaml"),
                start_script: None,
                api_endpoint: ApiEndpoint::UnixSocket(runtime_dir.join("mihomo.sock")),
                log_file: Some(inputs.home.join(".local/state/mihomo/mihomo.log")),
                service_file: Some(inputs.home.join(".config/systemd/user/mihomo.service")),
                backup_dir: config_dir.join("backups"),
                config_dir,
                runtime_dir: Some(runtime_dir),
            }
        }
        (TargetOs::Windows, InstanceMode::System) => {
            let install_root = inputs.program_data.join("mihomo");
            // ADR-02: 配置始终 per-user，即使系统服务模式；Windows 使用 %APPDATA%\mihomo
            let config_dir = inputs.app_data.join("mihomo");
            InstancePaths {
                core_binary: install_root.join("bin").join("mihomo.exe"),
                cli_binary: install_root.join("bin").join("mihomo-cli.exe"),
                config_file: config_dir.join("config.yaml"),
                start_script: None,
                runtime_dir: None,
                api_endpoint: ApiEndpoint::WindowsNamedPipe(r"\\.\pipe\mihomo-core".to_string()),
                log_file: Some(install_root.join("mihomo.log")),
                service_file: None,
                backup_dir: config_dir.join("backups"),
                config_dir,
            }
        }
        (TargetOs::Windows, InstanceMode::User) => {
            let install_root = inputs.local_app_data.join("mihomo");
            let config_dir = inputs.app_data.join("mihomo");
            InstancePaths {
                core_binary: install_root.join("bin").join("mihomo.exe"),
                cli_binary: install_root.join("bin").join("mihomo-cli.exe"),
                config_file: config_dir.join("config.yaml"),
                start_script: None,
                runtime_dir: None,
                api_endpoint: ApiEndpoint::WindowsNamedPipe(format!(
                    r"\\.\pipe\mihomo-{}",
                    inputs.username_or_sid
                )),
                log_file: Some(install_root.join("mihomo.log")),
                service_file: None,
                backup_dir: config_dir.join("backups"),
                config_dir,
            }
        }
    }
}

fn planned_service(
    os: TargetOs,
    mode: InstanceMode,
    inputs: &PathInputs,
    paths: &InstancePaths,
) -> ServiceTarget {
    match (os, mode) {
        (TargetOs::Macos, InstanceMode::System) => ServiceTarget::MacosLaunchDaemon {
            domain_label: "system/io.mihomo".to_string(),
            plist: paths.service_file.clone().expect("macOS system plist"),
        },
        (TargetOs::Macos, InstanceMode::User) => ServiceTarget::MacosLaunchAgent {
            domain_label: format!("gui/{}/io.mihomo", inputs.uid.unwrap_or(0)),
            plist: paths.service_file.clone().expect("macOS user plist"),
        },
        (TargetOs::Linux, InstanceMode::System) => ServiceTarget::LinuxSystemdSystem {
            unit: paths.service_file.clone().expect("linux system unit"),
        },
        (TargetOs::Linux, InstanceMode::User) => ServiceTarget::LinuxSystemdUser {
            unit: paths.service_file.clone().expect("linux user unit"),
        },
        (TargetOs::Windows, InstanceMode::System) => ServiceTarget::WindowsService {
            name: "mihomo".to_string(),
        },
        (TargetOs::Windows, InstanceMode::User) => ServiceTarget::WindowsUserProcess,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchdArtifacts {
    pub plist_path: PathBuf,
    pub plist_content: String,
    pub start_script_path: PathBuf,
    pub start_script_content: String,
    pub privileged: bool,
}

pub fn planned_macos_launchd_artifacts(ctx: &InstanceContext) -> Option<LaunchdArtifacts> {
    if ctx.os != TargetOs::Macos {
        return None;
    }
    let plist_path = ctx.paths.service_file.clone()?;
    let start_script_path = ctx.paths.start_script.clone()?;
    let config_dir = &ctx.paths.config_dir;
    let core_binary = &ctx.paths.core_binary;
    let cli_binary = &ctx.paths.cli_binary;
    let log_file = ctx.paths.log_file.as_ref()?;
    let runtime_dir = ctx.paths.runtime_dir.as_ref()?;
    let privileged = ctx.mode == InstanceMode::System;

    let plist_content = format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>io.mihomo</string>
<key>ProgramArguments</key><array><string>{}</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>Crashed</key><true/></dict>
<key>WorkingDirectory</key><string>{}</string>
<key>StandardOutPath</key><string>{}</string>
<key>StandardErrorPath</key><string>{}</string>
</dict></plist>
"#,
        start_script_path.display(),
        config_dir.display(),
        log_file.display(),
        log_file.display()
    );

    let mut start_script_content = format!(
        "#!/bin/bash\nset -e\nmkdir -p \"{}\"\n",
        runtime_dir.display()
    );
    if privileged {
        start_script_content.push_str("umask 000\n");
    } else {
        start_script_content.push_str(&format!("chmod 700 \"{}\"\n", runtime_dir.display()));
    }
    if privileged {
        start_script_content.push_str(&format!("exec \"{}\" daemon\n", cli_binary.display()));
    } else {
        start_script_content.push_str(&format!(
            "exec \"{}\" -d \"{}\"\n",
            core_binary.display(),
            config_dir.display()
        ));
    }

    Some(LaunchdArtifacts {
        plist_path,
        plist_content,
        start_script_path,
        start_script_content,
        privileged,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedDirectory {
    pub path: PathBuf,
    pub mode: u16,
    pub privileged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub mode: u16,
    pub privileged: bool,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub privileged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceInstallPlan {
    pub directories: Vec<PlannedDirectory>,
    pub files: Vec<PlannedFile>,
    pub commands: Vec<PlannedCommand>,
}

pub fn planned_install_plan(ctx: &InstanceContext) -> Option<InstanceInstallPlan> {
    match ctx.os {
        TargetOs::Macos => planned_macos_install_plan(ctx),
        TargetOs::Linux => planned_linux_install_plan(ctx),
        TargetOs::Windows => planned_windows_install_plan(ctx),
    }
}

pub fn planned_macos_install_plan(ctx: &InstanceContext) -> Option<InstanceInstallPlan> {
    let artifacts = planned_macos_launchd_artifacts(ctx)?;
    let privileged = ctx.permissions == PermissionModel::PrivilegedSystem;
    let mut directories = Vec::new();

    match ctx.mode {
        InstanceMode::System => {
            directories.push(PlannedDirectory {
                path: PathBuf::from("/Library/Application Support/mihomo"),
                mode: 0o755,
                privileged: true,
            });
            directories.push(PlannedDirectory {
                path: PathBuf::from("/Library/Application Support/mihomo/bin"),
                mode: 0o755,
                privileged: true,
            });
            directories.push(PlannedDirectory {
                path: ctx.paths.config_dir.clone(),
                mode: 0o755,
                privileged: false,
            });
            if let Some(log_file) = &ctx.paths.log_file {
                if let Some(parent) = log_file.parent() {
                    directories.push(PlannedDirectory {
                        path: parent.to_path_buf(),
                        mode: 0o755,
                        privileged: true,
                    });
                }
            }
            if let Some(runtime_dir) = &ctx.paths.runtime_dir {
                directories.push(PlannedDirectory {
                    path: runtime_dir.clone(),
                    mode: 0o755,
                    privileged: true,
                });
            }
        }
        InstanceMode::User => {
            directories.push(PlannedDirectory {
                path: ctx.paths.config_dir.clone(),
                mode: 0o755,
                privileged: false,
            });
            if let Some(runtime_dir) = &ctx.paths.runtime_dir {
                directories.push(PlannedDirectory {
                    path: runtime_dir.clone(),
                    mode: 0o700,
                    privileged: false,
                });
            }
            if let Some(log_file) = &ctx.paths.log_file {
                if let Some(parent) = log_file.parent() {
                    directories.push(PlannedDirectory {
                        path: parent.to_path_buf(),
                        mode: 0o755,
                        privileged: false,
                    });
                }
            }
            if let Some(service_file) = &ctx.paths.service_file {
                if let Some(parent) = service_file.parent() {
                    directories.push(PlannedDirectory {
                        path: parent.to_path_buf(),
                        mode: 0o755,
                        privileged: false,
                    });
                }
            }
        }
    }

    let files = vec![
        PlannedFile {
            path: artifacts.start_script_path,
            mode: 0o755,
            privileged,
            content: artifacts.start_script_content,
        },
        PlannedFile {
            path: artifacts.plist_path,
            mode: 0o644,
            privileged,
            content: artifacts.plist_content,
        },
    ];

    let (domain, plist) = match &ctx.service {
        ServiceTarget::MacosLaunchDaemon {
            domain_label,
            plist,
        }
        | ServiceTarget::MacosLaunchAgent {
            domain_label,
            plist,
        } => (domain_label.clone(), plist.display().to_string()),
        _ => return None,
    };

    let commands = vec![
        PlannedCommand {
            program: "launchctl".to_string(),
            args: vec!["bootout".to_string(), domain.clone()],
            privileged,
        },
        PlannedCommand {
            program: "launchctl".to_string(),
            args: vec![
                "bootstrap".to_string(),
                domain_parent(&domain).to_string(),
                plist,
            ],
            privileged,
        },
        PlannedCommand {
            program: "launchctl".to_string(),
            args: vec!["kickstart".to_string(), "-k".to_string(), domain],
            privileged,
        },
    ];

    Some(InstanceInstallPlan {
        directories,
        files,
        commands,
    })
}

fn domain_parent(domain_label: &str) -> &str {
    domain_label
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or(domain_label)
}

pub fn planned_linux_install_plan(ctx: &InstanceContext) -> Option<InstanceInstallPlan> {
    if ctx.os != TargetOs::Linux {
        return None;
    }
    let privileged = ctx.permissions == PermissionModel::PrivilegedSystem;
    let mut directories = vec![PlannedDirectory {
        path: ctx.paths.config_dir.clone(),
        mode: 0o755,
        privileged: false,
    }];
    if let Some(runtime_dir) = &ctx.paths.runtime_dir {
        directories.push(PlannedDirectory {
            path: runtime_dir.clone(),
            mode: if privileged { 0o755 } else { 0o700 },
            privileged,
        });
    }
    if let Some(parent) = ctx.paths.core_binary.parent() {
        directories.push(PlannedDirectory {
            path: parent.to_path_buf(),
            mode: 0o755,
            privileged,
        });
    }
    if let Some(parent) = ctx.paths.cli_binary.parent() {
        let path = parent.to_path_buf();
        if !directories.iter().any(|dir| dir.path == path) {
            directories.push(PlannedDirectory {
                path,
                mode: 0o755,
                privileged,
            });
        }
    }
    if let Some(log_file) = &ctx.paths.log_file {
        if let Some(parent) = log_file.parent() {
            let path = parent.to_path_buf();
            if !directories.iter().any(|dir| dir.path == path) {
                directories.push(PlannedDirectory {
                    path,
                    mode: 0o755,
                    privileged,
                });
            }
        }
    }
    if let Some(service_file) = &ctx.paths.service_file {
        if let Some(parent) = service_file.parent() {
            directories.push(PlannedDirectory {
                path: parent.to_path_buf(),
                mode: 0o755,
                privileged,
            });
        }
    }

    let service_file = ctx.paths.service_file.clone()?;
    let log_directives = ctx
        .paths
        .log_file
        .as_ref()
        .map_or_else(String::new, |log_file| {
            format!(
                "StandardOutput=append:{}\nStandardError=append:{}\n",
                log_file.display(),
                log_file.display()
            )
        });
    let unit_content = match ctx.mode {
        InstanceMode::System => format!(
            "[Unit]\nDescription=Mihomo CLI System Daemon\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={} daemon\nRestart=on-failure\nRuntimeDirectory=mihomo\nRuntimeDirectoryMode=0755\nWorkingDirectory={}\n{}\n[Install]\nWantedBy=multi-user.target\n",
            ctx.paths.cli_binary.display(),
            ctx.paths.config_dir.display(),
            log_directives
        ),
        InstanceMode::User => format!(
            "[Unit]\nDescription=Mihomo Proxy Service\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={} -d {}\nRestart=on-failure\nWorkingDirectory={}\n{}\n[Install]\nWantedBy=default.target\n",
            ctx.paths.core_binary.display(),
            ctx.paths.config_dir.display(),
            ctx.paths.config_dir.display(),
            log_directives
        ),
    };

    let files = vec![PlannedFile {
        path: service_file,
        mode: 0o644,
        privileged,
        content: unit_content,
    }];

    let commands = match ctx.mode {
        InstanceMode::System => vec![
            PlannedCommand {
                program: "systemctl".to_string(),
                args: vec!["daemon-reload".to_string()],
                privileged: true,
            },
            PlannedCommand {
                program: "systemctl".to_string(),
                args: vec![
                    "enable".to_string(),
                    "--now".to_string(),
                    "mihomo".to_string(),
                ],
                privileged: true,
            },
        ],
        InstanceMode::User => vec![
            PlannedCommand {
                program: "systemctl".to_string(),
                args: vec!["--user".to_string(), "daemon-reload".to_string()],
                privileged: false,
            },
            PlannedCommand {
                program: "systemctl".to_string(),
                args: vec![
                    "--user".to_string(),
                    "enable".to_string(),
                    "--now".to_string(),
                    "mihomo".to_string(),
                ],
                privileged: false,
            },
        ],
    };

    Some(InstanceInstallPlan {
        directories,
        files,
        commands,
    })
}

fn planned_windows_user_process_start(ctx: &InstanceContext) -> InstanceServicePlan {
    InstanceServicePlan {
        commands: vec![PlannedCommand {
            program: "cmd.exe".to_string(),
            args: vec![
                "/C".to_string(),
                "start".to_string(),
                format!("mihomo:{}", ctx.paths.config_dir.display()),
                "/B".to_string(),
                ctx.paths.core_binary.display().to_string(),
                "-d".to_string(),
                ctx.paths.config_dir.display().to_string(),
            ],
            privileged: false,
        }],
        remove_paths: Vec::new(),
    }
}

pub fn planned_windows_install_plan(ctx: &InstanceContext) -> Option<InstanceInstallPlan> {
    if ctx.os != TargetOs::Windows {
        return None;
    }
    let privileged = ctx.permissions == PermissionModel::PrivilegedSystem;
    let mut directories = vec![
        PlannedDirectory {
            path: ctx.paths.config_dir.clone(),
            mode: 0,
            privileged: false,
        },
        PlannedDirectory {
            path: ctx.paths.core_binary.parent()?.to_path_buf(),
            mode: 0,
            privileged,
        },
    ];
    if let Some(log_file) = &ctx.paths.log_file {
        directories.push(PlannedDirectory {
            path: log_file.parent()?.to_path_buf(),
            mode: 0,
            privileged,
        });
    }

    let commands = match ctx.mode {
        InstanceMode::System => vec![
            PlannedCommand {
                program: "sc.exe".to_string(),
                args: vec!["stop".to_string(), "mihomo".to_string()],
                privileged: true,
            },
            PlannedCommand {
                program: "sc.exe".to_string(),
                args: vec!["delete".to_string(), "mihomo".to_string()],
                privileged: true,
            },
            PlannedCommand {
                program: "sc.exe".to_string(),
                args: vec![
                    "create".to_string(),
                    "mihomo".to_string(),
                    format!("binPath= \"{}\" daemon", ctx.paths.cli_binary.display()),
                    "start= auto".to_string(),
                    "DisplayName= Mihomo Proxy Service".to_string(),
                ],
                privileged: true,
            },
            PlannedCommand {
                program: "sc.exe".to_string(),
                args: vec!["start".to_string(), "mihomo".to_string()],
                privileged: true,
            },
        ],
        InstanceMode::User => planned_windows_user_process_start(ctx).commands,
    };

    Some(InstanceInstallPlan {
        directories,
        files: Vec::new(),
        commands,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Uninstall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceServicePlan {
    pub commands: Vec<PlannedCommand>,
    pub remove_paths: Vec<PlannedDirectory>,
}

pub fn planned_service_plan(ctx: &InstanceContext, action: ServiceAction) -> InstanceServicePlan {
    match (ctx.os, ctx.mode, action) {
        (TargetOs::Macos, _, ServiceAction::Start) => launchctl_service_plan(ctx, false, false),
        (TargetOs::Macos, _, ServiceAction::Stop) => launchctl_bootout_plan(ctx, false),
        (TargetOs::Macos, _, ServiceAction::Restart) => launchctl_service_plan(ctx, true, false),
        (TargetOs::Macos, _, ServiceAction::Uninstall) => launchctl_uninstall_plan(ctx),

        (TargetOs::Linux, InstanceMode::System, ServiceAction::Start) => {
            systemctl_plan(ctx, ["start", "mihomo"], true)
        }
        (TargetOs::Linux, InstanceMode::System, ServiceAction::Stop) => {
            systemctl_plan(ctx, ["stop", "mihomo"], true)
        }
        (TargetOs::Linux, InstanceMode::System, ServiceAction::Restart) => {
            systemctl_plan(ctx, ["restart", "mihomo"], true)
        }
        (TargetOs::Linux, InstanceMode::System, ServiceAction::Uninstall) => {
            linux_uninstall_plan(ctx, true)
        }
        (TargetOs::Linux, InstanceMode::User, ServiceAction::Start) => {
            systemctl_plan(ctx, ["--user", "start", "mihomo"], false)
        }
        (TargetOs::Linux, InstanceMode::User, ServiceAction::Stop) => {
            systemctl_plan(ctx, ["--user", "stop", "mihomo"], false)
        }
        (TargetOs::Linux, InstanceMode::User, ServiceAction::Restart) => {
            systemctl_plan(ctx, ["--user", "restart", "mihomo"], false)
        }
        (TargetOs::Linux, InstanceMode::User, ServiceAction::Uninstall) => {
            linux_uninstall_plan(ctx, false)
        }

        (TargetOs::Windows, InstanceMode::System, ServiceAction::Start) => {
            windows_sc_plan(["start", "mihomo"], true)
        }
        (TargetOs::Windows, InstanceMode::System, ServiceAction::Stop) => {
            windows_sc_plan(["stop", "mihomo"], true)
        }
        (TargetOs::Windows, InstanceMode::System, ServiceAction::Restart) => InstanceServicePlan {
            commands: vec![
                PlannedCommand {
                    program: "sc.exe".to_string(),
                    args: vec!["stop".to_string(), "mihomo".to_string()],
                    privileged: true,
                },
                PlannedCommand {
                    program: "sc.exe".to_string(),
                    args: vec!["start".to_string(), "mihomo".to_string()],
                    privileged: true,
                },
            ],
            remove_paths: Vec::new(),
        },
        (TargetOs::Windows, InstanceMode::System, ServiceAction::Uninstall) => {
            windows_uninstall_plan(ctx, true)
        }
        (TargetOs::Windows, InstanceMode::User, ServiceAction::Start) => {
            planned_windows_user_process_start(ctx)
        }
        (TargetOs::Windows, InstanceMode::User, ServiceAction::Stop) => InstanceServicePlan {
            commands: vec![PlannedCommand {
                program: "taskkill".to_string(),
                args: vec![
                    "/F".to_string(),
                    "/FI".to_string(),
                    format!("WINDOWTITLE eq mihomo:{}", ctx.paths.config_dir.display()),
                ],
                privileged: false,
            }],
            remove_paths: Vec::new(),
        },
        (TargetOs::Windows, InstanceMode::User, ServiceAction::Restart) => {
            let mut stop = planned_service_plan(ctx, ServiceAction::Stop).commands;
            stop.extend(planned_service_plan(ctx, ServiceAction::Start).commands);
            InstanceServicePlan {
                commands: stop,
                remove_paths: Vec::new(),
            }
        }
        (TargetOs::Windows, InstanceMode::User, ServiceAction::Uninstall) => {
            windows_uninstall_plan(ctx, false)
        }
    }
}

fn launchctl_service_plan(
    ctx: &InstanceContext,
    restart: bool,
    privileged: bool,
) -> InstanceServicePlan {
    let privileged = privileged || ctx.permissions == PermissionModel::PrivilegedSystem;
    let domain = service_domain_label(ctx);
    let mut args = vec!["kickstart".to_string()];
    if restart {
        args.push("-k".to_string());
    }
    args.push(domain);
    InstanceServicePlan {
        commands: vec![PlannedCommand {
            program: "launchctl".to_string(),
            args,
            privileged,
        }],
        remove_paths: Vec::new(),
    }
}

fn launchctl_bootout_plan(ctx: &InstanceContext, include_file: bool) -> InstanceServicePlan {
    let privileged = ctx.permissions == PermissionModel::PrivilegedSystem;
    let mut remove_paths = Vec::new();
    if include_file {
        if let Some(service_file) = &ctx.paths.service_file {
            remove_paths.push(PlannedDirectory {
                path: service_file.clone(),
                mode: 0,
                privileged,
            });
        }
    }
    InstanceServicePlan {
        commands: vec![PlannedCommand {
            program: "launchctl".to_string(),
            args: vec!["bootout".to_string(), service_domain_label(ctx)],
            privileged,
        }],
        remove_paths,
    }
}

fn launchctl_uninstall_plan(ctx: &InstanceContext) -> InstanceServicePlan {
    let privileged = ctx.permissions == PermissionModel::PrivilegedSystem;
    let mut plan = launchctl_bootout_plan(ctx, true);

    // v3 keeps config per-user even for system service mode; removing config must
    // therefore not require root/admin. Installed binaries, logs, runtime state,
    // and LaunchDaemon artifacts remain privileged in system mode.
    plan.remove_paths.push(PlannedDirectory {
        path: ctx.paths.config_dir.clone(),
        mode: 0,
        privileged: false,
    });

    for path in [
        Some(ctx.paths.core_binary.clone()),
        Some(ctx.paths.cli_binary.clone()),
        ctx.paths.start_script.clone(),
        ctx.paths.log_file.clone(),
        ctx.paths.runtime_dir.clone(),
    ]
    .into_iter()
    .flatten()
    {
        plan.remove_paths.push(PlannedDirectory {
            path,
            mode: 0,
            privileged,
        });
    }
    plan
}

fn service_domain_label(ctx: &InstanceContext) -> String {
    match &ctx.service {
        ServiceTarget::MacosLaunchDaemon { domain_label, .. }
        | ServiceTarget::MacosLaunchAgent { domain_label, .. } => domain_label.clone(),
        _ => "mihomo".to_string(),
    }
}

fn systemctl_plan<const N: usize>(
    _: &InstanceContext,
    args: [&str; N],
    privileged: bool,
) -> InstanceServicePlan {
    InstanceServicePlan {
        commands: vec![PlannedCommand {
            program: "systemctl".to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            privileged,
        }],
        remove_paths: Vec::new(),
    }
}

fn linux_uninstall_plan(ctx: &InstanceContext, privileged: bool) -> InstanceServicePlan {
    let commands = if privileged {
        vec![
            PlannedCommand {
                program: "systemctl".to_string(),
                args: vec![
                    "disable".to_string(),
                    "--now".to_string(),
                    "mihomo".to_string(),
                ],
                privileged: true,
            },
            PlannedCommand {
                program: "systemctl".to_string(),
                args: vec!["daemon-reload".to_string()],
                privileged: true,
            },
        ]
    } else {
        vec![
            PlannedCommand {
                program: "systemctl".to_string(),
                args: vec![
                    "--user".to_string(),
                    "disable".to_string(),
                    "--now".to_string(),
                    "mihomo".to_string(),
                ],
                privileged: false,
            },
            PlannedCommand {
                program: "systemctl".to_string(),
                args: vec!["--user".to_string(), "daemon-reload".to_string()],
                privileged: false,
            },
        ]
    };
    let mut remove_paths = Vec::new();
    if let Some(service_file) = &ctx.paths.service_file {
        remove_paths.push(PlannedDirectory {
            path: service_file.clone(),
            mode: 0,
            privileged,
        });
    }
    // v3: config remains per-user in both modes, so --all config cleanup is
    // always a direct user removal. Binaries/service/runtime state follow the
    // selected instance privilege model.
    remove_paths.push(PlannedDirectory {
        path: ctx.paths.config_dir.clone(),
        mode: 0,
        privileged: false,
    });
    remove_paths.push(PlannedDirectory {
        path: ctx.paths.core_binary.clone(),
        mode: 0,
        privileged,
    });
    remove_paths.push(PlannedDirectory {
        path: ctx.paths.cli_binary.clone(),
        mode: 0,
        privileged,
    });
    if let Some(runtime_dir) = &ctx.paths.runtime_dir {
        remove_paths.push(PlannedDirectory {
            path: runtime_dir.clone(),
            mode: 0,
            privileged,
        });
    }
    InstanceServicePlan {
        commands,
        remove_paths,
    }
}

fn windows_sc_plan<const N: usize>(args: [&str; N], privileged: bool) -> InstanceServicePlan {
    InstanceServicePlan {
        commands: vec![PlannedCommand {
            program: "sc.exe".to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            privileged,
        }],
        remove_paths: Vec::new(),
    }
}

fn windows_uninstall_plan(ctx: &InstanceContext, privileged: bool) -> InstanceServicePlan {
    let commands = if privileged {
        vec![
            PlannedCommand {
                program: "sc.exe".to_string(),
                args: vec!["stop".to_string(), "mihomo".to_string()],
                privileged: true,
            },
            PlannedCommand {
                program: "sc.exe".to_string(),
                args: vec!["delete".to_string(), "mihomo".to_string()],
                privileged: true,
            },
        ]
    } else {
        planned_service_plan(ctx, ServiceAction::Stop).commands
    };
    let mut remove_paths = vec![
        PlannedDirectory {
            path: ctx.paths.config_dir.clone(),
            mode: 0,
            privileged: false,
        },
        PlannedDirectory {
            path: ctx.paths.core_binary.clone(),
            mode: 0,
            privileged,
        },
        PlannedDirectory {
            path: ctx.paths.cli_binary.clone(),
            mode: 0,
            privileged,
        },
    ];
    if let Some(log_file) = &ctx.paths.log_file {
        remove_paths.push(PlannedDirectory {
            path: log_file.clone(),
            mode: 0,
            privileged,
        });
    }
    InstanceServicePlan {
        commands,
        remove_paths,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMutationKind {
    FixRuntimeController,
    ImportConfig,
    UpdateRules,
    UpdateDns,
    RestoreBackup,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigWriteStrategy {
    DirectAtomicWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigMutationPlan {
    pub kind: ConfigMutationKind,
    pub target_config: PathBuf,
    pub config_dir: PathBuf,
    pub backup_dir: PathBuf,
    pub endpoint: ApiEndpoint,
    pub write_strategy: ConfigWriteStrategy,
    pub validate_command: PlannedCommand,
    pub restart_after_write: InstanceServicePlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigStoreOperationKind {
    Read,
    EnsureDir,
    AtomicWrite,
    RemoveFile,
    RemoveDirAll,
    CopyFile,
    CopyDirFiltered,
    Validate,
    RestartService,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStoreOperationPlan {
    pub kind: ConfigStoreOperationKind,
    pub target: PathBuf,
    pub privileged: bool,
    pub rollback_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStorePlan {
    pub mode: InstanceMode,
    pub config_dir: PathBuf,
    pub strategy: ConfigWriteStrategy,
    pub operations: Vec<ConfigStoreOperationPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivilegeFailureCategory {
    NotElevatedNonInteractive,
    AuthenticationFailed,
    PermissionDeniedPath,
    CommandFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegeInvocationPlan {
    pub command: PlannedCommand,
    pub direct_when_elevated: bool,
    pub may_prompt_in_tty: bool,
    pub non_interactive_failure: PrivilegeFailureCategory,
    pub manual_fallback: String,
}

pub fn planned_config_store(ctx: &InstanceContext) -> ConfigStorePlan {
    // v3 keeps configuration per-user even in system-service mode, so config
    // file operations must not require elevation.
    let config_privileged = false;
    let strategy = ConfigWriteStrategy::DirectAtomicWrite;
    ConfigStorePlan {
        mode: ctx.mode,
        config_dir: ctx.paths.config_dir.clone(),
        strategy,
        operations: vec![
            ConfigStoreOperationPlan {
                kind: ConfigStoreOperationKind::Read,
                target: ctx.paths.config_dir.clone(),
                privileged: false,
                rollback_required: false,
            },
            ConfigStoreOperationPlan {
                kind: ConfigStoreOperationKind::EnsureDir,
                target: ctx.paths.config_dir.clone(),
                privileged: config_privileged,
                rollback_required: false,
            },
            ConfigStoreOperationPlan {
                kind: ConfigStoreOperationKind::AtomicWrite,
                target: ctx.paths.config_file.clone(),
                privileged: config_privileged,
                rollback_required: true,
            },
            ConfigStoreOperationPlan {
                kind: ConfigStoreOperationKind::Validate,
                target: ctx.paths.config_dir.clone(),
                privileged: config_privileged,
                rollback_required: true,
            },
            ConfigStoreOperationPlan {
                kind: ConfigStoreOperationKind::RestartService,
                target: ctx
                    .paths
                    .service_file
                    .clone()
                    .unwrap_or_else(|| ctx.paths.config_dir.clone()),
                privileged: config_privileged,
                rollback_required: false,
            },
        ],
    }
}

pub fn planned_config_mutation(
    ctx: &InstanceContext,
    kind: ConfigMutationKind,
) -> ConfigMutationPlan {
    // v3 keeps configuration per-user even in system-service mode, so config
    // validation/writes are direct user operations. Service restart may still
    // be mediated by the system daemon/service manager.
    let config_privileged = false;
    ConfigMutationPlan {
        kind,
        target_config: ctx.paths.config_file.clone(),
        config_dir: ctx.paths.config_dir.clone(),
        backup_dir: ctx.paths.backup_dir.clone(),
        endpoint: ctx.paths.api_endpoint.clone(),
        write_strategy: ConfigWriteStrategy::DirectAtomicWrite,
        validate_command: PlannedCommand {
            program: ctx.paths.core_binary.display().to_string(),
            args: vec![
                "-t".to_string(),
                "-d".to_string(),
                ctx.paths.config_dir.display().to_string(),
            ],
            privileged: config_privileged,
        },
        restart_after_write: planned_service_plan(ctx, ServiceAction::Restart),
    }
}

pub fn privilege_invocation_plan(command: PlannedCommand) -> Option<PrivilegeInvocationPlan> {
    if !command.privileged {
        return None;
    }
    Some(PrivilegeInvocationPlan {
        manual_fallback: manual_fallback_for_command(&command),
        command,
        direct_when_elevated: true,
        may_prompt_in_tty: true,
        non_interactive_failure: PrivilegeFailureCategory::NotElevatedNonInteractive,
    })
}

fn manual_fallback_for_command(command: &PlannedCommand) -> String {
    let mut parts = Vec::with_capacity(command.args.len() + 2);
    parts.push("sudo".to_string());
    parts.push(command.program.clone());
    parts.extend(command.args.iter().cloned());
    parts.join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticProbeKind {
    BinaryExists,
    ConfigExists,
    ServiceInstalled,
    ServiceRunning,
    ExpectedEndpointExists,
    EndpointConnectable,
    ApiResponds,
    ConfiguredEndpointMatchesExpected,
    RecentLogsAvailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticProbe {
    pub kind: DiagnosticProbeKind,
    pub target: String,
    pub privileged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusDiagnosticPlan {
    pub mode: InstanceMode,
    pub service: ServiceTarget,
    pub binary: PathBuf,
    pub config_file: PathBuf,
    pub expected_endpoint: ApiEndpoint,
    pub log_file: Option<PathBuf>,
    pub probes: Vec<DiagnosticProbe>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessPlan {
    pub mode: InstanceMode,
    pub expected_endpoint: ApiEndpoint,
    pub service_running_probe: DiagnosticProbe,
    pub configured_endpoint_probe: DiagnosticProbe,
    pub endpoint_connect_probe: DiagnosticProbe,
    pub api_probe: DiagnosticProbe,
    pub failure_hint_command: Option<PrivilegeInvocationPlan>,
}

pub fn planned_status_diagnostics(ctx: &InstanceContext) -> StatusDiagnosticPlan {
    let mut probes = vec![
        DiagnosticProbe {
            kind: DiagnosticProbeKind::BinaryExists,
            target: ctx.paths.core_binary.display().to_string(),
            privileged: false,
        },
        DiagnosticProbe {
            kind: DiagnosticProbeKind::ConfigExists,
            target: ctx.paths.config_file.display().to_string(),
            privileged: false,
        },
        DiagnosticProbe {
            kind: DiagnosticProbeKind::ServiceInstalled,
            target: service_target_display(&ctx.service),
            privileged: false,
        },
        DiagnosticProbe {
            kind: DiagnosticProbeKind::ServiceRunning,
            target: service_target_display(&ctx.service),
            privileged: false,
        },
        DiagnosticProbe {
            kind: DiagnosticProbeKind::ConfiguredEndpointMatchesExpected,
            target: endpoint_display(&ctx.paths.api_endpoint),
            privileged: false,
        },
        DiagnosticProbe {
            kind: DiagnosticProbeKind::ExpectedEndpointExists,
            target: endpoint_display(&ctx.paths.api_endpoint),
            privileged: false,
        },
        DiagnosticProbe {
            kind: DiagnosticProbeKind::EndpointConnectable,
            target: endpoint_display(&ctx.paths.api_endpoint),
            privileged: false,
        },
        DiagnosticProbe {
            kind: DiagnosticProbeKind::ApiResponds,
            target: endpoint_display(&ctx.paths.api_endpoint),
            privileged: false,
        },
    ];
    if let Some(log_file) = &ctx.paths.log_file {
        probes.push(DiagnosticProbe {
            kind: DiagnosticProbeKind::RecentLogsAvailable,
            target: log_file.display().to_string(),
            privileged: false,
        });
    }

    StatusDiagnosticPlan {
        mode: ctx.mode,
        service: ctx.service.clone(),
        binary: ctx.paths.core_binary.clone(),
        config_file: ctx.paths.config_file.clone(),
        expected_endpoint: ctx.paths.api_endpoint.clone(),
        log_file: ctx.paths.log_file.clone(),
        probes,
    }
}

pub fn planned_readiness(ctx: &InstanceContext) -> ReadinessPlan {
    let restart = planned_service_plan(ctx, ServiceAction::Restart)
        .commands
        .into_iter()
        .next();
    ReadinessPlan {
        mode: ctx.mode,
        expected_endpoint: ctx.paths.api_endpoint.clone(),
        service_running_probe: DiagnosticProbe {
            kind: DiagnosticProbeKind::ServiceRunning,
            target: service_target_display(&ctx.service),
            privileged: false,
        },
        configured_endpoint_probe: DiagnosticProbe {
            kind: DiagnosticProbeKind::ConfiguredEndpointMatchesExpected,
            target: endpoint_display(&ctx.paths.api_endpoint),
            privileged: false,
        },
        endpoint_connect_probe: DiagnosticProbe {
            kind: DiagnosticProbeKind::EndpointConnectable,
            target: endpoint_display(&ctx.paths.api_endpoint),
            privileged: false,
        },
        api_probe: DiagnosticProbe {
            kind: DiagnosticProbeKind::ApiResponds,
            target: endpoint_display(&ctx.paths.api_endpoint),
            privileged: false,
        },
        failure_hint_command: restart.and_then(privilege_invocation_plan),
    }
}

fn service_target_display(service: &ServiceTarget) -> String {
    match service {
        ServiceTarget::MacosLaunchDaemon { domain_label, .. }
        | ServiceTarget::MacosLaunchAgent { domain_label, .. } => domain_label.clone(),
        ServiceTarget::LinuxSystemdSystem { unit } | ServiceTarget::LinuxSystemdUser { unit } => {
            unit.display().to_string()
        }
        ServiceTarget::WindowsService { name } => name.clone(),
        ServiceTarget::WindowsUserProcess => "user-process:mihomo".to_string(),
    }
}

fn endpoint_display(endpoint: &ApiEndpoint) -> String {
    match endpoint {
        ApiEndpoint::UnixSocket(path) => path.display().to_string(),
        ApiEndpoint::WindowsNamedPipe(pipe) => pipe.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInstanceObservation {
    pub service: bool,
    pub payload: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInstanceObservation {
    pub service: bool,
    pub payload: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyRootService {
    pub service_file: PathBuf,
    pub referenced_paths: Vec<PathBuf>,
    pub referenced_home: Option<PathBuf>,
    pub referenced_current_user_home: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserContamination {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceInventory {
    pub system: SystemInstanceObservation,
    pub user: UserInstanceObservation,
    pub legacy_root: Option<LegacyRootService>,
    pub user_contamination: Option<UserContamination>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationState {
    None,
    UserOnly,
    LegacyRootOnly,
    SystemOnly,
    LegacyRootAndUser,
    SystemAndUser,
    LegacyRootAndSystemConflict,
    ContaminatedUserConfig,
}

impl InstanceInventory {
    pub fn classify(&self) -> InstallationState {
        if self.user_contamination.is_some() {
            return InstallationState::ContaminatedUserConfig;
        }

        let system_present = self.system.service || self.system.payload;
        let user_present = self.user.service || self.user.payload;
        let legacy = self.legacy_root.is_some();

        match (legacy, system_present, user_present) {
            (true, true, _) => InstallationState::LegacyRootAndSystemConflict,
            (true, false, true) => InstallationState::LegacyRootAndUser,
            (true, false, false) => InstallationState::LegacyRootOnly,
            (false, true, true) => InstallationState::SystemAndUser,
            (false, true, false) => InstallationState::SystemOnly,
            (false, false, true) => InstallationState::UserOnly,
            (false, false, false) => InstallationState::None,
        }
    }

    pub fn service_presence_for_mode_resolution(&self) -> ServicePresence {
        // v3 keeps configuration per-user and shared between modes, so payload
        // presence alone cannot prove that either mode is installed. Resolution
        // must be driven by actual service artifacts/runtimes; payload-only
        // leftovers are diagnostics/cleanup inputs, not mode selectors.
        if self.legacy_root.is_some() || self.user_contamination.is_some() {
            return ServicePresence {
                system: false,
                user: false,
            };
        }
        ServicePresence {
            system: self.system.service,
            user: self.user.service,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeRequest {
    ExplicitSystem,
    ExplicitUser,
    Unspecified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServicePresence {
    pub system: bool,
    pub user: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandIntent {
    Install,
    ReadOnly,
    Mutating,
    StartLike,
    StopLike,
    UninstallLike,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSource {
    ExplicitFlag,
    ServicePresence,
    RuntimePresence,
    DefaultMode,
    EnvOverride,
    InteractivePrompt,
    LegacyDetection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeResolution {
    Resolved(InstanceMode),
    PromptRequired,
    AmbiguousBothInstalled,
    NotInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeResolutionWithSource {
    Resolved {
        mode: InstanceMode,
        source: ResolutionSource,
    },
    PromptRequired {
        source: ResolutionSource,
    },
    AmbiguousBothInstalled,
    NotInstalled,
}

pub fn resolve_instance_mode(
    request: ModeRequest,
    services: ServicePresence,
    intent: CommandIntent,
) -> ModeResolution {
    match resolve_instance_mode_with_source(request, services, intent) {
        ModeResolutionWithSource::Resolved { mode, .. } => ModeResolution::Resolved(mode),
        ModeResolutionWithSource::PromptRequired { .. } => ModeResolution::PromptRequired,
        ModeResolutionWithSource::AmbiguousBothInstalled => ModeResolution::AmbiguousBothInstalled,
        ModeResolutionWithSource::NotInstalled => ModeResolution::NotInstalled,
    }
}

pub fn resolve_instance_mode_with_source(
    request: ModeRequest,
    services: ServicePresence,
    intent: CommandIntent,
) -> ModeResolutionWithSource {
    match request {
        ModeRequest::ExplicitSystem => {
            return ModeResolutionWithSource::Resolved {
                mode: InstanceMode::System,
                source: ResolutionSource::ExplicitFlag,
            }
        }
        ModeRequest::ExplicitUser => {
            return ModeResolutionWithSource::Resolved {
                mode: InstanceMode::User,
                source: ResolutionSource::ExplicitFlag,
            }
        }
        ModeRequest::Unspecified => {}
    }

    if intent == CommandIntent::Install {
        return ModeResolutionWithSource::PromptRequired {
            source: ResolutionSource::InteractivePrompt,
        };
    }

    match (services.system, services.user) {
        (true, false) => ModeResolutionWithSource::Resolved {
            mode: InstanceMode::System,
            source: ResolutionSource::ServicePresence,
        },
        (false, true) => ModeResolutionWithSource::Resolved {
            mode: InstanceMode::User,
            source: ResolutionSource::ServicePresence,
        },
        (true, true) => ModeResolutionWithSource::AmbiguousBothInstalled,
        (false, false) => ModeResolutionWithSource::NotInstalled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn inputs() -> PathInputs {
        PathInputs::for_tests()
    }

    fn empty_inventory() -> InstanceInventory {
        InstanceInventory {
            system: SystemInstanceObservation {
                service: false,
                payload: false,
            },
            user: UserInstanceObservation {
                service: false,
                payload: false,
            },
            legacy_root: None,
            user_contamination: None,
        }
    }

    #[test]
    fn instance_inventory_classifies_legacy_before_v2_resolution() {
        let mut inv = empty_inventory();
        inv.legacy_root = Some(LegacyRootService {
            service_file: PathBuf::from("/Library/LaunchDaemons/io.mihomo.plist"),
            referenced_paths: vec![PathBuf::from("/Users/alice/.config/mihomo/start.sh")],
            referenced_home: Some(PathBuf::from("/Users/alice")),
            referenced_current_user_home: true,
        });
        assert_eq!(inv.classify(), InstallationState::LegacyRootOnly);
        assert_eq!(
            inv.service_presence_for_mode_resolution(),
            ServicePresence {
                system: false,
                user: false
            }
        );

        inv.user.service = true;
        assert_eq!(inv.classify(), InstallationState::LegacyRootAndUser);
        assert_eq!(
            inv.service_presence_for_mode_resolution(),
            ServicePresence {
                system: false,
                user: false
            }
        );
    }

    #[test]
    fn instance_inventory_exposes_only_current_services_to_mode_resolution() {
        let mut inv = empty_inventory();
        inv.system.service = true;
        assert_eq!(inv.classify(), InstallationState::SystemOnly);
        assert_eq!(
            inv.service_presence_for_mode_resolution(),
            ServicePresence {
                system: true,
                user: false
            }
        );

        inv.user.payload = true;
        assert_eq!(inv.classify(), InstallationState::SystemAndUser);
        assert_eq!(
            inv.service_presence_for_mode_resolution(),
            ServicePresence {
                system: true,
                user: false
            }
        );

        inv.user.service = true;
        assert_eq!(
            inv.service_presence_for_mode_resolution(),
            ServicePresence {
                system: true,
                user: true
            }
        );
    }

    #[test]
    fn shared_config_payload_does_not_create_mode_resolution_conflict() {
        let mut inv = empty_inventory();
        inv.system.payload = true;
        inv.user.payload = true;
        assert_eq!(inv.classify(), InstallationState::SystemAndUser);
        assert_eq!(
            inv.service_presence_for_mode_resolution(),
            ServicePresence {
                system: false,
                user: false
            },
            "shared per-user config/binary leftovers must not masquerade as installed services"
        );
    }

    #[test]
    fn instance_inventory_contamination_fails_closed() {
        let mut inv = empty_inventory();
        inv.user.service = true;
        inv.user_contamination = Some(UserContamination {
            paths: vec![PathBuf::from("/Users/alice/.config/mihomo/run")],
        });
        assert_eq!(inv.classify(), InstallationState::ContaminatedUserConfig);
        assert_eq!(
            inv.service_presence_for_mode_resolution(),
            ServicePresence {
                system: false,
                user: false
            }
        );
    }

    #[test]
    fn macos_system_config_uses_per_user_home_per_adr02() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs());
        assert_eq!(ctx.permissions, PermissionModel::PrivilegedSystem);
        assert_eq!(
            ctx.paths.config_dir,
            Path::new("/Users/alice/.config/mihomo")
        );
        assert_eq!(
            ctx.paths.core_binary,
            PathBuf::from("/Library/Application Support/mihomo/bin/mihomo")
        );
        assert_eq!(
            ctx.paths.api_endpoint,
            ApiEndpoint::UnixSocket(PathBuf::from("/var/run/mihomo/mihomo.sock"))
        );
        assert!(matches!(
            ctx.service,
            ServiceTarget::MacosLaunchDaemon { .. }
        ));
    }

    #[test]
    fn macos_user_paths_stay_under_user_home() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::User, &inputs());
        assert_eq!(ctx.permissions, PermissionModel::DirectUser);
        assert_eq!(
            ctx.paths.config_dir,
            Path::new("/Users/alice/.config/mihomo")
        );
        assert_eq!(
            ctx.paths.api_endpoint,
            ApiEndpoint::UnixSocket(PathBuf::from("/tmp/mihomo-501/mihomo.sock"))
        );
        assert!(matches!(
            ctx.service,
            ServiceTarget::MacosLaunchAgent { .. }
        ));
    }

    #[test]
    fn linux_system_and_user_paths_follow_spec_matrix() {
        let system = InstanceContext::planned(TargetOs::Linux, InstanceMode::System, &inputs());
        assert_eq!(
            system.paths.core_binary,
            PathBuf::from("/usr/local/lib/mihomo/mihomo")
        );
        assert_eq!(
            system.paths.config_dir,
            Path::new("/Users/alice/.config/mihomo")
        );
        assert_eq!(
            system.paths.api_endpoint,
            ApiEndpoint::UnixSocket(PathBuf::from("/var/run/mihomo/mihomo.sock"))
        );
        assert!(matches!(
            system.service,
            ServiceTarget::LinuxSystemdSystem { .. }
        ));

        let user = InstanceContext::planned(TargetOs::Linux, InstanceMode::User, &inputs());
        assert_eq!(
            user.paths.config_dir,
            Path::new("/Users/alice/.config/mihomo")
        );
        assert_eq!(
            user.paths.api_endpoint,
            ApiEndpoint::UnixSocket(PathBuf::from("/run/user/1000/mihomo/mihomo.sock"))
        );
        assert!(matches!(
            user.service,
            ServiceTarget::LinuxSystemdUser { .. }
        ));
    }

    #[test]
    fn windows_system_and_user_paths_are_separate() {
        let system = InstanceContext::planned(TargetOs::Windows, InstanceMode::System, &inputs());
        // ADR-02: system config_dir is per-user and follows Windows APPDATA convention.
        assert_eq!(
            system.paths.config_dir,
            PathBuf::from(r"C:\Users\alice\AppData\Roaming").join("mihomo")
        );
        // binary and logs remain under ProgramData
        assert_eq!(
            system.paths.core_binary,
            PathBuf::from(r"C:\ProgramData").join("mihomo/bin/mihomo.exe")
        );
        assert_eq!(
            system.paths.api_endpoint,
            ApiEndpoint::WindowsNamedPipe(r"\\.\pipe\mihomo-core".to_string())
        );
        assert!(matches!(
            system.service,
            ServiceTarget::WindowsService { .. }
        ));

        let user = InstanceContext::planned(TargetOs::Windows, InstanceMode::User, &inputs());
        assert_eq!(
            user.paths.config_dir,
            PathBuf::from(r"C:\Users\alice\AppData\Roaming").join("mihomo")
        );
        assert_eq!(
            user.paths.api_endpoint,
            ApiEndpoint::WindowsNamedPipe(r"\\.\pipe\mihomo-alice".to_string())
        );
        assert!(matches!(user.service, ServiceTarget::WindowsUserProcess));
    }

    #[test]
    fn system_contexts_are_privileged_and_user_contexts_are_direct() {
        for os in [TargetOs::Macos, TargetOs::Linux, TargetOs::Windows] {
            assert_eq!(
                InstanceContext::planned(os, InstanceMode::System, &inputs()).permissions,
                PermissionModel::PrivilegedSystem
            );
            assert_eq!(
                InstanceContext::planned(os, InstanceMode::User, &inputs()).permissions,
                PermissionModel::DirectUser
            );
        }
    }

    #[test]
    fn explicit_mode_request_overrides_detection() {
        let none = ServicePresence {
            system: false,
            user: false,
        };
        assert_eq!(
            resolve_instance_mode(ModeRequest::ExplicitSystem, none, CommandIntent::Mutating),
            ModeResolution::Resolved(InstanceMode::System)
        );
        assert_eq!(
            resolve_instance_mode(ModeRequest::ExplicitUser, none, CommandIntent::Mutating),
            ModeResolution::Resolved(InstanceMode::User)
        );
    }

    #[test]
    fn mode_resolution_reports_source_for_diagnostics() {
        assert_eq!(
            resolve_instance_mode_with_source(
                ModeRequest::ExplicitSystem,
                ServicePresence {
                    system: false,
                    user: false,
                },
                CommandIntent::ReadOnly,
            ),
            ModeResolutionWithSource::Resolved {
                mode: InstanceMode::System,
                source: ResolutionSource::ExplicitFlag,
            }
        );
        assert_eq!(
            resolve_instance_mode_with_source(
                ModeRequest::Unspecified,
                ServicePresence {
                    system: false,
                    user: true,
                },
                CommandIntent::ReadOnly,
            ),
            ModeResolutionWithSource::Resolved {
                mode: InstanceMode::User,
                source: ResolutionSource::ServicePresence,
            }
        );
        assert_eq!(
            resolve_instance_mode_with_source(
                ModeRequest::Unspecified,
                ServicePresence {
                    system: false,
                    user: false,
                },
                CommandIntent::StartLike,
            ),
            ModeResolutionWithSource::NotInstalled
        );
        assert_eq!(
            resolve_instance_mode_with_source(
                ModeRequest::Unspecified,
                ServicePresence {
                    system: false,
                    user: false,
                },
                CommandIntent::Install,
            ),
            ModeResolutionWithSource::PromptRequired {
                source: ResolutionSource::InteractivePrompt,
            }
        );
    }

    #[test]
    fn unspecified_mode_uses_single_installed_service() {
        assert_eq!(
            resolve_instance_mode(
                ModeRequest::Unspecified,
                ServicePresence {
                    system: true,
                    user: false,
                },
                CommandIntent::Mutating,
            ),
            ModeResolution::Resolved(InstanceMode::System)
        );
        assert_eq!(
            resolve_instance_mode(
                ModeRequest::Unspecified,
                ServicePresence {
                    system: false,
                    user: true,
                },
                CommandIntent::Mutating,
            ),
            ModeResolution::Resolved(InstanceMode::User)
        );
    }

    #[test]
    fn both_services_are_always_ambiguous_in_v3_mutually_exclusive_model() {
        let both = ServicePresence {
            system: true,
            user: true,
        };
        assert_eq!(
            resolve_instance_mode(ModeRequest::Unspecified, both, CommandIntent::ReadOnly),
            ModeResolution::AmbiguousBothInstalled
        );
        assert_eq!(
            resolve_instance_mode(ModeRequest::Unspecified, both, CommandIntent::Mutating),
            ModeResolution::AmbiguousBothInstalled
        );
    }

    #[test]
    fn install_prompts_and_start_like_ignore_stale_marker_when_no_service_exists() {
        let none = ServicePresence {
            system: false,
            user: false,
        };
        assert_eq!(
            resolve_instance_mode(ModeRequest::Unspecified, none, CommandIntent::Install),
            ModeResolution::PromptRequired
        );
        assert_eq!(
            resolve_instance_mode(ModeRequest::Unspecified, none, CommandIntent::StartLike),
            ModeResolution::NotInstalled
        );
        assert_eq!(
            resolve_instance_mode(ModeRequest::Unspecified, none, CommandIntent::ReadOnly),
            ModeResolution::NotInstalled
        );
    }

    #[test]
    fn macos_system_launchd_artifacts_use_system_binary_with_per_user_config() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs());
        let artifacts = planned_macos_launchd_artifacts(&ctx).unwrap();

        assert!(artifacts.privileged);
        assert_eq!(
            artifacts.plist_path,
            PathBuf::from("/Library/LaunchDaemons/io.mihomo.plist")
        );
        assert_eq!(
            artifacts.start_script_path,
            PathBuf::from("/Library/Application Support/mihomo/start.sh")
        );
        for forbidden in ["~", "%LOCALAPPDATA%"] {
            assert!(!artifacts.plist_content.contains(forbidden), "{forbidden}");
            assert!(
                !artifacts.start_script_content.contains(forbidden),
                "{forbidden}"
            );
        }
        // ADR-02: plist references per-user config dir
        assert!(artifacts
            .plist_content
            .contains("/Users/alice/.config/mihomo"));
        assert!(artifacts
            .start_script_content
            .contains("/Library/Application Support/mihomo/bin/mihomo"));
        assert!(artifacts.start_script_content.contains("umask 000"));
        assert!(artifacts.start_script_content.contains("/var/run/mihomo"));
    }

    #[test]
    fn macos_user_launchd_artifacts_use_user_paths_and_private_runtime_dir() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::User, &inputs());
        let artifacts = planned_macos_launchd_artifacts(&ctx).unwrap();

        assert!(!artifacts.privileged);
        assert_eq!(
            artifacts.plist_path,
            PathBuf::from("/Users/alice/Library/LaunchAgents/io.mihomo.plist")
        );
        assert!(artifacts
            .plist_content
            .contains("/Users/alice/.config/mihomo"));
        assert!(artifacts.start_script_content.contains("chmod 700"));
        assert!(!artifacts.start_script_content.contains("umask 000"));
    }

    #[test]
    fn macos_system_install_plan_keeps_config_dir_user_owned() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs());
        let plan = planned_macos_install_plan(&ctx).unwrap();

        assert!(plan
            .directories
            .iter()
            .any(|d| d.path == ctx.paths.config_dir && !d.privileged));
        for required_privileged_dir in [
            PathBuf::from("/Library/Application Support/mihomo"),
            PathBuf::from("/Library/Application Support/mihomo/bin"),
            PathBuf::from("/var/log/mihomo"),
            PathBuf::from("/var/run/mihomo"),
        ] {
            assert!(
                plan.directories
                    .iter()
                    .any(|d| d.path == required_privileged_dir && d.privileged),
                "missing privileged directory {} in {plan:#?}",
                required_privileged_dir.display()
            );
        }
        assert!(plan.files.iter().all(|f| f.privileged));
        assert!(plan.commands.iter().all(|c| c.privileged));

        let rendered = format!("{plan:#?}");
        for required in [
            "/Library/Application Support/mihomo/bin",
            "/Users/alice/.config/mihomo",
            "/var/log/mihomo",
            "/var/run/mihomo",
            "/Library/LaunchDaemons/io.mihomo.plist",
            "bootstrap",
            "system",
            "kickstart",
            "system/io.mihomo",
        ] {
            assert!(
                rendered.contains(required),
                "missing {required}\n{rendered}"
            );
        }
        for forbidden in ["~", "%LOCALAPPDATA%"] {
            assert!(
                !rendered.contains(forbidden),
                "forbidden {forbidden}\n{rendered}"
            );
        }
    }

    #[test]
    fn macos_user_install_plan_is_direct_and_uses_user_paths() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::User, &inputs());
        let plan = planned_macos_install_plan(&ctx).unwrap();

        assert!(plan.directories.iter().all(|d| !d.privileged));
        assert!(plan.files.iter().all(|f| !f.privileged));
        assert!(plan.commands.iter().all(|c| !c.privileged));

        let rendered = format!("{plan:#?}");
        for required in [
            "/Users/alice/.config/mihomo",
            "/tmp/mihomo-501",
            "/Users/alice/Library/LaunchAgents/io.mihomo.plist",
            "/Users/alice/Library/Logs/mihomo",
            "/Users/alice/Library/Logs/mihomo/mihomo.log",
            "bootstrap",
            "gui/501",
            "kickstart",
            "gui/501/io.mihomo",
        ] {
            assert!(
                rendered.contains(required),
                "missing {required}\n{rendered}"
            );
        }
        assert!(!rendered.contains("/Library/LaunchDaemons"));
        assert!(!rendered.contains("/var/run/mihomo"));
        assert!(!rendered.contains("/Users/alice/.config/mihomo/run"));
    }

    #[test]
    fn system_service_plans_use_per_user_config_paths() {
        // ADR-02: system config_dir is per-user on all platforms
        for os in [TargetOs::Macos, TargetOs::Linux, TargetOs::Windows] {
            let ctx = InstanceContext::planned(os, InstanceMode::System, &inputs());
            let plan = planned_install_plan(&ctx).unwrap();
            let rendered = format!("{plan:#?}");
            // config_dir should be per-user and follow the platform convention.
            let expected_config = ctx.paths.config_dir.clone();
            assert!(
                plan.directories.iter().any(|d| d.path == expected_config),
                "system {os:?} install plan missing per-user config path {}:\n{rendered}",
                expected_config.display()
            );
            // but not under legacy system config locations or env-var tokens
            for forbidden in ["~", "%USERPROFILE%", "%LOCALAPPDATA%"] {
                assert!(
                    !rendered.contains(forbidden),
                    "system {os:?} install plan contains forbidden token {forbidden}:\n{rendered}"
                );
            }
        }
    }

    #[test]
    fn linux_system_install_plan_matches_systemd_spec() {
        let ctx = InstanceContext::planned(TargetOs::Linux, InstanceMode::System, &inputs());
        let plan = planned_install_plan(&ctx).unwrap();
        let rendered = format!("{plan:#?}");

        // ADR-02: config is per-user even for system service, so only system
        // payload/service directories require privilege.
        assert!(plan
            .directories
            .iter()
            .any(|d| d.path == ctx.paths.config_dir && !d.privileged));
        for required_privileged_dir in [
            PathBuf::from("/usr/local/lib/mihomo"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/var/log/mihomo"),
            PathBuf::from("/etc/systemd/system"),
        ] {
            assert!(
                plan.directories
                    .iter()
                    .any(|d| d.path == required_privileged_dir && d.privileged),
                "missing privileged directory {} in {plan:#?}",
                required_privileged_dir.display()
            );
        }
        assert!(plan.files.iter().all(|f| f.privileged));
        assert!(plan.commands.iter().all(|c| c.privileged));
        for required in [
            "/usr/local/lib/mihomo",
            "/etc/systemd/system/mihomo.service",
            "/Users/alice/.config/mihomo",
            "/usr/local/bin/mihomo-cli",
            "ExecStart=/usr/local/bin/mihomo-cli daemon",
            "RuntimeDirectory=mihomo",
            "RuntimeDirectoryMode=0755",
            "StandardOutput=append:/var/log/mihomo/mihomo.log",
            "daemon-reload",
            "enable",
            "--now",
        ] {
            assert!(
                rendered.contains(required),
                "missing {required}\n{rendered}"
            );
        }
    }

    #[test]
    fn linux_user_install_plan_matches_systemd_user_spec() {
        let ctx = InstanceContext::planned(TargetOs::Linux, InstanceMode::User, &inputs());
        let plan = planned_install_plan(&ctx).unwrap();
        let rendered = format!("{plan:#?}");

        assert!(plan.directories.iter().all(|d| !d.privileged));
        assert!(plan.files.iter().all(|f| !f.privileged));
        assert!(plan.commands.iter().all(|c| !c.privileged));
        for required in [
            "/Users/alice/.config/mihomo",
            "/run/user/1000/mihomo",
            "/Users/alice/.config/systemd/user/mihomo.service",
            "/Users/alice/.local/state/mihomo",
            "ExecStart=/Users/alice/.local/bin/mihomo -d /Users/alice/.config/mihomo",
            "StandardOutput=append:/Users/alice/.local/state/mihomo/mihomo.log",
            "--user",
            "enable",
            "--now",
        ] {
            assert!(
                rendered.contains(required),
                "missing {required}\n{rendered}"
            );
        }
        assert!(!rendered.contains("/etc/systemd/system"));
        assert!(!rendered.contains("/run/mihomo"));
    }

    #[test]
    fn windows_system_install_plan_uses_program_data_and_service() {
        let ctx = InstanceContext::planned(TargetOs::Windows, InstanceMode::System, &inputs());
        let plan = planned_install_plan(&ctx).unwrap();
        let rendered = format!("{plan:#?}");

        assert!(plan.commands.iter().all(|c| c.privileged));
        // ADR-02: system config_dir is per-user and follows Windows APPDATA convention.
        assert!(plan.directories.iter().any(|d| {
            d.path == PathBuf::from(r"C:\Users\alice\AppData\Roaming").join("mihomo")
                && !d.privileged
        }));
        assert!(plan.directories.iter().any(|d| {
            d.path == PathBuf::from(r"C:\ProgramData").join("mihomo/bin") && d.privileged
        }));
        assert!(plan.directories.iter().any(|d| {
            d.path == PathBuf::from(r"C:\ProgramData").join("mihomo") && d.privileged
        }));
        assert_eq!(
            ctx.paths.api_endpoint,
            ApiEndpoint::WindowsNamedPipe(r"\\.\pipe\mihomo-core".to_string())
        );
        assert_eq!(plan.commands[0].args, vec!["stop", "mihomo"]);
        assert_eq!(plan.commands[1].args, vec!["delete", "mihomo"]);
        let create = plan
            .commands
            .iter()
            .find(|command| command.args.first().map(String::as_str) == Some("create"))
            .expect("windows service create command");
        assert!(create.args.contains(&format!(
            "binPath= \"{}\" daemon",
            ctx.paths.cli_binary.display()
        )));
        for required in ["sc.exe", "create", "start= auto"] {
            assert!(
                rendered.contains(required),
                "missing {required}\n{rendered}"
            );
        }
        assert!(!rendered.contains(r"C:\\Users\\alice\\AppData\\Local"));
    }

    #[test]
    fn windows_user_install_plan_uses_local_app_data_and_direct_process() {
        let ctx = InstanceContext::planned(TargetOs::Windows, InstanceMode::User, &inputs());
        let plan = planned_install_plan(&ctx).unwrap();
        let rendered = format!("{plan:#?}");

        assert!(plan.directories.iter().all(|d| !d.privileged));
        assert!(plan.commands.iter().all(|c| !c.privileged));
        assert!(plan.directories.iter().any(|d| {
            d.path == PathBuf::from(r"C:\Users\alice\AppData\Roaming").join("mihomo")
        }));
        assert!(plan.directories.iter().any(|d| {
            d.path == PathBuf::from(r"C:\Users\alice\AppData\Local").join("mihomo/bin")
        }));
        assert!(plan
            .directories
            .iter()
            .any(|d| { d.path == PathBuf::from(r"C:\Users\alice\AppData\Local").join("mihomo") }));
        assert_eq!(
            ctx.paths.api_endpoint,
            ApiEndpoint::WindowsNamedPipe(r"\\.\pipe\mihomo-alice".to_string())
        );
        assert!(rendered.contains("mihomo.exe"));
        assert!(rendered.contains("cmd.exe"));
        assert!(rendered.contains("start"));
        assert!(!rendered.contains(r"C:\\ProgramData"));
        assert!(!rendered.contains("sc.exe"));
    }

    #[test]
    fn service_control_plans_match_macos_domains_and_privileges() {
        let system = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs());
        let user = InstanceContext::planned(TargetOs::Macos, InstanceMode::User, &inputs());

        let system_restart = planned_service_plan(&system, ServiceAction::Restart);
        assert_eq!(system_restart.commands[0].program, "launchctl");
        assert_eq!(
            system_restart.commands[0].args,
            vec!["kickstart", "-k", "system/io.mihomo"]
        );
        assert!(system_restart.commands[0].privileged);

        let user_stop = planned_service_plan(&user, ServiceAction::Stop);
        assert_eq!(
            user_stop.commands[0].args,
            vec!["bootout", "gui/501/io.mihomo"]
        );
        assert!(!user_stop.commands[0].privileged);
    }

    #[test]
    fn service_control_plans_match_linux_systemd_modes() {
        let system = InstanceContext::planned(TargetOs::Linux, InstanceMode::System, &inputs());
        let user = InstanceContext::planned(TargetOs::Linux, InstanceMode::User, &inputs());

        let system_restart = planned_service_plan(&system, ServiceAction::Restart);
        assert_eq!(system_restart.commands[0].args, vec!["restart", "mihomo"]);
        assert!(system_restart.commands[0].privileged);

        let user_restart = planned_service_plan(&user, ServiceAction::Restart);
        assert_eq!(
            user_restart.commands[0].args,
            vec!["--user", "restart", "mihomo"]
        );
        assert!(!user_restart.commands[0].privileged);
    }

    #[test]
    fn service_control_plans_match_windows_system_and_user_modes() {
        let system = InstanceContext::planned(TargetOs::Windows, InstanceMode::System, &inputs());
        let user = InstanceContext::planned(TargetOs::Windows, InstanceMode::User, &inputs());

        let system_restart = planned_service_plan(&system, ServiceAction::Restart);
        assert_eq!(system_restart.commands.len(), 2);
        assert_eq!(system_restart.commands[0].args, vec!["stop", "mihomo"]);
        assert_eq!(system_restart.commands[1].args, vec!["start", "mihomo"]);
        assert!(system_restart.commands.iter().all(|c| c.privileged));

        let user_start = planned_service_plan(&user, ServiceAction::Start);
        assert_eq!(user_start.commands[0].program, "cmd.exe");
        assert_eq!(user_start.commands[0].args[0], "/C");
        assert_eq!(user_start.commands[0].args[1], "start");
        assert!(user_start.commands[0]
            .args
            .iter()
            .any(|arg| arg.contains("mihomo.exe")));
        assert!(!user_start.commands[0].privileged);

        let user_stop = planned_service_plan(&user, ServiceAction::Stop);
        assert_eq!(user_stop.commands[0].program, "taskkill");
        assert!(user_stop.commands[0].args.contains(&"/F".to_string()));
        assert!(user_stop.commands[0]
            .args
            .iter()
            .any(|arg| arg.starts_with("WINDOWTITLE eq mihomo:")));
    }

    #[test]
    fn uninstall_plans_remove_only_resolved_instance_paths() {
        let macos_root = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs());
        let macos_user = InstanceContext::planned(TargetOs::Macos, InstanceMode::User, &inputs());

        let system_plan = planned_service_plan(&macos_root, ServiceAction::Uninstall);
        let system_rendered = format!("{system_plan:#?}");
        assert!(system_rendered.contains("/Library/LaunchDaemons/io.mihomo.plist"));
        assert!(system_rendered.contains("/Library/Application Support/mihomo/bin/mihomo"));
        assert!(system_rendered.contains("/Library/Application Support/mihomo/bin/mihomo-cli"));
        // ADR-02: system uninstall targets per-user config dir, but that path is
        // not privileged; only service/binary/runtime/log artifacts require root.
        assert!(system_rendered.contains("/Users/alice/.config/mihomo"));
        assert!(system_plan
            .remove_paths
            .iter()
            .any(|p| { p.path == Path::new("/Users/alice/.config/mihomo") && !p.privileged }));
        assert!(system_plan
            .remove_paths
            .iter()
            .any(|p| { p.path == Path::new("/var/log/mihomo/mihomo.log") && p.privileged }));
        assert!(system_plan
            .remove_paths
            .iter()
            .any(|p| { p.path == Path::new("/var/run/mihomo") && p.privileged }));

        let linux_system = planned_service_plan(
            &InstanceContext::planned(TargetOs::Linux, InstanceMode::System, &inputs()),
            ServiceAction::Uninstall,
        );
        assert!(linux_system
            .remove_paths
            .iter()
            .any(|p| { p.path == Path::new("/Users/alice/.config/mihomo") && !p.privileged }));
        assert!(linux_system
            .remove_paths
            .iter()
            .any(|p| { p.path == Path::new("/usr/local/bin/mihomo-cli") && p.privileged }));

        let windows_system = planned_service_plan(
            &InstanceContext::planned(TargetOs::Windows, InstanceMode::System, &inputs()),
            ServiceAction::Uninstall,
        );
        assert!(windows_system.remove_paths.iter().any(|p| {
            p.path == PathBuf::from(r"C:\Users\alice\AppData\Roaming").join("mihomo")
                && !p.privileged
        }));
        assert!(windows_system.remove_paths.iter().any(|p| {
            p.path
                == PathBuf::from(r"C:\ProgramData")
                    .join("mihomo")
                    .join("bin")
                    .join("mihomo-cli.exe")
                && p.privileged
        }));

        let user = planned_service_plan(&macos_user, ServiceAction::Uninstall);
        let user_rendered = format!("{user:#?}");
        assert!(user.remove_paths.iter().all(|p| !p.privileged));
        assert!(user_rendered.contains("/Users/alice/.config/mihomo"));
        assert!(user_rendered.contains("/Users/alice/Library/LaunchAgents/io.mihomo.plist"));
        assert!(!user_rendered.contains("/Library/LaunchDaemons"));
        assert!(!user_rendered.contains("/var/run/mihomo"));
    }

    #[test]
    fn config_store_plan_keeps_system_config_writes_direct_and_rollback_capable() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs());
        let plan = planned_config_store(&ctx);
        assert_eq!(plan.mode, InstanceMode::System);
        assert_eq!(plan.strategy, ConfigWriteStrategy::DirectAtomicWrite);
        assert_eq!(plan.config_dir, Path::new("/Users/alice/.config/mihomo"));

        let write = plan
            .operations
            .iter()
            .find(|op| op.kind == ConfigStoreOperationKind::AtomicWrite)
            .expect("atomic write operation");
        assert_eq!(write.target, ctx.paths.config_file);
        assert!(!write.privileged);
        assert!(write.rollback_required);

        let validate = plan
            .operations
            .iter()
            .find(|op| op.kind == ConfigStoreOperationKind::Validate)
            .expect("validate operation");
        assert!(!validate.privileged);
        assert!(validate.rollback_required);
    }

    #[test]
    fn config_store_plan_keeps_user_writes_direct() {
        let ctx = InstanceContext::planned(TargetOs::Linux, InstanceMode::User, &inputs());
        let plan = planned_config_store(&ctx);
        assert_eq!(plan.mode, InstanceMode::User);
        assert_eq!(plan.strategy, ConfigWriteStrategy::DirectAtomicWrite);
        assert!(plan.operations.iter().all(|op| !op.privileged));
        assert!(plan
            .operations
            .iter()
            .any(|op| op.kind == ConfigStoreOperationKind::AtomicWrite && op.rollback_required));
    }

    #[test]
    fn config_mutation_plan_uses_direct_writes_for_system_instances() {
        for os in [TargetOs::Macos, TargetOs::Linux, TargetOs::Windows] {
            let ctx = InstanceContext::planned(os, InstanceMode::System, &inputs());
            let plan = planned_config_mutation(&ctx, ConfigMutationKind::FixRuntimeController);

            assert_eq!(plan.write_strategy, ConfigWriteStrategy::DirectAtomicWrite);
            assert!(!plan.validate_command.privileged);
            assert!(plan
                .restart_after_write
                .commands
                .iter()
                .all(|c| c.privileged));
            assert_eq!(plan.target_config, ctx.paths.config_file);
            assert_eq!(plan.endpoint, ctx.paths.api_endpoint);
        }
    }

    #[test]
    fn config_mutation_plan_uses_direct_atomic_write_for_user_instances() {
        for os in [TargetOs::Macos, TargetOs::Linux, TargetOs::Windows] {
            let ctx = InstanceContext::planned(os, InstanceMode::User, &inputs());
            let plan = planned_config_mutation(&ctx, ConfigMutationKind::UpdateRules);

            assert_eq!(plan.write_strategy, ConfigWriteStrategy::DirectAtomicWrite);
            assert!(!plan.validate_command.privileged);
            assert!(plan
                .restart_after_write
                .commands
                .iter()
                .all(|c| !c.privileged));
            assert_eq!(plan.config_dir, ctx.paths.config_dir);
            assert_eq!(plan.backup_dir, ctx.paths.backup_dir);
        }
    }

    #[test]
    fn privileged_invocation_plan_has_noninteractive_manual_fallback() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs());
        let restart = planned_service_plan(&ctx, ServiceAction::Restart)
            .commands
            .into_iter()
            .next()
            .unwrap();
        let plan = privilege_invocation_plan(restart).unwrap();

        assert!(plan.direct_when_elevated);
        assert!(plan.may_prompt_in_tty);
        assert_eq!(
            plan.non_interactive_failure,
            PrivilegeFailureCategory::NotElevatedNonInteractive
        );
        assert_eq!(
            plan.manual_fallback,
            "sudo launchctl kickstart -k system/io.mihomo"
        );
    }

    #[test]
    fn direct_commands_do_not_need_privilege_invocation() {
        let ctx = InstanceContext::planned(TargetOs::Linux, InstanceMode::User, &inputs());
        let restart = planned_service_plan(&ctx, ServiceAction::Restart)
            .commands
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(privilege_invocation_plan(restart), None);
    }

    #[test]
    fn status_diagnostics_use_resolved_paths_and_expected_endpoint() {
        let system = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs());
        let user = InstanceContext::planned(TargetOs::Windows, InstanceMode::User, &inputs());

        let system_plan = planned_status_diagnostics(&system);
        assert_eq!(system_plan.mode, InstanceMode::System);
        assert_eq!(
            system_plan.binary,
            PathBuf::from("/Library/Application Support/mihomo/bin/mihomo")
        );
        assert_eq!(
            system_plan.config_file,
            PathBuf::from("/Users/alice/.config/mihomo/config.yaml")
        );
        assert_eq!(
            system_plan.expected_endpoint,
            ApiEndpoint::UnixSocket(PathBuf::from("/var/run/mihomo/mihomo.sock"))
        );
        assert!(system_plan
            .probes
            .iter()
            .any(|p| p.kind == DiagnosticProbeKind::ConfiguredEndpointMatchesExpected));
        assert!(system_plan
            .probes
            .iter()
            .any(|p| p.kind == DiagnosticProbeKind::ApiResponds));
        // ADR-02: config-related probe targets may reference per-user config path

        let user_plan = planned_status_diagnostics(&user);
        assert_eq!(user_plan.mode, InstanceMode::User);
        assert_eq!(
            user_plan.expected_endpoint,
            ApiEndpoint::WindowsNamedPipe(r"\\.\pipe\mihomo-alice".to_string())
        );
        assert!(user_plan
            .probes
            .iter()
            .any(|p| p.target == r"\\.\pipe\mihomo-alice"));
    }

    #[test]
    fn readiness_plan_requires_service_endpoint_and_api_checks() {
        let ctx = InstanceContext::planned(TargetOs::Linux, InstanceMode::System, &inputs());
        let plan = planned_readiness(&ctx);

        assert_eq!(plan.mode, InstanceMode::System);
        assert_eq!(
            plan.service_running_probe.kind,
            DiagnosticProbeKind::ServiceRunning
        );
        assert_eq!(
            plan.configured_endpoint_probe.kind,
            DiagnosticProbeKind::ConfiguredEndpointMatchesExpected
        );
        assert_eq!(
            plan.endpoint_connect_probe.kind,
            DiagnosticProbeKind::EndpointConnectable
        );
        assert_eq!(plan.api_probe.kind, DiagnosticProbeKind::ApiResponds);
        assert_eq!(
            plan.expected_endpoint,
            ApiEndpoint::UnixSocket(PathBuf::from("/var/run/mihomo/mihomo.sock"))
        );
        assert_eq!(
            plan.failure_hint_command.unwrap().manual_fallback,
            "sudo systemctl restart mihomo"
        );
    }

    #[test]
    fn user_readiness_does_not_require_privileged_failure_hint() {
        let ctx = InstanceContext::planned(TargetOs::Macos, InstanceMode::User, &inputs());
        let plan = planned_readiness(&ctx);

        assert_eq!(
            plan.expected_endpoint,
            ApiEndpoint::UnixSocket(PathBuf::from("/tmp/mihomo-501/mihomo.sock"))
        );
        assert_eq!(plan.failure_hint_command, None);
    }

    #[test]
    fn target_os_current_matches_compile_time_platform() {
        let current = TargetOs::current().expect("supported test platform");
        if cfg!(target_os = "macos") {
            assert_eq!(current, TargetOs::Macos);
        } else if cfg!(target_os = "linux") {
            assert_eq!(current, TargetOs::Linux);
        } else if cfg!(target_os = "windows") {
            assert_eq!(current, TargetOs::Windows);
        }
    }

    #[test]
    fn current_context_adapter_uses_current_os_and_requested_mode() {
        let system =
            planned_current_context(InstanceMode::System).expect("supported test platform");
        let user = planned_current_context(InstanceMode::User).expect("supported test platform");

        assert_eq!(system.os, TargetOs::current().unwrap());
        assert_eq!(system.mode, InstanceMode::System);
        assert_eq!(system.permissions, PermissionModel::PrivilegedSystem);
        assert_eq!(user.mode, InstanceMode::User);
        assert_eq!(user.permissions, PermissionModel::DirectUser);
        // ADR-02: system and user share the same per-user config_dir, but differ in binary/service
        assert_ne!(system.paths.core_binary, user.paths.core_binary);
    }
}
