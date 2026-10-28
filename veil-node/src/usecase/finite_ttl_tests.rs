use super::*;
use crate::{
    domain::{Member, MAX_POLICY_TTL_SECS},
    ebpf_bridge::MockEbpfBridge,
    infra::wal::FileWal,
};

const IP: Ipv4Addr = Ipv4Addr::new(10, 99, 0, 66);

#[cfg(target_os = "linux")]
#[test]
fn live_arena_never_uses_simulated_deadlines_for_kernel_rules() {
    let app = crate::tui::TuiArenaApp::new_with_kernel_bridge(
        Some(Arc::new(MockEbpfBridge::default())),
        "test-no-attachment".into(),
    );
    for state in app.nodes.values() {
        assert!(state.policy_clock.is_some() || state.persistence_error.is_some());
        if state.policy_clock.is_some() {
            assert_ne!(state.boot_id, "simulation");
        }
    }
}

fn forbidden() -> Vec<MeshCommand> {
    let zero = DeadlinePeerTrustRule {
        rule: PeerTrustRule::new(IP, false, 0),
        not_after_unix_ns: 0,
    };
    vec![
        MeshCommand::set_trust(IP, false),
        MeshCommand::set_trust_with_ttl(IP, false, 0),
        MeshCommand::set_trust_with_ttl(IP, false, 30), // unsigned relative deadline
        MeshCommand::SetPeerTrustLeaseBatch {
            issued_at_unix_ns: SIMULATION_WALL_EPOCH_NS,
            rules: vec![zero.clone()],
        },
        MeshCommand::SetPeerTrustLeaseBatch {
            issued_at_unix_ns: SIMULATION_WALL_EPOCH_NS,
            rules: vec![
                DeadlinePeerTrustRule {
                    rule: PeerTrustRule::new(Ipv4Addr::new(192, 0, 2, 3), true, 0),
                    not_after_unix_ns: 0,
                },
                zero,
            ],
        },
    ]
}

fn cluster() -> Vec<ConsensusState> {
    let mut nodes = super::regression_tests::cluster();
    let members = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| Member {
            node_id: node.node_id.clone(),
            address: format!("127.0.0.1:{}", 26000 + i).parse().unwrap(),
            public_key: node
                .identity
                .public_key_bytes
                .as_slice()
                .try_into()
                .unwrap(),
        })
        .collect();
    let config = FixedConfiguration::new("finite-ban", 1, members, vec![], 1000).unwrap();
    for node in &mut nodes {
        node.configure_fixed_membership(config.clone(), node.identity.clone())
            .unwrap();
    }
    nodes
}

fn qc(nodes: &[ConsensusState], entry: &LogEntry, hash: [u8; 32]) -> QuorumCertificate {
    // Deliberately construct a valid certificate, as might exist in old history.
    // The stricter receiver must reject forbidden policy even with enough signatures.
    QuorumCertificate {
        term: entry.term,
        index: entry.index,
        entry_hash: hash,
        signatures: nodes
            .iter()
            .take(3)
            .map(|n| {
                (
                    n.node_id.clone(),
                    n.identity
                        .sign_qc_vote(n.signing_context(), entry.term, entry.index, &hash),
                )
            })
            .collect(),
    }
}

fn append(entry: LogEntry, qc: QuorumCertificate) -> ConsensusRpc {
    ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "a".into(),
        prev_log_index: entry.index - 1,
        prev_log_term: if entry.index == 1 { 0 } else { 1 },
        prev_log_hash: [0; 32],
        leader_commit: entry.index,
        entries: vec![entry],
        commit_qc: Some(qc),
    }
}

