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

    /// review MAJOR-9 + 回归-3：`override import` 只对**控制键**告警（它们会被拒绝），
    /// 非控制键（mtu/device/route-exclude-address 等）照常生效、不得误报。
    fn temp_paths_for(tag: &str) -> utils::AppPaths {
        let dir = std::env::temp_dir().join(format!("mihomo-replay-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        utils::AppPaths::new(dir)
    }

    /// review MAJOR-5：mirror 缺失信号必须由 CLI 结合**用户树**判定：
    /// 无持久化选点 → 删除该行（不得误导）；有持久化选点 → 明确要求重建。
    #[test]
    fn translate_selection_replay_signal_uses_user_tree_not_mirror() {
        let msg = "[selection-mirror-missing] selection mirror unavailable";
        let paths = temp_paths_for("replay");

        // 1) 没有 active 订阅 → 只陈述事实
        let out = translate_selection_replay_signal(msg, None, &paths);
        assert!(
            out.contains("Selections not replayed: selection mirror unavailable"),
            "{out}"
        );
        assert!(!out.contains("run any selection command"), "{out}");

        // 2) 有 active 订阅但没有持久化选点 → 整行删除（场景 B 的契约）
        config::set_active_id_at(&paths, "sub1").unwrap();
        let out = translate_selection_replay_signal(msg, Some("sub1"), &paths);
        assert!(out.trim().is_empty(), "必须删除该行: {out:?}");

        // 3) 有持久化选点 + mirror 缺失 → 必须给出逐条重建动作（真实的 mirror 丢失，
        // A3' 补救矩阵：逐条 select 命令 + 目标存在性未验证声明）
        let sel_dir = paths.selections_dir();
        std::fs::create_dir_all(&sel_dir).unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription("sub1"),
            "selections:\n  Test: DIRECT\n",
        )
        .unwrap();
        let out = translate_selection_replay_signal(msg, Some("sub1"), &paths);
        assert!(
            out.contains("mihomo-cli select -g 'Test' --node='DIRECT'"),
            "有持久化选点时必须逐条给出重建命令: {out}"
        );
        assert!(
            out.contains("If the target still exists"),
            "镜像缺失时目标存在性未验证，必须声明: {out}"
        );
        assert!(
            !out.contains("run any selection command"),
            "笼统的重建提示已被 A3' 逐条矩阵取代: {out}"
        );

        // 3b) 持久化文件存在但解析不出选点 → 视同无意图，整行删除
        std::fs::write(
            paths.selection_state_path_for_subscription("sub1"),
            "not: valid: selection: state\n",
        )
        .unwrap();
        let out = translate_selection_replay_signal(msg, Some("sub1"), &paths);
        assert!(out.trim().is_empty(), "解析失败不得误导: {out:?}");

        // 4) 非信号消息原样返回
        let plain = "system configuration promoted and runtime applied";
        assert_eq!(
            translate_selection_replay_signal(plain, None, &paths),
            plain
        );

        let _ = std::fs::remove_dir_all(paths.config_dir());
    }

    /// review 回归-1：非 root doctor 读不到 `/var/lib` 下的运行配置时，
    /// attested 必须回退到 intent（字节一致且可读），否则探测永远退化成 ❓。
    #[cfg(unix)]
    #[test]
    fn probe_config_source_prefers_intent_when_attested() {
        let dir = std::env::temp_dir().join(format!("mihomo-probesrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let intent = dir.join("config.yaml");
        let missing = dir.join("var-lib-config.yaml"); // 不存在 → 模拟 EACCES/无权读
        std::fs::write(&intent, "dns:\n  enhanced-mode: fake-ip\n").unwrap();

        // attested + intent 可读 → 用 intent（即使运行配置读不到）
        match probe_config_source(true, Some(&missing), &intent) {
            ProbeConfigSource::Text { label, text } => {
                assert!(label.contains("intent"), "{label}");
                assert!(text.contains("fake-ip"), "{text}");
            }
            ProbeConfigSource::Unavailable(reason) => panic!("必须回退到 intent: {reason}"),
        }

        // 非 attested + 运行配置可读 → 用运行配置
        std::fs::write(&missing, "dns:\n  enhanced-mode: normal\n").unwrap();
        match probe_config_source(false, Some(&missing), &intent) {
            ProbeConfigSource::Text { label, text } => {
                assert!(label.contains("运行配置"), "{label}");
                assert!(text.contains("normal"), "{text}");
            }
            ProbeConfigSource::Unavailable(reason) => panic!("必须读到运行配置: {reason}"),
        }

        // 非 attested + 无运行配置 → 明确原因（呈现为 ❓，不静默）
        match probe_config_source(false, None, &intent) {
            ProbeConfigSource::Unavailable(reason) => {
                assert!(reason.contains("未观察到 Core 的运行配置"), "{reason}")
            }
            ProbeConfigSource::Text { .. } => panic!("没有运行配置时不得给出判据"),
        }

        // 非 attested + 运行配置不可读 → 带原因的 ❓
        let gone = dir.join("gone.yaml");
        match probe_config_source(false, Some(&gone), &intent) {
            ProbeConfigSource::Unavailable(reason) => {
                assert!(reason.contains("运行配置"), "{reason}")
            }
            ProbeConfigSource::Text { .. } => panic!("读不到就不该给出判据"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn override_tun_control_keys_are_warned_as_rejected() {
        let control: serde_yaml::Value = serde_yaml::from_str(
            "sniffer:\n  enable: true\ntun:\n  enable: false\n  stack: system\n",
        )
        .expect("yaml");
        let warn = tun_owned_override_keys_warning(&control).expect("control keys must warn");
        assert!(warn.contains("TUN 事务管辖"), "{warn}");
        assert!(warn.contains("拒绝"), "{warn}");
        assert!(warn.contains("tun.enable"), "{warn}");
        assert!(warn.contains("tun.stack"), "{warn}");
        assert!(warn.contains("非控制键不受影响"), "{warn}");

        let non_control: serde_yaml::Value = serde_yaml::from_str(
            "tun:\n  mtu: 9000\n  route-exclude-address:\n  - 169.254.0.0/16\n",
        )
        .expect("yaml");
        assert!(
            tun_owned_override_keys_warning(&non_control).is_none(),
            "非控制键会被采用，不得告警"
        );

        let mixed: serde_yaml::Value =
            serde_yaml::from_str("tun:\n  strict-route: false\n  mtu: 9000\n").expect("yaml");
        let warn = tun_owned_override_keys_warning(&mixed).expect("mixed must warn for control");
        assert!(warn.contains("tun.strict-route"), "{warn}");
        assert!(
            !warn.contains("tun.mtu"),
            "非控制键不得出现在拒绝告警里: {warn}"
        );

        let no_tun: serde_yaml::Value =
            serde_yaml::from_str("sniffer:\n  enable: true\n").expect("yaml");
        assert!(
            tun_owned_override_keys_warning(&no_tun).is_none(),
            "override 不含 tun 键时不得告警"
        );
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
    fn doctor_check_warn_formats_warning_icon() {
        assert_eq!(
            DoctorCheck::warn("云链路本地路由", "测试警告", None::<String>).format(),
            "  ⚠ 云链路本地路由: 测试警告"
        );
    }

    #[test]
    fn link_local_route_ip_route_detects_configured_169_254_cidr() {
        let output = "\
default via 172.19.0.1 dev eth0 proto kernel onlink\n\
169.254.0.0/16 dev eth0 scope link\n\
172.19.0.0/20 dev eth0 proto kernel scope link src 172.19.2.140\n";
        let diag = parse_ip_route_main_output(output, None);
        assert_eq!(
            diag,
            LinkLocalRouteDiagnosis::DegradedMainTableOnly {
                interface: "eth0".to_string(),
                suggested_interface: "eth0".to_string(),
            }
        );
        let check = format_link_local_route_check(&diag);
        assert!(check.format().contains("⚠ 云链路本地路由:"));
        assert!(check
            .format()
            .contains("降级模式下未评估策略路由（ip rule）"));
    }

    #[test]
    fn link_local_route_classify_exit_devices_matrix() {
        // 1. 两者皆为物理网卡 -> 真正直连通过
        let diag = classify_link_local_exit_devices(
            Some("eth0".to_string()),
            Some("eth0".to_string()),
            Some("Meta"),
            Some("eth0".to_string()),
            None,
        );
        assert_eq!(
            diag,
            Some(LinkLocalRouteDiagnosis::CoveredByRoute {
                interface: "eth0".to_string()
            })
        );

        // 2. 自定义 tun.device 为 clash0 时，clash0 正确被识别为 TUN 网卡
        let diag_clash = classify_link_local_exit_devices(
            Some("clash0".to_string()),
            Some("eth0".to_string()),
            Some("clash0"),
            Some("eth0".to_string()),
            None,
        );
        assert_eq!(
            diag_clash,
            Some(LinkLocalRouteDiagnosis::Partial {
                interface: "eth0".to_string(),
                host_or_subnet: "169.254.169.254".to_string(),
                suggested_interface: "eth0".to_string(),
                config_partial: None,
            })
        );

        // 3. 镜像走物理网卡，IMDS 走 TUN -> 对称 Partial（169.254.0.3 直连，IMDS 被 TUN 捕获）
        let diag_mirror_phys = classify_link_local_exit_devices(
            Some("eth0".to_string()),
            Some("Meta".to_string()),
            Some("Meta"),
            Some("eth0".to_string()),
            None,
        );
        assert_eq!(
            diag_mirror_phys,
            Some(LinkLocalRouteDiagnosis::Partial {
                interface: "eth0".to_string(),
                host_or_subnet: "169.254.0.3".to_string(),
                suggested_interface: "eth0".to_string(),
                config_partial: None,
            })
        );

        // 4. 两者皆被 TUN 接管 -> Missing
        let diag_missing = classify_link_local_exit_devices(
            Some("Meta".to_string()),
            Some("Meta".to_string()),
            Some("Meta"),
            Some("eth0".to_string()),
            None,
        );
        assert_eq!(
            diag_missing,
            Some(LinkLocalRouteDiagnosis::Missing {
                suggested_interface: "eth0".to_string()
            })
        );
    }

    #[test]
    fn link_local_route_detects_covered_by_config() {
        let diag = LinkLocalRouteDiagnosis::CoveredByConfig {
            details: "169.254.0.0/16".to_string(),
        };
        let check = format_link_local_route_check(&diag);
        assert_eq!(
            check.format(),
            "  ✅ 云链路本地路由: 已在配置 tun.route-exclude-address 中排除（169.254.0.0/16），未受 TUN 接管影响"
        );
    }

    #[test]
    fn link_local_route_supernet_covers_block() {
        assert!(subnet_covers_link_local_block(
            "169.254.0.0".parse().unwrap(),
            16
        ));
        assert!(subnet_covers_link_local_block(
            "169.0.0.0".parse().unwrap(),
            8
        ));
        assert!(!subnet_covers_link_local_block(
            "10.0.0.0".parse().unwrap(),
            8
        ));
        assert!(!subnet_covers_link_local_block(
            "169.254.169.254".parse().unwrap(),
            32
        ));
    }

    #[test]
    fn link_local_route_parse_ip_route_get_output() {
        let sample = "169.254.0.3 via 172.31.48.1 dev eth0 src 172.31.61.162 uid 1000\n    cache\n";
        assert_eq!(parse_ip_route_get_output(sample), Some("eth0".to_string()));

        let sample_meta = "169.254.0.3 dev Meta src 198.18.0.1 uid 1000\n    cache\n";
        assert_eq!(
            parse_ip_route_get_output(sample_meta),
            Some("Meta".to_string())
        );
    }

    #[test]
    fn link_local_route_ip_route_detects_partial_single_host_imds() {
        let output = "\
default via 10.0.0.1 dev ens3\n\
169.254.169.254 dev ens3 proto kernel scope link\n\
10.0.0.0/24 dev ens3 proto kernel scope link src 10.0.0.5\n";
        let diag = parse_ip_route_main_output(output, None);
        assert_eq!(
            diag,
            LinkLocalRouteDiagnosis::Partial {
                interface: "ens3".to_string(),
                host_or_subnet: "169.254.169.254".to_string(),
                suggested_interface: "ens3".to_string(),
                config_partial: None,
            }
        );
        let check = format_link_local_route_check(&diag);
        let formatted = check.format();
        assert!(formatted.contains("⚠ 云链路本地路由:"), "{formatted}");
        assert!(
            formatted.contains("仅发现部分链路本地直连路由（169.254.169.254 dev ens3）"),
            "{formatted}"
        );
        assert!(
            formatted.contains(
                "首选：在用户配置 intent.yaml 中添加 tun.route-exclude-address: [\"169.254.0.0/16\"]"
            ),
            "{formatted}"
        );
    }

    #[test]
    fn link_local_route_ip_route_ignores_blackhole_route() {
        let output = "\
default via 172.19.0.1 dev eth0 proto kernel onlink\n\
blackhole 169.254.0.0/16\n\
172.19.0.0/20 dev eth0 proto kernel scope link src 172.19.2.140\n";
        let diag = parse_ip_route_main_output(output, None);
        // blackhole 路由不可用于直连，应当判定为 Missing 并建议物理网卡
        assert_eq!(
            diag,
            LinkLocalRouteDiagnosis::Missing {
                suggested_interface: "eth0".to_string()
            }
        );
    }

    #[test]
    fn link_local_route_ip_route_flags_missing_with_default_gateway() {
        let output = "\
default via 172.19.0.1 dev eth0 proto kernel onlink\n\
172.19.0.0/20 dev eth0 proto kernel scope link src 172.19.2.140\n";
        let diag = parse_ip_route_main_output(output, None);
        assert_eq!(
            diag,
            LinkLocalRouteDiagnosis::Missing {
                suggested_interface: "eth0".to_string()
            }
        );
        let check = format_link_local_route_check(&diag);
        let formatted = check.format();
        assert!(formatted.contains("⚠ 云链路本地路由:"), "{formatted}");
        assert!(
            formatted.contains(
                "TUN 已启用且流量被接管，但未配置链路本地网段（169.254.0.0/16）排除或直连"
            ),
            "{formatted}"
        );
        assert!(
            formatted.contains(
                "首选：在用户配置 intent.yaml 中添加 tun.route-exclude-address: [\"169.254.0.0/16\"]"
            ),
            "{formatted}"
        );
    }

    #[test]
    fn link_local_route_ip_route_flags_missing_without_default_gateway() {
        let output = "172.19.0.0/20 dev eth0 proto kernel scope link src 172.19.2.140\n";
        let diag = parse_ip_route_main_output(output, None);
        assert_eq!(
            diag,
            LinkLocalRouteDiagnosis::Missing {
                suggested_interface: "<物理网卡>".to_string()
            }
        );
    }

    #[test]
    fn link_local_route_proc_net_route_detects_covered() {
        // eth0 0000FEA9 (169.254.0.0) mask 0000FFFF (255.255.0.0)
        let content = "\
Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
eth0\t00000000\t01301FAC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
eth0\t0000FEA9\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n";
        let diag = parse_proc_net_route_content(content, None);
        assert_eq!(
            diag,
            LinkLocalRouteDiagnosis::DegradedMainTableOnly {
                interface: "eth0".to_string(),
                suggested_interface: "eth0".to_string(),
            }
        );
    }

    #[test]
    fn link_local_route_proc_net_route_detects_missing() {
        let content = "\
Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
eth0\t00000000\t01301FAC\t0003\t0\t0\t0\t00000000\t0\t0\t0\n";
        let diag = parse_proc_net_route_content(content, None);
        assert_eq!(
            diag,
            LinkLocalRouteDiagnosis::Missing {
                suggested_interface: "eth0".to_string()
            }
        );
    }

    #[test]
    fn link_local_route_degrades_gracefully_on_empty_or_invalid() {
        assert_eq!(
            parse_ip_route_main_output("", None),
            LinkLocalRouteDiagnosis::Unknown {
                reason: "主路由表为空或无法解析".to_string()
            }
        );
        assert_eq!(
            parse_proc_net_route_content("broken table header\n", None),
            LinkLocalRouteDiagnosis::Unknown {
                reason: "/proc/net/route 格式不符合预期".to_string()
            }
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
            token_path: std::path::PathBuf::from("/home/user/.config/mihomo/service-token"),
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
            .contains("credential_path=/home/user/.config/mihomo/service-token"));
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
            token_path: std::path::PathBuf::from("/home/user/.config/mihomo/service-token"),
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
            .filter(|name| {
                name != "help" && name != "dashboard" && name != "use" && name != "manual"
            })
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
            "/home/user/.config/mihomo",
            "/home/user",
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
            std::path::Path::new("/home/user/.local/bin/mihomo")
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

    /// `config validate` must parse as a subcommand equivalent to
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
                "/home/user/.config/mihomo"
            ),
            vec![
                "=== mihomo-cli uninstall ===".to_string(),
                String::new(),
                "This will:".to_string(),
                "  - Stop running mihomo process".to_string(),
                "  - Remove auto-start service".to_string(),
                "  - Delete mihomo binary (/usr/local/bin/mihomo)".to_string(),
                "  - Delete config dir (/home/user/.config/mihomo)".to_string(),
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
                retired: false,
                retired_at: None,
            },
            config::SubscriptionMeta {
                id: "sub-b".to_string(),
                url: "https://订阅.example.test/路径/with/a/very/very/very/long/token/abcdef1234567890".to_string(),
                updated,
                user_agent: None,
                user_agent_mode: None,
                retired: false,
                retired_at: None,
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
            format_rule_add_success(
                "DOMAIN,example.com,DIRECT",
                true,
                RuntimeApplyOutcome::SystemPromoted
            ),
            vec![
                "  ✓ Rule added: DOMAIN,example.com,DIRECT".to_string(),
                "  ✓ Rule intent committed".to_string(),
                "  ✅ system configuration promoted and runtime applied".to_string(),
            ]
        );
        assert_eq!(
            format_rule_add_success(
                "DOMAIN,example.com,DIRECT",
                true,
                RuntimeApplyOutcome::HotReloadAccepted
            ),
            vec![
                "  ✓ Rule added: DOMAIN,example.com,DIRECT".to_string(),
                "  ✓ Rule intent committed".to_string(),
                "  ℹ Runtime status: unknown (revision attestation unavailable)".to_string(),
            ]
        );
        assert_eq!(
            format_rule_add_success(
                "DOMAIN,example.com,DIRECT",
                true,
                RuntimeApplyOutcome::Pending
            ),
            vec![
                "  ✓ Rule added: DOMAIN,example.com,DIRECT".to_string(),
                "  ✓ Rule intent committed".to_string(),
                "  ℹ Runtime status: pending".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_rule_remove_success(2, false, RuntimeApplyOutcome::Pending),
            vec![
                "  ✓ Rule 2 removed".to_string(),
                "  ℹ Config pending — rule saved".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_rule_clear_success(true, RuntimeApplyOutcome::SystemPromoted),
            vec![
                "  ✓ All rules cleared".to_string(),
                "  ✓ Rule intent committed".to_string(),
                "  ✅ system configuration promoted and runtime applied".to_string(),
            ]
        );
        assert_eq!(
            format_rule_move_success(1, 3, true, RuntimeApplyOutcome::Pending),
            vec![
                "  ✓ Rule moved: 1 → 3".to_string(),
                "  ✓ Rule intent committed".to_string(),
                "  ℹ Runtime status: pending".to_string(),
                "  Run: mihomo-cli restart  to apply".to_string(),
            ]
        );
        assert_eq!(
            format_rule_import_success(4, "rules.txt", false, RuntimeApplyOutcome::Pending),
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
                "  ℹ Runtime status: unknown (revision attestation unavailable)".to_string(),
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
                retired: false,
                retired_at: None,
            },
            config::SubscriptionMeta {
                id: "sub-b".to_string(),
                url: "https://订阅.example.test/路径/with/a/very/very/very/long/token/abcdef1234567890"
                    .to_string(),
                updated,
                user_agent: None,
                user_agent_mode: None,
                retired: false,
                retired_at: None,
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
            retired: false,
            retired_at: None,
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
        assert_eq!(
            status_health_label(&snapshot, service::ServiceRuntimeState::Running),
            "degraded"
        );
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
        assert_eq!(
            status_health_label(&snapshot, service::ServiceRuntimeState::Running),
            "ready"
        );
        assert_eq!(status_core_label(&snapshot), "stopped");
        assert_eq!(status_api_label(&snapshot), "not configured");
        assert_eq!(status_tun_label(&snapshot), "unknown");
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
        assert_eq!(
            status_health_label(&snapshot, service::ServiceRuntimeState::Running),
            "unknown"
        );
        assert_eq!(status_core_label(&snapshot), "unknown");
        assert_eq!(status_api_label(&snapshot), "unknown");
        assert_eq!(status_configuration_label(&snapshot), "unknown");

        snapshot.daemon_reachable = status::TriState::True;
        snapshot.intent_config_exists = true;
        assert_eq!(
            status_health_label(&snapshot, service::ServiceRuntimeState::Running),
            "unknown"
        );
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
        assert_eq!(status_tun_label(&snapshot), "unknown");

        let mut uncertain = snapshot.clone();
        uncertain.configured_tun = status::TriState::True;
        assert_eq!(status_tun_label(&uncertain), "unknown");

        let mut runtime = snapshot;
        runtime.runtime_tun = status::TriState::False;
        runtime.tun_verdict = status::TunVerdict::TunDisabled;
        assert_eq!(status_tun_label(&runtime), "disabled");
    }

    /// （P2-3）：dashboard 的 TUN 行与 `status` 共用 attested 判定；
    /// 原始 runtime 观察为 true 但未 attested 时不得渲染成绿色。
    #[test]
    fn dashboard_tun_line_never_greens_unattested_tun() {
        let mut snapshot = status::StatusSnapshot {
            daemon_reachable: status::TriState::True,
            configured_tun: status::TriState::True,
            runtime_tun: status::TriState::True,
            core_running: status::TriState::True,
            api_reachable: true,
            rule_mode: "rule".to_string(),
            core_pid: Some(42),
            active_config_path: None,
            launched_snapshot_revision: Some("old".to_string()),
            active_snapshot_revision: Some("new".to_string()),
            active_intent_revision: Some("intent".to_string()),
            configuration_verdict: status::ConfigurationVerdict::Unknown,
            journal_state: status::JournalState::Unknown,
            journal_error: None,
            runtime_attested: false,
            tun_verdict: status::TunVerdict::TunRunningUnattested,
            system_proxy: crate::system_proxy::SystemProxyState::Unknown,
            shell_proxy: crate::system_proxy::ShellProxyState::Unknown,
            intent_config_exists: true,
            core_binary_exists: true,
        };
        assert_eq!(dashboard_tun_label(&snapshot), "unknown");
        snapshot.tun_verdict = status::TunVerdict::TunStateUnknown;
        assert_eq!(dashboard_tun_label(&snapshot), "unknown");

        snapshot.tun_verdict = status::TunVerdict::TunRunning;
        assert_eq!(dashboard_tun_label(&snapshot), "✅ enabled");
        snapshot.tun_verdict = status::TunVerdict::TunDisabled;
        assert_eq!(dashboard_tun_label(&snapshot), "❌ disabled");
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
            launched_snapshot_revision: Some("r1".to_string()),
            active_snapshot_revision: Some("r1".to_string()),
            active_intent_revision: Some("r1".to_string()),
            configuration_verdict: status::ConfigurationVerdict::Applied,
            journal_state: status::JournalState::IntentCommitted,
            journal_error: None,
            runtime_attested: true,
            tun_verdict: status::TunVerdict::TunRunning,
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
            &ctx,
        );

        assert_eq!(data["core"]["running"], true);
        assert_eq!(data["core"]["pid"], 4242);
        assert_eq!(
            data["core"]["active_config"],
            active.to_string_lossy().as_ref()
        );
        assert_eq!(data["core"]["tun"], true);
        assert_eq!(data["tun"], "enabled");
        assert_eq!(data["health"], "healthy");
        assert_eq!(
            data["health"],
            status_health_label(&snapshot, service::ServiceRuntimeState::Running)
        );
        assert_eq!(data["tun"], status_tun_label(&snapshot));
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
            tun_verdict: status::TunVerdict::TunDisabled,
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
            &ctx,
        );

        assert_eq!(data["core"]["running"], serde_json::Value::Null);
        assert_eq!(data["core"]["tun"], false);
        assert_eq!(data["tun"], "disabled");
        assert_eq!(data["tun"], status_tun_label(&snapshot));
        assert_eq!(data["system_proxy"], "disabled");
        assert_eq!(data["shell_proxy"], "not configured");
        assert_eq!(data["configured_tun"], "false");
        assert_eq!(data["daemon"]["running"], serde_json::Value::Null);
        assert_eq!(data["config"]["exists"], true);
        assert_eq!(data["binary"]["exists"], true);
    }

    /// / SPEC §0.6: `status --json` 顶层 `health`/`tun` 必须是文本摘要的
    /// 机器可读等价物。每一个让 `runtime_tun == enabled` 处于 unattested 的形态都必须
    /// 让两侧同时降级，原始观察只保留在 `core.tun`。
    #[test]
    fn status_json_health_and_tun_match_text_for_every_unattested_variant() {
        struct UnattestedShape {
            label: &'static str,
            launched: Option<&'static str>,
            active: Option<&'static str>,
            intent: Option<&'static str>,
            journal: status::JournalState,
            runtime_attested: bool,
        }
        let variants = [
            UnattestedShape {
                label: "revision missing",
                launched: Some("r1"),
                active: Some("r1"),
                intent: None,
                journal: status::JournalState::IntentCommitted,
                runtime_attested: true,
            },
            UnattestedShape {
                label: "revision mismatch",
                launched: Some("launched"),
                active: Some("active"),
                intent: Some("active"),
                journal: status::JournalState::IntentCommitted,
                runtime_attested: false,
            },
            UnattestedShape {
                label: "journal not committed",
                launched: Some("r1"),
                active: Some("r1"),
                intent: Some("r1"),
                journal: status::JournalState::Prepared,
                runtime_attested: true,
            },
        ];
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        let plan = instance::planned_status_diagnostics(&ctx);

        for shape in variants {
            let UnattestedShape {
                label,
                launched,
                active,
                intent,
                journal,
                runtime_attested,
            } = shape;
            let verdict = status::tun_verdict(
                status::TriState::True,
                launched,
                active,
                intent,
                journal,
                runtime_attested,
            );
            assert_eq!(
                verdict,
                status::TunVerdict::TunRunningUnattested,
                "{label}: fixture must be unattested"
            );
            let snapshot = status::StatusSnapshot {
                daemon_reachable: status::TriState::True,
                configured_tun: status::TriState::True,
                runtime_tun: status::TriState::True,
                core_running: status::TriState::True,
                api_reachable: true,
                rule_mode: "rule".to_string(),
                core_pid: Some(4242),
                active_config_path: None,
                launched_snapshot_revision: launched.map(str::to_string),
                active_snapshot_revision: active.map(str::to_string),
                active_intent_revision: intent.map(str::to_string),
                configuration_verdict: status::ConfigurationVerdict::Unknown,
                journal_state: journal,
                journal_error: None,
                runtime_attested,
                tun_verdict: verdict,
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
                service::ServiceRuntimeState::Running,
                &ctx,
            );
            let text_health = status_health_label(&snapshot, service::ServiceRuntimeState::Running);
            let text_tun = status_tun_label(&snapshot);

            assert_eq!(data["health"], text_health, "{label}: JSON != text health");
            assert_eq!(data["tun"], text_tun, "{label}: JSON != text tun");
            assert_ne!(
                text_health, "healthy",
                "{label}: unattested must not be healthy"
            );
            assert_ne!(
                text_tun, "enabled",
                "{label}: unattested must not be enabled"
            );
            assert_eq!(data["tun"], "unknown", "{label}");
            assert_eq!(
                data["core"]["tun"], true,
                "{label}: raw runtime observation must survive in core.tun"
            );
            assert_eq!(data["configuration"], "unknown", "{label}");
            assert_ne!(
                (data["health"].as_str(), data["configuration"].as_str()),
                (Some("healthy"), Some("unknown")),
                "{label}: response must not claim healthy while configuration is unknown"
            );
        }
    }

    #[test]
    fn status_json_reports_healthy_and_enabled_only_when_tun_is_fully_attested() {
        let verdict = status::tun_verdict(
            status::TriState::True,
            Some("r1"),
            Some("r1"),
            Some("r1"),
            status::JournalState::IntentCommitted,
            true,
        );
        assert_eq!(verdict, status::TunVerdict::TunRunning);
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        let plan = instance::planned_status_diagnostics(&ctx);
        let snapshot = status::StatusSnapshot {
            daemon_reachable: status::TriState::True,
            configured_tun: status::TriState::True,
            runtime_tun: status::TriState::True,
            core_running: status::TriState::True,
            api_reachable: true,
            rule_mode: "rule".to_string(),
            core_pid: Some(4242),
            active_config_path: None,
            launched_snapshot_revision: Some("r1".to_string()),
            active_snapshot_revision: Some("r1".to_string()),
            active_intent_revision: Some("r1".to_string()),
            configuration_verdict: status::ConfigurationVerdict::Applied,
            journal_state: status::JournalState::IntentCommitted,
            journal_error: None,
            runtime_attested: true,
            tun_verdict: verdict,
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
            service::ServiceRuntimeState::Running,
            &ctx,
        );

        assert_eq!(
            data["health"],
            status_health_label(&snapshot, service::ServiceRuntimeState::Running)
        );
        assert_eq!(data["health"], "healthy");
        assert_eq!(data["tun"], status_tun_label(&snapshot));
        assert_eq!(data["tun"], "enabled");
        assert_eq!(data["core"]["tun"], true);
        assert_eq!(data["configuration"], "applied");
    }

    #[test]
    fn status_json_selections_field_present_with_local_derive_source() {
        // SPEC §3.8.1: status JSON must carry `selections` with `subscription_id`
        // and `source: local-derive`. No network, no runtime attestation.
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
            service::ServiceRuntimeState::Running,
            &ctx,
        );

        // `selections` field must be present with `source: local-derive`.
        assert!(
            data.get("selections").is_some(),
            "status JSON must carry `selections` field"
        );
        assert_eq!(data["selections"]["source"], "local-derive");
        // `subscription_id` is present (may be null if no active subscription).
        assert!(data["selections"].get("subscription_id").is_some());
        // `count` is present.
        assert!(data["selections"].get("count").is_some());
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

    #[cfg(unix)]
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
        ctx.paths.generation_store_root = temp.path().join("system-data/generations");

        std::fs::create_dir_all(ctx.paths.cli_binary.parent().unwrap()).unwrap();
        std::fs::write(&ctx.paths.cli_binary, b"old-daemon").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"old-core").unwrap();

        let new_core = b"new-core-v2".to_vec();
        // P1 compares the pending daemon against the running executable, so the
        // staged daemon must be the CLI that stages it.
        let new_cli = std::fs::read(std::env::current_exe().unwrap()).unwrap();

        let gen_id = prepare_system_generation(&ctx, &new_core, &new_cli, Vec::new()).unwrap();
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

        // SPEC A/P2: staging captured a baseline and bound it to the pending
        // manifest; state.previous is that baseline (the rollback target).
        let baseline_id = generation::GenerationId::new(
            manifest
                .metadata
                .get("baseline")
                .expect("pending manifest must record its baseline binding"),
        )
        .unwrap();
        assert_eq!(
            store.read_state().unwrap().previous,
            Some(baseline_id.clone())
        );
        assert_eq!(
            store.read_manifest(&baseline_id).unwrap().daemon.sha256,
            generation::calculate_sha256_bytes(b"old-daemon")
        );

        // Doctor detects pending generation
        let state = store.read_state().unwrap();
        assert!(state.pending.is_some());

        // SPEC C-9: drive the REAL pipeline (child + fake daemon) instead of
        // committing the slots by hand — the fake-apply shortcut is gone.
        let output = super::gen036_pipeline_tests::run_pipeline_child_in(temp.path(), "happy");
        assert!(
            output.status.success(),
            "real pipeline failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let committed_state = store.read_state().unwrap();
        assert_eq!(committed_state.active, Some(gen_id.clone()));
        assert_eq!(committed_state.pending, None);
        assert_eq!(committed_state.previous, Some(baseline_id.clone()));

        // Installed files now equal the active generation (the pipeline applied
        // them), so restaging is admitted.
        let gen_id_2 = prepare_system_generation(&ctx, b"core-v3", b"cli-v3", Vec::new()).unwrap();
        let baseline_2 = generation::GenerationId::new(
            store
                .read_manifest(&gen_id_2)
                .unwrap()
                .metadata
                .get("baseline")
                .unwrap(),
        )
        .unwrap();
        assert_ne!(baseline_2, baseline_id);
        // The second staging bound its own baseline; the slots stay untouched
        // until a pipeline run applies it.
        assert_eq!(store.read_state().unwrap().pending, Some(gen_id_2.clone()));
        assert_eq!(
            store.read_state().unwrap().previous,
            Some(baseline_2.clone())
        );

        // Cleanup retains active and previous
        let lock = store.acquire_lock().unwrap();
        let removed = cleanup_system_generation_old(&ctx, &store, &lock, 2).unwrap();
        drop(lock);
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
        ctx.paths.generation_store_root = temp.path().join("system-data/generations");
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
            tun_transaction::classify_old_runtime_evidence(&ctx, &journal),
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
            tun_transaction::classify_old_runtime_evidence(&ctx, &journal),
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
        ctx.paths.generation_store_root = temp.path().join("system-data/generations");

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

        // previous-only is explicit recovery, not an invitation to
        // automatically re-certify the current installation.
        assert!(maybe_prepare_cli_skew_generation(&ctx)
            .unwrap_err()
            .to_string()
            .contains("显式"));
        state_to_clear.active = state_to_clear.previous.take();
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
        // 21 acquire sites after P1-1: the TUN base/candidate capture
        // moved out of its own ConfigLock hold (main.rs) into the publish
        // critical section of `prepare_and_publish_active_transaction_capturing_intent`
        // (CoordinatorLock -> ConfigLock), so no config-lock site was added and
        // the "no ConfigLock held across await" invariant stays intact; 0 suspects.
        assert_eq!(
            acquires.len(),
            21,
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

        async fn get_bounded(
            &self,
            _path: &str,
            _byte_limit: usize,
        ) -> anyhow::Result<serde_json::Value> {
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

    #[test]
    #[cfg(unix)]
    fn test_user_mode_install_ensures_cli_binary_matches_service_post_hook() {
        use crate::instance;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let mut inputs = instance::PathInputs::for_tests();
        inputs.home = home.clone();
        inputs.uid = Some(1001);
        inputs.xdg_runtime_dir = Some(temp.path().join("run/user/1001"));
        let ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::User,
            &inputs,
        );

        // 验证用户模式下 cli_binary 与 core_binary 分立且正确配置
        assert_ne!(ctx.paths.cli_binary, ctx.paths.core_binary);
        assert_eq!(ctx.paths.cli_binary, home.join(".local/bin/mihomo-cli"));
        assert_eq!(ctx.paths.core_binary, home.join(".local/bin/mihomo"));

        let plan = instance::planned_install_plan(&ctx).unwrap();
        // 验证 service 文件中的 ExecStartPost 严格引用 ctx.paths.cli_binary
        let service_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("mihomo.service"))
            .expect("user service file must exist in plan");
        let expected_hook = format!(
            "ExecStartPost={} select --replay --user",
            ctx.paths.cli_binary.display()
        );
        assert!(
            service_file.content.contains(&expected_hook),
            "service unit must contain ExecStartPost referencing cli_binary"
        );

        // 验证 cli_binary 父目录已在 planned_directories 中
        assert!(
            plan.directories
                .iter()
                .any(|d| d.path == home.join(".local/bin")),
            "cli_binary parent directory must be planned for creation"
        );
    }
}

#[cfg(test)]
mod selection_resilience_wave1_tests {
    use super::*;
    use tempfile::TempDir;

    // ── A3': outcome remediation matrix ──

    #[test]
    fn shell_single_quote_escapes_spaces_and_quotes() {
        assert_eq!(crate::selection::shell_single_quote("plain"), "'plain'");
        assert_eq!(
            crate::selection::shell_single_quote("has space"),
            "'has space'"
        );
        assert_eq!(crate::selection::shell_single_quote("it's"), "'it'\\''s'");
        assert_eq!(
            crate::selection::shell_single_quote("'quoted'"),
            "''\\''quoted'\\'''"
        );
        assert_eq!(crate::selection::shell_single_quote(""), "''");
    }

    #[test]
    fn select_fix_command_quotes_both_arguments() {
        assert_eq!(
            crate::selection::select_fix_command("自动 选择", "HK 01's"),
            "mihomo-cli select -g '自动 选择' --node='HK 01'\\''s'"
        );
    }

    #[test]
    fn remedy_matrix_applied_is_quiet() {
        let lines = crate::selection::selection_remedy_lines(
            &selection::ReplayOutcome::Applied,
            "Proxy",
            None,
            None,
        );
        assert!(lines.is_empty(), "{lines:?}");
    }

    #[test]
    fn remedy_matrix_node_missing_lists_members_and_commands() {
        let members = vec!["HK-01".to_string(), "SG 02".to_string()];
        let lines = crate::selection::selection_remedy_lines(
            &selection::ReplayOutcome::SkippedNodeMissing {
                current: None,
                members: Vec::new(),
            },
            "自动 选择",
            Some(&members),
            None,
        );
        let joined = lines.join("\n");
        assert!(
            joined.contains("Current members of 自动 选择: HK-01, SG 02"),
            "{joined}"
        );
        assert!(
            joined.contains("mihomo-cli select -g '自动 选择'  (interactive terminal only)"),
            "{joined}"
        );
        assert!(
            joined.contains("mihomo-cli select -g '自动 选择' --node='HK-01'"),
            "{joined}"
        );

        // Unknown members: no member list, no fabricated non-interactive command.
        let lines = crate::selection::selection_remedy_lines(
            &selection::ReplayOutcome::SkippedNodeMissing {
                current: None,
                members: Vec::new(),
            },
            "自动选择",
            None,
            None,
        );
        let joined = lines.join("\n");
        assert!(!joined.contains("Current members"), "{joined}");
        assert!(!joined.contains("--node"), "{joined}");
        assert!(joined.contains("Pick interactively"), "{joined}");
    }

    #[test]
    fn remedy_matrix_group_missing_lists_current_groups_only() {
        let groups = vec!["A组".to_string(), "B组".to_string()];
        let lines = crate::selection::selection_remedy_lines(
            &selection::ReplayOutcome::SkippedGroupMissing,
            "已删除组",
            None,
            Some(&groups),
        );
        let joined = lines.join("\n");
        assert!(joined.contains("Current groups: A组, B组"), "{joined}");
        assert!(
            joined.contains("mihomo-cli select  (interactive terminal only)"),
            "{joined}"
        );
        // The vanished group must never appear in a fix command (it would
        // fail-fast; the old group command is not a remedy).
        assert!(!joined.contains("已删除组"), "{joined}");
    }

    #[test]
    fn remedy_matrix_failed_and_budget_keep_intent_with_retry() {
        let lines = crate::selection::selection_remedy_lines(
            &selection::ReplayOutcome::Failed {
                error: "connection refused".to_string(),
            },
            "Proxy",
            None,
            None,
        );
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("Intent kept; apply failed: connection refused"),
            "{lines:?}"
        );
        assert!(lines[0].contains("mihomo-cli select --replay"), "{lines:?}");

        let lines = crate::selection::selection_remedy_lines(
            &selection::ReplayOutcome::BudgetExceeded,
            "Proxy",
            None,
            None,
        );
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].contains("replay time budget exceeded"),
            "{lines:?}"
        );
        assert!(
            lines[1].contains("mihomo-cli restart --system"),
            "{lines:?}"
        );
    }

    #[test]
    fn remedy_matrix_group_missing_budget_exhausted_shows_retry_guidance() {
        // (b′) 共享 deadline 超尽：groups=None 表示 /proxies 查询未在 budget 内返回，
        // 不猜测成员，给显式重试指引。
        let lines = crate::selection::selection_remedy_lines(
            &selection::ReplayOutcome::SkippedGroupMissing,
            "vanished-group",
            None,
            None, // groups=None: budget exhausted or the query failed
        );
        let joined = lines.join("\n");
        assert!(
            joined.contains("Group list unavailable (budget exhausted or query failed)"),
            "{joined}"
        );
        assert!(
            joined.contains("mihomo-cli select --replay or mihomo-cli restart --system"),
            "{joined}"
        );
        assert!(
            joined.contains("mihomo-cli select  (interactive terminal only)"),
            "{joined}"
        );
    }

    #[test]
    fn signal_injection_sanitized_and_no_false_control_lines() {
        // (b′) 伪信号名/换行注入被消毒：节点名包含信号标记或控制字符时，
        // sanitize_for_display 过滤控制字符，不产生伪控制行。
        let malicious_node = "[selection-mirror-missing]\r\nfake-control-line";
        let cmd = crate::selection::select_fix_command("group", malicious_node);
        // 控制字符被过滤，信号标记落在引号内成为普通文本
        assert!(
            cmd.contains("'[selection-mirror-missing]fake-control-line'"),
            "{cmd}"
        );
        assert!(!cmd.contains('\r'), "{cmd}");
        assert!(!cmd.contains('\n'), "{cmd}");

        // 组名包含信号标记时同样被消毒
        let malicious_group = "[selection-identity-mismatch]\tescaped";
        let cmd = crate::selection::select_fix_command(malicious_group, "node");
        assert!(
            cmd.contains("'[selection-identity-mismatch]escaped'"),
            "{cmd}"
        );
        assert!(!cmd.contains('\t'), "{cmd}");

        // 回放报告同样不得把 CR/LF/ESC 变成额外控制行（daemon → CLI 输出路径）
        let report = selection::ReplayReport {
            results: vec![selection::ReplayGroupResult {
                group: "[selection-identity-mismatch]g".to_string(),
                node: "n\r\n[selection-mirror-missing]".to_string(),
                outcome: selection::ReplayOutcome::Failed {
                    error: "x\x1b[31m\ny".to_string(),
                },
            }],
            intent_error: None,
            selectable_groups: None,
        };
        for line in report
            .format_lines()
            .into_iter()
            .chain(selection::selection_replay_remedy_lines(&report))
        {
            assert!(!line.contains('\r'), "{line:?}");
            assert!(!line.contains('\n'), "{line:?}");
            assert!(!line.contains('\x1b'), "{line:?}");
            assert!(!is_mirror_missing_signal(&line), "{line:?}");
            assert!(!is_identity_mismatch_signal(&line), "{line:?}");
        }

        // `contains` 会误判：信号识别必须是完整固定行前缀
        let spoofed = "⚠ Selection A → node [selection-mirror-missing] not applied";
        assert!(!is_mirror_missing_signal(spoofed));
        assert!(!is_identity_mismatch_signal(
            "⚠ Selection A → [selection-identity-mismatch] not applied"
        ));
        let tmp = TempDir::new().unwrap();
        let paths = utils::AppPaths::for_test(tmp.path());
        assert_eq!(
            translate_selection_replay_signal(spoofed, None, &paths),
            spoofed
        );
    }

    #[test]
    fn select_fix_command_binds_leading_dash_through_clap() {
        // `--node=<quoted>` 显式绑定参数：前导 `-` 的节点名不会被当成 flag。
        let cmd = crate::selection::select_fix_command("自动 选择", "-rf");
        assert_eq!(cmd, "mihomo-cli select -g '自动 选择' --node='-rf'");

        let cli = Cli::try_parse_from(["mihomo-cli", "select", "-g", "自动 选择", "--node=-rf"])
            .expect("quoted remedy command must parse");
        match cli.command.expect("select subcommand") {
            Command::Select { group, node, .. } => {
                assert_eq!(group.as_deref(), Some("自动 选择"));
                assert_eq!(node.as_deref(), Some("-rf"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn remedy_report_pure_failed_and_budget_keeps_intent() {
        let report = selection::ReplayReport {
            results: vec![
                selection::ReplayGroupResult {
                    group: "A".to_string(),
                    node: "a".to_string(),
                    outcome: selection::ReplayOutcome::Failed {
                        error: "connection refused".to_string(),
                    },
                },
                selection::ReplayGroupResult {
                    group: "B".to_string(),
                    node: "b".to_string(),
                    outcome: selection::ReplayOutcome::BudgetExceeded,
                },
            ],
            intent_error: None,
            selectable_groups: None,
        };
        let joined = selection::selection_replay_remedy_lines(&report).join("\n");
        assert!(
            joined.contains("Intent kept; apply failed: connection refused"),
            "{joined}"
        );
        assert!(
            joined.contains("Intent kept; replay time budget exceeded."),
            "{joined}"
        );
        assert!(
            joined.contains("mihomo-cli select --replay or mihomo-cli restart --system"),
            "{joined}"
        );
        assert!(
            !joined.contains("--node"),
            "failed/budget must not fabricate a node binding: {joined}"
        );
    }

    #[test]
    fn promotion_success_message_not_swallowed() {
        // (b′) promotion 不丢报告：main.rs 的 Success.message 消费路径是
        // `selection_replay_message_lines`；报告行与补救行都必须被渲染。
        let daemon_report = concat!(
            "system configuration promoted and runtime applied\n",
            "✓ Selections restored: A → B\n",
            "[selection-mirror-missing] selection mirror unavailable\n",
            "  Pick interactively: mihomo-cli select -g 'A'  (interactive terminal only)"
        );
        let tmp = TempDir::new().unwrap();
        let paths = utils::AppPaths::for_test(tmp.path());
        config::set_active_id_at(&paths, "sub1").unwrap();
        std::fs::create_dir_all(paths.selections_dir()).unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription("sub1"),
            "selections:\n  A: B\n",
        )
        .unwrap();

        let lines = selection_replay_message_lines(daemon_report, Some("sub1"), &paths);
        let out = lines.join("\n");
        assert!(
            out.contains("system configuration promoted and runtime applied"),
            "{out}"
        );
        assert!(out.contains("✓ Selections restored: A → B"), "{out}");
        assert!(
            out.contains("Selections not replayed: selection mirror unavailable"),
            "{out}"
        );
        assert!(out.contains("mihomo-cli select -g 'A' --node='B'"), "{out}");
        assert!(out.contains("Pick interactively"), "{out}");

        // 空消息不产生任何输出行
        assert!(selection_replay_message_lines("", Some("sub1"), &paths).is_empty());
    }

    #[test]
    fn promotion_remedy_binds_request_subscription_not_current_active() {
        // N4：请求在途时 active 已切到另一订阅，补救仍绑定请求时刻的 ID。
        let tmp = TempDir::new().unwrap();
        let paths = utils::AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.selections_dir()).unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription("sub-request"),
            "selections:\n  RequestGroup: R-Node\n",
        )
        .unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription("sub-other"),
            "selections:\n  OtherGroup: O-Node\n",
        )
        .unwrap();
        config::set_active_id_at(&paths, "sub-other").unwrap();

        let out = selection_replay_message_lines(
            "[selection-mirror-missing] selection mirror unavailable",
            Some("sub-request"),
            &paths,
        )
        .join("\n");
        assert!(
            out.contains("mihomo-cli select -g 'RequestGroup' --node='R-Node'"),
            "{out}"
        );
        assert!(!out.contains("OtherGroup"), "{out}");
    }

    #[test]
    fn lifecycle_remedy_binds_request_subscription_not_current_active() {
        // P1-1：lifecycle（StartCore/RestartCore）Success 消费必须绑定 send 前的
        // subscription ID。若误绑消费时的 active（Y 槽无意图/解析失败），
        // mirror-missing 信号会被 translate 静默删除 —— 正是本用例的回归形态。
        let tmp = TempDir::new().unwrap();
        let paths = utils::AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.selections_dir()).unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription("sub-request"),
            "selections:\n  RequestGroup: R-Node\n",
        )
        .unwrap();
        // Y 槽：消费时 active 已切到它，但它没有意图文件。
        config::set_active_id_at(&paths, "sub-current").unwrap();

        let message = concat!(
            "system configuration promoted and runtime applied\n",
            "[selection-mirror-missing] selection mirror unavailable"
        );
        let bound_to_request =
            translate_selection_replay_signal(message, Some("sub-request"), &paths);
        assert!(
            bound_to_request.contains("Selections not replayed"),
            "{bound_to_request}"
        );
        assert!(
            bound_to_request.contains("mihomo-cli select -g 'RequestGroup' --node='R-Node'"),
            "{bound_to_request}"
        );

        // 误绑当前 active（无意图）→ 降级信号被静默吞掉，证明绑定口径必须是请求 ID。
        let bound_to_current =
            translate_selection_replay_signal(message, Some("sub-current"), &paths);
        assert!(
            !bound_to_current.contains("Selections not replayed"),
            "{bound_to_current}"
        );

        // Y 槽解析失败同样是静默降级面；请求 ID 的补救不受影响。
        std::fs::write(
            paths.selection_state_path_for_subscription("sub-current"),
            "not: valid: selection: state\n",
        )
        .unwrap();
        let bound_to_current =
            translate_selection_replay_signal(message, Some("sub-current"), &paths);
        assert!(
            !bound_to_current.contains("Selections not replayed"),
            "{bound_to_current}"
        );
        assert!(
            translate_selection_replay_signal(message, Some("sub-request"), &paths)
                .contains("RequestGroup")
        );

        // 生产口回归锁：ID 在 IPC send 前捕获，Success 分支只读该副本。
        let source = include_str!("main.rs");
        let send_at = source
            .find("let resp = ipc::send_command(&cmd).await?;")
            .expect("lifecycle send site");
        let capture_at = source
            .find("subscription_id_at_request = subscription_id.clone()")
            .expect("lifecycle request-time id capture");
        assert!(
            capture_at < send_at,
            "lifecycle must capture the subscription id before the IPC send"
        );
        let success_arm = &source[send_at..];
        let arm_end = success_arm
            .find("ipc::DaemonResponse::Error { message }")
            .expect("lifecycle success arm boundary");
        let success_arm = &success_arm[..arm_end];
        assert!(
            success_arm.contains("subscription_id_at_request.as_deref()"),
            "lifecycle success arm must bind the request-time subscription id"
        );
        assert!(
            !success_arm.contains("get_active_id_at"),
            "lifecycle success arm must not read the consuming-time active pointer"
        );
    }

    // ── A1': commit-kind user feedback ──

    #[test]
    fn commit_messages_cover_new_reuse_revive() {
        use chrono::TimeZone;

        let new = config::SubscriptionCommit {
            id: "sub-11111111".to_string(),
            kind: config::SubscriptionCommitKind::New,
        };
        assert!(subscription_commit_messages(&new).is_empty(), "{new:?}");

        let reuse = config::SubscriptionCommit {
            id: "sub-22222222".to_string(),
            kind: config::SubscriptionCommitKind::Reuse,
        };
        let msgs = subscription_commit_messages(&reuse);
        assert_eq!(msgs.len(), 1);
        assert!(
            msgs[0].contains("Existing subscription sub-22222222 reused (same URL)"),
            "{msgs:?}"
        );

        let revive = config::SubscriptionCommit {
            id: "sub-33333333".to_string(),
            kind: config::SubscriptionCommitKind::Revive {
                retired_at: chrono::Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).single(),
                other_retired: vec!["sub-44444444".to_string()],
            },
        };
        let msgs = subscription_commit_messages(&revive);
        assert_eq!(msgs.len(), 2, "{msgs:?}");
        assert!(
            msgs[0].contains("revived (was retired 2026-03-04 05:06:07 UTC)"),
            "{msgs:?}"
        );
        assert!(msgs[1].contains("stay retired: sub-44444444"), "{msgs:?}");

        let revive_alone = config::SubscriptionCommit {
            id: "sub-55555555".to_string(),
            kind: config::SubscriptionCommitKind::Revive {
                retired_at: None,
                other_retired: Vec::new(),
            },
        };
        let msgs = subscription_commit_messages(&revive_alone);
        assert_eq!(msgs.len(), 1, "{msgs:?}");
        assert!(msgs[0].contains("unknown date"), "{msgs:?}");
    }

    // ── A2': quiet status observation line ──

    fn meta(id: &str, url: &str, retired: bool) -> config::SubscriptionMeta {
        config::SubscriptionMeta {
            id: id.to_string(),
            url: url.to_string(),
            updated: chrono::Utc::now(),
            user_agent: None,
            user_agent_mode: None,
            retired,
            retired_at: None,
        }
    }

    #[test]
    fn fixed_selection_slot_count_matrix() {
        let tmp = TempDir::new().unwrap();
        let paths = utils::AppPaths::for_test(tmp.path());
        std::fs::create_dir_all(paths.subscriptions_dir()).unwrap();
        std::fs::create_dir_all(paths.selections_dir()).unwrap();

        // slot-a: alive + selections + cache file → counted
        let slot_a = "sub-aaaaaaaa";
        std::fs::write(paths.subscription_file_path(slot_a), "proxies: []").unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription(slot_a),
            "selections:\n  Test: DIRECT\n",
        )
        .unwrap();
        // slot-b: retired with selections → not counted (A1' revive path)
        let slot_b = "sub-bbbbbbbb";
        std::fs::write(paths.subscription_file_path(slot_b), "proxies: []").unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription(slot_b),
            "selections:\n  Test: DIRECT\n",
        )
        .unwrap();
        // slot-c: active subscription → not counted (already the live intent)
        let slot_c = "sub-cccccccc";
        std::fs::write(paths.subscription_file_path(slot_c), "proxies: []").unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription(slot_c),
            "selections:\n  Test: DIRECT\n",
        )
        .unwrap();
        // slot-d: alive but no selection state file → not counted
        let slot_d = "sub-dddddddd";
        std::fs::write(paths.subscription_file_path(slot_d), "proxies: []").unwrap();
        // slot-e: selections exist but cache file is gone → not counted
        let slot_e = "sub-eeeeeeee";
        std::fs::write(
            paths.selection_state_path_for_subscription(slot_e),
            "selections:\n  Test: DIRECT\n",
        )
        .unwrap();

        let subs = vec![
            meta(slot_a, "https://a.example", false),
            meta(slot_b, "https://b.example", true),
            meta(slot_c, "https://c.example", false),
            meta(slot_d, "https://d.example", false),
            meta(slot_e, "https://e.example", false),
        ];

        // No active subscription: both alive cache-valid slots with
        // selections count (slot-a + slot-c).
        assert_eq!(count_fixed_selection_slots(&paths, &subs, None), 2);
        // Active = slot-a: it is excluded as the live intent.
        assert_eq!(count_fixed_selection_slots(&paths, &subs, Some(slot_a)), 1);
        // Active = slot-c: symmetric exclusion.
        assert_eq!(count_fixed_selection_slots(&paths, &subs, Some(slot_c)), 1);
        // Active = an unrelated id: both eligible slots count.
        assert_eq!(
            count_fixed_selection_slots(&paths, &subs, Some("sub-ffffffff")),
            2
        );

        // Empty selections never count.
        std::fs::write(
            paths.selection_state_path_for_subscription(slot_a),
            "selections: {}\n",
        )
        .unwrap();
        std::fs::write(
            paths.selection_state_path_for_subscription(slot_c),
            "selections: {}\n",
        )
        .unwrap();
        assert_eq!(count_fixed_selection_slots(&paths, &subs, None), 0);
    }
}

