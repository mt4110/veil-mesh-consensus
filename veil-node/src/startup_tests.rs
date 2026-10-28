use super::*;

fn args() -> Args {
    let key = NodeCryptoIdentity::generate_random("node-a");
    let public: String = key
        .public_key_bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Args::try_parse_from([
        "veil-node",
        "--cluster-id",
        "site",
        "--config-epoch",
        "1",
        "--node-id",
        "node-a",
        "--listen",
        "127.0.0.1:5000",
        "--approval-public-key",
        "1122334455667788112233445566778811223344556677881122334455667788",
        "--scope",
        "192.0.2.10/tcp/443",
        "--member",
        &format!("node-a@127.0.0.1:5000={public}"),
    ])
    .unwrap()
}

#[test]
fn startup_requires_fixed_members_and_a_consistent_local_bind() {
    let a = args();
    assert!(daemon_configuration(&a).is_ok());
    let mut wildcard = a.clone();
    wildcard.listen = "0.0.0.0:5000".parse().unwrap();
    assert!(daemon_configuration(&wildcard).is_ok());
    for case in 0..7 {
        let mut bad = a.clone();
        match case {
            0 => bad.member.clear(),
            1 => bad.member.push(bad.member[0].clone()),
            2 => bad.node_id = "absent".into(),
            3 => bad.listen = "127.0.0.1:0".parse().unwrap(),
            4 => bad.listen = "127.0.0.2:5000".parse().unwrap(),
            5 => bad.listen = "[::]:5000".parse().unwrap(),
            _ => bad.cluster_id = None,
        }
        assert!(daemon_configuration(&bad).is_err(), "case {case}");
    }
    assert!(Args::try_parse_from(["veil-node", "--peer", "127.0.0.1:5000"]).is_err());
    assert!(Args::try_parse_from(["veil-node", "--init-key"]).is_err());
}

#[test]
fn missing_key_is_not_generated_by_the_daemon_loader() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("veil-key-check-{}-{stamp}", std::process::id()));
    let path = root.join("key.pk8");
    assert!(NodeCryptoIdentity::load_existing_file("node-a", &path).is_err());
    assert!(!root.exists());
    let provisioned = NodeCryptoIdentity::load_or_generate_file("node-a", &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let loaded = NodeCryptoIdentity::load_existing_file("node-a", &path).unwrap();
    assert_eq!(loaded.public_key_bytes, provisioned.public_key_bytes);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn local_commands_require_explicit_finite_ban_and_disallow_ambiguous_modes() {
    assert!(Args::try_parse_from([
        "veil-node",
        "--ban-ip",
        "192.0.2.1",
        "--ttl-secs",
        "30",
        "--control-socket",
        "/tmp/control"
    ])
    .is_ok());
    for extra in [
        vec![],
        vec!["--ttl-secs", "0"],
        vec!["--ttl-secs", "86401"],
        vec!["--ttl-secs", "10", "--status-local"],
        vec!["--ttl-secs", "10", "--bypass-local", "on"],
    ] {
        let mut argv = vec![
            "veil-node",
            "--ban-ip",
            "192.0.2.1",
            "--control-socket",
            "/tmp/control",
        ];
        argv.extend(extra);
        assert!(Args::try_parse_from(argv).is_err());
    }
    assert!(Args::try_parse_from(["veil-node", "--entry-index", "1"]).is_err());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn non_linux_daemon_clock_refuses_ttl_instead_of_using_simulator_time() {
    let mut state = ConsensusState::new("solo", vec![]);
    state
        .configure_policy_clock(Arc::new(infra::policy_clock::UnavailableLeaseClock(
            std::time::Instant::now(),
        )))
        .unwrap();
    state.start_signed_election().unwrap();
    assert!(state
        .propose(domain::MeshCommand::set_trust_with_ttl(
            "192.0.2.1".parse().unwrap(),
            false,
            30
        ))
        .is_err());
    assert_eq!(state.last_log_index(), 0);
    assert!(state.lease_clock_error.is_some());
}

#[test]
fn operator_modes_are_exclusive_and_require_explicit_inputs() {
    assert!(Args::try_parse_from([
        "veil-node",
        "--init-approver",
        "--approval-key-file",
        "/tmp/operator"
    ])
    .is_ok());
    for args in [
        vec!["veil-node", "--init-approver"],
        vec![
            "veil-node",
            "--init-approver",
            "--approval-key-file",
            "/tmp/operator",
            "--init-key",
            "--key-file",
            "/tmp/node",
        ],
        vec![
            "veil-node",
            "--submit-approval",
            "/tmp/approval",
            "--control-socket",
            "/tmp/control",
            "--status-local",
        ],
        vec!["veil-node", "--approve-unban-ip", "192.0.2.77"],
    ] {
        assert!(Args::try_parse_from(args).is_err());
    }
}