#[test]
fn proposal_requires_positive_ttl_and_stamps_relative_input() {
    let mut nodes = cluster();
    super::regression_tests::elect(&mut nodes);
    for (i, command) in forbidden().into_iter().enumerate() {
        if i == 2 {
            continue;
        } // finite relative TTL is accepted only at local input
        let before = nodes[0].wal_bytes().unwrap();
        assert!(nodes[0].propose(command).is_err());
        assert_eq!(nodes[0].wal_bytes().unwrap(), before);
    }
    for ttl in [1, MAX_POLICY_TTL_SECS] {
        let entry = nodes[0]
            .propose(MeshCommand::set_trust_with_ttl(IP, false, ttl))
            .unwrap();
        assert!(matches!(
            entry.command,
            MeshCommand::SetPeerTrustLeaseBatch { .. }
        ));
        nodes[0]
            .policy_guard
            .validate_log_command(&entry.command)
            .unwrap();
    }
    assert!(nodes[0]
        .propose(MeshCommand::set_trust_with_ttl(
            IP,
            false,
            MAX_POLICY_TTL_SECS + 1
        ))
        .is_err());
    nodes[0].propose(MeshCommand::set_trust(IP, true)).unwrap();
    nodes[0]
        .propose(MeshCommand::set_trust_with_ttl(IP, true, 0))
        .unwrap();
}