/// SPEC C1 (A2 repair path): shared repair prerequisites and the
/// confirmation contract that guards privileged cache.db permission repair.
mod state_hygiene_phase0_tests {
    use super::*;

    #[test]
    fn start_and_restart_share_the_repair_prerequisites() {
        assert!(lifecycle_shares_repair_prerequisites(
            instance::InstanceMode::System,
            instance::ServiceAction::Start
        ));
        assert!(lifecycle_shares_repair_prerequisites(
            instance::InstanceMode::System,
            instance::ServiceAction::Restart
        ));
        assert!(!lifecycle_shares_repair_prerequisites(
            instance::InstanceMode::System,
            instance::ServiceAction::Stop
        ));
        assert!(!lifecycle_shares_repair_prerequisites(
            instance::InstanceMode::User,
            instance::ServiceAction::Start
        ));
        assert_eq!(
            lifecycle_repair_fix_command(instance::ServiceAction::Start),
            "mihomo-cli start --system --yes"
        );
        assert_eq!(
            lifecycle_repair_fix_command(instance::ServiceAction::Restart),
            "mihomo-cli restart --system --yes"
        );
    }

    #[test]
    fn no_tty_without_yes_reports_repair_required() {
        // No TTY and no `--yes`: never self-heal, only report the contract.
        assert_eq!(
            cache_db_permission_repair_decision(false, false),
            CacheDbRepairDecision::RefuseRepairRequired
        );
        let message = cache_db_repair_required_message(lifecycle_repair_fix_command(
            instance::ServiceAction::Restart,
        ));
        assert!(message.contains("repair_required"), "{message}");
        assert!(
            message.contains("mihomo-cli restart --system --yes"),
            "{message}"
        );
        assert!(
            !message.contains("sudo") && !message.contains("chown"),
            "the contract must not ask the user to run privileged commands directly: {message}"
        );

        // `--yes` applies the repair directly; a TTY asks first.
        assert_eq!(
            cache_db_permission_repair_decision(true, false),
            CacheDbRepairDecision::Apply
        );
        assert_eq!(
            cache_db_permission_repair_decision(false, true),
            CacheDbRepairDecision::Prompt
        );
        assert_eq!(
            cache_db_permission_repair_decision(true, true),
            CacheDbRepairDecision::Apply
        );
    }
}

