#[cfg(test)]
use super::*;

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

    #[cfg(not(unix))]
    #[test]
    fn doctor_classifies_daemon_auth_failures_without_core_restart_hint() {
        let cases = [
            (
                "invalid or missing auth token",
                "Daemon authorization",
                "install --system --force",
            ),
            (
                "auth token does not belong to IPC peer uid",
                "Daemon authorization",
                "install --system --force",
            ),
        ];

        for (message, label, expected_hint) in cases {
            let output = doctor_daemon_error_check(instance::TargetOs::Windows, message).format();
            assert!(output.contains(label), "{output}");
            assert!(output.contains(expected_hint), "{output}");
            assert!(!output.contains("mihomo-cli restart --system"), "{output}");
        }
    }

    #[test]
    fn lifecycle_auth_failure_points_to_install_repair_not_core_restart() {
        let message = daemon_command_error_message(
            instance::TargetOs::Linux,
            "invalid or missing auth token",
        );
        assert!(message.contains("mihomo-cli doctor --system"), "{message}");
        assert!(
            message.contains("sudo mihomo-cli install --system"),
            "{message}"
        );
        assert!(
            !message.contains("mihomo-cli restart --system"),
            "{message}"
        );
    }

    #[cfg(not(unix))]
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
                assert!(output.contains("mihomo-cli doctor --system"), "{output}");
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

    #[test]
    fn access_subcommand_is_not_accessible_in_cli() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["mihomo-cli", "access"]).is_err());
        assert!(Cli::try_parse_from(["mihomo-cli", "access", "status"]).is_err());
        assert!(Cli::try_parse_from(["mihomo-cli", "access", "grant", "--user", "alice"]).is_err());
        assert!(Cli::try_parse_from(["mihomo-cli", "access", "list"]).is_err());

        use clap::CommandFactory;
        let mut cmd = Cli::command();
        let mut help = Vec::new();
        cmd.write_help(&mut help).unwrap();
        let help_str = String::from_utf8_lossy(&help);
        assert!(
            !help_str.contains("access"),
            "help should not contain access subcommand"
        );
    }

    #[cfg(unix)]
    #[test]
    fn doctor_owner_auth_output_is_diagnostic_and_never_contains_token_material() {
        let location = ipc::ClientTokenLocation {
            token_path: std::path::PathBuf::from("/home/alice/.config/mihomo/service-token"),
        };
        let check = format_doctor_owner_auth_check(
            instance::TargetOs::Linux,
            &location,
            true,
            OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::InvalidOrMissingToken),
            Some("invalid or missing auth token"),
        );
        assert!(!check.passed);
        assert!(check.detail.contains("invalid or missing auth token"));
        assert!(check.detail.contains("token_file_readable=true"));
        assert!(check
            .detail
            .contains("credential_path=/home/alice/.config/mihomo/service-token"));
        assert!(check
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("sudo mihomo-cli install --system"));
        assert!(!check.detail.contains("token="));
    }

    #[cfg(unix)]
    #[test]
    fn macos_doctor_owner_auth_hints_use_launchctl_not_systemctl() {
        let location = ipc::ClientTokenLocation {
            token_path: std::path::PathBuf::from("/Users/alice/.config/mihomo/service-token"),
        };
        let check = format_doctor_owner_auth_check(
            instance::TargetOs::Macos,
            &location,
            true,
            OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::OwnerRecordUnreadable),
            Some("cannot read owner record"),
        );
        let hint = check.hint.as_deref().unwrap_or_default();
        assert!(hint.contains("launchctl"), "{hint}");
        assert!(!hint.contains("systemctl"), "{hint}");
    }

    #[cfg(unix)]
    #[test]
    fn format_doctor_owner_auth_check_covers_all_variants() {
        let location = ipc::ClientTokenLocation {
            token_path: std::path::PathBuf::from("/home/alice/.config/mihomo/service-token"),
        };

        // Authorized
        let check = format_doctor_owner_auth_check(
            instance::TargetOs::Linux,
            &location,
            true,
            OwnerAuthDaemonStatus::Authorized,
            None,
        );
        assert!(check.passed);
        assert!(check.detail.contains("daemon_authenticated=true"));

        // Rejected(PeerUidMismatch)
        let check = format_doctor_owner_auth_check(
            instance::TargetOs::Linux,
            &location,
            true,
            OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::PeerUidMismatch),
            Some("auth token does not belong to IPC peer uid"),
        );
        assert!(!check.passed);
        assert!(check.detail.contains("uid_mismatch"));
        assert!(check
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("HOME/XDG_CONFIG_HOME"));

        // Rejected(Other)
        let check = format_doctor_owner_auth_check(
            instance::TargetOs::Linux,
            &location,
            true,
            OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::Other),
            Some("some unexpected daemon error"),
        );
        assert!(!check.passed);
        assert!(check.detail.contains("some unexpected daemon error"));

        // token_file_readable=false
        let check = format_doctor_owner_auth_check(
            instance::TargetOs::Linux,
            &location,
            false,
            OwnerAuthDaemonStatus::Rejected(DaemonIpcErrorKind::InvalidOrMissingToken),
            None,
        );
        assert!(!check.passed);
        assert!(check.detail.contains("token_file_readable=false"));
    }

    #[test]
    fn classify_daemon_ipc_error_covers_all_variants() {
        assert_eq!(
            classify_daemon_ipc_error("invalid or missing auth token"),
            DaemonIpcErrorKind::InvalidOrMissingToken
        );
        assert_eq!(
            classify_daemon_ipc_error("auth token does not belong to IPC peer uid"),
            DaemonIpcErrorKind::PeerUidMismatch
        );
        assert_eq!(
            classify_daemon_ipc_error("cannot read owner record: Permission denied"),
            DaemonIpcErrorKind::OwnerRecordUnreadable
        );
        // Backward-compatible: old "authorized clients" message also maps to OwnerRecordUnreadable
        assert_eq!(
            classify_daemon_ipc_error("cannot read authorized clients: Permission denied"),
            DaemonIpcErrorKind::OwnerRecordUnreadable
        );
        // Unknown messages fall through to Other
        assert_eq!(
            classify_daemon_ipc_error("connection reset by peer"),
            DaemonIpcErrorKind::Other
        );
        assert_eq!(classify_daemon_ipc_error(""), DaemonIpcErrorKind::Other);
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
                "install" | "uninstall" | "autostart" | "doctor" | "select"
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

    /// Issue #013: `config validate` must parse as a subcommand equivalent to
    /// `config --validate`.
    #[test]
    fn config_validate_subcommand_parses() {
        match parse(&["config", "validate"]).command {
            Some(Command::Config { command, .. }) => {
                assert!(matches!(command, Some(ConfigSubcommand::Validate)));
            }
            _ => panic!("expected config validate"),
        }
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
            match_pattern: "custom.example.com".to_string(),
            target: "1.1.1.1".to_string(),
        };
        assert_eq!(
            format_dns_policy_added("custom.example.com", "1.1.1.1"),
            vec!["  ✓ Policy added: custom.example.com → 1.1.1.1".to_string(),]
        );
        assert_eq!(
            format_dns_policy_removed(&policy),
            vec!["  ✓ Policy removed: custom.example.com → 1.1.1.1".to_string(),]
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
                "  1. custom.example.com → 1.1.1.1".to_string(),
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
                "  - custom.example.com → 1.1.1.1".to_string(),
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
        let policies = vec![(1, "nameserver-policy: +.corp -> 127.0.0.1".to_string())];

        assert_eq!(
            format_dns_status(&dns, &policies),
            vec![
                "  DNS: enabled (fake-ip)".to_string(),
                "  Default nameservers: 223.5.5.5, 1.1.1.1".to_string(),
                "  Fake-IP range: 198.18.0.1/16".to_string(),
                "  Listen: 127.0.0.1:1053".to_string(),
                String::new(),
                "  Policies:".to_string(),
                "    1. nameserver-policy: +.corp -> 127.0.0.1".to_string(),
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
    fn service_runtime_classification_covers_all_four_states() {
        use service::{classify_service_runtime, ServiceRuntimeState};
        assert_eq!(
            classify_service_runtime(false, false, false),
            ServiceRuntimeState::SystemdUnavailable
        );
        assert_eq!(
            classify_service_runtime(false, true, true),
            ServiceRuntimeState::SystemdUnavailable
        );
        assert_eq!(
            classify_service_runtime(true, false, false),
            ServiceRuntimeState::NotInstalled
        );
        assert_eq!(
            classify_service_runtime(true, true, false),
            ServiceRuntimeState::InstalledNotRunning
        );
        assert_eq!(
            classify_service_runtime(true, true, true),
            ServiceRuntimeState::Running
        );
    }

    #[test]
    fn systemd_unavailable_message_is_actionable_without_service_manager_commands() {
        let message = systemd_unavailable_message();
        assert!(message.contains("systemd 不可用"));
        assert!(message.contains("/etc/wsl.conf"));
        assert!(message.contains("systemd=true"));
        assert!(message.contains("wsl --shutdown"));
        assert!(message.contains("mihomo-cli install --system"));
        // 规则 18/19：不要求用户手敲平台服务管理器命令
        assert!(!message.contains("systemctl"));
    }

    #[test]
    fn service_block_message_distinguishes_all_diagnoses() {
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        let no_systemd = service_block_message_for_state(
            "restart",
            &ctx,
            service::ServiceRuntimeState::SystemdUnavailable,
        );
        let not_installed = service_block_message_for_state(
            "restart",
            &ctx,
            service::ServiceRuntimeState::NotInstalled,
        );
        let not_running = service_block_message_for_state(
            "restart",
            &ctx,
            service::ServiceRuntimeState::InstalledNotRunning,
        );
        let daemon_gone =
            service_block_message_for_state("restart", &ctx, service::ServiceRuntimeState::Running);

        assert!(no_systemd.contains("systemd 不可用"));
        assert!(not_installed.contains("mihomo-cli install --system"));
        assert!(not_running.contains("installed but not running"));
        assert!(not_running.contains("mihomo-cli start --system"));
        assert!(not_running.contains("mihomo-cli restart"));
        assert!(daemon_gone.contains("system daemon IPC is not running"));

        // 验收标准 2：未 enable / 未运行场景与无 systemd 场景文案可区分
        assert!(!not_running.contains("systemd 不可用"));
        assert!(!no_systemd.contains("installed but not running"));
    }

    #[test]
    fn service_runtime_health_labels_present_four_way_diagnosis() {
        assert_eq!(
            service_runtime_health_label(service::ServiceRuntimeState::SystemdUnavailable, false),
            Some("no-systemd")
        );
        assert_eq!(
            service_runtime_health_label(service::ServiceRuntimeState::NotInstalled, false),
            Some("not-installed")
        );
        assert_eq!(
            service_runtime_health_label(service::ServiceRuntimeState::InstalledNotRunning, false),
            Some("installed-not-running")
        );
        // Running 但实例未起：交回 rich labels；实例已起：不覆盖
        assert_eq!(
            service_runtime_health_label(service::ServiceRuntimeState::Running, false),
            None
        );
        assert_eq!(
            service_runtime_health_label(service::ServiceRuntimeState::SystemdUnavailable, true),
            None
        );
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

    #[cfg(unix)]
    #[test]
    fn test_heal_local_config_permissions_repairs_mode_and_setgid() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join(".config/mihomo");
        let sub_dir = config_dir.join("subscriptions");
        std::fs::create_dir_all(&sub_dir).unwrap();

        // 初始设置 subscriptions 目录为 0o755 (无 setgid)
        let _ = utils::set_directory_mode_no_follow(&sub_dir, 0o755);

        // 创建 active 文件，权限设置为 0o600 (缺少 group read)
        let active_file = sub_dir.join("active");
        std::fs::write(&active_file, "sub-test").unwrap();
        let _ = utils::set_file_mode_no_follow(&active_file, 0o600);

        // 创建一个订阅 yaml 文件，权限同样为 0o600
        let sub_yaml = sub_dir.join("sub-test.yaml");
        std::fs::write(&sub_yaml, "proxies: []\n").unwrap();
        let _ = utils::set_file_mode_no_follow(&sub_yaml, 0o600);

        let my_uid = unsafe { libc::geteuid() };
        heal_local_config_permissions(&config_dir, my_uid).unwrap();

        // 验证 subscriptions 目录已补上 setgid (如果系统支持)
        let sub_meta = std::fs::metadata(&sub_dir).unwrap();
        assert_eq!(sub_meta.mode() & 0o777, 0o755);
        if utils::mode_has_setgid(0o2000) {
            assert!(utils::mode_has_setgid(sub_meta.mode()));
        }

        // 验证 active 文件和订阅 yaml 文件的权限已收敛至 0o640
        let active_meta = std::fs::metadata(&active_file).unwrap();
        assert_eq!(active_meta.mode() & 0o777, 0o640);

        let yaml_meta = std::fs::metadata(&sub_yaml).unwrap();
        assert_eq!(yaml_meta.mode() & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn test_heal_local_config_permissions_covers_selections_and_transactions() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join(".config/mihomo");

        let selections_dir = config_dir.join("selections");
        std::fs::create_dir_all(&selections_dir).unwrap();
        let _ = utils::set_directory_mode_no_follow(&selections_dir, 0o755);

        let sel_yaml = selections_dir.join("sub-test.yaml");
        std::fs::write(&sel_yaml, "selections: {}").unwrap();
        let _ = utils::set_file_mode_no_follow(&sel_yaml, 0o600);

        let transactions_dir = config_dir.join("transactions");
        std::fs::create_dir_all(&transactions_dir).unwrap();
        let _ = utils::set_directory_mode_no_follow(&transactions_dir, 0o750);

        let journal = transactions_dir.join("tun-journal.json");
        std::fs::write(&journal, "{}").unwrap();
        let _ = utils::set_file_mode_no_follow(&journal, 0o600);

        let active_dir = transactions_dir.join("active");
        std::fs::create_dir_all(&active_dir).unwrap();
        let _ = utils::set_directory_mode_no_follow(&active_dir, 0o750);

        let active_journal = active_dir.join("journal.json");
        std::fs::write(&active_journal, "{}").unwrap();
        let _ = utils::set_file_mode_no_follow(&active_journal, 0o600);

        let my_uid = unsafe { libc::geteuid() };
        heal_local_config_permissions(&config_dir, my_uid).unwrap();

        let sel_meta = std::fs::metadata(&selections_dir).unwrap();
        assert_eq!(sel_meta.mode() & 0o7777, 0o2755);

        let sel_file_meta = std::fs::metadata(&sel_yaml).unwrap();
        assert_eq!(sel_file_meta.mode() & 0o777, 0o640);

        let tx_meta = std::fs::metadata(&transactions_dir).unwrap();
        assert_eq!(tx_meta.mode() & 0o7777, 0o2755);

        let journal_meta = std::fs::metadata(&journal).unwrap();
        assert_eq!(journal_meta.mode() & 0o777, 0o640);

        let active_meta = std::fs::metadata(&active_dir).unwrap();
        assert_eq!(active_meta.mode() & 0o7777, 0o2755);

        let active_journal_meta = std::fs::metadata(&active_journal).unwrap();
        assert_eq!(active_journal_meta.mode() & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn test_detect_config_assets_repair_detects_root_assets() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join(".config/mihomo");
        let sub_dir = config_dir.join("subscriptions");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let config_path = config_dir.join("config.yaml");
        std::fs::write(&config_path, "mixed-port: 7890\n").unwrap();

        let my_uid = unsafe { libc::geteuid() };

        // 当所有资产归属当前用户时，返回 NotNeeded
        let repair = detect_config_assets_repair(&config_dir, &config_path, my_uid, false).unwrap();
        assert_eq!(repair, ConfigOwnershipRepair::NotNeeded);

        // 当期望 uid 与实际不匹配且实际 uid 不为 0 时，返回 Err
        let fake_expected = my_uid + 9999;
        assert!(
            detect_config_assets_repair(&config_dir, &config_path, fake_expected, false).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn legacy_service_runtime_artifact_predicate_matrix() {
        use std::ffi::OsStr;

        // 命中：本实例服务 uid + 名单内文件名 + 常规文件
        assert!(is_legacy_service_runtime_artifact(
            OsStr::new("cache.db"),
            994,
            true,
            Some(994)
        ));
        // 名单外的服务归属文件不放行（金丝雀语义）
        assert!(!is_legacy_service_runtime_artifact(
            OsStr::new("authorized_keys"),
            994,
            true,
            Some(994)
        ));
        assert!(!is_legacy_service_runtime_artifact(
            OsStr::new("cache.db.bak"),
            994,
            true,
            Some(994)
        ));
        // 其它第三方 uid 不放行
        assert!(!is_legacy_service_runtime_artifact(
            OsStr::new("cache.db"),
            666,
            true,
            Some(994)
        ));
        // root 归属走既有 repair 分支，不算 legacy
        assert!(!is_legacy_service_runtime_artifact(
            OsStr::new("cache.db"),
            0,
            true,
            Some(994)
        ));
        // 非普通文件（符号链接/目录）不放行
        assert!(!is_legacy_service_runtime_artifact(
            OsStr::new("cache.db"),
            994,
            false,
            Some(994)
        ));
        // 服务账号解析不到时一律不放行
        assert!(!is_legacy_service_runtime_artifact(
            OsStr::new("cache.db"),
            994,
            true,
            None
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn system_service_uid_resolves_only_via_passwd_or_none() {
        // 无 mihomo 账号的机器返回 None；有则必须是合法非 root uid。
        match system_service_uid() {
            Some(uid) => assert_ne!(uid, 0),
            None => assert!(lookup_user("mihomo").is_err()),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn detect_config_assets_repair_converges_legacy_service_runtime_file() {
        let Some(service_uid) = system_service_uid() else {
            return;
        };
        if unsafe { libc::geteuid() } != 0 {
            return;
        }
        let (_, service_gid, _) = lookup_user("mihomo").unwrap();
        let chown_to_service = |p: &std::path::Path| {
            use std::ffi::CString;
            let c = CString::new(p.to_string_lossy().as_ref()).unwrap();
            assert_eq!(
                unsafe { libc::chown(c.as_ptr(), service_uid, service_gid) },
                0,
                "chown to service user failed for {}",
                p.display()
            );
        };

        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("cfg");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.yaml");
        std::fs::write(&config_path, "mixed-port: 7890\n").unwrap();

        let legacy = config_dir.join("cache.db");
        std::fs::write(&legacy, "sqlite-stale").unwrap();
        chown_to_service(&legacy);

        let intruder = config_dir.join("authorized_keys");
        std::fs::write(&intruder, "ssh").unwrap();
        chown_to_service(&intruder);

        // 名单内的服务归属遗留文件被收敛删除，生命周期预检放行；
        // 名单外的服务归属文件仍维持硬失败。
        let err = detect_config_assets_repair(&config_dir, &config_path, 0, true)
            .expect_err("non-allowlisted service-owned file must still fail closed");
        assert!(err
            .to_string()
            .contains("owned by another user and cannot be repaired automatically"));
        assert!(legacy.exists(), "failed scan must not delete anything");

        std::fs::remove_file(&intruder).unwrap();
        let repair = detect_config_assets_repair(&config_dir, &config_path, 0, true).unwrap();
        assert_eq!(repair, ConfigOwnershipRepair::NotNeeded);
        assert!(!legacy.exists(), "legacy cache.db must be converged away");
    }

    #[cfg(unix)]
    #[test]
    fn test_ensure_system_config_ownership_end_to_end_heals_legacy_0600() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let mut inputs = instance::PathInputs::from_current_env();
        inputs.home = temp.path().join("home");
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );

        let config_dir = &ctx.paths.config_dir;
        let sub_dir = config_dir.join("subscriptions");
        std::fs::create_dir_all(&sub_dir).unwrap();

        // 手动构造真实的 Bug 初始现场（遗留坏状态）：
        // 创建 subscriptions 目录，赋权限 0o755（无 setgid）
        let _ = utils::set_directory_mode_no_follow(&sub_dir, 0o755);

        // 写入 subscriptions/active，赋权限 0o600（无 group 读权限）
        let active_file = sub_dir.join("active");
        std::fs::write(&active_file, "sub-legacy-0600").unwrap();
        let _ = utils::set_file_mode_no_follow(&active_file, 0o600);

        // 写入 config.yaml
        let config_file = config_dir.join("config.yaml");
        std::fs::write(&config_file, "mixed-port: 7890\n").unwrap();

        // 调用顶层生命周期钩子
        let result = ensure_system_config_ownership_for_lifecycle(&ctx);
        assert!(result.is_ok());

        // 验证 subscriptions/ 成功自愈补上了 setgid（系统支持时）
        let sub_meta = std::fs::metadata(&sub_dir).unwrap();
        assert_eq!(sub_meta.mode() & 0o777, 0o755);
        if utils::mode_has_setgid(0o2000) {
            assert!(utils::mode_has_setgid(sub_meta.mode()));
        }

        // 验证 active 文件成功自愈为 0o640
        let active_meta = std::fs::metadata(&active_file).unwrap();
        assert_eq!(active_meta.mode() & 0o777, 0o640);
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
            // 运行时诊断不覆盖字段断言：Running 时不套用四态 overlay
            service::ServiceRuntimeState::Running,
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
            // 运行时诊断不覆盖字段断言：Running 时不套用四态 overlay
            service::ServiceRuntimeState::Running,
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
            "127.0.0.1",
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
                assert_eq!(target, "127.0.0.1");
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

    #[test]
    fn test_maybe_prepare_cli_skew_generation_flow() {
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
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-content-v1").unwrap();

        let current_cli = std::env::current_exe().unwrap();
        let current_cli_bytes = std::fs::read(&current_cli).unwrap();

        // 1. 当没有 skew 时（cli_binary 内容与 current_cli 一致），不生成 pending generation
        std::fs::write(&ctx.paths.cli_binary, &current_cli_bytes).unwrap();
        assert!(!is_system_cli_skewed(&ctx));
        let prepared = maybe_prepare_cli_skew_generation(&ctx).unwrap();
        assert!(!prepared);
        let store = system_generation_store(&ctx);
        let state = store.read_state().unwrap();
        assert_eq!(state.pending, None);

        // 2. 当存在 skew 且已有 pending generation 时，不覆盖已有的 pending generation
        std::fs::write(&ctx.paths.cli_binary, b"old-cli-different-from-current").unwrap();
        assert!(is_system_cli_skewed(&ctx));
        let existing_gen_id =
            prepare_system_generation(&ctx, b"existing-core", b"existing-cli", Vec::new()).unwrap();
        let state = store.read_state().unwrap();
        assert_eq!(state.pending, Some(existing_gen_id.clone()));

        let prepared = maybe_prepare_cli_skew_generation(&ctx).unwrap();
        assert!(!prepared);
        let state = store.read_state().unwrap();
        assert_eq!(state.pending, Some(existing_gen_id));

        // 3. 当存在 skew 且没有 pending 时，成功创建 pending generation 并包含当前 CLI 内容
        let mut state_to_clear = store.read_state().unwrap();
        state_to_clear.pending = None;
        store.write_state(&state_to_clear).unwrap();

        let prepared = maybe_prepare_cli_skew_generation(&ctx).unwrap();
        assert!(prepared);
        let state = store.read_state().unwrap();
        let new_pending_id = state
            .pending
            .expect("should have created pending generation");
        let manifest = store.validate_generation(&new_pending_id).unwrap();
        let gen_dir = store.generation_dir(&new_pending_id);
        let pending_cli_file = gen_dir.join(&manifest.daemon.relative_path);
        assert_eq!(std::fs::read(&pending_cli_file).unwrap(), current_cli_bytes);
        let pending_core_file = gen_dir.join(&manifest.core.relative_path);
        assert_eq!(
            std::fs::read(&pending_core_file).unwrap(),
            b"core-content-v1"
        );
    }
}

// ── Test-first scaffolding: R1 lock-with-await static scan + R2 fail-open /
// change-classification skeletons. Test-only additions; no business logic. ──

#[cfg(test)]
mod r1_r2_testfirst_tests {
    use super::*;

    /// Lock binding introduced on an acquire line (`let _lock = ...`,
    /// `let _config_lock = ...`). `None` when the acquire is not a `let`
    /// binding (e.g. `Some(ConfigLock::acquire(..)?)`), in which case the
    /// conservative lock-name fallback is used for `drop` matching.
    fn lock_binding_name(acquire_line: &str) -> Option<&str> {
        let rest = acquire_line.get(acquire_line.find("let ")? + 4..)?;
        let name = rest.get(..rest.find('=')?)?.trim();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }

    /// True when `line` drops the config lock identified by `lock_name`
    /// (or, when the name is unknown, any conservatively lock-shaped
    /// binding: `_lock` / `lock` / `config_lock` / `_config_lock`).
    fn is_drop_of_lock(line: &str, lock_name: Option<&str>) -> bool {
        let Some(pos) = line.find("drop(") else {
            return false;
        };
        let arg = &line[pos + "drop(".len()..];
        match lock_name {
            Some(name) => arg.contains(name),
            None => ["_lock", "config_lock", "_config_lock", "lock"]
                .iter()
                .any(|candidate| arg.contains(candidate)),
        }
    }

    /// Heuristic scan over `src/main.rs` source text (R1.1, drop-aware):
    /// every line mentioning the config-lock acquire call is an acquire
    /// site. From that line the scanner walks forward to the function
    /// boundary (`fn `/`async fn `/`pub fn ` at line start) with **no
    /// 200-line cap**, so long functions (e.g. `cmd_group`) keep their
    /// reload awaits visible. While the lock is still held (it starts held
    /// right after acquire and stops being held at a matching `drop(..)`),
    /// any `.await` flags the site as a suspected hold-lock-across-await
    /// (R1 suspect); awaits after `drop` are not flagged. Scanning stops at
    /// the `mod r1_r2_testfirst_tests` marker so this test module can never
    /// self-match its own synthetic fixtures.
    fn scan_lock_await(source: &str) -> (Vec<usize>, Vec<usize>) {
        let lines: Vec<&str> = source.lines().collect();
        let scan_end = lines
            .iter()
            .position(|line| line.contains("mod r1_r2_testfirst_tests"))
            .unwrap_or(lines.len());
        let acquire = concat!("ConfigLock", "::", "acquire");
        let mut acquires = Vec::new();
        let mut suspects = Vec::new();
        for (idx, line) in lines[..scan_end].iter().enumerate() {
            if !line.contains(acquire) {
                continue;
            }
            let acquire_line = idx + 1;
            acquires.push(acquire_line);
            let lock_name = lock_binding_name(line);
            let window = &lines[(idx + 1).min(scan_end)..scan_end];
            let stop = window
                .iter()
                .position(|candidate| {
                    candidate.starts_with("fn ")
                        || candidate.starts_with("async fn ")
                        || candidate.starts_with("pub fn ")
                })
                .unwrap_or(window.len());
            let mut held = true;
            for window_line in &window[..stop] {
                if is_drop_of_lock(window_line, lock_name) {
                    held = false;
                } else if held && window_line.contains(".await") {
                    suspects.push(acquire_line);
                    break;
                }
            }
        }
        (acquires, suspects)
    }

    #[test]
    fn r1_lock_await_scanner_flags_synthetic_cases() {
        let acquire = concat!("ConfigLock", "::", "acquire");
        let flagged = format!("let _lock = {acquire}(dir)?;\nstep().await;\n");
        let clean = format!("let _lock = {acquire}(dir)?;\nstep();\n");
        // acquire → await → drop: the await happens while held → flag.
        let await_then_drop =
            format!("let _lock = {acquire}(dir)?;\nstep().await;\ndrop(_lock);\n");
        // acquire → drop → await: lock already released → must NOT flag.
        let drop_then_await =
            format!("let _config_lock = {acquire}(dir)?;\ndrop(_config_lock);\nstep().await;\n");
        // Long function (>200 lines, no `fn ` boundary): the await sits far
        // past the old 200-line hard window and must still be seen.
        let mut long_fn = format!("let _lock = {acquire}(dir)?;\n");
        for _ in 0..250 {
            long_fn.push_str("step();\n");
        }
        long_fn.push_str("reload().await;\n");

        let (acq_flagged, sus_flagged) = scan_lock_await(&flagged);
        assert_eq!(acq_flagged, vec![1]);
        assert_eq!(
            sus_flagged,
            vec![1],
            "lock followed by await inside window must be flagged"
        );

        let (acq_clean, sus_clean) = scan_lock_await(&clean);
        assert_eq!(acq_clean, vec![1]);
        assert!(
            sus_clean.is_empty(),
            "lock without await inside window must not be flagged"
        );

        let (_, sus_await_drop) = scan_lock_await(&await_then_drop);
        assert_eq!(
            sus_await_drop,
            vec![1],
            "await before drop must still be flagged"
        );

        let (_, sus_drop_await) = scan_lock_await(&drop_then_await);
        assert!(
            sus_drop_await.is_empty(),
            "await after drop(_config_lock) must not be flagged, got {sus_drop_await:?}"
        );

        let (acq_long, sus_long) = scan_lock_await(&long_fn);
        assert_eq!(acq_long, vec![1]);
        assert_eq!(
            sus_long,
            vec![1],
            "await past the old 200-line window inside one fn must be flagged"
        );
    }

    #[test]
    fn r1_lock_await_static_scan_finds_known_baseline() {
        let (acquires, suspects) = scan_lock_await(include_str!("main.rs"));
        println!(
            "R1 baseline: {} acquire sites / {} suspects: {suspects:?}",
            acquires.len(),
            suspects.len()
        );
        println!("R1 acquires (line list): {acquires:?}");
        println!("R1 suspects  (line list, to unlock site-by-site): {suspects:?}");

        // R1.1 complete: drop-aware scanner + function-boundary window.
        // 27 acquire sites (22 original + 4 re-acquire around network awaits
        // in cmd_config add/refresh/refresh-all/legacy-url + 1 short lock in
        // update_group_overlay); 0 suspects — no ConfigLock held across await.
        assert_eq!(
            acquires.len(),
            27,
            "R1.1 acquire snapshot; update only when intentionally changing lock sites"
        );
        assert_eq!(
            suspects.len(),
            0,
            "R1 DoD: no lock held across await; leftover: {suspects:?}"
        );
        assert!(suspects.len() <= acquires.len());
    }

    struct ReloadFailingClient;

    impl MihomoApiClient for ReloadFailingClient {
        async fn get(&self, _path: &str) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("not used in this test")
        }

        async fn put(
            &self,
            _path: &str,
            _body: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("reload endpoint unavailable (simulated)")
        }

        async fn patch(
            &self,
            _path: &str,
            _body: serde_json::Value,
        ) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("not used in this test")
        }

        async fn delete(&self, _path: &str) -> anyhow::Result<serde_json::Value> {
            anyhow::bail!("not used in this test")
        }
    }

    /// Status-quo lock for the R2 fail-open work: a failed hot-reload PUT
    /// surfaces as `Err` from the API layer — there is no fallback at this
    /// boundary today, so callers own the fail-open/promotion decision (R2).
    #[tokio::test]
    async fn reload_configs_with_client_propagates_put_error() {
        let client = ReloadFailingClient;
        let err = mihomo_api::reload_configs_with_client(&client, "/tmp/mihomo/config.yaml")
            .await
            .expect_err("reload PUT failure must propagate as Err, never a silent Ok");
        assert!(err
            .to_string()
            .contains("reload endpoint unavailable (simulated)"));
    }

    #[test]
    fn r2_change_kind_classifies_effective_config_to_hot_reload() {
        let before = r#"
mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
external-controller-unix: /var/run/mihomo/mihomo.sock
proxies: []
proxy-groups: []
rules:
  - MATCH,DIRECT
"#;
        let after = r#"
mixed-port: 7891
allow-lan: true
mode: global
log-level: warning
external-controller-unix: /var/run/mihomo/mihomo.sock
proxies: []
proxy-groups: []
rules:
  - DOMAIN-SUFFIX,google.com,PROXY
  - MATCH,DIRECT
"#;
        let kind = config::ChangeKind::classify_diff(before, after);
        assert_eq!(kind, config::ChangeKind::HotReload);
    }

    #[test]
    fn r2_change_kind_classifies_tun_and_controller_to_promote() {
        let base = r#"
mixed-port: 7890
external-controller: 127.0.0.1:9090
external-controller-unix: /var/run/mihomo/mihomo.sock
secret: "old-secret"
tun:
  enable: false
"#;

        // Controller endpoint changed -> Promote
        let controller_changed = r#"
mixed-port: 7890
external-controller: 127.0.0.1:9091
external-controller-unix: /var/run/mihomo/mihomo.sock
secret: "old-secret"
tun:
  enable: false
"#;
        assert_eq!(
            config::ChangeKind::classify_diff(base, controller_changed),
            config::ChangeKind::Promote
        );

        // Controller unix socket changed -> Promote
        let unix_changed = r#"
mixed-port: 7890
external-controller: 127.0.0.1:9090
external-controller-unix: /var/run/mihomo/other.sock
secret: "old-secret"
tun:
  enable: false
"#;
        assert_eq!(
            config::ChangeKind::classify_diff(base, unix_changed),
            config::ChangeKind::Promote
        );

        // Secret changed -> Promote
        let secret_changed = r#"
mixed-port: 7890
external-controller: 127.0.0.1:9090
external-controller-unix: /var/run/mihomo/mihomo.sock
secret: "new-secret"
tun:
  enable: false
"#;
        assert_eq!(
            config::ChangeKind::classify_diff(base, secret_changed),
            config::ChangeKind::Promote
        );

        // TUN enable changed -> Promote
        let tun_changed = r#"
mixed-port: 7890
external-controller: 127.0.0.1:9090
external-controller-unix: /var/run/mihomo/mihomo.sock
secret: "old-secret"
tun:
  enable: true
"#;
        assert_eq!(
            config::ChangeKind::classify_diff(base, tun_changed),
            config::ChangeKind::Promote
        );
    }

    #[tokio::test]
    async fn r2_hot_reload_error_fails_open_to_promotion() {
        let client = ReloadFailingClient;
        let promoted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let promoted_clone = std::sync::Arc::clone(&promoted);
        let outcome = config::apply_with_fail_open(
            config::ChangeKind::HotReload,
            || async {
                mihomo_api::reload_configs_with_client(&client, "/tmp/mihomo/config.yaml").await
            },
            move || {
                let p = promoted_clone;
                async move {
                    p.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .await
        .expect("promotion fallback must succeed");

        assert!(
            promoted.load(std::sync::atomic::Ordering::SeqCst),
            "must fall back to promotion on reload error"
        );
        assert_eq!(outcome.applied_via, config::AppliedVia::Promotion);

        // Verify successful hot-reload does not trigger promotion fallback
        let outcome_ok = config::apply_with_fail_open(
            config::ChangeKind::HotReload,
            || async { Ok(()) },
            || async { panic!("must not promote when reload succeeds") },
        )
        .await
        .expect("hot reload success must succeed");
        assert_eq!(outcome_ok.applied_via, config::AppliedVia::HotReload);
    }

    // ── R1.2 autostart: static scan for direct platform primitive calls
    // inside set_autostart / query_autostart / run_autostart_command ──────
    // (run_autostart_command was deleted in R1.2; the scanner still looks
    // for that name in case a future change reintroduces it.)

    /// A line that can terminate an autostart fn body: any top-level fn
    /// signature (incl. `async`/`pub`/`pub(crate)`) or a `#[cfg` attribute
    /// (cfg attributes conventionally sit on the line above the next fn).
    fn is_source_boundary(line: &str) -> bool {
        line.starts_with("fn ")
            || line.starts_with("async fn ")
            || line.starts_with("pub fn ")
            || line.starts_with("pub async fn ")
            || line.starts_with("pub(crate) fn ")
            || line.starts_with("#[cfg")
    }

    /// Line ranges (1-based, `[start, end)`) of the autostart fn family in
    /// `source`. Scanning stops at the `mod r1_r2_testfirst_tests` marker so
    /// this test module can never self-match its own synthetic fixtures.
    /// `kind` is `"set"` / `"query"` / `"run"`.
    fn autostart_fn_intervals(source: &str) -> Vec<(&'static str, usize, usize)> {
        let lines: Vec<&str> = source.lines().collect();
        let scan_end = lines
            .iter()
            .position(|line| line.contains("mod r1_r2_testfirst_tests"))
            .unwrap_or(lines.len());
        let mut intervals = Vec::new();
        for (idx, line) in lines[..scan_end].iter().enumerate() {
            let kind = if line.contains("fn set_autostart") {
                "set"
            } else if line.contains("fn query_autostart") {
                "query"
            } else if line.contains("fn run_autostart_command") {
                "run"
            } else {
                continue;
            };
            let start = idx + 1;
            let end = lines[..scan_end]
                .iter()
                .skip(idx + 1)
                .position(|candidate| is_source_boundary(candidate))
                .map(|offset| idx + offset + 2)
                .unwrap_or(scan_end);
            intervals.push((kind, start, end));
        }
        intervals
    }

    /// Heuristic scan over `source`: collect `Command::new("<program>")`
    /// invocations for the known platform primitives (systemctl, launchctl,
    /// sc.exe, reg.exe) that sit inside an autostart fn interval; return
    /// `(line_no, program)` in line order. The `Command` + `::new` needle is
    /// assembled with `concat!` so this test source never self-matches, and
    /// scanning additionally stops at the test-module marker.
    fn scan_autostart_direct_platform_calls(source: &str) -> Vec<(usize, String)> {
        let lines: Vec<&str> = source.lines().collect();
        let scan_end = lines
            .iter()
            .position(|line| line.contains("mod r1_r2_testfirst_tests"))
            .unwrap_or(lines.len());
        let intervals = autostart_fn_intervals(source);
        let cmd_new = concat!("Command", "::", "new", "(\"");
        let needles: Vec<(String, &'static str)> = ["systemctl", "launchctl", "sc.exe", "reg.exe"]
            .iter()
            .map(|program| (format!("{cmd_new}{program}\")"), *program))
            .collect();
        let mut hits = Vec::new();
        for (idx, line) in lines[..scan_end].iter().enumerate() {
            let line_no = idx + 1;
            let in_autostart = intervals
                .iter()
                .any(|(_, start, end)| line_no >= *start && line_no < *end);
            if !in_autostart {
                continue;
            }
            for (needle, program) in &needles {
                if line.contains(needle.as_str()) {
                    hits.push((line_no, (*program).to_string()));
                    break;
                }
            }
        }
        hits
    }

    #[test]
    fn r1_autostart_direct_call_scanner_flags_synthetic_cases() {
        let cmd_new = concat!("Command", "::", "new", "(\"");

        let flagged =
            format!("async fn set_autostart() {{\n    let _ = {cmd_new}systemctl\");\n}}\n");
        let hits = scan_autostart_direct_platform_calls(&flagged);
        assert_eq!(
            hits.len(),
            1,
            "direct call inside set_autostart must be flagged: {hits:?}"
        );
        assert_eq!(hits[0].0, 2, "hit must point at the call line");
        assert_eq!(hits[0].1, "systemctl");

        let clean = "async fn set_autostart() {\n    init_marker();\n}\n";
        let clean_hits = scan_autostart_direct_platform_calls(clean);
        assert!(
            clean_hits.is_empty(),
            "fn without a direct call must not be flagged: {clean_hits:?}"
        );

        let query = format!("fn query_autostart() {{\n    let _ = {cmd_new}launchctl\");\n}}\n");
        let query_hits = scan_autostart_direct_platform_calls(&query);
        assert_eq!(
            query_hits.len(),
            1,
            "query_autostart must be scanned too: {query_hits:?}"
        );
        assert_eq!(query_hits[0].1, "launchctl");

        let outsider =
            format!("async fn install_service() {{\n    let _ = {cmd_new}systemctl\");\n}}\n");
        let outsider_hits = scan_autostart_direct_platform_calls(&outsider);
        assert!(
            outsider_hits.is_empty(),
            "calls outside the autostart fns must not be flagged: {outsider_hits:?}"
        );
    }

    #[test]
    fn r1_autostart_direct_calls_baseline_present() {
        let source = include_str!("main.rs");
        let hits = scan_autostart_direct_platform_calls(source);
        let intervals = autostart_fn_intervals(source);
        let mut set_hits: Vec<&(usize, String)> = Vec::new();
        let mut query_hits: Vec<&(usize, String)> = Vec::new();
        let mut run_hits: Vec<&(usize, String)> = Vec::new();
        for hit in &hits {
            for (kind, start, end) in &intervals {
                if hit.0 >= *start && hit.0 < *end {
                    match *kind {
                        "set" => set_hits.push(hit),
                        "query" => query_hits.push(hit),
                        "run" => run_hits.push(hit),
                        _ => {}
                    }
                }
            }
        }
        println!(
            "R1.2 autostart baseline: {} total = {} set / {} query / {} run: {hits:?}",
            hits.len(),
            set_hits.len(),
            query_hits.len(),
            run_hits.len()
        );

        // Phase after R1.2 (this gate): set_autostart executes the planned
        // ServiceAction::{Enable,Disable} via service::run_instance_command
        // and must contain ZERO direct platform primitive calls. The former
        // run_autostart_command helper was deleted (Linux user now goes
        // through the plan). query_autostart still has direct calls — that
        // is intentionally out of scope this round (keep its hits visible).
        // Original pre-R1.2 baseline was 11 hits (set=7, query=4, run=0);
        // remaining floor is query-only and must stay >= 3 so the scanner
        // itself cannot silently go blind.
        assert!(
            set_hits.is_empty(),
            "R1.2 DoD: set_autostart has no direct platform calls; got {set_hits:?}"
        );
        assert!(
            hits.len() >= 3,
            "scanner must still see the known query_autostart direct calls, got {}",
            hits.len()
        );
        assert!(
            hits.len() == query_hits.len(),
            "after R1.2 every remaining hit must come from query_autostart (set and run are zero); got {hits:?}"
        );
        assert!(set_hits.len() + query_hits.len() + run_hits.len() == hits.len());
    }

    #[test]
    fn r1_autostart_plan_enable_disable_variants_match_direct_primitives() {
        use instance::{InstanceContext, InstanceMode, PathInputs, ServiceAction, TargetOs};

        let inputs = PathInputs::for_tests();

        // ── Linux user ──────────────────────────────────────────────────
        // Direct primitive (pre-R1.2, run_autostart_command):
        //   systemctl --user enable|mihomo  /  systemctl --user disable|mihomo
        //   (privileged=false; failure text `systemctl {enable|disable} mihomo failed`)
        let linux_user = InstanceContext::planned(TargetOs::Linux, InstanceMode::User, &inputs);
        let linux_enable = instance::planned_service_plan(&linux_user, ServiceAction::Enable);
        assert_eq!(
            linux_enable.commands.len(),
            1,
            "Linux user Enable must be one systemctl command"
        );
        assert_eq!(linux_enable.commands[0].program, "systemctl");
        assert_eq!(
            linux_enable.commands[0].args,
            vec!["--user", "enable", "mihomo"],
            "Linux user Enable args must match direct primitive systemctl --user enable mihomo"
        );
        assert!(
            !linux_enable.commands[0].privileged,
            "Linux user Enable must not be privileged"
        );
        let linux_disable = instance::planned_service_plan(&linux_user, ServiceAction::Disable);
        assert_eq!(
            linux_disable.commands[0].args,
            vec!["--user", "disable", "mihomo"],
            "Linux user Disable args must match direct primitive systemctl --user disable mihomo"
        );
        assert!(!linux_disable.commands[0].privileged);

        // ── Linux system ────────────────────────────────────────────────
        // ADR-19: system autostart is the daemon SetAutostart IPC marker —
        // there is no systemctl primitive, and set_autostart returns via IPC
        // before consulting the plan. Plan must therefore be empty.
        let linux_system = InstanceContext::planned(TargetOs::Linux, InstanceMode::System, &inputs);
        assert!(
            instance::planned_service_plan(&linux_system, ServiceAction::Enable)
                .commands
                .is_empty(),
            "Linux system Enable has no planned primitive (IPC-owned)"
        );
        assert!(
            instance::planned_service_plan(&linux_system, ServiceAction::Disable)
                .commands
                .is_empty(),
            "Linux system Disable has no planned primitive (IPC-owned)"
        );

        // ── macOS system & user ─────────────────────────────────────────
        // Direct primitive (pre-R1.2):
        //   launchctl enable|disable system/io.mihomo      (system, privileged)
        //   launchctl enable|disable gui/<uid>/io.mihomo    (user, unprivileged)
        let mac_system = InstanceContext::planned(TargetOs::Macos, InstanceMode::System, &inputs);
        let mac_enable_sys = instance::planned_service_plan(&mac_system, ServiceAction::Enable);
        assert_eq!(mac_enable_sys.commands.len(), 1);
        assert_eq!(mac_enable_sys.commands[0].program, "launchctl");
        assert_eq!(
            mac_enable_sys.commands[0].args,
            vec!["enable", "system/io.mihomo"],
            "macOS system Enable must match launchctl enable system/io.mihomo"
        );
        assert!(
            mac_enable_sys.commands[0].privileged,
            "macOS system Enable must be privileged"
        );
        let mac_disable_sys = instance::planned_service_plan(&mac_system, ServiceAction::Disable);
        assert_eq!(
            mac_disable_sys.commands[0].args,
            vec!["disable", "system/io.mihomo"],
            "macOS system Disable must match launchctl disable system/io.mihomo"
        );

        let mac_user = InstanceContext::planned(TargetOs::Macos, InstanceMode::User, &inputs);
        let mac_enable_user = instance::planned_service_plan(&mac_user, ServiceAction::Enable);
        assert_eq!(
            mac_enable_user.commands[0].args,
            vec!["enable", "gui/501/io.mihomo"],
            "macOS user Enable must match launchctl enable gui/<uid>/io.mihomo (uid from inputs)"
        );
        assert!(
            !mac_enable_user.commands[0].privileged,
            "macOS user Enable must not be privileged"
        );
        let mac_disable_user = instance::planned_service_plan(&mac_user, ServiceAction::Disable);
        assert_eq!(
            mac_disable_user.commands[0].args,
            vec!["disable", "gui/501/io.mihomo"],
            "macOS user Disable must match launchctl disable gui/<uid>/io.mihomo"
        );

        // ── Windows system ──────────────────────────────────────────────
        // Direct primitive (pre-R1.2):
        //   sc.exe config mihomo start= auto    (enable, elevated/privileged)
        //   sc.exe config mihomo start= demand   (disable, elevated/privileged)
        let win_system = InstanceContext::planned(TargetOs::Windows, InstanceMode::System, &inputs);
        let win_enable_sys = instance::planned_service_plan(&win_system, ServiceAction::Enable);
        assert_eq!(win_enable_sys.commands.len(), 1);
        assert_eq!(win_enable_sys.commands[0].program, "sc.exe");
        assert_eq!(
            win_enable_sys.commands[0].args,
            vec!["config", "mihomo", "start=", "auto"],
            "Windows system Enable must match sc.exe config mihomo start= auto"
        );
        assert!(
            win_enable_sys.commands[0].privileged,
            "Windows system Enable must be privileged"
        );
        let win_disable_sys = instance::planned_service_plan(&win_system, ServiceAction::Disable);
        assert_eq!(
            win_disable_sys.commands[0].args,
            vec!["config", "mihomo", "start=", "demand"],
            "Windows system Disable must match sc.exe config mihomo start= demand"
        );
        assert!(win_disable_sys.commands[0].privileged);

        // ── Windows user ────────────────────────────────────────────────
        // Direct primitive (pre-R1.2):
        //   reg.exe ADD    HKCU\...\Run /v mihomo-cli /t REG_SZ
        //                  /d "wscript.exe //B //NoLogo \"<vbs>\"" /f
        //   reg.exe DELETE HKCU\...\Run /v mihomo-cli /f
        // (.vbs write stays as std::fs in set_autostart; unprivileged)
        let win_user = InstanceContext::planned(TargetOs::Windows, InstanceMode::User, &inputs);
        let vbs_path = win_user.paths.config_dir.join("autostart.vbs");
        let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let win_enable_user = instance::planned_service_plan(&win_user, ServiceAction::Enable);
        assert_eq!(win_enable_user.commands.len(), 1);
        assert_eq!(win_enable_user.commands[0].program, "reg.exe");
        let expected_add = vec![
            "ADD".to_string(),
            run_key.to_string(),
            "/v".to_string(),
            "mihomo-cli".to_string(),
            "/t".to_string(),
            "REG_SZ".to_string(),
            "/d".to_string(),
            format!("wscript.exe //B //NoLogo \"{}\"", vbs_path.display()),
            "/f".to_string(),
        ];
        assert_eq!(
            win_enable_user.commands[0].args, expected_add,
            "Windows user Enable must match reg.exe ADD Run key pointing at autostart.vbs"
        );
        assert!(
            !win_enable_user.commands[0].privileged,
            "Windows user Enable must not be privileged"
        );
        let win_disable_user = instance::planned_service_plan(&win_user, ServiceAction::Disable);
        assert_eq!(
            win_disable_user.commands[0].args,
            vec![
                "DELETE".to_string(),
                run_key.to_string(),
                "/v".to_string(),
                "mihomo-cli".to_string(),
                "/f".to_string(),
            ],
            "Windows user Disable must match reg.exe DELETE Run key"
        );
        assert!(!win_disable_user.commands[0].privileged);
    }
}

#[cfg(test)]
mod r3_2_selection_mirror_tests {
    use super::*;

    fn expected_map(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn classify_separates_ok_drift_and_missing() {
        let expected = expected_map(&[
            ("sub-aaaaaaaa", "ra"),
            ("sub-bbbbbbbb", "rb"),
            ("sub-cccccccc", "rc"),
        ]);
        let mut actual: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        actual.insert("sub-aaaaaaaa".to_string(), Some("ra".to_string()));
        actual.insert("sub-bbbbbbbb".to_string(), Some("changed".to_string()));
        actual.insert("sub-cccccccc".to_string(), None);
        actual.insert("sub-dddddddd".to_string(), Some("extra".to_string()));

        let drift = classify_selection_mirror_drift(&expected, &actual);
        assert_eq!(drift.get("sub-aaaaaaaa"), Some(&SelectionMirrorDrift::Ok));
        assert_eq!(
            drift.get("sub-bbbbbbbb"),
            Some(&SelectionMirrorDrift::Drifted)
        );
        assert_eq!(
            drift.get("sub-cccccccc"),
            Some(&SelectionMirrorDrift::Missing)
        );
        assert!(!drift.contains_key("sub-dddddddd"));
    }

    #[test]
    fn classify_reports_missing_when_daemon_never_saw_the_id() {
        let expected = expected_map(&[("sub-abcdef12", "r1")]);
        let drift = classify_selection_mirror_drift(&expected, &std::collections::BTreeMap::new());
        assert_eq!(
            drift.get("sub-abcdef12"),
            Some(&SelectionMirrorDrift::Missing)
        );
    }

    #[test]
    fn user_tree_revisions_cover_pushed_bytes_and_skip_invalid_ids() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = utils::AppPaths::new(tmp.path().to_path_buf());
        std::fs::create_dir_all(paths.selections_dir()).unwrap();

        let good = "selections:\n  G: N\n";
        std::fs::write(
            paths.selection_state_path_for_subscription("sub-abcdef12"),
            good,
        )
        .unwrap();
        // Outside the mirror contract: invalid id names and non-yaml entries.
        std::fs::write(
            paths.selections_dir().join("legacy-name.yaml"),
            "selections: {}\n",
        )
        .unwrap();
        std::fs::write(paths.selections_dir().join("sub-abcdef12.txt"), "x").unwrap();

        let revisions = user_tree_selection_revisions(&paths);
        assert_eq!(revisions.len(), 1);
        assert_eq!(
            revisions.get("sub-abcdef12").map(String::as_str),
            Some(tun_transaction::content_revision(good.as_bytes()).as_str()),
            "expected revision must cover the exact bytes a push transmits"
        );
    }
}

// ── R3.2 writer gate: runtime-path writes restricted to whitelisted modules ──

#[cfg(test)]
mod r3_runtime_writer_gate_tests {

    /// Modules allowed to write daemon-side runtime state directly. Every
    /// other module must go through the daemon IPC write path
    /// (RecordSelectionIntent et al.), so the daemon stays the only runtime
    /// writer (SPEC §3.8.1 mirror contract, R3.2).
    const WHITELIST: [&str; 3] = ["daemon.rs", "tun_transaction.rs", "service.rs"];

    /// Same-line (or within-line-window) filesystem write calls.
    const WRITE_TOKENS: [&str; 6] = [
        "fs::write(",
        "fs::write_bytes(",
        "atomic_write",
        "File::create(",
        "remove_file(",
        "remove_dir_all(",
    ];

    /// Identifiers that name daemon-side runtime state locations.
    const RUNTIME_TOKENS: [&str; 5] = [
        "/var/lib/mihomo-cli",
        "tun_config_file",
        "daemon_config_dir",
        "selection_mirror",
        "daemon_credential",
    ];

    /// Even a bare mention of these daemon-private writer helpers outside the
    /// whitelist signals an intent to bypass the IPC write path.
    const DIRECT_WRITER_TOKENS: [&str; 3] = [
        "record_selection_mirror(",
        "record_active_subscription_mirror(",
        "write_selection_mirror_file(",
    ];

    /// Remove `#[cfg(test)]`-gated items (attribute + following braced item)
    /// so test fixtures — which legitimately fake runtime paths under temp
    /// dirs — cannot self-trip the gate. Brace counting relies on top-level
    /// items closing at column 0, which cargo fmt guarantees.
    fn strip_test_regions(source: &str) -> Vec<(usize, String)> {
        let lines: Vec<&str> = source.lines().collect();
        let mut kept = Vec::new();
        let mut idx = 0usize;
        while idx < lines.len() {
            let line = lines[idx];
            if line.trim_start().starts_with("#[cfg(test)]") {
                // Walk past the attribute and the item's full brace block.
                idx += 1;
                while idx < lines.len() && !lines[idx].contains('{') {
                    idx += 1;
                }
                let mut depth = 0usize;
                while idx < lines.len() {
                    depth += lines[idx].matches('{').count();
                    depth = depth.saturating_sub(lines[idx].matches('}').count());
                    idx += 1;
                    if depth == 0 {
                        break;
                    }
                }
                continue;
            }
            kept.push((idx + 1, line.to_string()));
            idx += 1;
        }
        kept
    }

    /// Scan one module's non-test code. `lookahead` extra lines travel with a
    /// write-call line so multi-line `fs::write(` argument lists still see
    /// their runtime-path argument on the following lines.
    fn scan_runtime_writes(kept: &[(usize, String)], file: &str) -> Vec<String> {
        let mut violations = Vec::new();
        for pos in 0..kept.len() {
            let (line_no, line) = &kept[pos];
            if DIRECT_WRITER_TOKENS.iter().any(|t| line.contains(t)) {
                violations.push(format!("{file}:{line_no}: daemon-private writer: {line}"));
                continue;
            }
            if !WRITE_TOKENS.iter().any(|t| line.contains(t)) {
                continue;
            }
            let window_end = (pos + 3).min(kept.len());
            let window: String = kept[pos..window_end]
                .iter()
                .map(|(_, l)| l.as_str())
                .collect();
            if RUNTIME_TOKENS.iter().any(|t| window.contains(t)) {
                violations.push(format!("{file}:{line_no}: runtime write: {line}"));
            }
        }
        violations
    }

    #[test]
    fn r3_runtime_writer_gate_scanner_flags_synthetic_cases() {
        let flagged = "std::fs::write(&ctx.paths.tun_config_file, bytes).unwrap();\n";
        assert_eq!(
            scan_runtime_writes(&strip_test_regions(flagged), "x.rs").len(),
            1
        );

        // Multi-line call: the runtime token sits on an argument line.
        let multiline =
            "std::fs::write(\n    &daemon_config_dir().join(\"active\"),\n    id,\n)?;\n";
        assert_eq!(
            scan_runtime_writes(&strip_test_regions(multiline), "x.rs").len(),
            1
        );

        // A #[cfg(test)]-gated item is stripped before scanning.
        let test_gated =
            "fn ok() {}\n#[cfg(test)]\nmod fake {\n    fn t() {\n        std::fs::write(&ctx.paths.tun_config_file, b\"x\").unwrap();\n    }\n}\nfn after() {}\n";
        assert_eq!(
            scan_runtime_writes(&strip_test_regions(test_gated), "x.rs").len(),
            0
        );

        // Reads and user-tree writes are not runtime writes.
        let clean = "let content = std::fs::read_to_string(&paths.config_path())?;\n";
        assert_eq!(
            scan_runtime_writes(&strip_test_regions(clean), "x.rs").len(),
            0
        );

        // Daemon-private writer mention trips even without a fs token.
        let direct = "record_selection_mirror(&subscription_id, &yaml)?;\n";
        assert_eq!(
            scan_runtime_writes(&strip_test_regions(direct), "x.rs").len(),
            1
        );
    }

    #[test]
    fn r3_runtime_writer_gate_scan() {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        let mut scanned_files = 0usize;
        let mut entries: Vec<_> = std::fs::read_dir(&src_dir)
            .expect("src directory must exist")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
            .collect();
        entries.sort();
        for path in entries {
            let file = path.file_name().unwrap().to_string_lossy().to_string();
            if WHITELIST.contains(&file.as_str()) {
                continue;
            }
            let raw = std::fs::read_to_string(&path).unwrap();
            scanned_files += 1;
            violations.extend(scan_runtime_writes(&strip_test_regions(&raw), &file));
        }
        println!("R3.2 writer gate: {scanned_files} modules scanned (whitelist: {WHITELIST:?})");
        assert!(
            scanned_files > 10,
            "writer gate must cover the CLI modules, got {scanned_files}"
        );
        assert!(
            violations.is_empty(),
            "runtime state may only be written by whitelisted daemon-side modules; found:\n{}",
            violations.join("\n")
        );
    }
}