#[test]
fn forbidden_signed_entries_and_valid_qcs_cannot_change_votes_log_or_wal() {
    let mut nodes = cluster();
    let path = super::regression_tests::fixture_dir().join("b.wal");
    nodes[1]
        .enable_persistence(Arc::new(FileWal::create_new(&path).unwrap()))
        .unwrap();
    for command in forbidden() {
        let entry = LogEntry {
            term: 1,
            index: 1,
            command,
        };
        let certificate = qc(&nodes, &entry, entry.digest());
        nodes[1].verify_qc_signatures_only(&certificate).unwrap();
        let rpc = nodes[0].sign_rpc(append(entry, certificate));
        let before = std::fs::read(&path).unwrap();
        let reply = nodes[1].handle_signed_rpc(rpc).unwrap().unwrap();
        assert!(
            matches!(reply, ConsensusRpc::AppendEntriesResponse { success: false, qc_vote_signature, .. } if qc_vote_signature.is_empty())
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(nodes[1].log.is_empty() && nodes[1].voted_entry_hashes.is_empty());
        assert_eq!(nodes[1].current_term, 0);
    }
}

#[test]
fn forbidden_committed_history_never_reaches_backend_even_when_injected() {
    for command in forbidden() {
        let bridge = Arc::new(MockEbpfBridge::default());
        let mut s = ConsensusState::new("b", vec![]).with_ebpf_bridge(bridge.clone());
        s.push_log_entry(LogEntry {
            term: 1,
            index: 1,
            command,
        });
        s.commit_index = 1;
        s.retry_pending_apply();
        assert!(s.last_apply_error.is_some());
        assert_eq!(s.last_applied, 0);
        assert!(s.lease_deadlines.is_empty());
        assert!(bridge.kernel_peer_map.lock().unwrap().is_empty());
    }
}

#[test]
fn forbidden_wal_is_preserved_and_latches_stop_for_committed_and_uncommitted_entries() {
    for command in forbidden() {
        for committed in [false, true] {
            let path = super::regression_tests::fixture_dir().join("legacy.wal");
            let bridge = Arc::new(MockEbpfBridge::default());
            let mut nodes = cluster();
            let entry = LogEntry {
                term: 1,
                index: 1,
                command: command.clone(),
            };
            let hash = nodes[1].push_log_entry(entry);
            nodes[1].current_term = 1;
            nodes[1].voted_for = Some("a".into());
            nodes[1].voted_entry_hashes.insert((1, 1), hash);
            nodes[1].commit_index = u64::from(committed);
            nodes[1].save_wal(&path).unwrap();
            let before = std::fs::read(&path).unwrap();
            let mut restored = ConsensusState::new("b", vec![]).with_ebpf_bridge(bridge.clone());
            restored
                .configure_fixed_membership(
                    nodes[1].fixed_configuration.clone().unwrap(),
                    nodes[1].identity.clone(),
                )
                .unwrap();
            assert!(restored
                .load_wal(&path)
                .unwrap_err()
                .to_string()
                .contains("forbidden policy"));
            assert_eq!(std::fs::read(&path).unwrap(), before);
            assert!(restored.log.is_empty());
            assert!(bridge.kernel_peer_map.lock().unwrap().is_empty());
            assert!(restored.start_signed_election().is_err());
            assert!(restored.trigger_view_timeout().is_err());
        }
    }
}

async fn network_check(
    bridge: Arc<dyn EbpfBridge>,
    clock: Option<Arc<dyn PolicyClock>>,
    check_packet: impl Fn(bool),
) -> anyhow::Result<()> {
    use crate::infra::{
        handle_authenticated_connection, node_tls::NodeTls, send_authenticated_rpc,
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root = super::regression_tests::fixture_dir();
    let mut nodes = cluster();
    for node in &mut nodes {
        if let Some(clock) = &clock {
            node.configure_policy_clock(clock.clone())?;
        }
    }
    super::regression_tests::elect(&mut nodes);
    let config = nodes[0].fixed_configuration.clone().unwrap();
    let ca = crate::infra::node_tls_tests::Authority::new();
    let a_tls = Arc::new(NodeTls::load(
        &ca.bundle(&root.join("tls-a"), &config, "a"),
        &config,
        "a",
    )?);
    let b_tls = Arc::new(NodeTls::load(
        &ca.bundle(&root.join("tls-b"), &config, "b"),
        &config,
        "b",
    )?);
    let client = a_tls.make_endpoint("127.0.0.1:0".parse()?)?;
    let server = b_tls.make_endpoint("127.0.0.1:0".parse()?)?;
    let receiver_id = nodes[1].identity.clone();
    let mut receiver = ConsensusState::new("b", vec![]).with_ebpf_bridge(bridge.clone());
    receiver
        .configure_fixed_membership(config.clone(), receiver_id.clone())
        .unwrap();
    if let Some(clock) = &clock {
        receiver.configure_policy_clock(clock.clone())?;
    }
    receiver
        .enable_persistence(Arc::new(FileWal::create_new(&root.join("b.wal"))?))
        .unwrap();
    receiver.set_emergency_local_bypass(false).unwrap();
    let state = Arc::new(tokio::sync::Mutex::new(receiver));
    let endpoint = server.clone();
    let st = state.clone();
    let task = tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            if let Ok(conn) = incoming.await {
                let _ = handle_authenticated_connection(conn, "b", st.clone(), b_tls.clone()).await;
            }
        }
    });
    for command in forbidden() {
        let entry = LogEntry {
            term: 1,
            index: 1,
            command,
        };
        let certificate = qc(&nodes, &entry, entry.digest());
        let before = std::fs::read(root.join("b.wal"))?;
        let signed = nodes[0].sign_rpc(append(entry, certificate));
        let response = send_authenticated_rpc(&client, server.local_addr()?, signed, &a_tls, "b")
            .await?
            .unwrap();
        assert!(
            matches!(response.rpc, ConsensusRpc::AppendEntriesResponse { success: false, qc_vote_signature, .. } if qc_vote_signature.is_empty())
        );
        assert_eq!(std::fs::read(root.join("b.wal"))?, before);
        assert!(state.lock().await.voted_entry_hashes.is_empty());
        check_packet(false);
    }
    let entry = nodes[0]
        .propose(MeshCommand::set_trust_with_ttl(IP, false, 30))
        .unwrap();
    let certificate = qc(&nodes, &entry, entry.digest());
    let signed = nodes[0].sign_rpc(append(entry, certificate));
    let reply = send_authenticated_rpc(&client, server.local_addr()?, signed, &a_tls, "b")
        .await?
        .unwrap();
    assert!(matches!(
        reply.rpc,
        ConsensusRpc::AppendEntriesResponse { success: true, .. }
    ));
    assert_eq!(state.lock().await.check_xdp_packet(IP), XdpVerdict::Drop);
    check_packet(true);
    // Expiry-free unban remains usable and removes the finite BAN.
    let entry = nodes[0].propose(MeshCommand::set_trust(IP, true)).unwrap();
    let certificate = qc(&nodes, &entry, nodes[0].entry_hash_at(2).unwrap());
    let mut rpc = append(entry, certificate);
    if let ConsensusRpc::AppendEntries { prev_log_hash, .. } = &mut rpc {
        *prev_log_hash = nodes[0].entry_hash_at(1).unwrap();
    }
    let signed = nodes[0].sign_rpc(rpc);
    let reply = send_authenticated_rpc(&client, server.local_addr()?, signed, &a_tls, "b")
        .await?
        .unwrap();
    assert!(matches!(
        reply.rpc,
        ConsensusRpc::AppendEntriesResponse { success: true, .. }
    ));
    check_packet(false);
    // Actual backend must stay unblocked when restoration of historical BAN fails.
    nodes[1].push_log_entry(LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(IP, false),
    });
    nodes[1].commit_index = 1;
    let legacy = root.join("legacy.wal");
    nodes[1].save_wal(&legacy)?;
    let bytes = std::fs::read(&legacy)?;
    let mut restored = ConsensusState::new("b", vec![]).with_ebpf_bridge(bridge);
    restored
        .configure_fixed_membership(config, receiver_id)
        .unwrap();
    if let Some(clock) = clock {
        restored.configure_policy_clock(clock)?;
    }
    assert!(restored.load_wal(&legacy).is_err());
    assert!(restored.start_signed_election().is_err());
    assert_eq!(std::fs::read(&legacy)?, bytes);
    check_packet(false);
    task.abort();
    let _ = task.await;
    client.close(0u32.into(), b"done");
    server.close(0u32.into(), b"done");
    println!("finite_ban_ingress_evidence={}", root.display());
    Ok(())
}