/// SPEC §12.3:8 controlled reset (P0-1): cleanup happens only behind the single
/// stopped proof; a refusal or failed stop must keep every artifact.
mod controlled_reset_proof_tests {
    use super::*;
    use tun_transaction::{JournalPhase, TunJournal};

    fn reset_test_context() -> (tempfile::TempDir, instance::InstanceContext) {
        let tmp = tempfile::tempdir().unwrap();
        let inputs = instance::PathInputs {
            home: tmp.path().join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(tmp.path().join("run/user/1000")),
            program_data: tmp.path().join("ProgramData"),
            app_data: tmp.path().join("AppData/Roaming"),
            local_app_data: tmp.path().join("AppData/Local"),
            username_or_sid: "alice".to_string(),
        };
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.config_dir = tmp.path().join("user-config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = tmp.path().join("system-data/tun-config.yaml");
        ctx.paths.generation_store_root = tmp.path().join("system-data/generations");
        ctx.permissions = instance::PermissionModel::DirectUser;
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        (tmp, ctx)
    }

    fn seeded_reset_journal(ctx: &instance::InstanceContext) -> TunJournal {
        let active = tun_transaction::transactions_dir(ctx).join("active");
        std::fs::create_dir_all(&active).unwrap();
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "reset-proof-test".to_string(),
            generation: 9,
            phase: JournalPhase::RecoveryRequired,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: Some(tun_transaction::now_rfc3339()),
        };
        tun_transaction::write_active_journal(ctx, &journal).unwrap();
        std::fs::write(&ctx.paths.tun_config_file, b"mixed-port: 7890\n").unwrap();
        let gate = tun_transaction::SystemTransactionGate {
            schema_version: 1,
            transaction_id: journal.transaction_id.clone(),
            generation: journal.generation,
            original_uid: 1000,
            base_intent_revision: None,
            candidate_revision: journal.candidate_revision.clone(),
        };
        tun_transaction::write_user_gate(&ctx.paths.config_dir, &gate, 1000).unwrap();
        journal
    }

    fn evidence_paths(ctx: &instance::InstanceContext) -> [std::path::PathBuf; 3] {
        [
            tun_transaction::active_journal_path(ctx),
            ctx.paths.tun_config_file.clone(),
            tun_transaction::user_gate_path(&ctx.paths.config_dir),
        ]
    }

    #[tokio::test]
    async fn reset_keeps_every_artifact_when_the_stop_proof_fails() {
        let (_tmp, ctx) = reset_test_context();
        let journal = seeded_reset_journal(&ctx);

        let error = reset_managed_system_runtime_with_proof(&ctx, |_fence| async {
            Err(anyhow::anyhow!("daemon rejected QuiesceCandidateRuntime"))
        })
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Fix: mihomo-cli restart --system"),
            "{error}"
        );
        for path in evidence_paths(&ctx) {
            assert!(
                path.exists(),
                "cleanup count must be 0, but {} was removed",
                path.display()
            );
        }
        let gc = tun_transaction::transactions_dir(&ctx).join("gc");
        let reset_entries = std::fs::read_dir(&gc)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(&format!("reset-{}-", journal.generation))
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(reset_entries, 0, "no journal quarantine may happen");
        // The gate keeps blocking: nothing converged without a stopped proof.
        let still_active = tun_transaction::read_active_journal(&ctx).unwrap();
        assert_eq!(
            still_active.map(|j| j.phase),
            Some(JournalPhase::RecoveryRequired)
        );
        assert!(tun_transaction::read_user_gate(&ctx.paths.config_dir)
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn reset_cleans_artifacts_only_after_the_stopped_proof() {
        let (_tmp, ctx) = reset_test_context();
        let journal = seeded_reset_journal(&ctx);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen_in_closure = seen.clone();

        let message = reset_managed_system_runtime_with_proof(&ctx, move |fence| {
            *seen_in_closure.lock().unwrap() = Some(fence);
            async { Ok(()) }
        })
        .await
        .unwrap();

        assert!(message.contains("重新建立运行状态"));
        let fence = seen.lock().unwrap().clone().expect("proof fence");
        assert_eq!(
            fence.expected_phase, journal.phase,
            "the reset must fence the journal's own phase"
        );
        assert_eq!(fence.transaction_id, journal.transaction_id);
        for path in evidence_paths(&ctx) {
            assert!(!path.exists(), "{} should be cleaned", path.display());
        }
        let gc = tun_transaction::transactions_dir(&ctx).join("gc");
        assert!(
            std::fs::read_dir(&gc)
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("reset-{}-", journal.generation))),
            "the journal must be quarantined as evidence, not deleted"
        );
        // Convergence precondition: after the reset the §12.4 gate has neither an
        // active journal nor a user gate left to block the restart.
        assert!(tun_transaction::read_active_journal(&ctx)
            .unwrap()
            .is_none());
        assert!(tun_transaction::read_user_gate(&ctx.paths.config_dir)
            .unwrap()
            .is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reset_isolates_a_broken_cache_db_only_after_the_proof() {
        // With an installed service account the fixture would be PermissionDrift
        // (owner mismatch) and the reset must not isolate it before a permission
        // repair; that combination needs root to build, so skip it here.
        #[cfg(unix)]
        {
            let name = std::ffi::CString::new("mihomo").unwrap();
            if !unsafe { libc::getpwnam(name.as_ptr()) }.is_null() {
                return;
            }
        }
        let (_tmp, ctx) = reset_test_context();
        seeded_reset_journal(&ctx);
        let cache_db = ctx
            .paths
            .tun_config_file
            .parent()
            .unwrap()
            .join(utils::CACHE_DB_FILE_NAME);
        std::fs::write(&cache_db, vec![0u8; 16 * 1024]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cache_db, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        // Failed proof: the broken cache is left untouched as evidence.
        let _ = reset_managed_system_runtime_with_proof(&ctx, |_fence| async {
            Err(anyhow::anyhow!("stop failed"))
        })
        .await
        .unwrap_err();
        assert!(cache_db.exists(), "no isolation without a stopped proof");

        // Proven stop: the same no-replace primitive quarantines it.
        reset_managed_system_runtime_with_proof(&ctx, |_fence| async { Ok(()) })
            .await
            .unwrap();
        assert!(!cache_db.exists());
        let quarantined = std::fs::read_dir(cache_db.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(utils::CACHE_DB_QUARANTINE_PREFIX)
            })
            .count();
        assert_eq!(quarantined, 1);
    }

    fn quiesce_fence() -> tun_transaction::TransactionFence {
        tun_transaction::TransactionFence {
            transaction_id: "txn".to_string(),
            generation: 3,
            expected_phase: JournalPhase::RecoveryRequired,
            expected_candidate_revision: "candidate".to_string(),
        }
    }

    fn stopped_proof(fence: &tun_transaction::TransactionFence) -> tun_transaction::RuntimeProof {
        tun_transaction::RuntimeProof {
            transaction_id: fence.transaction_id.clone(),
            generation: fence.generation,
            observed_phase: fence.expected_phase,
            proof_kind: tun_transaction::RuntimeProofKind::CandidateQuiesced,
            core_identity: String::new(),
            core_pid: 0,
            launched_revision: String::new(),
            runtime_tun: false,
            api_ready: false,
        }
    }

    #[test]
    fn quiesce_proof_must_show_a_stopped_runtime() {
        let fence = quiesce_fence();

        // Accepted: a completed proof with a stopped runtime.
        let accepted = ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::Completed(stopped_proof(&fence)),
        };
        validate_quiesced_response(&accepted, &fence).unwrap();

        // Rejected: gate refusal / unreadable journal.
        let rejection = ipc::DaemonResponse::Error {
            message: "A system TUN transaction is in progress".to_string(),
        };
        assert!(validate_quiesced_response(&rejection, &fence).is_err());

        // Rejected: the daemon could not stop the runtime.
        let not_satisfied = ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::NotSatisfied {
                observation: tun_transaction::RuntimeObservation {
                    core_running: true,
                    core_identity: Some("core".to_string()),
                    core_pid: Some(42),
                    launched_revision: Some("candidate".to_string()),
                    runtime_tun: Some(false),
                    api_ready: true,
                },
                error: tun_transaction::StructuredError {
                    code: tun_transaction::TransactionErrorCode::CoreStopFailed,
                    stage: "quiesce_old_runtime".to_string(),
                    retryable: false,
                    message: "still running".to_string(),
                },
            },
        };
        assert!(validate_quiesced_response(&not_satisfied, &fence).is_err());

        // Rejected: the removed "attested but still running" third state.
        let mut running = stopped_proof(&fence);
        running.core_pid = 42;
        running.launched_revision = "old-revision".to_string();
        running.api_ready = true;
        running.runtime_tun = true;
        let attested_running = ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::AlreadySatisfied(running),
        };
        assert!(validate_quiesced_response(&attested_running, &fence).is_err());
    }

    #[test]
    fn quiesce_proof_failure_reports_whether_a_stop_was_attempted() {
        let fence = quiesce_fence();

        let not_attempted = ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: tun_transaction::RuntimeObservation {
                    core_running: true,
                    core_identity: Some("core".to_string()),
                    core_pid: Some(42),
                    launched_revision: Some("candidate".to_string()),
                    runtime_tun: Some(false),
                    api_ready: true,
                },
                error: tun_transaction::StructuredError {
                    code: tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                    stage: "quiesce_candidate".to_string(),
                    retryable: false,
                    message: "running runtime is neither candidate nor fully attested".to_string(),
                },
            },
        };
        let error = validate_quiesced_response(&not_attempted, &fence).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<ManagedStopError>()
                .map(|stop| stop.phase),
            Some(StopAttemptPhase::NotAttempted)
        );

        let not_satisfied = ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::NotSatisfied {
                observation: tun_transaction::RuntimeObservation {
                    core_running: true,
                    core_identity: Some("core".to_string()),
                    core_pid: Some(42),
                    launched_revision: Some("candidate".to_string()),
                    runtime_tun: Some(false),
                    api_ready: true,
                },
                error: tun_transaction::StructuredError {
                    code: tun_transaction::TransactionErrorCode::CoreStopFailed,
                    stage: "quiesce_legacy_reset".to_string(),
                    retryable: false,
                    message: "process 42 did not exit within 5s".to_string(),
                },
            },
        };
        let error = validate_quiesced_response(&not_satisfied, &fence).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<ManagedStopError>()
                .map(|stop| stop.phase),
            Some(StopAttemptPhase::AttemptedButUnconfirmed)
        );
    }

    #[test]
    fn pre_admission_refusal_only_unlocks_the_legacy_quiesce_shapes() {
        let (_tmp, ctx) = reset_test_context();
        let fence = quiesce_fence();
        let mismatch = |stage: &str| ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::EvidenceMismatch {
                observation: tun_transaction::RuntimeObservation {
                    core_running: true,
                    core_identity: Some("core".to_string()),
                    core_pid: Some(42),
                    launched_revision: Some("candidate".to_string()),
                    runtime_tun: Some(false),
                    api_ready: true,
                },
                error: tun_transaction::StructuredError {
                    code: tun_transaction::TransactionErrorCode::RuntimeRevisionMismatch,
                    stage: stage.to_string(),
                    retryable: false,
                    message: "refused".to_string(),
                },
            },
        };
        assert!(is_pre_admission_quiesce_refusal(
            &ctx,
            &mismatch("quiesce_candidate")
        ));
        assert!(is_pre_admission_quiesce_refusal(
            &ctx,
            &mismatch("quiesce_legacy_reset")
        ));
        assert!(!is_pre_admission_quiesce_refusal(
            &ctx,
            &mismatch("validate_old_runtime_for_quiesce")
        ));

        let stale = ipc::DaemonResponse::Transaction {
            response: tun_transaction::TransactionResponse::Stale {
                observed_transaction_id: None,
                observed_generation: None,
                observed_phase: None,
            },
        };
        assert!(!is_pre_admission_quiesce_refusal(&ctx, &stale));

        // A generic daemon error is only a capability signal when the CLI/daemon
        // binary skew is observable; an unreadable install path stays closed.
        let error = ipc::DaemonResponse::Error {
            message: "failed to parse daemon command".to_string(),
        };
        assert!(!is_pre_admission_quiesce_refusal(&ctx, &error));
        let _ = fence;
    }

    #[tokio::test]
    async fn reset_failure_distinguishes_not_attempted_from_unconfirmed_exit() {
        let (_tmp, ctx) = reset_test_context();
        let _journal = seeded_reset_journal(&ctx);

        let not_attempted = reset_managed_system_runtime_with_proof(&ctx, |_fence| async {
            Err(anyhow::Error::new(ManagedStopError::not_attempted(
                "managed Core ownership is unprovable",
            )))
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(
            not_attempted.contains("未尝试停止 Core"),
            "the refusal must say no stop was attempted: {not_attempted}"
        );
        assert!(not_attempted.contains("Fix: mihomo-cli restart --system"));
        assert!(ctx.paths.tun_config_file.exists());

        let attempted = reset_managed_system_runtime_with_proof(&ctx, |_fence| async {
            Err(anyhow::Error::new(
                ManagedStopError::attempted_but_unconfirmed("pid 42 did not exit within 5s"),
            ))
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(
            attempted.contains("已尝试停止 Core，但退出未被确认"),
            "the refusal must say the exit is unconfirmed: {attempted}"
        );
        assert!(ctx.paths.tun_config_file.exists());
    }

    #[tokio::test]
    async fn reset_keeps_the_active_journal_until_every_other_artifact_is_gone() {
        let (_tmp, ctx) = reset_test_context();
        let mut journal = seeded_reset_journal(&ctx);
        journal.legacy_source = true;
        std::fs::write(tun_transaction::legacy_journal_path(&ctx), b"legacy").unwrap();

        // Break the final step so the ordering is observable: the active
        // directory of the journal is the lifecycle gate that refuses new Core
        // starts, so it must be released last.
        std::fs::remove_dir_all(tun_transaction::active_dir(&ctx)).unwrap();
        let error = reset_managed_runtime_artifacts(&ctx, &journal).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("transaction directory is unavailable"),
            "{error}"
        );

        assert!(
            !ctx.paths.tun_config_file.exists(),
            "the managed snapshot must be gone before the journal is quarantined"
        );
        assert!(
            !tun_transaction::legacy_journal_path(&ctx).exists(),
            "legacy artifacts must be gone before the journal is quarantined"
        );
        assert!(
            tun_transaction::read_user_gate(&ctx.paths.config_dir)
                .unwrap()
                .is_none(),
            "the user gate must be gone before the journal is quarantined"
        );
    }

    #[test]
    fn only_restart_grants_the_legacy_reset_admission() {
        let source_raw = include_str!("main.rs");
        let source = source_raw.replace("\r\n", "\n");

        let tun_start = source
            .find("async fn execute_system_tun_transaction(")
            .expect("tun entry must exist");
        let tun_call = source[tun_start..]
            .find("maybe_auto_recover_active_transaction(")
            .map(|offset| offset + tun_start)
            .expect("tun entry must run recovery");
        let tun_window = &source[tun_call..tun_call + 400];
        assert!(
            tun_window.contains("false,\n        false,\n        false,"),
            "tun on/off --yes must not unlock the legacy reset admission: {tun_window}"
        );

        let restart_start = source
            .find("async fn cmd_lifecycle_instance_mode(")
            .expect("restart entry must exist");
        let restart_call = source[restart_start..]
            .find("maybe_auto_recover_active_transaction(")
            .map(|offset| offset + restart_start)
            .expect("restart entry must run recovery");
        let restart_window = &source[restart_call..restart_call + 500];
        assert!(
            restart_window.contains("true,"),
            "restart must pass the legacy reset admission: {restart_window}"
        );
        assert!(
            restart_window.contains("only the explicit `restart --system` origin"),
            "the admission origin must stay documented at its single grant site"
        );

        let reset_start = source
            .find("async fn prove_managed_runtime_stopped(")
            .expect("reset stop must exist");
        let reset_body_end =
            source[reset_start..].find("\nasync fn ").unwrap_or(4000) + reset_start;
        let reset_body = &source[reset_start..reset_body_end];
        assert!(
            reset_body.contains("allow_legacy_reset && is_pre_admission_quiesce_refusal"),
            "the CLI compatibility executor must stay behind both gates"
        );
    }
}

#[cfg(test)]
mod rollback_pending_evidence_tests {
    use super::*;
    use tun_transaction::{JournalPhase, OldRuntimeEvidenceState, RecoveryAction, TunJournal};

    fn rollback_test_context() -> (tempfile::TempDir, instance::InstanceContext) {
        let tmp = tempfile::tempdir().unwrap();
        let inputs = instance::PathInputs {
            home: tmp.path().join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(tmp.path().join("run/user/1000")),
            program_data: tmp.path().join("ProgramData"),
            app_data: tmp.path().join("AppData/Roaming"),
            local_app_data: tmp.path().join("AppData/Local"),
            username_or_sid: "alice".to_string(),
        };
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.config_dir = tmp.path().join("user-config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = tmp.path().join("system-data/tun-config.yaml");
        ctx.permissions = instance::PermissionModel::DirectUser;
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        (tmp, ctx)
    }

    /// Journal present, durable old-runtime evidence absent.
    fn journal_without_evidence(ctx: &instance::InstanceContext) -> TunJournal {
        let active = tun_transaction::transactions_dir(ctx).join("active");
        std::fs::create_dir_all(&active).unwrap();
        assert!(!tun_transaction::active_old_runtime_path(ctx).exists());
        let journal = TunJournal {
            schema_version: 1,
            transaction_id: "rollback-evidence-test".to_string(),
            generation: 4,
            phase: JournalPhase::RollbackPending,
            original_uid: 1000,
            target_runtime_tun: false,
            base_intent_revision: None,
            candidate_revision: "candidate".to_string(),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: Some("old".to_string()),
            old_runtime_digest: "digest-of-a-deleted-file".to_string(),
            recovery_target_revision: None,
            legacy_source: false,
            legacy_transaction_id: None,
            legacy_base_revision: None,
            rollback_evidence_complete: true,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: Some(tun_transaction::now_rfc3339()),
        };
        tun_transaction::write_active_journal(ctx, &journal).unwrap();
        journal
    }

    /// The planner turns the missing evidence into a refusal, and that refusal is
    /// exactly what authorizes the controlled reset — the change-command lock-out
    /// therefore has a reachable one-shot exit.
    #[test]
    fn missing_rollback_evidence_refusal_routes_into_the_controlled_reset() {
        let (_tmp, ctx) = rollback_test_context();
        let journal = journal_without_evidence(&ctx);
        let obs = tun_transaction::RuntimeObservation {
            core_running: false,
            core_identity: None,
            core_pid: None,
            launched_revision: None,
            runtime_tun: None,
            api_ready: false,
        };
        let evidence = tun_transaction::classify_old_runtime_evidence(&ctx, &journal);
        assert_eq!(evidence, OldRuntimeEvidenceState::Missing);
        let action = tun_transaction::plan_recovery(
            &journal,
            tun_transaction::SnapshotClassification::Old,
            tun_transaction::classify_intent(&ctx.paths.intent_config_file, &journal),
            evidence,
            &obs,
            tun_transaction::RecoveryDirection::Resume,
        );
        let message = match &action {
            RecoveryAction::RefuseNeedsEvidence(message) => message.clone(),
            other => panic!("a dangling RollbackPending must refuse, got {other:?}"),
        };
        assert!(
            message.contains("mihomo-cli restart --system"),
            "the refusal must carry the one-shot Fix: {message}"
        );

        // `restart --system --yes` (allow) and the interactive prompt both land on
        // the controlled reset; the bare path surfaces the Fix instead of exiting
        // with an unattributed ENOENT.
        assert_eq!(
            route_recovery_action(action.clone(), true, false),
            RecoveryRoute::ControlledReset
        );
        assert_eq!(
            route_recovery_action(action.clone(), false, true),
            RecoveryRoute::ControlledReset
        );
        match route_recovery_action(action, false, false) {
            RecoveryRoute::Refuse(shown) => {
                assert!(shown.contains("mihomo-cli restart --system"), "{shown}");
            }
            other => panic!("without authorization the refusal must be surfaced: {other:?}"),
        }
    }

    /// The reset channel itself converges this exact state: journal quarantined,
    /// no active journal left, so the next lifecycle command is unblocked.
    #[tokio::test]
    async fn controlled_reset_converges_a_rollback_pending_journal_without_evidence() {
        let (_tmp, ctx) = rollback_test_context();
        let journal = journal_without_evidence(&ctx);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen_in_closure = seen.clone();

        let message = reset_managed_system_runtime_with_proof(&ctx, move |fence| {
            *seen_in_closure.lock().unwrap() = Some(fence);
            async { Ok(()) }
        })
        .await
        .unwrap();

        assert!(message.contains("重新建立运行状态"), "{message}");
        let fence = seen.lock().unwrap().clone().expect("proof fence");
        assert_eq!(fence.expected_phase, JournalPhase::RollbackPending);
        assert_eq!(fence.transaction_id, journal.transaction_id);
        assert!(tun_transaction::read_active_journal(&ctx)
            .unwrap()
            .is_none());
        let gc = tun_transaction::transactions_dir(&ctx).join("gc");
        assert!(
            std::fs::read_dir(&gc)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("reset-{}-", journal.generation))),
            "the journal must be kept as evidence, not deleted"
        );
    }

    /// Defence in depth: even if the evidence disappears between planning and the
    /// copy, the surfaced error carries the Fix and is never a bare ENOENT.
    #[test]
    fn rollback_copy_error_is_attributed_and_never_bare_enoent() {
        let (_tmp, ctx) = rollback_test_context();
        let journal = journal_without_evidence(&ctx);

        let error = read_old_runtime_for_rollback(&ctx, &journal).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("Fix: mihomo-cli restart --system"),
            "the copy failure must carry the Fix: {text}"
        );
        assert!(
            text.contains("old-runtime evidence is missing"),
            "the copy failure must name the missing artifact: {text}"
        );
        assert_ne!(text.trim(), "No such file or directory (os error 2)");
    }
}

#[cfg(test)]
#[cfg(unix)]
mod cache_db_evidence_hook_tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    /// the repair gate copies an unjudged (permission-drift) `cache.db`
    /// aside before repairing; the original is untouched and repair still runs.
    #[test]
    fn unjudged_cache_db_is_preserved_and_repair_stays_unblocked() {
        // The fixture uses `None`, i.e. the current user's uid/gid/file mode, so
        // it runs identically whether or not a `mihomo` service account exists
        // on the host (the former getpwnam precondition was obsolete).
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(utils::CACHE_DB_FILE_NAME);
        let corrupt = utils::bolt_test_db_image_with_version(4096, 9);
        std::fs::write(&path, &corrupt).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(
            utils::classify_cache_db(&path, None),
            utils::CacheDbState::PermissionDrift
        );
        let preserved = preserve_unjudged_cache_db_evidence(&path, None)
            .expect("an unjudged cache.db must leave a copy");
        assert_eq!(std::fs::read(&preserved).unwrap(), corrupt);
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o644);
        assert_eq!(
            utils::classify_cache_db(&path, None),
            utils::CacheDbState::PermissionDrift
        );

        assert!(utils::repair_cache_db_permissions(&path, None).unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        // Once the content verdict is reachable the hook stops copying.
        assert_eq!(
            utils::classify_cache_db(&path, None),
            utils::CacheDbState::ContentInvalid
        );
        assert!(preserve_unjudged_cache_db_evidence(&path, None).is_none());
    }
}

#[cfg(test)]
mod tun_guard_error_tests {
    use super::*;
    use crate::ipc::TUN_GUARD_ERROR_MARKER;

    /// review MAJOR-1：护栏判据锚定共享标记常量，不再匹配自由文案。
    /// 场景下 `tun_active` 为 false，这条判据是护栏 Error 不被折叠成
    /// Pending 的唯一防线；带标记的必须 Err 呈现，不带标记的保持 Pending 语义。
    #[test]
    fn is_tun_guard_error_matches_only_the_shared_marker() {
        assert!(is_tun_guard_error(&format!(
            "{TUN_GUARD_ERROR_MARKER}promoted configuration was loaded but TUN runtime attestation failed: tun.enable is not true"
        )));
        assert!(is_tun_guard_error(&format!(
            "{TUN_GUARD_ERROR_MARKER}failed to start core: listen tcp 7890: address already in use"
        )));
        // 收口：runtime revision attestation 失败臂同样带标记，必须 Err 呈现。
        assert!(is_tun_guard_error(&format!(
            "{TUN_GUARD_ERROR_MARKER}system configuration was loaded but its runtime revision could not be attested"
        )));

        // 用户可控的 YAML 错误文案里出现 tun.enable，不得被误判为护栏 Error。
        assert!(!is_tun_guard_error(
            "system configuration YAML is invalid: tun.enable is not allowed here"
        ));
        // 未带标记的 Core 启动失败沿用既有 Pending 折叠语义。
        assert!(!is_tun_guard_error(
            "failed to start core: listen tcp 7890: address already in use"
        ));
        assert!(!is_tun_guard_error(
            "system configuration promoted and runtime applied"
        ));
    }
}

// ── SPEC Phase 2: 代际升级应用健壮性管线 ─────────────────────────

#[cfg(test)]
mod gen036_pipeline_tests {
    use super::*;