#[tokio::test]
async fn authenticated_quic_rejects_legacy_and_zero_bans_but_accepts_finite_and_unban(
) -> anyhow::Result<()> {
    network_check(Arc::new(MockEbpfBridge::default()), None, |_| {}).await
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires root and unused isolated veth-victim; never use a physical NIC"]
async fn linux_finite_ban_ingress_and_wal_rejection() -> anyhow::Result<()> {
    use crate::{ebpf_bridge::AyaKernelBridge, infra::policy_clock::LinuxBootClock};
    use std::{
        path::Path,
        process::{Command, Stdio},
    };
    anyhow::ensure!(unsafe { libc::geteuid() } == 0, "requires root");
    let link = Command::new("ip")
        .args(["-details", "link", "show", "veth-victim"])
        .output()?;
    anyhow::ensure!(
        link.status.success() && !String::from_utf8_lossy(&link.stdout).contains("prog/xdp"),
        "fixture absent or already attached"
    );
    let ping = || {
        Command::new("ip")
            .args([
                "netns",
                "exec",
                "ns-veil-rogue",
                "ping",
                "-c",
                "1",
                "-W",
                "1",
                "10.99.0.1",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(ping());
    let object = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("assets/veil_xdp_kern.o");
    let bridge = Arc::new(AyaKernelBridge::load_and_attach(
        object.to_str().unwrap(),
        "veth-victim",
    )?);
    let clock = Arc::new(LinuxBootClock::new()?);
    network_check(bridge.clone(), Some(clock), |blocked| {
        assert_eq!(ping(), !blocked)
    })
    .await?;
    let (pass, dropped) = bridge.read_xdp_counters().unwrap();
    assert!(pass > 0 && dropped > 0);
    println!("finite_ban_real_packets_pass={pass} drop={dropped}");
    drop(bridge);
    let link = Command::new("ip")
        .args(["-details", "link", "show", "veth-victim"])
        .output()?;
    assert!(!String::from_utf8_lossy(&link.stdout).contains("prog/xdp"));
    assert!(ping());
    Ok(())
}