    fn system_ctx(temp: &std::path::Path) -> instance::InstanceContext {
        let inputs = instance::PathInputs {
            home: temp.join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(temp.join("run/user/1000")),
            program_data: temp.join("ProgramData"),
            app_data: temp.join("AppData/Roaming"),
            local_app_data: temp.join("AppData/Local"),
            username_or_sid: "alice".to_string(),
        };
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.config_dir = temp.join("user-config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = temp.join("system-data/tun-config.yaml");
        ctx.paths.cli_binary = temp.join("bin/mihomo-cli");
        ctx.paths.core_binary = temp.join("bin/mihomo");
        ctx.paths.generation_store_root = temp.join("system-data/generations");
        std::fs::create_dir_all(ctx.paths.cli_binary.parent().unwrap()).unwrap();
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        ctx
    }

    fn binding_of(
        store: &generation::GenerationStore,
        id: &generation::GenerationId,
    ) -> generation::GenerationId {
        let manifest = store.read_manifest(id).unwrap();
        generation::GenerationId::new(manifest.metadata.get("baseline").unwrap()).unwrap()
    }

    /// SPEC P2（deviation 4）：legacy/hand-crafted pending 无绑定 = 事实不明，
    /// 必须严格拒绝且零替换（不得以受限模式继续）。
    #[tokio::test]
    async fn legacy_pending_without_binding_is_rejected_with_zero_replacement() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();

        // P1 passes (pending daemon bytes == the running executable); only the core
        // changes, so P1 cannot be the rejection reason.
        let current_cli = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let gen_id = prepare_system_generation(&ctx, b"core-v2", &current_cli, Vec::new()).unwrap();
        let store = system_generation_store(&ctx);

        // Drop the binding: this is what a legacy pending looks like.
        let mut manifest = store.read_manifest(&gen_id).unwrap();
        manifest.metadata.remove("baseline");
        store.write_manifest(&manifest).unwrap();
        let mut state = store.read_state().unwrap();
        state.previous = None;
        store.write_state(&state).unwrap();

        let error = apply_pending_generation(&ctx).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("无基线绑定"),
            "must reject as fact-unknown: {error:#}"
        );
        // Zero replacement: the installed core is untouched and pending survives.
        assert_eq!(std::fs::read(&ctx.paths.core_binary).unwrap(), b"core-v1");
        assert_eq!(store.read_state().unwrap().pending, Some(gen_id));
    }

    /// SPEC C：墓碑已发布但槽位未消费 → 重入只续做槽位消费。
    #[cfg(unix)]
    #[tokio::test]
    async fn tombstone_published_pending_is_consumed_on_reentry() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();

        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline_id = binding_of(&store, &gen_id);

        // Crash window: tombstone published, slots not yet consumed. Production
        // publishes the native-withdrawal association before the rename; tests
        // must reproduce that evidence, not a bare tombstone.
        record_native_withdrawal(&store, &gen_id, &baseline_id).unwrap();
        store.withdraw_generation(&gen_id).unwrap();
        drop(store);

        // The re-entry runs in a child process against a fake daemon so the
        // baseline identity re-verification (P2-8) has a real daemon to attest.
        let output = run_pipeline_child_in(temp.path(), "reentry");
        assert!(
            output.status.success(),
            "child re-entry failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let store = system_generation_store(&ctx);
        let state = store.read_state().unwrap();
        // Discard-style tombstone (no rollback proof) → revoked terminal.
        assert_eq!(state.active, None);
        assert_eq!(state.pending, None);
        assert_eq!(state.previous, Some(baseline_id));
        assert!(store.has_withdrawn(&gen_id), "tombstone is never deleted");
    }

    /// SPEC N5：混合态拒绝重暂存；干净态（与基线一致）允许。
    #[test]
    fn mixed_state_is_refused_by_restage_admission() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();

        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline_id = binding_of(&store, &gen_id);
        let lock = store.acquire_lock().unwrap();
        store
            .commit_active_cas(&lock, &gen_id, Some(&baseline_id))
            .unwrap();
        drop(lock);

        // Partial replacement: core matches neither the active nor the baseline.
        std::fs::write(&ctx.paths.core_binary, b"core-partial").unwrap();
        let error =
            prepare_system_generation(&ctx, b"core-v3", b"daemon-v1", Vec::new()).unwrap_err();
        assert!(
            format!("{error:#}").contains("混合态"),
            "mixed state must be refused: {error:#}"
        );

        // Clean state (installed == baseline) is admitted again.
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        assert!(prepare_system_generation(&ctx, b"core-v3", b"daemon-v1", Vec::new()).is_ok());
    }

    /// SPEC D/P6：双 writer CAS —— 过期期望 ID 与被抢占的 pending 都被拒绝。
    #[test]
    fn cas_commit_rejects_stale_and_preempted_writers() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();

        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline_id = binding_of(&store, &gen_id);
        let stale = generation::GenerationId::new("gen-stale-snapshot").unwrap();

        let lock = store.acquire_lock().unwrap();
        match store.commit_active_cas(&lock, &stale, Some(&baseline_id)) {
            Err(generation::GenerationError::GenerationMismatch { .. }) => {}
            other => panic!("stale writer must be rejected, got {other:?}"),
        }
        let committed = store
            .commit_active_cas(&lock, &gen_id, Some(&baseline_id))
            .unwrap();
        assert_eq!(committed.active, Some(gen_id.clone()));
        // A second writer holding the same (now stale) expectation is refused.
        match store.commit_active_cas(&lock, &gen_id, Some(&baseline_id)) {
            Err(generation::GenerationError::NoPendingGeneration) => {}
            other => panic!("preempted writer must be rejected, got {other:?}"),
        }
    }

    /// SPEC P4：daemon-only 升级时 core 内容相等 → 不换 core inode。
    #[cfg(unix)]
    #[test]
    fn daemon_only_upgrade_keeps_the_core_inode() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        let core_inode_before = std::fs::metadata(&ctx.paths.core_binary).unwrap().ino();

        let gen_id = prepare_system_generation(&ctx, b"core-v1", b"daemon-v2", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline_id = binding_of(&store, &gen_id);
        let manifest = store.validate_generation(&gen_id).unwrap();
        let baseline_manifest = store.validate_generation(&baseline_id).unwrap();
        let targets = build_artifact_targets(&ctx, &store, &gen_id, &manifest).unwrap();
        let plan = ApplyPlan {
            pending_id: gen_id,
            manifest,
            baseline_id,
            baseline_manifest,
            targets,
        };

        let change = change_set(&plan).unwrap();
        assert!(change.daemon, "daemon bytes changed");
        assert!(!change.core, "equal core bytes must not be rewritten");
        assert!(change.extras.is_empty());

        let replaced = replace_artifacts(&plan, &change).unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].relative_path, "mihomo-cli");
        assert!(replaced[0].had_pre_image);
        assert_eq!(std::fs::read(&ctx.paths.cli_binary).unwrap(), b"daemon-v2");
        assert_eq!(
            std::fs::metadata(&ctx.paths.core_binary).unwrap().ino(),
            core_inode_before,
            "core inode must be preserved on a daemon-only upgrade"
        );
    }

    /// SPEC C：补偿逐 artifact 从基线换回，基线声明为缺失的 artifact 以
    /// no-replace 改名移出（不删除），覆盖「升级新增 artifact」的缺失态恢复。
    #[cfg(unix)]
    #[test]
    fn compensation_restores_baseline_and_moves_aside_artifacts_that_never_existed() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        // No core and no geo data installed before the upgrade.
        let state_dir = ctx.paths.tun_config_file.parent().unwrap().to_path_buf();

        let extras = vec![(
            "GeoSite.dat".to_string(),
            b"geo-v2".to_vec(),
            Some(0o644u32),
        )];
        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", extras).unwrap();
        let store = system_generation_store(&ctx);
        let baseline_id = binding_of(&store, &gen_id);
        let manifest = store.validate_generation(&gen_id).unwrap();
        let baseline_manifest = store.validate_generation(&baseline_id).unwrap();
        assert!(manifest_missing_set(&baseline_manifest).contains("mihomo"));
        assert!(manifest_missing_set(&baseline_manifest).contains("GeoSite.dat"));

        let targets = build_artifact_targets(&ctx, &store, &gen_id, &manifest).unwrap();
        let plan = ApplyPlan {
            pending_id: gen_id.clone(),
            manifest,
            baseline_id: baseline_id.clone(),
            baseline_manifest: baseline_manifest.clone(),
            targets,
        };

        // Apply (P4) then compensate the artifacts.
        replace_artifacts(&plan, &change_set(&plan).unwrap()).unwrap();
        assert!(ctx.paths.core_binary.exists());
        assert!(state_dir.join("GeoSite.dat").exists());

        let baseline_targets =
            build_artifact_targets(&ctx, &store, &baseline_id, &baseline_manifest).unwrap();
        let missing = manifest_missing_set(&baseline_manifest);
        restore_baseline_artifacts(&baseline_targets, &missing).unwrap();

        assert_eq!(std::fs::read(&ctx.paths.cli_binary).unwrap(), b"daemon-v1");
        assert!(!ctx.paths.core_binary.exists(), "core did not exist before");
        assert!(ctx
            .paths
            .core_binary
            .with_file_name("mihomo.rollback-removed-00")
            .exists());
        assert!(!state_dir.join("GeoSite.dat").exists());
        assert!(state_dir.join("GeoSite.dat.rollback-removed-00").exists());
    }

    /// SPEC P1-1/P2：迁移来的 pending/绑定未经认证 → 事实不明 → 零替换。
    #[tokio::test]
    async fn migrated_pending_is_not_trusted_by_binding_verification() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();

        let current_cli = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let gen_id = prepare_system_generation(&ctx, b"core-v2", &current_cli, Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline_id = binding_of(&store, &gen_id);
        // Simulate the legacy migration marker: migrated bytes confer no trust.
        store.mark_generation_untrusted(&gen_id).unwrap();

        let error = apply_pending_generation(&ctx).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("未经认证"),
            "migrated pending must be rejected: {error:#}"
        );
        assert_eq!(std::fs::read(&ctx.paths.core_binary).unwrap(), b"core-v1");
        assert_eq!(store.read_state().unwrap().pending, Some(gen_id));
        assert!(!store.is_generation_untrusted(&baseline_id));
    }

    /// SPEC P6：提交后崩溃（pending 已清空）→ 重入走普通 restart，重跑管线
    /// 不改槽位、不报成功。
    #[tokio::test]
    async fn post_commit_crash_reentry_is_a_no_op() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();

        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline_id = binding_of(&store, &gen_id);
        let lock = store.acquire_lock().unwrap();
        store
            .commit_active_cas(&lock, &gen_id, Some(&baseline_id))
            .unwrap();
        drop(lock);

        // New process re-enters the pipeline after the commit crashed.
        assert!(!apply_pending_generation(&ctx).await.unwrap());
        let state = store.read_state().unwrap();
        assert_eq!(state.active, Some(gen_id));
        assert_eq!(state.pending, None);
        assert_eq!(state.previous, Some(baseline_id));
    }

    /// SPEC N3/C：同 sha 的墓碑机械阻断 skew helper 自动重暂存（不靠文案）。
    #[test]
    fn tombstone_gate_blocks_skew_helper_auto_restage() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        let current_cli = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        // Installed service binary is skewed relative to the running CLI.
        std::fs::write(&ctx.paths.cli_binary, b"older-daemon").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        assert!(is_system_cli_skewed(&ctx));

        // A withdrawn generation shipped the very daemon revision we run now.
        let gen_id = prepare_system_generation(&ctx, b"core-v1", &current_cli, Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        store.withdraw_generation(&gen_id).unwrap();
        let mut state = store.read_state().unwrap();
        state.pending = None;
        state.previous = None;
        store.write_state(&state).unwrap();

        let error = maybe_prepare_cli_skew_generation(&ctx).unwrap_err();
        assert!(
            format!("{error:#}").contains("墓碑"),
            "tombstone must mechanically block auto re-staging: {error:#}"
        );
        assert!(store.read_state().unwrap().pending.is_none());
        // The tombstone manifest stays readable after the rename.
        assert_eq!(
            store
                .read_withdrawn_manifest(&gen_id)
                .unwrap()
                .daemon
                .sha256,
            generation::calculate_sha256_bytes(&current_cli)
        );
    }

    /// SPEC N3：显式重试成功（提交同一 daemon 版本）后退休对应墓碑。
    #[test]
    fn successful_retry_retires_matching_tombstone() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();

        let withdrawn_id =
            prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        store.withdraw_generation(&withdrawn_id).unwrap();
        // A withdrawn pending is consumed by the discard/compensation terminal.
        store.clear_pending().unwrap();

        // A different daemon revision is a different upgrade source: the
        // tombstone must survive (核心字节属于失败候选，不参与同源判定).
        let other_id =
            prepare_system_generation(&ctx, b"core-v2", b"daemon-v2", Vec::new()).unwrap();
        let other_baseline = binding_of(&store, &other_id);
        let other_manifest = store.validate_generation(&other_id).unwrap();
        let other_baseline_manifest = store.validate_generation(&other_baseline).unwrap();
        let other_targets =
            build_artifact_targets(&ctx, &store, &other_id, &other_manifest).unwrap();
        retire_successful_tombstones(
            &store,
            &ApplyPlan {
                pending_id: other_id,
                manifest: other_manifest,
                baseline_id: other_baseline,
                baseline_manifest: other_baseline_manifest,
                targets: other_targets,
            },
        );
        assert!(store.has_withdrawn(&withdrawn_id));
        // A pending generation is never silently overwritten (裁决 A1): clear the
        // intermediate candidate before staging the retry.
        store.clear_pending().unwrap();

        // Same daemon revision as the withdrawn generation → the retry retires it.
        let retry_id =
            prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let baseline_id = binding_of(&store, &retry_id);
        let manifest = store.validate_generation(&retry_id).unwrap();
        let baseline_manifest = store.validate_generation(&baseline_id).unwrap();
        let targets = build_artifact_targets(&ctx, &store, &retry_id, &manifest).unwrap();
        let plan = ApplyPlan {
            pending_id: retry_id,
            manifest,
            baseline_id,
            baseline_manifest,
            targets,
        };

        assert!(store.has_withdrawn(&withdrawn_id));
        retire_successful_tombstones(&store, &plan);
        assert!(!store.has_withdrawn(&withdrawn_id));
    }

    /// SPEC P5/P0-1：启动因果必须是三态判定——「receipt 缺失」属不可证，
    /// 必须补一次重启再验，绝不允许当作满足（daemon reattach 旧 core 反例）。
    #[test]
    fn unknown_start_receipt_is_never_satisfied() {
        let frozen = FrozenConfig {
            content: "config-bytes".to_string(),
            revision: "rev-frozen".to_string(),
            subscription_id: None,
            ownership_proven: true,
        };
        let status = |running: bool, launched: Option<&str>| DaemonStatusView {
            running,
            launched_config_revision: launched.map(str::to_string),
        };

        assert_eq!(
            start_causality(Some(&frozen), &status(false, None)),
            StartCausality::NeedsStart
        );
        // 旧 core 被 daemon reattach：running=true 但没有任何 receipt。
        match start_causality(Some(&frozen), &status(true, None)) {
            StartCausality::NeedsRestart(reason) => {
                assert!(reason.contains("receipt"), "{reason}")
            }
            other => panic!("unknown receipt must force a restart, got {other:?}"),
        }
        match start_causality(Some(&frozen), &status(true, Some("rev-other"))) {
            StartCausality::NeedsRestart(_) => {}
            other => panic!("stale snapshot must force a restart, got {other:?}"),
        }
        assert_eq!(
            start_causality(Some(&frozen), &status(true, Some("rev-frozen"))),
            StartCausality::Satisfied
        );
        // No managed config ⇒ no start causality to bind (documented degrade path).
        assert_eq!(
            start_causality(None, &status(true, None)),
            StartCausality::Satisfied
        );
    }

    /// SPEC P1（冻结判据）：pending daemon 只认「= sha256(current_exe)」，
    /// 已安装 daemon 相同不再放行（否则可应用一个不是当前进程的 daemon）。
    #[tokio::test]
    async fn pending_daemon_must_match_the_running_executable() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-installed").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        // Pending daemon == installed daemon, but NOT the running executable.
        let gen_id =
            prepare_system_generation(&ctx, b"core-v2", b"daemon-installed", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);

        let error = apply_pending_generation(&ctx).await.unwrap_err();
        assert!(
            format!("{error:#}").contains("当前运行的可执行文件"),
            "P1 must compare against current_exe only: {error:#}"
        );
        assert_eq!(std::fs::read(&ctx.paths.core_binary).unwrap(), b"core-v1");
        assert_eq!(store.read_state().unwrap().pending, Some(gen_id));
    }

    /// SPEC A/P2-1：完全缺失的安装也能捕获基线（全 missing 基线），
    /// 从而不会产生「无绑定 pending 永不可应用 + 指引死循环」。
    #[test]
    fn all_missing_install_still_yields_a_bound_baseline() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        // Deliberately no installed daemon/core on disk.
        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v2", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let state = store.read_state().unwrap();
        let manifest = store.validate_generation(&gen_id).unwrap();
        let baseline_id = generation::GenerationId::new(
            manifest
                .metadata
                .get("baseline")
                .expect("staging always records a baseline binding"),
        )
        .unwrap();
        assert_eq!(state.previous, Some(baseline_id.clone()));

        let baseline = store.validate_generation(&baseline_id).unwrap();
        assert!(baseline.is_baseline());
        let missing = manifest_missing_set(&baseline);
        assert!(missing.contains(&baseline.daemon.relative_path));
        assert!(missing.contains(&baseline.core.relative_path));
        // The binding is accepted by P2 — no deadlock for a first install.
        assert!(verify_baseline_binding(&store, &manifest, &state, &gen_id).is_ok());
    }

    // ── 子进程假 daemon 夹具：驱动**真实**管线（非纯函数矩阵） ──────────
    #[cfg(unix)]
    const FAKE_DAEMON_ROOT_ENV: &str = "MIHOMO036_FAKE_DAEMON_ROOT";
    #[cfg(unix)]
    const FAKE_DAEMON_SCENARIO_ENV: &str = "MIHOMO036_FAKE_DAEMON_SCENARIO";

    #[cfg(unix)]
    #[derive(Default)]
    struct FakeDaemonCounters {
        restarts: std::sync::atomic::AtomicUsize,
        starts: std::sync::atomic::AtomicUsize,
    }

    #[cfg(unix)]
    async fn serve_fake_daemon(
        listener: tokio::net::UnixListener,
        daemon_sha: String,
        launched_revision: Option<String>,
        counters: std::sync::Arc<FakeDaemonCounters>,
    ) {
        serve_fake_daemon_logged(
            listener,
            daemon_sha,
            launched_revision,
            counters,
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            false,
        )
        .await
    }

    #[cfg(unix)]
    async fn serve_fake_daemon_logged(
        listener: tokio::net::UnixListener,
        daemon_sha: String,
        launched_revision: Option<String>,
        counters: std::sync::Arc<FakeDaemonCounters>,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        fail_records: bool,
    ) {
        use std::sync::atomic::Ordering;
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let sha = daemon_sha.clone();
            let launched = launched_revision.clone();
            let counters = counters.clone();
            let log = std::sync::Arc::clone(&log);
            tokio::spawn(async move {
                let mut reader = tokio::io::BufReader::new(stream);
                let Ok(command) =
                    ipc::read_json_message::<_, ipc::DaemonCommand>(&mut reader, "daemon command")
                        .await
                else {
                    return;
                };
                let label = match &command {
                    ipc::DaemonCommand::GetStatus { .. } => "GetStatus".to_string(),
                    ipc::DaemonCommand::RestartCore { .. } => "RestartCore".to_string(),
                    ipc::DaemonCommand::StartCore {
                        subscription_id,
                        config_revision,
                        ..
                    } => format!(
                        "StartCore subscription={subscription_id:?} revision={}",
                        config_revision.chars().take(16).collect::<String>()
                    ),
                    ipc::DaemonCommand::RecordActiveSubscription {
                        subscription_id,
                        config_revision,
                        ..
                    } => format!(
                        "RecordActiveSubscription subscription={subscription_id} revision={:?}",
                        config_revision
                            .as_deref()
                            .map(|rev| rev.chars().take(16).collect::<String>())
                    ),
                    ipc::DaemonCommand::RecordSelectionIntent {
                        subscription_id,
                        config_revision,
                        ..
                    } => format!(
                        "RecordSelectionIntent subscription={subscription_id} revision={:?}",
                        config_revision
                            .as_deref()
                            .map(|rev| rev.chars().take(16).collect::<String>())
                    ),
                    ipc::DaemonCommand::StopCore { .. } => "StopCore".to_string(),
                    ipc::DaemonCommand::CoreApiRequest { .. } => "CoreApiRequest".to_string(),
                    other => format!("{other:?}"),
                };
                log.lock().unwrap().push(label);
                let response = match command {
                    ipc::DaemonCommand::GetStatus { .. } => ipc::DaemonResponse::Status {
                        running: true,
                        core_pid: Some(std::process::id()),
                        config_path: None,
                        tun_snapshot_revision: None,
                        launched_config_revision: launched,
                        autostart_enabled: false,
                        daemon_executable_revision: Some(sha),
                        tun_journal_state: None,
                        tun_journal_error: None,
                    },
                    ipc::DaemonCommand::RestartCore { .. } => {
                        counters.restarts.fetch_add(1, Ordering::SeqCst);
                        ipc::DaemonResponse::Success {
                            message: String::new(),
                        }
                    }
                    ipc::DaemonCommand::StartCore { .. } => {
                        counters.starts.fetch_add(1, Ordering::SeqCst);
                        ipc::DaemonResponse::Success {
                            message: String::new(),
                        }
                    }
                    ipc::DaemonCommand::RecordActiveSubscription { .. }
                    | ipc::DaemonCommand::RecordSelectionIntent { .. }
                        if fail_records =>
                    {
                        ipc::DaemonResponse::Error {
                            message: "mirror channel rejected the record".to_string(),
                        }
                    }
                    ipc::DaemonCommand::StopCore { .. }
                    | ipc::DaemonCommand::RecordActiveSubscription { .. }
                    | ipc::DaemonCommand::RecordSelectionIntent { .. } => {
                        ipc::DaemonResponse::Success {
                            message: String::new(),
                        }
                    }
                    ipc::DaemonCommand::CoreApiRequest { .. } => ipc::DaemonResponse::CoreApi {
                        data: serde_json::json!({}),
                    },
                    other => ipc::DaemonResponse::Error {
                        message: format!("fake daemon does not support {other:?}"),
                    },
                };
                let mut writer = reader.into_inner();
                let _ = ipc::write_json_message(&mut writer, &response).await;
            });
        }
    }

    #[cfg(unix)]
    fn pipeline_plan(
        ctx: &instance::InstanceContext,
        store: &generation::GenerationStore,
        id: &generation::GenerationId,
    ) -> ApplyPlan {
        let manifest = store.validate_generation(id).unwrap();
        let baseline_id =
            generation::GenerationId::new(manifest.metadata.get("baseline").unwrap()).unwrap();
        let baseline_manifest = store.validate_generation(&baseline_id).unwrap();
        let targets = build_artifact_targets(ctx, store, id, &manifest).unwrap();
        ApplyPlan {
            pending_id: id.clone(),
            manifest,
            baseline_id,
            baseline_manifest,
            targets,
        }
    }

    /// 子进程内运行：真实 `start_and_prove` / `apply_pending_generation` 对假 daemon。
    #[cfg(unix)]
    #[tokio::test]
    async fn child_drives_the_apply_pipeline_against_a_fake_daemon() {
        let Ok(root) = std::env::var(FAKE_DAEMON_ROOT_ENV) else {
            return;
        };
        let scenario = std::env::var(FAKE_DAEMON_SCENARIO_ENV).unwrap_or_default();
        let socket_path: std::path::PathBuf = std::env::var_os(ipc::TEST_DAEMON_SOCKET_ENV)
            .map(std::path::PathBuf::from)
            .expect("parent must pass the fake daemon socket path");
        let ctx = system_ctx(std::path::Path::new(&root));

        if scenario == "reentry" {
            // The parent left durable state: pending P + tombstone P.withdrawn.
            let store = system_generation_store(&ctx);
            let state = store.read_state().unwrap();
            let pending = state
                .pending
                .clone()
                .expect("parent staged a pending generation");
            assert!(
                store.has_withdrawn(&pending),
                "parent published the tombstone"
            );
            let baseline_id = state.previous.clone().expect("baseline recorded");
            let baseline_manifest = store.validate_generation(&baseline_id).unwrap();
            let socket_path_parent = socket_path.clone();
            let listener = tokio::net::UnixListener::bind(&socket_path_parent).unwrap();
            let counters = std::sync::Arc::new(FakeDaemonCounters::default());
            tokio::spawn(serve_fake_daemon(
                listener,
                baseline_manifest.daemon.sha256.clone(),
                None,
                counters.clone(),
            ));
            assert!(apply_pending_generation(&ctx).await.unwrap());
            let state = store.read_state().unwrap();
            // SPEC 裁决 A1: a tombstone without the rollback-proof marker is
            // an explicit discard → metadata-only revoked terminal state.
            assert_eq!(state.active, None, "revoked state must not claim a runtime");
            assert_eq!(state.pending, None);
            assert_eq!(state.previous, Some(baseline_id));
            assert!(store.has_withdrawn(&pending), "tombstone is never deleted");
            return;
        }

        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::write(
            &ctx.paths.intent_config_file,
            format!(
                "mixed-port: 7890\n{}\n",
                ctx.paths.api_endpoint.controller_line()
            ),
        )
        .unwrap();

        let store = system_generation_store(&ctx);
        let prepared = store.read_state().ok().and_then(|state| state.pending);
        let gen_id = match prepared {
            // A parent test already staged this generation: materialise the
            // artifacts so P4 has nothing to replace (no downtime in the child).
            Some(existing) => {
                let manifest = store.validate_generation(&existing).unwrap();
                for target in build_artifact_targets(&ctx, &store, &existing, &manifest).unwrap() {
                    if let Ok(bytes) = std::fs::read(&target.source) {
                        std::fs::write(&target.dest, bytes).unwrap();
                    }
                }
                existing
            }
            None => {
                let cli_bytes = std::fs::read(std::env::current_exe().unwrap()).unwrap();
                let core_bytes = b"core-v1".to_vec();
                std::fs::write(&ctx.paths.cli_binary, &cli_bytes).unwrap();
                std::fs::write(&ctx.paths.core_binary, &core_bytes).unwrap();
                prepare_system_generation(&ctx, &core_bytes, &cli_bytes, Vec::new()).unwrap()
            }
        };
        let manifest = store.validate_generation(&gen_id).unwrap();
        let frozen = freeze_config_triple(&ctx).unwrap().expect("config exists");

        if scenario == "mirror_push_error" {
            let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
            config::set_active_id_at(&paths, "sub-aaaaaaaa").unwrap();
            let selection = paths.selection_state_path_for_subscription("sub-aaaaaaaa");
            if let Some(parent) = selection.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&selection, "selections:\n  G: N\n").unwrap();
            let frozen = freeze_config_triple(&ctx).unwrap().expect("config exists");
            let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
            tokio::spawn(serve_fake_daemon_logged(
                listener,
                "sha".to_string(),
                None,
                std::sync::Arc::new(FakeDaemonCounters::default()),
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                true,
            ));
            // Business Error from the mirror channel = channel unavailable.
            let error = push_mirror_strict(&ctx, &frozen).await.unwrap_err();
            assert!(
                format!("{error:#}").contains("被拒绝"),
                "record Error must be treated as an unavailable channel: {error:#}"
            );
            return;
        }

        if scenario == "cas_child" {
            // Cross-process CAS: commit first, then let the parent lose.
            let baseline =
                generation::GenerationId::new(manifest.metadata.get("baseline").unwrap()).unwrap();
            let lock = store.acquire_lock().unwrap();
            store
                .commit_active_cas(&lock, &gen_id, Some(&baseline))
                .expect("child must win the commit race");
            return;
        }

        if scenario == "mirror_order" || scenario == "request_binding" {
            let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
            config::set_active_id_at(&paths, "sub-aaaaaaaa").unwrap();
            let selection = paths.selection_state_path_for_subscription("sub-aaaaaaaa");
            if let Some(parent) = selection.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&selection, "selections:\n  G: N\n").unwrap();
            let frozen = freeze_config_triple(&ctx).unwrap().expect("config exists");
            let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
            // Receipt missing → the pipeline must push the mirror first and only
            // then restart the managed Core.
            tokio::spawn(serve_fake_daemon_logged(
                listener,
                manifest.daemon.sha256.clone(),
                None,
                std::sync::Arc::new(FakeDaemonCounters::default()),
                std::sync::Arc::clone(&log),
                false,
            ));
            if scenario == "request_binding" {
                // The active pointer moves to B after the triple was frozen: the
                // request must stay bound to the frozen identity A.
                config::set_active_id_at(&paths, "sub-bbbbbbbb").unwrap();
                let _ = request_core_start(&ctx, "binding", Some(&frozen)).await;
                let entries = log.lock().unwrap().clone();
                let start = entries
                    .iter()
                    .find(|entry| entry.starts_with("StartCore "))
                    .unwrap_or_else(|| panic!("no StartCore request: {entries:?}"));
                assert!(
                    start.contains("subscription=Some(\"sub-aaaaaaaa\")"),
                    "request must bind the frozen subscription: {start}"
                );
                assert!(
                    start.contains(&frozen.revision.chars().take(16).collect::<String>()),
                    "request must carry the frozen revision: {start}"
                );
                return;
            }
            let plan = pipeline_plan(&ctx, &store, &gen_id);
            let _ = run_apply_pipeline(&ctx, &plan, Some(&frozen)).await;
            let entries = log.lock().unwrap().clone();
            let position = |prefix: &str| {
                entries
                    .iter()
                    .position(|entry| entry.starts_with(prefix))
                    .unwrap_or_else(|| panic!("{prefix} missing: {entries:?}"))
            };
            let active = position("RecordActiveSubscription ");
            let intent = position("RecordSelectionIntent ");
            let restart = position("RestartCore");
            assert!(
                active < restart && intent < restart,
                "mirror push must precede the Core start: {entries:?}"
            );
            return;
        }

        let launched = if scenario == "receipt_missing" {
            None
        } else {
            Some(frozen.revision.clone())
        };
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let counters = std::sync::Arc::new(FakeDaemonCounters::default());
        tokio::spawn(serve_fake_daemon(
            listener,
            manifest.daemon.sha256.clone(),
            launched,
            counters.clone(),
        ));

        if scenario == "receipt_missing" {
            // D6-①: receipt missing → one restart → still missing → never success.
            let plan = pipeline_plan(&ctx, &store, &gen_id);
            let mut degraded = Vec::new();
            let error = start_and_prove(&ctx, &plan, Some(&frozen), true, None, &mut degraded)
                .await
                .unwrap_err();
            assert!(
                format!("{error:#}").contains("启动因果不可证"),
                "unknown receipt must never be reported as success: {error:#}"
            );
            assert_eq!(
                counters.restarts.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "exactly one managed restart before re-verification"
            );
            assert_eq!(
                counters.starts.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "a running core must be restarted, not started"
            );
            return;
        }

        // C-9: the real apply pipeline end-to-end (no downtime) commits the slots.
        assert!(apply_pending_generation(&ctx).await.unwrap());
        let state = store.read_state().unwrap();
        assert_eq!(state.active, Some(gen_id.clone()));
        assert_eq!(state.pending, None);
        assert_eq!(
            state.previous.as_ref().map(|id| id.as_str().to_string()),
            Some(frozen.revision.clone()).map(|_| {
                generation::GenerationId::new(
                    store
                        .read_manifest(&gen_id)
                        .unwrap()
                        .metadata
                        .get("baseline")
                        .unwrap(),
                )
                .unwrap()
                .as_str()
                .to_string()
            })
        );
        assert_eq!(
            counters.restarts.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a proven receipt must not restart the managed core"
        );
    }

    #[cfg(unix)]
    pub(super) fn run_pipeline_child_in(
        root: &std::path::Path,
        scenario: &str,
    ) -> std::process::Output {
        let socket_path = root.join("fake-daemon.sock");
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::gen036_pipeline_tests::child_drives_the_apply_pipeline_against_a_fake_daemon",
                "--nocapture",
            ])
            .env(FAKE_DAEMON_ROOT_ENV, root)
            .env(FAKE_DAEMON_SCENARIO_ENV, scenario)
            .env(ipc::TEST_DAEMON_SOCKET_ENV, &socket_path)
            .output()
            .unwrap()
    }

    #[cfg(unix)]
    fn run_pipeline_child(scenario: &str) -> std::process::Output {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("fake-daemon.sock");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::gen036_pipeline_tests::child_drives_the_apply_pipeline_against_a_fake_daemon",
                "--nocapture",
            ])
            .env(FAKE_DAEMON_ROOT_ENV, temp.path())
            .env(FAKE_DAEMON_SCENARIO_ENV, scenario)
            .env(ipc::TEST_DAEMON_SOCKET_ENV, &socket_path)
            .output()
            .unwrap();
        // Keep the temp root alive until the child exits, then drop it.
        drop(socket_path);
        drop(temp);
        output
    }

    /// D6-①：子进程真实驱动 `start_and_prove`——receipt 恒缺 → 单次重启 → 不得报成功。
    #[cfg(unix)]
    #[test]
    fn pipeline_never_reports_success_on_an_unprovable_start_receipt() {
        let output = run_pipeline_child("receipt_missing");
        assert!(
            output.status.success(),
            "child assertions failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// C-9：子进程真实驱动 `apply_pending_generation`（非直调 CAS）完成提交。
    #[cfg(unix)]
    #[test]
    fn apply_pipeline_end_to_end_commits_the_generation() {
        let output = run_pipeline_child("happy");
        assert!(
            output.status.success(),
            "child assertions failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// 镜像桩①：Record* 返回业务 Error = 通道不可用（不得当成推送成功）。
    #[cfg(unix)]
    #[test]
    fn mirror_push_treats_business_error_as_unavailable_channel() {
        let output = run_pipeline_child("mirror_push_error");
        assert!(
            output.status.success(),
            "child assertions failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// 镜像桩②：旧 daemon 先推时序——镜像推送严格早于 Core 启动/重启请求。
    #[cfg(unix)]
    #[test]
    fn mirror_push_precedes_core_start() {
        let output = run_pipeline_child("mirror_order");
        assert!(
            output.status.success(),
            "child assertions failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// 镜像桩③：请求 ID 绑定——冻结后 active 指针切到 B，请求仍绑定冻结身份 A。
    #[cfg(unix)]
    #[test]
    fn request_identity_is_frozen_across_active_pointer_changes() {
        let output = run_pipeline_child("request_binding");
        assert!(
            output.status.success(),
            "child assertions failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// C-11：跨进程真双 writer CAS——先提交者胜，后者的期望 ID 已被消费。
    #[cfg(unix)]
    #[test]
    fn cross_process_cas_rejects_the_loser() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket_path = temp.path().join("fake-daemon.sock");
        let root = temp.path().to_path_buf();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::gen036_pipeline_tests::child_drives_the_apply_pipeline_against_a_fake_daemon",
                "--nocapture",
            ])
            .env(FAKE_DAEMON_ROOT_ENV, &root)
            .env(FAKE_DAEMON_SCENARIO_ENV, "cas_child")
            .env(ipc::TEST_DAEMON_SOCKET_ENV, &socket_path)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "child (winner) failed:\n{}",
            String::from_utf8_lossy(&child.stderr)
        );

        let ctx = system_ctx(&root);
        let store = system_generation_store(&ctx);
        let state = store.read_state().unwrap();
        let active = state.active.expect("child committed");
        let lock = store.acquire_lock().unwrap();
        match store.commit_active_cas(&lock, &active, state.previous.as_ref()) {
            Err(generation::GenerationError::NoPendingGeneration) => {}
            other => panic!("the losing writer must be rejected, got {other:?}"),
        }
    }

    /// C-10：信任根/state/manifest 的权限合同——服务账号不可写（无账户也能验证模式）。
    #[cfg(unix)]
    #[test]
    fn generation_state_is_not_writable_by_the_service_identity() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline = binding_of(&store, &gen_id);

        let mut paths = vec![
            store.root_dir().to_path_buf(),
            store.generations_dir(),
            store.state_file_path(),
            store.manifest_path(&gen_id),
            store.manifest_path(&baseline),
        ];
        paths.push(store.generation_dir(&gen_id).join("mihomo-cli"));
        for path in paths {
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            let mode = metadata.permissions().mode() & 0o777;
            assert_eq!(
                mode & 0o022,
                0,
                "{} must not be group/other writable (mode {mode:#o})",
                path.display()
            );
        }
        // The trust root itself is owner-traversable only.
        let root_mode = std::fs::metadata(store.root_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(root_mode, 0o750, "trust root must be 0750");
    }

    /// C-12：锁序契约——generation 锁先于 config 锁，且无反向持锁路径。
    #[test]
    fn generation_lock_precedes_config_lock_and_never_reverses() {
        let source_raw = include_str!("main.rs");
        let source = source_raw.replace("\r\n", "\n");

        // Within the pipeline, the generation lock precedes the frozen config
        // snapshot (the only place that takes the config lock).
        let start = source
            .find("pub(crate) async fn apply_pending_generation(")
            .unwrap();
        let body_end = source[start..].find("\n}\n").unwrap() + start;
        let body = &source[start..body_end];
        let gen_lock = body.find("acquire_lock()").unwrap();
        let config_snapshot = body.find("freeze_config_triple(").unwrap();
        assert!(
            gen_lock < config_snapshot,
            "generation lock must be taken before the config snapshot"
        );

        // No function may hold the config lock while staging/applying generations.
        for function in split_functions(&source) {
            if function.contains("ConfigLock::acquire") {
                assert!(
                    !function.contains("prepare_system_generation(")
                        && !function.contains("apply_pending_generation(")
                        && !function.contains("acquire_lock()"),
                    "config-lock holder must not enter the generation pipeline (reverse lock order)"
                );
            }
        }
    }

    /// Splits `main.rs` source into top-level function bodies (brace depth 0).
    fn split_functions(source: &str) -> Vec<String> {
        let mut functions = Vec::new();
        let mut current: Option<String> = None;
        let mut depth = 0i32;
        for line in source.lines() {
            if current.is_none()
                && (line.starts_with("fn ")
                    || line.starts_with("async fn ")
                    || line.starts_with("pub fn ")
                    || line.starts_with("pub(crate) fn ")
                    || line.starts_with("pub(crate) async fn ")
                    || line.starts_with("pub async fn ")
                    || line.starts_with("async fn "))
            {
                current = Some(String::new());
            }
            if let Some(buffer) = current.as_mut() {
                buffer.push_str(line);
                buffer.push('\n');
                depth += line.matches('{').count() as i32;
                depth -= line.matches('}').count() as i32;
                if depth == 0 && buffer.contains('{') {
                    functions.push(buffer.clone());
                    current = None;
                }
            }
        }
        functions
    }

    /// 裁决 A1：显式撤销只动元数据——active=∅、pending=∅、previous=B（保留基线），
    /// 墓碑保留候选字节，安装文件一字未改、无停启。
    #[test]
    fn discard_pending_revokes_metadata_and_keeps_the_baseline() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline_id = binding_of(&store, &gen_id);

        let installed_before = (
            std::fs::read(&ctx.paths.cli_binary).unwrap(),
            std::fs::read(&ctx.paths.core_binary).unwrap(),
        );
        discard_pending_generation(&ctx, &gen_id).unwrap();

        let state = store.read_state().unwrap();
        assert_eq!(state.active, None);
        assert_eq!(state.pending, None);
        assert_eq!(state.previous, Some(baseline_id.clone()));
        assert!(
            store.has_withdrawn(&gen_id),
            "candidate bytes kept as evidence"
        );
        assert!(
            store.validate_generation(&baseline_id).is_ok(),
            "baseline intact"
        );
        assert!(
            !store.has_rollback_proof(&gen_id),
            "discard is not a rollback"
        );
        assert_eq!(
            installed_before,
            (
                std::fs::read(&ctx.paths.cli_binary).unwrap(),
                std::fs::read(&ctx.paths.core_binary).unwrap()
            ),
            "discard must not touch installed artifacts"
        );
    }

    /// 裁决 A1：撤销准入 fail-closed——目标身份不符、无绑定、绑定与 previous 不一致。
    #[test]
    fn discard_pending_is_fail_closed_on_insufficient_evidence() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);

        // Unknown target identity.
        let unknown = generation::GenerationId::new("gen-not-the-pending").unwrap();
        assert!(discard_pending_generation(&ctx, &unknown).is_err());

        // Binding mismatch: pending manifest no longer binds state.previous.
        let mut manifest = store.read_manifest(&gen_id).unwrap();
        manifest
            .metadata
            .insert("baseline".to_string(), "gen-other".to_string());
        store.write_manifest(&manifest).unwrap();
        assert!(discard_pending_generation(&ctx, &gen_id).is_err());
        // Nothing was revoked by the rejected attempts.
        let state = store.read_state().unwrap();
        assert_eq!(state.pending, Some(gen_id.clone()));
        assert!(!store.has_withdrawn(&gen_id));
    }

    /// 裁决 A1：普通 install（暂存）不得隐式丢弃 pending。
    #[test]
    fn staging_never_silently_discards_a_pending_generation() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);

        let error =
            prepare_system_generation(&ctx, b"core-v3", b"daemon-v1", Vec::new()).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("--discard-pending"), "{message}");
        assert_eq!(store.read_state().unwrap().pending, Some(gen_id));
    }

    /// 裁决 A1 恢复态：撤销后新候选 C 继续绑定已认证基线 B，不捕获混合安装态，
    /// 且暂存准入把「安装态 = 已撤销候选」视为已知现场（不是混合态）。
    #[test]
    fn restaging_after_discard_binds_the_authenticated_baseline() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        let first = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline = binding_of(&store, &first);
        discard_pending_generation(&ctx, &first).unwrap();

        // The install tree still carries the revoked candidate's bytes (candidate
        // == installed for daemon, so the mixed-scene guard must not fire).
        let candidate =
            prepare_system_generation(&ctx, b"core-v3", b"daemon-v1", Vec::new()).unwrap();
        let manifest = store.read_manifest(&candidate).unwrap();
        assert_eq!(
            manifest.metadata.get("baseline").map(String::as_str),
            Some(baseline.as_str()),
            "candidate C must bind the authenticated baseline B, not the mixed scene"
        );
        let state = store.read_state().unwrap();
        assert_eq!(state.previous, Some(baseline), "previous stays B");
        assert_eq!(state.pending, Some(candidate));
    }

    const CRASH_CHILD_ENV: &str = "MIHOMO036_CRASH_CHILD_ROOT";

    /// 只在崩溃子进程中真正运行：准备好 pending、发布墓碑后**真实退出进程**
    /// （abort，不走任何清理路径），模拟「墓碑已发布 / 槽位未消费」断点。
    #[test]
    fn crash_child_publishes_tombstone_then_aborts() {
        let Ok(root) = std::env::var(CRASH_CHILD_ENV) else {
            return;
        };
        let ctx = system_ctx(std::path::Path::new(&root));
        std::fs::write(&ctx.paths.cli_binary, b"daemon-v1").unwrap();
        std::fs::write(&ctx.paths.core_binary, b"core-v1").unwrap();
        let gen_id = prepare_system_generation(&ctx, b"core-v2", b"daemon-v1", Vec::new()).unwrap();
        let store = system_generation_store(&ctx);
        let baseline = store.read_state().unwrap().previous.unwrap();
        record_native_withdrawal(&store, &gen_id, &baseline).unwrap();
        store.withdraw_generation(&gen_id).unwrap();
        std::process::abort();
    }

    /// SPEC C/P6：真崩溃测试——子进程持锁期间真实退出，新进程重入后只做
    /// 槽位消费并收敛（不依赖任何进程内信号）。
    #[cfg(unix)]
    #[tokio::test]
    async fn crashed_process_state_converges_in_a_new_process() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let exe = std::env::current_exe().unwrap();
        let status = std::process::Command::new(exe)
            .args([
                "--exact",
                "tests::gen036_pipeline_tests::crash_child_publishes_tombstone_then_aborts",
                "--nocapture",
            ])
            .env(CRASH_CHILD_ENV, &root)
            .status()
            .unwrap();
        assert!(
            !status.success(),
            "the child process must die mid-pipeline: {status:?}"
        );

        // Durable state written by the dead process.
        let ctx = system_ctx(&root);
        let store = system_generation_store(&ctx);
        let state = store.read_state().unwrap();
        let pending = state
            .pending
            .clone()
            .expect("crash child staged a pending generation");
        let baseline = state.previous.clone().expect("baseline binding recorded");
        assert!(store.has_withdrawn(&pending));

        // A new process re-enters and converges without any in-process signal.
        drop(store);
        let output = run_pipeline_child_in(&root, "reentry");
        assert!(
            output.status.success(),
            "re-entry child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let store = system_generation_store(&ctx);
        let state = store.read_state().unwrap();
        assert_eq!(state.active, None);
        assert_eq!(state.pending, None);
        assert_eq!(state.previous, Some(baseline));
        assert!(store.has_withdrawn(&pending));
    }

    /// SPEC P1-1：信任根必须是显式 root:root 0750（非 umask 继承），
    /// 模式或祖先可写性漂移即拒绝。
    #[cfg(unix)]
    #[test]
    fn generation_trust_root_drift_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("generations");
        utils::ensure_generation_trust_root(&root).unwrap();
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o750,
            "trust root must not inherit the process umask"
        );
        assert!(utils::verify_generation_trust_root(&root).is_ok());

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(utils::verify_generation_trust_root(&root).is_err());
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(utils::verify_generation_trust_root(&root).is_ok());

        // A world-writable ancestor without the sticky bit invalidates the root.
        let loose_parent = temp.path().join("loose");
        std::fs::create_dir_all(&loose_parent).unwrap();
        std::fs::set_permissions(&loose_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let nested = loose_parent.join("generations");
        utils::ensure_dir_all_no_follow(&nested).unwrap();
        utils::set_directory_mode_no_follow(&nested, 0o750).unwrap();
        assert!(utils::verify_generation_trust_root(&nested).is_err());
        // Creating it through the contract refuses the loose ancestor outright.
        assert!(utils::ensure_generation_trust_root(&nested).is_err());
    }

    /// SPEC P0/P1-3：恢复入口**单点**留证——证据在任何恢复动作之前落盘，
    /// 且恢复本身失败也不影响证据与原文件。
    #[cfg(unix)]
    #[tokio::test]
    async fn evidence_is_preserved_before_any_recovery_action() {
        // With an installed service account the fixture would be PermissionDrift
        // (owner mismatch) and `needs_isolation()` is false, so nothing would be
        // preserved; that combination needs root to build, so skip it here.
        #[cfg(unix)]
        {
            let name = std::ffi::CString::new("mihomo").unwrap();
            if !unsafe { libc::getpwnam(name.as_ptr()) }.is_null() {
                return;
            }
        }
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        let state_dir = ctx.paths.tun_config_file.parent().unwrap().to_path_buf();
        let cache_db = state_dir.join(utils::CACHE_DB_FILE_NAME);
        std::fs::write(&cache_db, b"not-a-bbolt-database").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cache_db, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        // A legacy journal forces the recovery path past its "no journal" return.
        let legacy_journal = tun_transaction::legacy_journal_path(&ctx);
        if let Some(parent) = legacy_journal.parent() {
            utils::ensure_dir_all_no_follow(parent).unwrap();
        }
        std::fs::write(&legacy_journal, b"{not-a-journal").unwrap();

        let result = maybe_auto_recover_active_transaction(
            &ctx,
            tun_transaction::RecoveryDirection::Abort,
            false,
            false,
            false,
        )
        .await;
        assert!(
            result.is_err(),
            "an unreadable legacy journal must fail the recovery: {result:?}"
        );

        // Evidence exists even though the recovery aborted before taking any action.
        let evidence: Vec<_> = std::fs::read_dir(&state_dir)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(utils::is_quarantine_artifact_name)
            })
            .collect();
        assert_eq!(
            evidence.len(),
            1,
            "exactly one evidence copy, written before any recovery action"
        );
        assert_eq!(
            std::fs::read(evidence[0].path()).unwrap(),
            b"not-a-bbolt-database"
        );
        // Copy, not move; the journal evidence is untouched as well.
        assert_eq!(
            std::fs::read(state_dir.join(utils::CACHE_DB_FILE_NAME)).unwrap(),
            b"not-a-bbolt-database"
        );
        assert_eq!(std::fs::read(&legacy_journal).unwrap(), b"{not-a-journal");

        // The stop/uninstall direction funnels through the same single point:
        // a second recovery entry (Abort) must not create a second evidence copy.
        let again = maybe_auto_recover_active_transaction(
            &ctx,
            tun_transaction::RecoveryDirection::Resume,
            false,
            false,
            false,
        )
        .await;
        assert!(again.is_err());
        let evidence_after: Vec<_> = std::fs::read_dir(&state_dir)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(utils::is_quarantine_artifact_name)
            })
            .collect();
        assert_eq!(
            evidence_after.len(),
            1,
            "every recovery entry shares one idempotent preservation point"
        );
    }

    /// SPEC P0/P1-3：结构性断言——恢复入口里留证只出现一次，且必须先于
    /// recovery marker 与任何恢复动作（所有恢复入口共享该单点）。
    #[test]
    fn recovery_entry_preserves_evidence_before_marker_and_actions() {
        let source_raw = include_str!("main.rs");
        let source = source_raw.replace("\r\n", "\n");
        let start = source
            .find("pub(crate) async fn maybe_auto_recover_active_transaction(")
            .expect("recovery entry must exist");
        let body_end = source[start..].find("\n}\n").expect("entry must end") + start;
        let body = &source[start..body_end];

        assert_eq!(
            body.matches("preserve_upgrade_evidence(").count(),
            1,
            "the recovery entry must contain exactly one preservation point"
        );
        let preserve = body.find("preserve_upgrade_evidence(").unwrap();
        let marker = body
            .find("try_acquire_recovery_marker")
            .expect("entry must acquire the recovery marker");
        let first_action = body.find("execute_recovery_action").unwrap_or(usize::MAX);
        assert!(
            preserve < marker,
            "preservation must precede the recovery marker"
        );
        assert!(
            preserve < first_action,
            "preservation must precede every recovery action"
        );
    }

    /// SPEC P0：cache.db 证据保全 = 复制留证（不移动原件）、no-replace 命名、
    /// K=3 保留、未完成副本不用最终命名。
    #[cfg(unix)]
    #[test]
    fn cache_db_evidence_is_copied_aside_without_moving_and_retained_three() {
        let temp = tempfile::TempDir::new().unwrap();
        let cache_db = temp.path().join(utils::CACHE_DB_FILE_NAME);
        std::fs::write(&cache_db, b"broken-bbolt-bytes").unwrap();

        let mut preserved = Vec::new();
        for round in 0..5u8 {
            // Distinct content per round: identical bytes are de-duplicated (see
            // the idempotence assertion below) and would not exercise K=3.
            std::fs::write(&cache_db, [b"broken-bbolt".as_slice(), &[round]].concat()).unwrap();
            preserved.push(utils::preserve_cache_db_evidence(&cache_db).unwrap());
        }
        // The live file is untouched (copy, not move).
        assert_eq!(
            std::fs::read(&cache_db).unwrap(),
            [b"broken-bbolt".as_slice(), &[4u8]].concat()
        );

        for path in &preserved {
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(utils::is_quarantine_artifact_name(name), "{name}");
            assert!(!name.contains(".partial"), "{name}");
        }
        // Retention evicts the oldest artifacts only (K=3 survives).
        for (offset, path) in preserved
            .iter()
            .rev()
            .take(utils::CACHE_DB_QUARANTINE_RETENTION)
            .enumerate()
        {
            assert!(path.exists(), "{} must survive retention", path.display());
            let round = 4 - offset as u8;
            assert_eq!(
                std::fs::read(path).unwrap(),
                [b"broken-bbolt".as_slice(), &[round]].concat()
            );
        }
        // Idempotent: re-preserving identical bytes reuses the existing artifact
        // instead of burning the K=3 budget on a duplicate.
        let reused = utils::preserve_cache_db_evidence(&cache_db).unwrap();
        assert_eq!(reused, *preserved.last().unwrap());
        // K=3 retention, and no staged `.partial` leftovers.
        let quarantined = std::fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(utils::is_quarantine_artifact_name)
            })
            .count();
        assert_eq!(quarantined, utils::CACHE_DB_QUARANTINE_RETENTION);
        let partials = std::fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(".partial"))
            })
            .count();
        assert_eq!(partials, 0);
        // The newest copy survives eviction.
        assert!(preserved.last().unwrap().exists());
    }
}

/// wave 1：缺陷 1/2/3 的 CLI 侧验收（candidate 捕获、控制键告警、正常顺序对照）。
#[cfg(test)]
mod issue041_wave1_tests {
    use super::*;

    const INTENT_WITH_TUN: &str = "\
mode: rule
mixed-port: 7897
proxies: []
rules:
- MATCH,DIRECT
tun:
  enable: true
  stack: mixed
  auto-route: true
  strict-route: true
  dns-hijack:
  - any:53
  - tcp://any:53
  route-exclude-address:
  - 224.0.0.0/4
";

    fn system_ctx(temp: &std::path::Path) -> instance::InstanceContext {
        let inputs = instance::PathInputs {
            home: temp.join("home"),
            uid: Some(1000),
            gid: Some(1000),
            xdg_runtime_dir: Some(temp.join("run/user/1000")),
            program_data: temp.join("ProgramData"),
            app_data: temp.join("AppData/Roaming"),
            local_app_data: temp.join("AppData/Local"),
            username_or_sid: "alice".to_string(),
        };
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &inputs,
        );
        ctx.paths.config_dir = temp.join("user-config");
        ctx.paths.intent_config_file = ctx.paths.config_dir.join("config.yaml");
        ctx.paths.tun_config_file = temp.join("system-data/tun-config.yaml");
        ctx.permissions = instance::PermissionModel::DirectUser;
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        ctx
    }

    #[cfg(unix)]
    const TUN_PREPARE_ROOT_ENV: &str = "MIHOMO_TEST_TUN_PREPARE_ROOT";

    /// 在给定 root 上完整跑一次 `tun on` 的 prepare（捕获 + 发布同一临界区）。
    #[cfg(unix)]
    fn prepare_tun_transaction_in(root: &std::path::Path) -> anyhow::Result<()> {
        let ctx = system_ctx(root);
        std::fs::write(&ctx.paths.intent_config_file, INTENT_WITH_TUN)?;
        tun_transaction::prepare_and_publish_active_transaction_capturing_intent(
            &ctx,
            1000,
            true,
            &evidence(),
            |intent_bytes| build_tun_candidate(&ctx, intent_bytes, true, None, None),
        )?;
        Ok(())
    }

    fn evidence() -> tun_transaction::OldRuntimeEvidence {
        tun_transaction::OldRuntimeEvidence {
            core_running: true,
            core_identity: "core-1".to_string(),
            core_pid: 1234,
            launched_revision: "old-rev".to_string(),
            launch_source: tun_transaction::LaunchSource::SystemTunSnapshot,
            runtime_tun: true,
            api_endpoint: "unix".to_string(),
            recorded_at_secs: Some(100),
        }
    }

    /// `tun on` 的 prepare 必须先 reconcile override，再捕获 intent；
    /// 否则只写在 override 的非控制键不会进入 candidate/snapshot。
    #[test]
    fn issue060_tun_on_reconciles_override_before_candidate_capture() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.intent_config_file, INTENT_WITH_TUN).unwrap();
        let paths = utils::AppPaths::new(ctx.paths.config_dir.clone());
        std::fs::write(
            paths.override_path(),
            "tun:\n  route-exclude-address:\n  - 192.0.2.0/24\n  - 198.51.100.0/24\n",
        )
        .unwrap();

        let journal =
            tun_transaction::prepare_and_publish_active_transaction_capturing_intent_with_prepare(
                &ctx,
                1000,
                true,
                &evidence(),
                || {
                    let reconciled = config::reconcile_override_into_intent_checked(&paths, None)?;
                    assert!(reconciled.changed, "override must be reconciled");
                    Ok(())
                },
                |intent_bytes| build_tun_candidate(&ctx, intent_bytes, true, None, None),
            )
            .expect("tun on prepare must succeed");

        let candidate = std::fs::read_to_string(tun_transaction::active_candidate_path(&ctx))
            .expect("candidate must exist");
        let candidate: serde_yaml::Value = serde_yaml::from_str(&candidate).unwrap();
        assert_eq!(
            candidate["tun"]["route-exclude-address"],
            serde_yaml::from_str::<serde_yaml::Value>("- 192.0.2.0/24\n- 198.51.100.0/24\n")
                .unwrap()
        );
        let intent_revision = tun_transaction::sha256_revision(
            std::fs::read(&ctx.paths.intent_config_file)
                .unwrap()
                .as_slice(),
        );
        assert_eq!(
            journal.base_intent_revision.as_deref(),
            Some(intent_revision.as_str())
        );
    }

    /// 缺陷 2（裁决定稿）：override 里的 TUN 控制键必须**明确告警**（不能静默丢弃），
    /// 非控制键（route-exclude-address）不得误报。
    #[test]
    fn issue041_override_control_key_warning_is_explicit() {
        let control: serde_yaml::Value = serde_yaml::from_str(
            "tun:\n  enable: false\n  stack: system\n  dns-hijack:\n  - any:53\n",
        )
        .unwrap();
        let warning = tun_owned_override_keys_warning(&control).expect("control keys must warn");
        assert!(warning.contains("tun.enable"), "{warning}");
        assert!(warning.contains("tun.stack"), "{warning}");
        assert!(warning.contains("tun.dns-hijack"), "{warning}");
        assert!(warning.contains("tun on"), "{warning}");

        let non_control: serde_yaml::Value =
            serde_yaml::from_str("tun:\n  route-exclude-address:\n  - 224.0.0.0/4\n").unwrap();
        assert!(
            tun_owned_override_keys_warning(&non_control).is_none(),
            "non-control keys are not transaction-owned"
        );
    }

    /// 缺陷 1（裁决点 3）：candidate 必须由**调用方捕获的** intent 字节构建，
    /// 不得自行重新读盘（否则 base 与 candidate 可能来自不同快照）。
    #[test]
    fn issue041_tun_candidate_is_built_from_captured_bytes() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(
            &ctx.paths.intent_config_file,
            "mode: rule\nrules:\n- MATCH,DIRECT\n",
        )
        .unwrap();

        let captured = INTENT_WITH_TUN.as_bytes();
        let candidate = build_tun_candidate(&ctx, captured, true, None, None).unwrap();
        let doc: serde_yaml::Value = serde_yaml::from_slice(&candidate).unwrap();
        let tun = &doc["tun"];
        assert_eq!(tun["enable"], serde_yaml::Value::Bool(true));
        assert_eq!(
            tun["route-exclude-address"],
            serde_yaml::from_str::<serde_yaml::Value>("- 224.0.0.0/4").unwrap(),
            "candidate must derive from the captured bytes, not the on-disk intent"
        );
    }

    /// （P2-3）：`tun off` 正向用例——intent 带完整 `tun` 块时，candidate
    /// 只翻转 `enable: false`；提交不得被“丢 tun 块”防线误拒，且整张 map 逐键保留。
    #[test]
    fn issue050_tun_off_candidate_commits_with_the_whole_tun_map_preserved() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::create_dir_all(&ctx.paths.config_dir).unwrap();
        std::fs::write(&ctx.paths.intent_config_file, INTENT_WITH_TUN).unwrap();

        let journal = tun_transaction::prepare_and_publish_active_transaction_capturing_intent(
            &ctx,
            1000,
            false,
            &evidence(),
            |intent_bytes| build_tun_candidate(&ctx, intent_bytes, false, None, None),
        )
        .expect("tun off prepare must succeed against a complete tun intent");

        let result = tun_transaction::compare_and_commit_user_intent(&ctx, &journal)
            .expect("tun off commit must not be rejected as a tun-dropping candidate");
        assert_eq!(result, tun_transaction::IntentCommitResult::Committed);

        let committed: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap())
                .unwrap();
        assert_eq!(committed["tun"]["enable"], serde_yaml::Value::Bool(false));
        let expected_tun: serde_yaml::Value = serde_yaml::from_str(
            "\
enable: false
stack: mixed
auto-route: true
strict-route: true
dns-hijack:
- any:53
- tcp://any:53
route-exclude-address:
- 224.0.0.0/4
",
        )
        .unwrap();
        assert_eq!(
            committed["tun"], expected_tun,
            "tun off must only flip enable and keep the whole TUN map"
        );
        assert_eq!(
            committed["mixed-port"],
            serde_yaml::Value::Number(7897.into())
        );
        assert_eq!(
            committed["rules"],
            serde_yaml::from_str::<serde_yaml::Value>("- MATCH,DIRECT").unwrap()
        );
    }

    /// 取 `main.rs` 中某个函数的函数体（到第一个顶格 `}` 为止，与既有静态扫描同构）。
    fn function_body(source: &str, signature: &str) -> String {
        let normalized = source.replace("\r\n", "\n");
        let start = normalized
            .find(signature)
            .unwrap_or_else(|| panic!("signature not found: {signature}"));
        let body_end = normalized[start..]
            .find("\n}\n")
            .unwrap_or_else(|| panic!("function end not found for {signature}"))
            + start;
        normalized[start..body_end].to_string()
    }

    /// 缺陷 3 P1-1：owner 闸门必须与首次写入同处 ConfigLock 临界区
    /// （acquire → 闸门 → 首次写入），否则「闸门通过 → 并发 prepare 发布 journal →
    /// 写入」的 TOCTOU 窗口仍然存在。
    #[test]
    fn issue041_override_guard_runs_inside_config_lock_before_first_write() {
        let body = function_body(include_str!("main.rs"), "async fn apply_override_change(");
        let acquire = body
            .find("ConfigLock::acquire")
            .expect("apply_override_change must take the config lock");
        let guard = body
            .find("ensure_no_transaction_owner_before_config_write")
            .expect("apply_override_change must run the transaction-owner gate");
        let first_write = body
            .find("atomic_write_file_for_original_user")
            .or_else(|| body.find("remove_file_if_exists"))
            .expect("apply_override_change must write the override file");
        assert!(
            acquire < guard,
            "the owner gate must run under the config lock (acquire then gate)"
        );
        assert!(
            guard < first_write,
            "the owner gate must run before the first write of the override file"
        );
    }

    /// 缺陷 3 P1-1/P1-2：捕获→发布、以及提交重验，都必须处在 CoordinatorLock →
    /// ConfigLock 的同一临界区内（锁序与 CAS/journal 写方一致）。
    #[test]
    fn issue041_capture_commit_take_coordinator_before_config_lock() {
        let source = include_str!("tun_transaction.rs");
        let capture = function_body(
            source,
            "pub fn prepare_and_publish_active_transaction_capturing_intent_with_prepare<F, P>(",
        );
        let coordinator = capture
            .find("CoordinatorLock::acquire")
            .expect("capturing prepare must take the coordinator lock");
        let config = capture
            .find("ConfigLock::acquire")
            .expect("capturing prepare must take the config lock");
        let capture_read = capture
            .find("intent_config_file")
            .expect("capturing prepare must read the intent");
        let publish = capture
            .find("prepare_and_publish_active_locked(")
            .expect("capturing prepare must publish under the same span");
        assert!(
            coordinator < config,
            "lock order must be coordinator -> config"
        );
        assert!(
            config < capture_read && capture_read < publish,
            "intent capture and publish must share one critical section"
        );

        let commit = function_body(source, "pub fn compare_and_commit_user_intent(");
        let commit_coordinator = commit
            .find("CoordinatorLock::acquire")
            .expect("commit must take the coordinator lock");
        let commit_config = commit
            .find("ConfigLock::acquire")
            .expect("commit must take the config lock");
        let recheck = commit
            .find("read_active_journal")
            .expect("commit must re-verify the canonical journal");
        assert!(
            commit_coordinator < commit_config,
            "commit lock order must be coordinator -> config"
        );
        assert!(
            commit_config < recheck,
            "the canonical journal re-verification must run under both locks"
        );
    }

    /// 缺陷 3 P1-2：提交必须与 journal 写方（CoordinatorLock 持有者）串行——等待而不是
    /// 并发重验，且不引入死锁。
    #[test]
    fn issue041_commit_waits_for_the_journal_writer() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.intent_config_file, INTENT_WITH_TUN).unwrap();
        let mut journal = tun_transaction::prepare_and_publish_active_transaction_capturing_intent(
            &ctx,
            1000,
            true,
            &evidence(),
            |intent_bytes| build_tun_candidate(&ctx, intent_bytes, true, None, None),
        )
        .unwrap();
        for next in [
            tun_transaction::JournalPhase::PromotionPending,
            tun_transaction::JournalPhase::SnapshotPromoted,
            tun_transaction::JournalPhase::CoreApplied,
        ] {
            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: journal.phase,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            journal = tun_transaction::cas_update_phase(&ctx, &fence, next, None, None).unwrap();
        }

        let held = Arc::new(AtomicBool::new(false));
        let writer_ctx = ctx.clone();
        let writer_held = Arc::clone(&held);
        let writer = std::thread::spawn(move || {
            let _coordinator = tun_transaction::CoordinatorLock::acquire(&writer_ctx).unwrap();
            writer_held.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(150));
        });
        while !held.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }

        let started = Instant::now();
        let result = tun_transaction::compare_and_commit_user_intent(&ctx, &journal).unwrap();
        let waited = started.elapsed();
        writer.join().unwrap();

        assert_eq!(result, tun_transaction::IntentCommitResult::Committed);
        assert!(
            waited >= Duration::from_millis(120),
            "commit must serialize with the journal writer, waited only {waited:?}"
        );
    }

    /// 缺陷 3 P2-2：`base_intent_revision` 缺失时一律 Conflict——不得凭 legacy 身份
    /// 绕过「当前 intent 是否本事务前置」的证明。
    #[test]
    fn issue041_legacy_without_base_is_never_committed() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        let candidate = "mode: rule\ntun:\n  enable: true\nrules:\n- MATCH,DIRECT\n";
        std::fs::create_dir_all(tun_transaction::active_dir(&ctx)).unwrap();
        std::fs::write(&ctx.paths.intent_config_file, INTENT_WITH_TUN).unwrap();
        std::fs::write(
            tun_transaction::active_candidate_path(&ctx),
            candidate.as_bytes(),
        )
        .unwrap();

        let mut journal = tun_transaction::TunJournal {
            schema_version: 1,
            transaction_id: "legacy-no-base".to_string(),
            generation: 2,
            phase: tun_transaction::JournalPhase::CoreApplied,
            original_uid: 1000,
            target_runtime_tun: true,
            base_intent_revision: None,
            candidate_revision: tun_transaction::sha256_revision(candidate.as_bytes()),
            candidate_core_identity: None,
            candidate_core_pid: None,
            old_snapshot_revision: None,
            old_runtime_digest: "digest".to_string(),
            recovery_target_revision: None,
            legacy_source: true,
            legacy_transaction_id: Some("legacy".to_string()),
            legacy_base_revision: None,
            rollback_evidence_complete: false,
            last_error: None,
            outcome: None,
            recovery_core_identity: None,
            recovery_core_pid: None,
            updated_at: None,
        };
        tun_transaction::write_active_journal(&ctx, &journal).unwrap();

        let result = tun_transaction::compare_and_commit_user_intent(&ctx, &journal).unwrap();
        assert_eq!(result, tun_transaction::IntentCommitResult::Conflict);
        assert_eq!(
            std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap(),
            INTENT_WITH_TUN,
            "a journal without a base must never overwrite the intent"
        );

        // With a base recorded, the same journal is no longer blocked by this rule.
        journal.base_intent_revision =
            Some(tun_transaction::sha256_revision(INTENT_WITH_TUN.as_bytes()));
        tun_transaction::write_active_journal(&ctx, &journal).unwrap();
        assert_eq!(
            tun_transaction::compare_and_commit_user_intent(&ctx, &journal).unwrap(),
            tun_transaction::IntentCommitResult::Committed
        );
    }

    /// 缺陷 3（并发封口）：两个**真实进程**经 barrier 同时抢同一实例的事务 owner，
    /// 只能有一个成为 owner（第二个被 journal/user-gate 拒绝）。
    #[cfg(unix)]
    #[test]
    fn issue041_single_owner_survives_two_process_barrier() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::issue041_wave1_tests::issue041_child_races_tun_prepare",
                "--nocapture",
            ])
            .env(TUN_PREPARE_ROOT_ENV, &root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        // Release both sides at (nearly) the same instant.
        std::fs::write(root.join("go"), b"").unwrap();
        let parent = prepare_tun_transaction_in(&root);
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            output.status.success(),
            "child failed to run:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("CHILD_OK") || stdout.contains("CHILD_ERR"),
            "child must have raced the prepare, got: {stdout}"
        );
        let child_ok = stdout.contains("CHILD_OK");
        let parent_ok = parent.is_ok();

        assert!(
            parent_ok ^ child_ok,
            "exactly one process may own the transaction; parent_ok={parent_ok} child_ok={child_ok} child_out={stdout}"
        );
        let loser_message = if parent_ok {
            stdout.clone()
        } else {
            parent
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        assert!(
            loser_message.contains("already in progress")
                || loser_message.contains("gate already exists"),
            "loser must be refused by the durable owner check: {loser_message}"
        );
    }

    /// 上面 barrier 用例的失败方/成功方子进程入口（仅由父进程以 `--exact` 拉起）。
    #[cfg(unix)]
    #[test]
    fn issue041_child_races_tun_prepare() {
        let Ok(root) = std::env::var(TUN_PREPARE_ROOT_ENV) else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let go = root.join("go");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !go.exists() {
            if std::time::Instant::now() >= deadline {
                println!("CHILD_ERR: barrier timeout");
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match prepare_tun_transaction_in(&root) {
            Ok(()) => println!("CHILD_OK"),
            Err(error) => println!("CHILD_ERR: {error}"),
        }
    }

    /// 正常顺序对照（无残留事务）：`override import`（带 route-exclude）→ `tun on`，
    /// candidate 提交后 intent 的 tun 块逐键完整，且 route-exclude 保留。
    #[test]
    fn issue041_normal_order_import_then_tun_on_keeps_tun_block() {
        let temp = tempfile::TempDir::new().unwrap();
        let ctx = system_ctx(temp.path());
        std::fs::write(&ctx.paths.intent_config_file, INTENT_WITH_TUN).unwrap();

        let journal = tun_transaction::prepare_and_publish_active_transaction_capturing_intent(
            &ctx,
            1000,
            true,
            &evidence(),
            |intent_bytes| build_tun_candidate(&ctx, intent_bytes, true, None, None),
        )
        .unwrap();
        assert_eq!(
            journal.base_intent_revision.as_deref(),
            Some(tun_transaction::sha256_revision(INTENT_WITH_TUN.as_bytes()).as_str()),
            "the journal base must describe the intent captured in the same critical section"
        );
        // Walk the journal to CoreApplied, then commit as the CLI does.
        let mut journal = journal;
        for next in [
            tun_transaction::JournalPhase::PromotionPending,
            tun_transaction::JournalPhase::SnapshotPromoted,
            tun_transaction::JournalPhase::CoreApplied,
        ] {
            let fence = tun_transaction::TransactionFence {
                transaction_id: journal.transaction_id.clone(),
                generation: journal.generation,
                expected_phase: journal.phase,
                expected_candidate_revision: journal.candidate_revision.clone(),
            };
            journal = tun_transaction::cas_update_phase(&ctx, &fence, next, None, None).unwrap();
        }
        assert_eq!(
            tun_transaction::compare_and_commit_user_intent(&ctx, &journal).unwrap(),
            tun_transaction::IntentCommitResult::Committed
        );

        let committed: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&ctx.paths.intent_config_file).unwrap())
                .unwrap();
        // 裁决验收口径：整块 TUN map 比对（不是只看 tun:/enable 存在）。
        let expected_tun: serde_yaml::Value = serde_yaml::from_str(
            "\
enable: true
stack: mixed
auto-route: true
strict-route: true
dns-hijack:
- any:53
- tcp://any:53
route-exclude-address:
- 224.0.0.0/4
",
        )
        .unwrap();
        assert_eq!(
            committed["tun"], expected_tun,
            "the whole TUN map must survive import -> tun on"
        );
    }
}
// ──: 删除 proxy group 后孤儿 selection（机械子集） ──

#[cfg(test)]
mod issue_034_orphan_selection_tests {
    use super::*;
    use tempfile::TempDir;

    fn paths_in(tmp: &TempDir) -> utils::AppPaths {
        utils::AppPaths::new(tmp.path().join(".config/mihomo"))
    }

    fn scope_for(paths: &utils::AppPaths, subscription_id: &str) -> selection::SelectionScope {
        std::fs::create_dir_all(paths.selections_dir()).unwrap();
        selection::SelectionScope {
            subscription_id: subscription_id.to_string(),
            path: paths.selection_state_path_for_subscription(subscription_id),
        }
    }

    fn body_of(source: &'static str, start: &str, end: &str) -> &'static str {
        let start_at = source.find(start).expect("锚点缺失");
        let end_at = source[start_at..].find(end).expect("结束锚点缺失") + start_at;
        &source[start_at..end_at]
    }

    /// 删组只清理本组在本订阅的条目：不跨组、不跨订阅、不 GC 其他槽的用户树
    /// （明确砍掉统一孤儿 GC 器——派生镜像不得回写用户树）。
    #[test]
    fn delete_cleanup_removes_only_the_deleted_group_in_its_own_scope() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(&tmp);
        let scope = scope_for(&paths, "sub-aaaaaaaa");
        let other = scope_for(&paths, "sub-bbbbbbbb");
        selection::remember_selection_for_scope(&scope, "Antigravity", "韩国KR-HY2").unwrap();
        selection::remember_selection_for_scope(&scope, "Proxy", "NodeA").unwrap();
        selection::remember_selection_for_scope(&other, "Antigravity", "NodeB").unwrap();

        let outcome = cleanup_deleted_group_selection(&scope, "Antigravity");
        assert!(
            matches!(outcome, DeletedGroupSelection::Unpinned),
            "{outcome:?}"
        );
        assert!(deleted_group_selection_notice("Antigravity", &outcome).is_none());

        let left = selection::load_selection_state_for_scope(&scope).unwrap();
        assert_eq!(left.get("Antigravity"), None);
        assert_eq!(left.get("Proxy"), Some(&"NodeA".to_string()));
        assert_eq!(
            selection::load_selection_state_for_scope(&other)
                .unwrap()
                .get("Antigravity"),
            Some(&"NodeB".to_string()),
            "其他订阅槽的用户树不得被触碰"
        );

        // 没有该组记录时既无清理也无提示（Absent 不推送、不报告）。
        let outcome = cleanup_deleted_group_selection(&scope, "Antigravity");
        assert!(
            matches!(outcome, DeletedGroupSelection::Absent),
            "{outcome:?}"
        );
        assert!(deleted_group_selection_notice("Antigravity", &outcome).is_none());
    }

    /// unpin 失败（EACCES/锁超时/解析错误同一面）不得静默：非致命但可见，
    /// 且必须给出 `select --unpin` 修复路径。
    #[test]
    fn delete_cleanup_failure_is_visible_with_a_repair_path() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(&tmp);
        let scope = scope_for(&paths, "sub-cccccccc");
        std::fs::write(&scope.path, "not: valid: selection: state\n").unwrap();

        let outcome = cleanup_deleted_group_selection(&scope, "Antigravity");
        assert!(
            matches!(outcome, DeletedGroupSelection::Failed(_)),
            "{outcome:?}"
        );
        let notice = deleted_group_selection_notice("Antigravity", &outcome)
            .expect("清理失败必须可见，不得 `let _ =` 吞掉");
        assert!(notice.starts_with('⚠'), "{notice}");
        assert!(notice.contains("was not cleaned up"), "{notice}");
        assert!(
            notice.contains("mihomo-cli select --unpin --group 'Antigravity'"),
            "{notice}"
        );
    }

    /// 顺序断言：清理只在 overlay merge 提交成功之后执行，且失败回滚路径不带
    /// 任何 selection 副作用（merge 前 unpin 会留下"意图已删、组仍在"的反向不一致）。
    ///
    /// （P3-1）文本代理边界：运行时验证需要真实的 `config` merge 管线
    /// （Core 二进制等价校验、`ConfigLock`、endpoint 解析）。这些依赖没有测试注入缝，
    /// 本仓既有做法只能锁源码结构；因此这里断言的是「调用点位于 save → merge →
    /// rollback 之后的相对次序」，而不是一次真实 merge 失败注入。语义边界与
    /// `delete_cleanup_pushes_the_runtime_selection_mirror` 相同。
    #[test]
    fn delete_cleanup_runs_after_the_overlay_merge_commits() {
        let source = include_str!("main.rs");
        let body = body_of(
            source,
            "async fn update_group_overlay(",
            "async fn cmd_rule(",
        );

        let save_at = body
            .find("overlay.save(&overlay_path)?")
            .expect("overlay save");
        let merge_at = body
            .find("config::merge_user_config_checked_at_endpoint(")
            .expect("merge 站点");
        let rollback_at = body
            .find("restore_file_snapshot(&overlay_path, previous.clone())")
            .expect("merge 失败回滚站点");
        let cleanup_at = body
            .find("cleanup_deleted_group_selection(scope, &name)")
            .expect("delete 清理站点");
        assert!(save_at < merge_at, "merge 必须在 overlay.save 之后");
        assert!(rollback_at < cleanup_at, "回滚路径必须先于清理");
        assert!(
            merge_at < cleanup_at,
            "清理必须在 merge 成功之后（否则 merge 失败不可回滚 selection）"
        );

        // 吞错形态全仓清零。
        assert!(
            !source.contains("let _ = crate::selection::unpin_selection_for_scope"),
            "unpin 结果不得再被 `let _ =` 吞掉"
        );
    }

    /// 镜像推送接线：清理掉记录后必须推镜像，且只在 system 模式推、只在真的
    /// 移除记录时推。IPC 无注入缝（socket 是固定绝对路径，测试内不得触碰真实
    /// 运行态），故沿用本仓既有做法——生产口结构锁（同 main_tests 中
    /// lifecycle 成功臂的断言）。
    ///
    /// （P3-2）：极性判据已抽成纯函数
    /// [`should_push_selection_mirror_after_delete`]，其真值表由下一项测试在运行时
    /// 覆盖；本测试只锁「调用点存在且以环境探测结果为参数」这一接线事实。
    #[test]
    fn delete_cleanup_pushes_the_runtime_selection_mirror() {
        let source = include_str!("main.rs");
        let body = body_of(
            source,
            "async fn update_group_overlay(",
            "async fn cmd_rule(",
        );
        let cleanup_at = body
            .find("let outcome = cleanup_deleted_group_selection(scope, &name)")
            .expect("delete 清理站点");
        let tail = &body[cleanup_at..];

        assert!(
            tail.contains("push_selection_mirror_update(paths, &scope.subscription_id, json)"),
            "清理后必须推镜像，否则 daemon 独立重启继续重放孤儿（§2.3）"
        );
        assert!(
            tail.contains("selection_mirror_target_is_system(system, user)"),
            "镜像只存在于 system 模式，user 模式推送只会产生无意义告警"
        );
        assert!(
            tail.contains("should_push_selection_mirror_after_delete("),
            "polarity must go through the extracted predicate"
        );
    }

    /// （P3-3）：`--json` 报告字段 `selection_cleanup` 是纯追加面：
    /// 三种清理结果各有稳定词，非 delete 动作由调用方填 `null`。
    #[test]
    fn delete_cleanup_json_labels_are_stable() {
        assert_eq!(
            selection_cleanup_label(&DeletedGroupSelection::Absent),
            "absent"
        );
        assert_eq!(
            selection_cleanup_label(&DeletedGroupSelection::Unpinned),
            "unpinned"
        );
        assert_eq!(
            selection_cleanup_label(&DeletedGroupSelection::Failed(anyhow::anyhow!("boom"))),
            "failed"
        );
    }

    /// （P3-2）：推送极性的运行时真值表——只有「system 模式的镜像
    /// 目标」且「真的移除了记录」才推送；Absent/Failed/user 模式一律不推。
    #[test]
    fn delete_cleanup_mirror_push_polarity_is_runtime_checkable() {
        use DeletedGroupSelection::{Absent, Failed, Unpinned};

        assert!(should_push_selection_mirror_after_delete(&Unpinned, true));
        assert!(!should_push_selection_mirror_after_delete(&Unpinned, false));
        assert!(!should_push_selection_mirror_after_delete(&Absent, true));
        assert!(!should_push_selection_mirror_after_delete(
            &Failed(anyhow::anyhow!("unpin failed")),
            true
        ));
    }

    /// 渲染：干净报告保留成功图标。
    #[test]
    fn lifecycle_success_lines_keep_the_glyph_for_clean_reports() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(&tmp);

        assert_eq!(
            lifecycle_success_lines(
                "system configuration promoted and runtime applied",
                None,
                &paths
            ),
            vec!["  ✅ system configuration promoted and runtime applied".to_string()]
        );
        assert_eq!(
            lifecycle_success_lines("", None, &paths),
            vec!["  ✅".to_string()]
        );
    }

    /// 渲染：混合结果（基线成功行 + replay 的 ⚠ 行）不得被成功图标包裹。
    #[test]
    fn lifecycle_success_lines_never_wrap_warnings_in_the_success_glyph() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(&tmp);

        let mixed = "system configuration promoted and runtime applied\n\
                     ✓ Selections restored: Proxy → NodeA\n\
                     ⚠ Selection Antigravity → 韩国KR-HY2 not applied: group no longer exists";
        let lines = lifecycle_success_lines(mixed, None, &paths);
        assert!(
            lines.iter().all(|line| !line.contains('✅')),
            "混合结果不得带成功图标: {lines:?}"
        );
        assert_eq!(
            lines[0],
            "system configuration promoted and runtime applied"
        );
        assert_eq!(
            lines[2],
            "⚠ Selection Antigravity → 韩国KR-HY2 not applied: group no longer exists"
        );

        // 只有告警行（无基线行）同样原样呈现。
        let only_warning =
            "⚠ Selection Antigravity → 韩国KR-HY2 not applied: group no longer exists";
        assert_eq!(
            lifecycle_success_lines(only_warning, None, &paths),
            vec![only_warning.to_string()]
        );
    }

    /// 生产口回归锁：lifecycle 成功臂走共享格式化器，不再硬编码成功图标前缀。
    #[test]
    fn lifecycle_success_site_uses_the_shared_formatter() {
        let source = include_str!("main.rs");
        let site = source
            .find("let resp = ipc::send_command(&cmd).await?;")
            .expect("lifecycle send site");
        let arm = &source[site..];
        let arm_end = arm
            .find("ipc::DaemonResponse::Error { message }")
            .expect("lifecycle success arm boundary");
        let success_arm = &arm[..arm_end];
        assert!(
            success_arm.contains("lifecycle_success_lines("),
            "lifecycle 成功臂必须复用共享格式化器"
        );
        assert!(
            !success_arm.contains('✅'),
            "成功图标不得直接包裹 daemon message"
        );
    }
}

#[cfg(test)]
mod issue_044_json_mode_stdout_pollution_tests {
    use super::*;
    use tempfile::TempDir;

    fn paths_in(tmp: &TempDir) -> utils::AppPaths {
        utils::AppPaths::new(tmp.path().to_path_buf())
    }

    fn body_of<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let start_at = source.find(start).expect("锚点缺失");
        let end_at = source[start_at..].find(end).expect("结束锚点缺失") + start_at;
        &source[start_at..end_at]
    }

    /// push_selection_mirror_update 必须包含 json 通道参数并在 json 模式路由至 eprintln。
    #[test]
    fn push_selection_mirror_update_routes_to_eprintln_when_json_is_true() {
        let source_raw = include_str!("main.rs");
        let source = source_raw.replace("\r\n", "\n");
        assert!(
            source.contains("async fn push_selection_mirror_update(paths: &utils::AppPaths, subscription_id: &str, json: bool)"),
            "push_selection_mirror_update 必须包含 json 参数"
        );
        let fn_start = source
            .find("async fn push_selection_mirror_update(")
            .expect("push_selection_mirror_update 签名");
        let fn_body = &source[fn_start..fn_start + 1500];
        assert!(
            fn_body.contains("if json {\n                    eprintln!("),
            "json 模式下 mirror push 诊断输出必须走 eprintln! 避免污染 stdout"
        );
        assert!(
            fn_body.contains("println!("),
            "非 json 模式下必须保留原 println! 输出"
        );
    }

    /// update_group_overlay 原生组隐藏与 reset no-op 必须在 json 模式下避免污染 stdout。
    #[test]
    fn update_group_overlay_routes_notices_to_eprintln_in_json_mode() {
        let source_raw = include_str!("main.rs");
        let source = source_raw.replace("\r\n", "\n");
        let body = body_of(
            &source,
            "async fn update_group_overlay(",
            "async fn cmd_rule(",
        );
        assert!(
            body.contains("if json {\n                    eprintln!(\"proxy group `{name}` is already at upstream state (no-op)\");"),
            "reset no-op 提示在 json 模式下必须输出至 stderr"
        );
        assert!(
            body.contains("if json {\n                    eprintln!(\"原生组 `{name}` 已标记隐藏（将在配置合成时被过滤）\");"),
            "原生组隐藏提示在 json 模式下必须输出至 stderr"
        );
    }

    /// apply_selected_proxy 在 json 模式下输出结构化 JSON envelope，非 json 模式输出 Switched。
    #[test]
    fn apply_selected_proxy_emits_json_envelope_when_json_is_true() {
        let source_raw = include_str!("main.rs");
        let source = source_raw.replace("\r\n", "\n");
        let body = body_of(
            &source,
            "async fn apply_selected_proxy(",
            "async fn print_selection_replay_after_ready(",
        );
        assert!(
            body.contains("if json {\n        print_json_envelope("),
            "apply_selected_proxy 在 json 模式下必须输出 json envelope"
        );
        assert!(
            body.contains("println!(\"Switched {group_name} → {node_name}\");"),
            "apply_selected_proxy 在非 json 模式下必须保留 Switched 文本输出"
        );
    }

    /// main 错误退出时，在 json 模式下向 stdout 打印结构化错误 envelope。
    #[test]
    fn main_error_exit_emits_error_envelope_on_stdout_when_json_is_true() {
        let source_raw = include_str!("main.rs");
        let source = source_raw.replace("\r\n", "\n");
        let body = body_of(&source, "async fn main() {", "async fn run(cli: Cli)");
        assert!(
            body.contains(
                "let error_envelope = json_error_envelope(cmd_name, &format!(\"{e:#}\"));"
            ),
            "main 错误退出在 json 模式下必须向 stdout 打印错误 envelope"
        );
        assert!(
            body.contains("println!(\"{json_str}\");"),
            "错误 envelope 必须写到 stdout"
        );
        let envelope = body_of(
            &source,
            "fn json_error_envelope(",
            "fn validate_config_at_paths_with_binary_json(",
        );
        assert!(
            envelope.contains("\"ok\": false"),
            "错误 envelope 必须标记 ok: false"
        );
        assert!(
            body.contains("eprintln!(\"\\n  Error: {e}\");"),
            "错误细节必须同时保留在 stderr"
        );
    }

    /// push_selection_mirror_update 缺失文件时静默返回，不 panic。
    #[tokio::test]
    async fn push_selection_mirror_update_missing_file_is_silent() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(&tmp);
        // 无 selection 文件，不应有任何输出或 panic
        push_selection_mirror_update(&paths, "non-existent-sub", true).await;
        push_selection_mirror_update(&paths, "non-existent-sub", false).await;
    }
}

/// 把「我读不到」与「它不对」分开——身份不足只能落到 `Undeterminable`，
/// 既不得索要提权，也不得把自由文案里出现的 permission 字样当作判据。
#[cfg(test)]
mod eacces_classification_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn undeterminable_state_dir_never_escalates_privileges() {
        use utils::SystemStateDirVerdict;

        for is_root in [false, true] {
            assert_eq!(
                system_state_repair_action(SystemStateDirVerdict::Healthy, is_root),
                SystemStateRepairAction::None
            );
        }

        // 无 sudo 调用计划：三态里只有确证漂移才产生提权动作。
        for is_root in [false, true] {
            let action = system_state_repair_action(SystemStateDirVerdict::Undeterminable, is_root);
            assert_eq!(action, SystemStateRepairAction::ReportUndeterminable);
            assert_ne!(
                action,
                SystemStateRepairAction::ReexecAsRoot,
                "an identity that cannot judge must never trigger sudo (root={is_root})"
            );
            assert_ne!(action, SystemStateRepairAction::RepairInPlace);
        }

        assert_eq!(
            system_state_repair_action(SystemStateDirVerdict::NeedsRepair, false),
            SystemStateRepairAction::ReexecAsRoot
        );
        assert_eq!(
            system_state_repair_action(SystemStateDirVerdict::NeedsRepair, true),
            SystemStateRepairAction::RepairInPlace
        );
    }

    #[test]
    fn generation_read_classification_is_structural_not_textual() {
        let denied = generation::GenerationError::Io(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ));
        assert!(generation_read_is_identity_limited(&denied));

        // 文案里出现 permission denied 但错误种类不是 EACCES：不得触发提权。
        let textual = generation::GenerationError::Serialization(
            "Permission denied while decoding state.json".to_string(),
        );
        assert!(!generation_read_is_identity_limited(&textual));

        let other_io =
            generation::GenerationError::Io(std::io::Error::from(std::io::ErrorKind::InvalidData));
        assert!(!generation_read_is_identity_limited(&other_io));
    }

    /// 裁决 B 拆分之一：**观测语境零提权**。generation state 读不动（EACCES）时，
    /// 「是否已有 pending」这一观测只提示后继续，绝不触发 sudo 重入。
    /// 业务阶段（暂存/应用）的受控授权由
    /// `protected_domain_authorization_tests::unreadable_generation_root_requires_business_authorization`
    /// 单独覆盖——两者是不同决策，不得互相顶替。
    #[cfg(target_os = "linux")]
    #[test]
    fn unreadable_system_generation_state_does_not_request_sudo() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            // root 绕过模式位，构造不出不可观测的 state 目录。
            return;
        }

        let dir =
            std::env::temp_dir().join(format!("mihomo-035-generation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let generation_root = dir.join("generations");
        std::fs::create_dir_all(&generation_root).unwrap();
        std::fs::set_permissions(&generation_root, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::from_current_env(),
        );
        ctx.paths.tun_config_file = dir.join("tun-config.yaml");
        ctx.paths.cli_binary = dir.join("mihomo-cli");
        // 独立注入 trust root：只改 tun_config_file 从未真的构造出 EACCES。
        ctx.paths.generation_store_root = generation_root.clone();

        // 若判据仍走字符串/误判，这里会进入 sudo 重入并 exit，断言必然失败。
        assert!(maybe_sudo_reexec_for_system_generation(&ctx).is_ok());

        std::fs::set_permissions(&generation_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// SPEC §12.3.1（裁决 B）：root 专属域的**业务操作授权**边界。
/// 与三态权限判定分离：授权只看「当前显式操作是否必须由特权执行者完成」，
/// 既不是漂移判定，也不产生任何修复动作。
#[cfg(test)]
mod protected_domain_authorization_tests {
    use super::*;

    fn planned_system_ctx(root: &std::path::Path) -> instance::InstanceContext {
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::from_current_env(),
        );
        ctx.paths.tun_config_file = root.join("system-data/tun-config.yaml");
        ctx.paths.generation_store_root = root.join("system-data/generations");
        std::fs::create_dir_all(ctx.paths.tun_config_file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&ctx.paths.generation_store_root).unwrap();
        ctx
    }

    #[test]
    fn healthy_installation_requests_no_authorization() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(tmp.path());
        assert_eq!(
            protected_domains_requiring_authorization(&ctx),
            Vec::new(),
            "可观测的健康安装态不得产生任何授权请求"
        );
        // 观测得到时授权边界是纯 no-op：既不询问也不 exit。
        assert!(authorize_protected_domain_operations(&ctx, "restart --system", "重启").is_ok());
    }

    #[test]
    fn user_mode_never_requests_system_authorization() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ctx = planned_system_ctx(tmp.path());
        ctx.mode = instance::InstanceMode::User;
        assert!(authorize_protected_domain_operations(&ctx, "restart", "重启").is_ok());
    }

    #[test]
    fn generation_root_authorization_matrix() {
        // First install: only a non-root process needs one authorization to create
        // the root-owned trust root.
        assert!(generation_trust_root_requires_authorization(
            false, false, false
        ));
        assert!(!generation_trust_root_requires_authorization(
            false, true, false
        ));
        // Existing readable state is a no-op.
        assert!(!generation_trust_root_requires_authorization(
            true, false, false
        ));
        // Existing identity-limited root still needs authorization.
        assert!(generation_trust_root_requires_authorization(
            true, false, true
        ));
        assert!(generation_trust_root_requires_authorization(
            true, true, true
        ));
    }

    /// First install on macOS/Linux: the trust root does not exist yet, so a
    /// non-root process must ask for one administrator authorization before the
    /// first root-owned write. Root must not ask again.
    #[cfg(unix)]
    #[test]
    fn missing_generation_root_requires_authorization_for_non_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(tmp.path());
        std::fs::remove_dir_all(&ctx.paths.generation_store_root).unwrap();

        let expected = if is_current_process_root() {
            Vec::new()
        } else {
            vec![ProtectedDomain::GenerationTrustRoot]
        };
        assert_eq!(protected_domains_requiring_authorization(&ctx), expected);
    }

    /// 拆分之二：**必要业务阶段受控授权**。generation 信任根不可读时，业务阶段
    /// （暂存/应用）必须识别出「需要授权」，而不是把它折叠成「无需处理」。
    #[cfg(target_os = "linux")]
    #[test]
    fn unreadable_generation_root_requires_business_authorization() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            // root 绕过模式位，构造不出不可观测的 trust root。
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(tmp.path());
        std::fs::set_permissions(
            &ctx.paths.generation_store_root,
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        assert_eq!(
            protected_domains_requiring_authorization(&ctx),
            vec![ProtectedDomain::GenerationTrustRoot]
        );
    }

    /// runtime 事务域与代际域是两个独立域：一个不可读不得把另一个也算进去。
    #[cfg(target_os = "linux")]
    #[test]
    fn unreadable_transaction_journal_requires_authorization_independently() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(tmp.path());
        let transactions = tun_transaction::transactions_dir(&ctx);
        std::fs::create_dir_all(&transactions).unwrap();
        std::fs::set_permissions(&transactions, std::fs::Permissions::from_mode(0o000)).unwrap();

        assert_eq!(
            protected_domains_requiring_authorization(&ctx),
            vec![ProtectedDomain::RuntimeTransactions],
            "generation 域仍可观测，不得被一并判为需要授权"
        );
    }

    /// 裁决原文措辞：不宣称检测到权限异常，也不给任何修复/递归 chown 建议。
    #[test]
    fn authorization_notice_never_claims_a_permission_problem() {
        let notice = protected_domain_authorization_notice("restart --system", "重启");
        assert!(notice.contains("为执行你请求的 restart --system"));
        assert!(notice.contains("需要管理员授权检查受保护的运行状态"));
        assert!(notice.contains("并按原命令范围恢复事务或应用更新"));
        assert!(notice.contains("当前尚未判定权限异常"));
        assert!(notice.contains("重启可能暂时中断连接"));
        for forbidden in [
            "检测到",
            "正在修复",
            "chown",
            "chmod",
            "权限问题",
            "异常修复",
        ] {
            assert!(
                !notice.contains(forbidden),
                "授权提示不得出现 {forbidden}: {notice}"
            );
        }
    }

    /// 「已授权但仍不可读」必须是结构化失败：不递归 sudo、不假装运行态被改动。
    #[test]
    fn authorized_but_still_unobservable_is_structured() {
        let message = protected_domain_still_unobservable_message(&[
            ProtectedDomain::RuntimeTransactions,
            ProtectedDomain::GenerationTrustRoot,
        ]);
        assert!(message.contains("已取得管理员授权"));
        assert!(message.contains("受保护的运行事务状态与受保护的代际信任根"));
        assert!(message.contains("未判定权限漂移"));
        assert!(message.contains("运行态未改动"));
        assert!(message.contains("mihomo-cli doctor --system"));
        for forbidden in ["chown", "chmod", "sudo "] {
            assert!(
                !message.contains(forbidden),
                "结构化失败不得含 {forbidden}: {message}"
            );
        }
    }

    /// 代际信任根的两种拒绝必须分开报：授权不足 ≠ 确证违约。
    #[test]
    fn generation_trust_root_refusals_are_classified_separately() {
        let denied = anyhow::anyhow!(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let unavailable = generation_trust_root_refusal("暂存代际", &denied).to_string();
        assert!(unavailable.contains("授权不足"));
        assert!(unavailable.contains("未判定权限漂移"));
        assert!(unavailable.contains("在可取得管理员授权的会话重试原 mihomo-cli 命令"));
        assert!(!unavailable.contains("chown"));

        let drifted = anyhow::anyhow!("generation trust root /x drifted from root:root 0750");
        let violation = generation_trust_root_refusal("暂存代际", &drifted).to_string();
        assert!(violation.contains("代际信任根安全条件不满足"));
        assert!(violation.contains("证据保留"));
        assert!(violation.contains("mihomo-cli doctor --system"));
        assert!(!violation.contains("chown"));
        assert!(!violation.contains("授权不足"));
    }

    /// 身份不足判据必须结构化：文案里出现 permission denied 不算。
    #[test]
    fn identity_limited_detection_is_structural() {
        let denied = anyhow::anyhow!(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(identity_limited_io(&denied));

        let not_found = anyhow::anyhow!(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(!identity_limited_io(&not_found));

        let textual = anyhow::anyhow!("Permission denied while decoding state.json");
        assert!(!identity_limited_io(&textual));

        let wrapped = anyhow::anyhow!(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            .context("代际状态不可读");
        assert!(identity_limited_io(&wrapped), "错误链包裹后仍须识别");
    }

    /// 无 TTY 时不等待交互输入：sudo 以 `--non-interactive` 启动，认证不成立即非零；
    /// 业务参数原样保留（授权不扩大业务范围，`--yes` 也不替代 OS 认证）。
    #[test]
    fn non_interactive_authorization_keeps_the_business_arguments() {
        let exe = std::path::Path::new("/usr/local/bin/mihomo-cli");
        let args = vec!["restart".to_string(), "--system".to_string()];
        let command = sudo_reexec_command_with(exe, &args, true);
        let rendered: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(command.get_program(), "sudo");
        assert!(rendered.iter().any(|arg| arg == "--non-interactive"));
        assert!(rendered
            .windows(2)
            .any(|pair| pair == ["restart", "--system"]));
        assert!(!rendered.iter().any(|arg| arg == "--yes"));
    }

    fn offset_after(source: &'static str, start: &str, needle: &str) -> usize {
        let base = source.find(start).expect("缺少起点锚点");
        source[base..]
            .find(needle)
            .map(|offset| offset + base)
            .unwrap_or_else(|| panic!("缺少锚点: {needle}"))
    }

    /// 授权必须发生在首次特权读取/留证/写入之前，且至多一次重入（root 不再 sudo）。
    #[test]
    fn authorization_precedes_privileged_work_and_never_recurses() {
        let source = include_str!("main.rs");

        let gate = offset_after(
            source,
            "async fn cmd_lifecycle_instance_mode(",
            "authorize_protected_domain_operations(",
        );
        for later in [
            "shared_repair_prerequisites(",
            "preserve_upgrade_evidence(&ctx)",
            "maybe_auto_recover_active_transaction(",
            "ensure_instance_controller_endpoint(&ctx)",
            "apply_pending_generation(&ctx)",
        ] {
            assert!(
                gate < offset_after(source, "async fn cmd_lifecycle_instance_mode(", later),
                "生命周期入口的授权必须早于 {later}"
            );
        }

        let apply_gate = offset_after(
            source,
            "pub(crate) async fn apply_pending_generation(",
            "authorize_protected_domain_operations(",
        );
        assert!(
            apply_gate
                < offset_after(
                    source,
                    "pub(crate) async fn apply_pending_generation(",
                    "verify_generation_trust_root("
                ),
            "应用代际的授权必须早于信任根读取"
        );

        for entry in [
            "pub(crate) fn prepare_system_generation(",
            "fn discard_pending_generation(",
        ] {
            assert!(
                offset_after(source, entry, "authorize_protected_domain_operations(")
                    < offset_after(source, entry, "verify_generation_trust_root("),
                "{entry} 的授权必须早于信任根读取"
            );
        }

        let gate_body = offset_after(source, "fn authorize_protected_domain_operations(", "");
        let recursion_guard = source[gate_body..]
            .find("if is_current_process_root()")
            .expect("授权边界必须先判特权");
        let reexec = source[gate_body..]
            .find("sudo_reexec_command_with(")
            .expect("授权边界必须使用非交互提权重入");
        assert!(
            recursion_guard < reexec,
            "特权执行者不得再次 sudo（至多一次协调提权重入）"
        );
    }

    /// 只读观测面（status/doctor/tun status）零提权：授权边界只能出现在四个业务入口。
    #[test]
    fn authorization_gate_has_no_read_only_call_sites() {
        let source = include_str!("main.rs");
        assert_eq!(
            source
                .matches("authorize_protected_domain_operations(")
                .count(),
            5,
            "授权边界只允许：定义 + 生命周期/应用/暂存/撤销 四个业务入口"
        );
        for call in [
            "authorize_protected_domain_operations(&ctx, operation, impact)",
            "authorize_protected_domain_operations(ctx, \"restart --system\", \"重启\")",
            "authorize_protected_domain_operations(ctx, \"install --system\", \"安装\")",
            "authorize_protected_domain_operations(ctx, \"install --discard-pending\", \"安装\")",
        ] {
            assert_eq!(
                source.matches(call).count(),
                1,
                "缺少或重复的业务入口: {call}"
            );
        }
    }
}

/// 配置树修复探测同样把「我读不到」与「它不对」分开——
/// 判据必须 stat 化，身份不足落到 `Undeterminable`，零自动提权。
#[cfg(all(test, unix))]
mod config_assets_eacces_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn undeterminable_config_assets_never_escalates_privileges() {
        for is_root in [false, true] {
            assert_eq!(
                config_assets_repair_action(ConfigAssetsVerdict::Healthy, is_root),
                ConfigOwnershipRepair::NotNeeded
            );
        }

        // 无 sudo 调用计划：三态里只有确证漂移才产生提权动作。
        for is_root in [false, true] {
            let action = config_assets_repair_action(ConfigAssetsVerdict::Undeterminable, is_root);
            assert_eq!(action, ConfigOwnershipRepair::ReportUndeterminable);
            assert_ne!(
                action,
                ConfigOwnershipRepair::ReexecAsRoot,
                "an identity that cannot judge config assets must never trigger sudo (root={is_root})"
            );
            assert_ne!(action, ConfigOwnershipRepair::RepairAsRoot);
        }

        assert_eq!(
            config_assets_repair_action(ConfigAssetsVerdict::NeedsRepair, false),
            ConfigOwnershipRepair::ReexecAsRoot
        );
        assert_eq!(
            config_assets_repair_action(ConfigAssetsVerdict::NeedsRepair, true),
            ConfigOwnershipRepair::RepairAsRoot
        );
    }

    #[cfg(unix)]
    #[test]
    fn config_assets_verdict_merge_and_rank() {
        assert!(ConfigAssetsVerdict::Healthy.rank() < ConfigAssetsVerdict::Undeterminable.rank());
        assert!(
            ConfigAssetsVerdict::Undeterminable.rank() < ConfigAssetsVerdict::NeedsRepair.rank()
        );

        assert_eq!(
            ConfigAssetsVerdict::Healthy.merge(Some(ConfigAssetsVerdict::Undeterminable)),
            ConfigAssetsVerdict::Undeterminable
        );
        assert_eq!(
            ConfigAssetsVerdict::Healthy.merge(Some(ConfigAssetsVerdict::NeedsRepair)),
            ConfigAssetsVerdict::NeedsRepair
        );
        assert_eq!(
            ConfigAssetsVerdict::Undeterminable.merge(Some(ConfigAssetsVerdict::NeedsRepair)),
            ConfigAssetsVerdict::NeedsRepair
        );
        assert_eq!(
            ConfigAssetsVerdict::NeedsRepair.merge(Some(ConfigAssetsVerdict::Healthy)),
            ConfigAssetsVerdict::NeedsRepair
        );
        assert_eq!(
            ConfigAssetsVerdict::NeedsRepair.merge(Some(ConfigAssetsVerdict::Undeterminable)),
            ConfigAssetsVerdict::NeedsRepair
        );
        assert_eq!(
            ConfigAssetsVerdict::Undeterminable.merge(Some(ConfigAssetsVerdict::Healthy)),
            ConfigAssetsVerdict::Undeterminable
        );
        assert_eq!(
            ConfigAssetsVerdict::Healthy.merge(None),
            ConfigAssetsVerdict::Healthy
        );
    }

    /// 0770 / unopenable EACCES 夹具：文件不可 open 读（EACCES），但 stat 观测属主相符时判 Healthy，零提权。
    #[cfg(unix)]
    #[test]
    fn unopenable_eacces_config_file_maps_to_healthy() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.yaml");
        std::fs::write(&config_path, "mixed-port: 7890\n").unwrap();

        let my_uid = unsafe { libc::geteuid() };

        // 设置为 0o000，当前用户无法 open(O_RDONLY)，但 lstat 仍可观测属主
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // 验证 open 确实失败（EACCES）
        if my_uid != 0 {
            let open_res = std::fs::File::open(&config_path);
            assert!(
                open_res.is_err(),
                "file with mode 0000 must not be openable by non-root"
            );
            assert_eq!(
                open_res.unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }

        // stat-based 探测只读 lstat，不应因为打不开而误判漂移
        let verdict = check_config_assets_verdict(&config_dir, &config_path, my_uid).unwrap();
        assert_eq!(verdict, ConfigAssetsVerdict::Healthy);

        let repair = detect_config_assets_repair(&config_dir, &config_path, my_uid, false).unwrap();
        assert_eq!(repair, ConfigOwnershipRepair::NotNeeded);

        // 恢复权限以便清理
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// 0770 / unopenable 订阅条目：条目无法 open 但 stat 属主相符时判 Healthy，零提权。
    #[cfg(unix)]
    #[test]
    fn unopenable_eacces_subscription_entry_maps_to_healthy() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("config");
        let sub_dir = config_dir.join("subscriptions");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let config_path = config_dir.join("config.yaml");
        std::fs::write(&config_path, "mixed-port: 7890\n").unwrap();

        let sub_file = sub_dir.join("default.yaml");
        std::fs::write(&sub_file, "proxies: []\n").unwrap();

        let my_uid = unsafe { libc::geteuid() };

        // 设置订阅条目为 0o000
        std::fs::set_permissions(&sub_file, std::fs::Permissions::from_mode(0o000)).unwrap();

        if my_uid != 0 {
            assert_eq!(
                std::fs::File::open(&sub_file).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }

        let verdict = check_config_assets_verdict(&config_dir, &config_path, my_uid).unwrap();
        assert_eq!(verdict, ConfigAssetsVerdict::Healthy);

        let repair = detect_config_assets_repair(&config_dir, &config_path, my_uid, false).unwrap();
        assert_eq!(repair, ConfigOwnershipRepair::NotNeeded);

        std::fs::set_permissions(&sub_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// 确证 root 漂移：当配置属于 root（uid 0）而 expected_uid != 0 时，必须判 NeedsRepair。
    #[cfg(unix)]
    #[test]
    fn confirmed_root_drift_maps_to_needs_repair() {
        let my_uid = unsafe { libc::geteuid() };
        if my_uid == 0 {
            let temp = tempfile::tempdir().unwrap();
            let config_dir = temp.path().join("config");
            std::fs::create_dir_all(&config_dir).unwrap();
            let config_path = config_dir.join("config.yaml");
            std::fs::write(&config_path, "mixed-port: 7890\n").unwrap();

            let verdict = check_config_assets_verdict(&config_dir, &config_path, 1000).unwrap();
            assert_eq!(verdict, ConfigAssetsVerdict::NeedsRepair);

            let repair =
                detect_config_assets_repair(&config_dir, &config_path, 1000, true).unwrap();
            assert_eq!(repair, ConfigOwnershipRepair::RepairAsRoot);
        } else {
            let verdict = probe_config_file_verdict(1000, 0, true, 1).unwrap();
            assert_eq!(verdict, ConfigAssetsVerdict::NeedsRepair);

            let action_non_root = config_assets_repair_action(verdict, false);
            assert_eq!(action_non_root, ConfigOwnershipRepair::ReexecAsRoot);

            let action_root = config_assets_repair_action(verdict, true);
            assert_eq!(action_root, ConfigOwnershipRepair::RepairAsRoot);
        }
    }

    /// 不可观测路径：父目录权限为 0o000 时，stat 失败（EACCES）必须落入 Undeterminable，零提权。
    #[cfg(target_os = "linux")]
    #[test]
    fn unobservable_path_maps_to_undeterminable() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("unsearchable");
        std::fs::create_dir_all(&parent).unwrap();
        let config_dir = parent.join("mihomo");
        let config_path = config_dir.join("config.yaml");

        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();

        let my_uid = unsafe { libc::geteuid() };
        let verdict = check_config_assets_verdict(&config_dir, &config_path, my_uid).unwrap();
        assert_eq!(verdict, ConfigAssetsVerdict::Undeterminable);

        let repair = detect_config_assets_repair(&config_dir, &config_path, my_uid, false).unwrap();
        assert_eq!(repair, ConfigOwnershipRepair::ReportUndeterminable);

        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// subscriptions 目录不可读时（mode 0o000），read_dir 失败落入 Undeterminable，零提权。
    #[cfg(target_os = "linux")]
    #[test]
    fn unreadable_subscription_dir_maps_to_undeterminable() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("config");
        let sub_dir = config_dir.join("subscriptions");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let config_path = config_dir.join("config.yaml");
        std::fs::write(&config_path, "mixed-port: 7890\n").unwrap();

        std::fs::set_permissions(&sub_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        let my_uid = unsafe { libc::geteuid() };
        let verdict = check_config_assets_verdict(&config_dir, &config_path, my_uid).unwrap();
        assert_eq!(verdict, ConfigAssetsVerdict::Undeterminable);

        let repair = detect_config_assets_repair(&config_dir, &config_path, my_uid, false).unwrap();
        assert_eq!(repair, ConfigOwnershipRepair::ReportUndeterminable);

        std::fs::set_permissions(&sub_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// 调用点回归：`ensure_system_config_ownership_for_lifecycle` 遇到 Undeterminable 时
    /// 只提示后以 Ok(()) 放行，不得执行 sudo 重入或 exit。
    #[cfg(target_os = "linux")]
    #[test]
    fn ensure_system_config_ownership_undeterminable_does_not_request_sudo() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("unsearchable_lifecycle");
        std::fs::create_dir_all(&parent).unwrap();
        let config_dir = parent.join("mihomo");

        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::from_current_env(),
        );
        ctx.paths.config_dir = config_dir.clone();
        ctx.paths.intent_config_file = config_dir.join("config.yaml");

        // 若 Undeterminable 误走 ReexecAsRoot，这里会执行 sudo 重入并终止进程导致测试失败；
        // 预期必须是 Ok(())。
        assert!(ensure_system_config_ownership_for_lifecycle(&ctx).is_ok());

        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

/// 非特权 `doctor`/`status` 的 pending generation 三态可见性——
/// 代际信任根 EACCES 不得等同「无 pending」、不得静默、不得提权；可读时行为不变。
#[cfg(all(test, unix))]
mod issue_048_pending_visibility_tests {
    use super::*;

    fn planned_system_ctx(root: &std::path::Path) -> instance::InstanceContext {
        let mut ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::System,
            &instance::PathInputs::for_tests(),
        );
        ctx.paths.generation_store_root = root.join("generations");
        ctx
    }

    fn generation_id(label: &str) -> generation::GenerationId {
        generation::GenerationId::new(label).unwrap()
    }

    /// 按花括号配平提取函数源码（含内联 format 花括号，天然配平）。
    fn function_source<'a>(source: &'a str, entry: &str) -> &'a str {
        let start = source.find(entry).expect("缺少函数入口锚点");
        let body_start = start + source[start..].find('{').expect("缺少函数体");
        let bytes = source.as_bytes();
        let mut depth = 1usize;
        let mut idx = body_start + 1;
        while depth > 0 {
            match bytes[idx] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            idx += 1;
        }
        &source[start..idx]
    }

    /// 三态分类：可读无 pending → Applied；可读有 pending → HasPending；读不动 → Undeterminable。
    #[test]
    fn observe_pending_distinguishes_read_failure_from_no_pending() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(temp.path());
        let store = system_generation_store(&ctx);

        assert!(matches!(
            store.observe_pending(),
            generation::PendingObservation::Applied
        ));

        let mut state = generation::GenerationState::default();
        let id = generation_id("gen-048-pending");
        state.pending = Some(id.clone());
        store.write_state(&state).unwrap();
        match store.observe_pending() {
            generation::PendingObservation::HasPending(observed) => assert_eq!(observed, id),
            other => panic!("可读且有 pending 须报 HasPending: {other:?}"),
        }
    }

    /// 非 root + 0o000 信任根：EACCES 必须映射为 Undeterminable，而不是 Applied。
    #[cfg(target_os = "linux")]
    #[test]
    fn unreadable_trust_root_maps_to_undeterminable() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            // root 绕过模式位，构造不出不可观测的 trust root。
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(temp.path());
        std::fs::create_dir_all(&ctx.paths.generation_store_root).unwrap();
        std::fs::set_permissions(
            &ctx.paths.generation_store_root,
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        assert!(matches!(
            system_generation_store(&ctx).observe_pending(),
            generation::PendingObservation::Undeterminable(_)
        ));

        std::fs::set_permissions(
            &ctx.paths.generation_store_root,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }

    /// doctor 三态消费：可读无 pending → 静默；可读有 pending → 既有 fail 告警不变。
    #[test]
    fn doctor_pending_check_keeps_silence_and_existing_warning() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(temp.path());
        let store = system_generation_store(&ctx);

        assert!(doctor_pending_generation_check(&store).is_none());

        let mut state = generation::GenerationState::default();
        let id = generation_id("gen-048-doctor");
        state.pending = Some(id.clone());
        store.write_state(&state).unwrap();
        let warned = doctor_pending_generation_check(&store).unwrap();
        assert!(!warned.passed);
        assert!(warned
            .detail
            .contains(&format!("发现 pending generation ({id})")));
        assert_eq!(
            warned.hint.as_deref(),
            Some("应用更新: mihomo-cli restart --system")
        );
    }

    /// doctor + status 对不可读信任根：❓「无法判定」、诚实且可执行、零 sudo 计划。
    #[cfg(target_os = "linux")]
    #[test]
    fn unreadable_root_reports_undeterminable_without_escalation() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            // root 绕过模式位，构造不出不可观测的 trust root。
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(temp.path());
        std::fs::create_dir_all(&ctx.paths.generation_store_root).unwrap();
        std::fs::set_permissions(
            &ctx.paths.generation_store_root,
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        let check = doctor_pending_generation_check(&system_generation_store(&ctx)).unwrap();
        assert!(
            check.passed && check.neutral,
            "不可读是「无法判定」（❓），不是问题（❌）"
        );
        assert!(check.hint.is_none(), "无法判定不携带修复型 hint（不提权）");
        assert!(check.detail.contains("无法判定"));
        assert!(check.detail.contains("可能存在未应用更新"));
        assert!(check
            .detail
            .contains("mihomo-cli doctor --system --privileged"));
        assert!(!check.detail.contains("mihomo-cli restart --system"));
        for forbidden in [
            "检测到",
            "正在修复",
            "chown",
            "chmod",
            "权限问题",
            "无待应用更新",
        ] {
            assert!(
                !check.detail.contains(forbidden),
                "无法判定提示不得出现 {forbidden}: {}",
                check.detail
            );
        }

        // 只读面零 sudo 计划：三个消费点不得触碰提权/授权边界。
        let source = include_str!("main.rs");
        for entry in [
            "fn doctor_pending_generation_check(",
            "fn pending_generation_unknown_notice(",
            "pub(crate) fn check_and_warn_daemon_binary_skew(",
            "fn status_json_pending_generation(",
        ] {
            let body = function_source(source, entry);
            for forbidden in [
                "maybe_sudo_reexec_for_system_generation(",
                "sudo_reexec_command",
                "authorize_protected_domain_operations(",
            ] {
                assert!(
                    !body.contains(forbidden),
                    "{entry} 不得包含 {forbidden}（只读观测零提权）"
                );
            }
        }

        std::fs::set_permissions(
            &ctx.paths.generation_store_root,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }

    /// 「无法判定」文案：身份不足带授权检查指引；非身份类失败只报事实。
    #[test]
    fn unknown_notice_is_honest_and_actionable() {
        let limited = pending_generation_unknown_notice(&generation::GenerationError::Io(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ));
        assert!(limited.contains("无法判定"));
        assert!(limited.contains("可能存在未应用更新"));
        assert!(limited.contains("mihomo-cli doctor --system --privileged"));
        assert!(!limited.contains("mihomo-cli restart --system"));
        for forbidden in [
            "检测到",
            "正在修复",
            "chown",
            "chmod",
            "无待应用更新",
            "没有待应用更新",
        ] {
            assert!(
                !limited.contains(forbidden),
                "文案不得出现 {forbidden}: {limited}"
            );
        }

        let broken = pending_generation_unknown_notice(
            &generation::GenerationError::Serialization("corrupt".to_string()),
        );
        assert!(broken.contains("无法判定"));
        assert!(
            !broken.contains("restart --system"),
            "非身份类读取失败不给管理员授权指引"
        );
    }

    /// status --json：system 实例三态结构化为 none / pending+id / unknown；user 实例为 null。
    #[test]
    fn status_json_pending_generation_is_structured() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(temp.path());

        assert_eq!(
            status_json_pending_generation(ctx.mode, &ctx)["state"],
            "none"
        );

        let mut state = generation::GenerationState::default();
        let id = generation_id("gen-048-json");
        state.pending = Some(id.clone());
        system_generation_store(&ctx).write_state(&state).unwrap();
        let value = status_json_pending_generation(ctx.mode, &ctx);
        assert_eq!(value["state"], "pending");
        assert_eq!(value["id"], id.as_str());

        let user_ctx = instance::InstanceContext::planned(
            instance::TargetOs::Linux,
            instance::InstanceMode::User,
            &instance::PathInputs::for_tests(),
        );
        assert!(status_json_pending_generation(user_ctx.mode, &user_ctx).is_null());
    }

    /// status --json 不可读 → `{"state":"unknown"}`：不是 none，也不伪造 pending。
    #[cfg(target_os = "linux")]
    #[test]
    fn status_json_pending_generation_reports_unknown_for_unreadable_root() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let ctx = planned_system_ctx(temp.path());
        std::fs::create_dir_all(&ctx.paths.generation_store_root).unwrap();
        std::fs::set_permissions(
            &ctx.paths.generation_store_root,
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        assert_eq!(
            status_json_pending_generation(ctx.mode, &ctx)["state"],
            "unknown"
        );

        std::fs::set_permissions(
            &ctx.paths.generation_store_root,
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }

    /// 接线回归：status 文本与 JSON 均消费同一三态观测点。
    #[test]
    fn status_paths_consume_the_shared_three_state_observation() {
        let source = include_str!("main.rs");
        let text_body = function_source(source, "pub(crate) fn check_and_warn_daemon_binary_skew(");
        assert!(text_body.contains("observe_pending()"));
        assert!(text_body.contains("存在待应用的系统更新"));
        assert!(text_body.contains("pending_generation_unknown_notice("));

        let json_body = function_source(source, "fn status_json_data(");
        assert!(json_body
            .contains("\"pending_generation\": status_json_pending_generation(plan.mode, ctx),"));
    }

    #[test]
    fn doctor_daemon_binary_skew_hint_includes_system_flag() {
        assert_eq!(
            doctor_daemon_binary_skew_hint(),
            "应用更新: mihomo-cli restart --system"
        );
    }

    #[cfg(unix)]
    #[test]
    fn doctor_split_dns_intent_detection() {
        let split_dns_config = r#"
dns:
  enable: true
  nameserver-policy:
    '+.corp.example': 10.10.1.1
"#;
        assert!(!has_split_dns_intent(split_dns_config, &[]));

        let matching_dns_config = r#"
dns:
  enable: true
  nameserver-policy:
    '+.example.com': 10.10.1.1
"#;
        assert!(has_split_dns_intent(matching_dns_config, &[]));

        let route_exclude_config = r#"
tun:
  enable: true
  route-exclude-address:
    - 10.0.0.0/8
"#;
        assert!(!has_split_dns_intent(route_exclude_config, &[]));

        let normal_config = r#"
dns:
  enable: true
tun:
  enable: true
"#;
        assert!(!has_split_dns_intent(normal_config, &[]));
    }

    #[cfg(unix)]
    #[test]
    fn doctor_split_dns_requires_resolver_or_probe_domain_evidence() {
        use std::net::{IpAddr, SocketAddr};

        let route_exclude_config = r#"
tun:
  enable: true
  route-exclude-address:
    - 10.0.0.0/8
"#;
        assert!(has_split_dns_intent(
            route_exclude_config,
            &[SocketAddr::new(IpAddr::from([10, 0, 0, 1]), 53)]
        ));
        assert!(!has_split_dns_intent(
            route_exclude_config,
            &[SocketAddr::new(IpAddr::from([192, 0, 2, 53]), 53)]
        ));

        let matching_dns_config = r#"
dns:
  enable: true
  nameserver-policy:
    '+.example.com': 10.10.1.1
"#;
        assert!(has_split_dns_intent(matching_dns_config, &[]));
    }

    #[test]
    fn format_rule_list_annotated_marks_active_and_fallback_rules() {
        let rules = vec![
            "DOMAIN-SUFFIX,corp.example,DIRECT".to_string(),
            "DOMAIN-SUFFIX,reject.example,REJECT-NO-DROP".to_string(),
            "AND,((NETWORK,udp),(DST-PORT,443)),REJECT".to_string(),
            "DOMAIN-KEYWORD,openai,OpenRouter".to_string(),
        ];
        let policies = vec!["DIRECT".to_string(), "Proxy".to_string()];
        let lines =
            format_rule_list_annotated(&rules, crate::rules::RulePosition::Front, Some(&policies));
        assert!(
            lines
                .iter()
                .any(|l| l.contains("DOMAIN-SUFFIX,corp.example,DIRECT") && l.contains("[Active]")),
            "Expected active annotation for valid rule, got: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("REJECT-NO-DROP") && l.contains("[Active]")),
            "REJECT-NO-DROP is a built-in policy and must be active, got: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("DOMAIN-KEYWORD,openai,OpenRouter") && l.contains("Fallback")),
            "Expected fallback annotation for missing policy rule, got: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("AND,((NETWORK,udp)") && l.contains("[Active]")),
            "Logical rule with a built-in target must be annotated active, got: {lines:?}"
        );
    }

    #[test]
    fn doctor_intranet_guard_checks_identifies_risk_topology() {
        let temp = tempfile::tempdir().unwrap();
        let config_file = temp.path().join("config.yaml");
        std::fs::write(
            &config_file,
            r#"
sniffer:
  enable: true
  override-destination: true
tun:
  enable: true
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
  - MATCH,Proxy
"#,
        )
        .unwrap();

        let checks = doctor_intranet_guard_checks(&config_file);
        assert_eq!(checks.len(), 1);
        assert!(checks[0].warning);
        assert!(checks[0].label.contains("Intranet Guard"));
        assert!(checks[0].detail.contains("Sniffer"));
        assert!(checks[0]
            .hint
            .as_ref()
            .unwrap()
            .contains("route-exclude-address"));
    }

    #[test]
    fn doctor_intranet_guard_checks_clean_on_safe_topology() {
        let temp = tempfile::tempdir().unwrap();
        let config_file = temp.path().join("config.yaml");
        std::fs::write(
            &config_file,
            r#"
sniffer:
  enable: true
  override-destination: true
tun:
  enable: true
  route-exclude-address:
    - 10.0.0.0/8
    - 172.16.0.0/12
    - 192.168.0.0/16
rules:
  - DOMAIN-SUFFIX,google.com,Proxy
  - MATCH,Proxy
"#,
        )
        .unwrap();

        let checks = doctor_intranet_guard_checks(&config_file);
        assert!(checks.is_empty());
    }

    #[test]
    fn test_resolve_system_install_operator_matrix() {
        // 1. SUDO_USER priority
        assert_eq!(
            resolve_system_install_operator_from(Some("alice"), Some("root"), true),
            Some("alice".to_string())
        );
        assert_eq!(
            resolve_system_install_operator_from(Some("alice"), Some("bob"), false),
            Some("alice".to_string())
        );
        // 2. SUDO_USER is root or empty
        assert_eq!(
            resolve_system_install_operator_from(Some("root"), Some("root"), true),
            None
        );
        assert_eq!(
            resolve_system_install_operator_from(Some(""), Some("root"), true),
            None
        );
        assert_eq!(
            resolve_system_install_operator_from(Some("   "), Some("root"), true),
            None
        );
        // 3. Fallback to current user when not root
        assert_eq!(
            resolve_system_install_operator_from(None, Some("alice"), false),
            Some("alice".to_string())
        );
        // 4. Root without SUDO_USER
        assert_eq!(
            resolve_system_install_operator_from(None, Some("root"), true),
            None
        );
        assert_eq!(resolve_system_install_operator_from(None, None, true), None);
        assert_eq!(
            resolve_system_install_operator_from(None, None, false),
            None
        );
    }

    #[test]
    fn test_decide_system_install_group_hint_matrix() {
        // 在组内不提示 (SilentAlreadyInGroup)
        assert_eq!(
            decide_system_install_group_hint(Some("alice"), Some(true)),
            SystemInstallGroupHintDecision::SilentAlreadyInGroup
        );
        // 不在组内提示 (Prompt)
        assert_eq!(
            decide_system_install_group_hint(Some("alice"), Some(false)),
            SystemInstallGroupHintDecision::Prompt
        );
        // 无法确定用户不提示 (SkipNoOperator)
        assert_eq!(
            decide_system_install_group_hint(None, Some(false)),
            SystemInstallGroupHintDecision::SkipNoOperator
        );
        assert_eq!(
            decide_system_install_group_hint(Some(""), Some(false)),
            SystemInstallGroupHintDecision::SkipNoOperator
        );
        assert_eq!(
            decide_system_install_group_hint(Some("root"), Some(false)),
            SystemInstallGroupHintDecision::SkipNoOperator
        );
        // 组检查不可用/失败跳过 (SkipGroupCheckUnavailable)
        assert_eq!(
            decide_system_install_group_hint(Some("alice"), None),
            SystemInstallGroupHintDecision::SkipGroupCheckUnavailable
        );
    }

    #[test]
    fn test_format_system_install_group_hint_text() {
        let lines = format_system_install_group_hint("testuser");
        let text = lines.join("\n");
        assert!(text.contains("mihomo"), "{text}");
        assert!(text.contains("usermod"), "{text}");
        assert!(text.contains("testuser"), "{text}");
        assert!(text.contains("sudo usermod -aG mihomo testuser"), "{text}");
        assert!(text.contains("override import"), "{text}");
        assert!(text.contains("status"), "{text}");
        assert!(
            !text.contains("Fix: mihomo-cli status"),
            "must not suggest status as fix: {text}"
        );
    }

    #[test]
    fn test_system_install_group_hint_mode_gate_and_text() {
        use instance::InstanceMode;

        // user 模式绝不输出。
        assert_eq!(
            system_install_group_hint_lines(InstanceMode::User, Some("alice"), Some(false)),
            None
        );
        // system 模式但已在组内 / 无操作者 / 组检查不可用都不输出。
        assert_eq!(
            system_install_group_hint_lines(InstanceMode::System, Some("alice"), Some(true)),
            None
        );
        assert_eq!(
            system_install_group_hint_lines(InstanceMode::System, Some("root"), Some(false)),
            None
        );
        assert_eq!(
            system_install_group_hint_lines(InstanceMode::System, Some("alice"), None),
            None
        );
        // system 模式且确实不在组内时输出可执行指引。
        let lines =
            system_install_group_hint_lines(InstanceMode::System, Some("alice"), Some(false))
                .expect("system mode + not in group must produce hint lines");
        let text = lines.join("\n");
        assert!(text.contains("alice"), "{text}");
        assert!(text.contains("usermod"), "{text}");
        assert!(text.contains("sudo usermod -aG mihomo alice"), "{text}");
    }
}
