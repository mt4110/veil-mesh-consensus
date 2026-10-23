use crate::adapter::NodeCryptoIdentity;
use crate::domain::{ConsensusRpc, LogEntry, MeshCommand, QuorumCertificate, Role};
use crate::ebpf_bridge::{EbpfBridge, MockEbpfBridge};
use crate::infra::{make_quic_endpoint, send_rpc_over_quic, ClusterMtlsPki, MeshNode};
use crate::tui;
use crate::usecase::ConsensusState;
use anyhow::Result;
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use veil_common::{evaluate_lsm_policy, evaluate_peer_policy, XdpVerdict};

fn make_four_node_cluster() -> (
    ConsensusState,
    ConsensusState,
    ConsensusState,
    ConsensusState,
) {
    let names = ["node-alpha", "node-beta", "node-gamma", "node-delta"];
    let mut states: Vec<ConsensusState> = names
        .iter()
        .map(|&me| {
            let peers = names
                .iter()
                .filter(|&&n| n != me)
                .map(|&s| s.to_string())
                .collect();
            ConsensusState::new(me, peers)
        })
        .collect();
    let pub_keys: BTreeMap<String, Vec<u8>> = states
        .iter()
        .map(|st| (st.node_id.clone(), st.identity.public_key_bytes.clone()))
        .collect();
    for st in &mut states {
        st.register_peer_public_keys(pub_keys.clone());
    }
    let delta = states.pop().unwrap();
    let gamma = states.pop().unwrap();
    let beta = states.pop().unwrap();
    let alpha = states.pop().unwrap();
    (alpha, beta, gamma, delta)
}

#[test]
fn proof_1_election_safety_and_log_up_to_date_check() {
    let (mut alpha, mut beta, mut gamma, mut delta) = make_four_node_cluster();
    assert_eq!(alpha.quorum_size(), 3);

    let vote_req_alpha = alpha.start_signed_election();
    assert_eq!(alpha.current_term, 1);
    assert_eq!(alpha.role, Role::Candidate);

    let resp_beta = beta
        .handle_signed_rpc(vote_req_alpha.clone())
        .unwrap()
        .unwrap();
    let resp_delta = delta.handle_signed_rpc(vote_req_alpha).unwrap().unwrap();
    assert_eq!(
        resp_beta,
        ConsensusRpc::RequestVoteResponse {
            term: 1,
            voter_id: "node-beta".into(),
            vote_granted: true,
        }
    );

    let vote_req_gamma = gamma.start_signed_election();
    let resp_beta_to_gamma = beta.handle_signed_rpc(vote_req_gamma).unwrap().unwrap();
    assert_eq!(
        resp_beta_to_gamma,
        ConsensusRpc::RequestVoteResponse {
            term: 1,
            voter_id: "node-beta".into(),
            vote_granted: false,
        }
    );

    let signed_beta = beta.sign_rpc(resp_beta);
    let signed_delta = delta.sign_rpc(resp_delta);
    alpha.handle_signed_rpc(signed_beta).unwrap();
    alpha.handle_signed_rpc(signed_delta).unwrap();
    assert_eq!(alpha.role, Role::Leader);
    assert_ne!(gamma.role, Role::Leader);
}

#[tokio::test]
async fn proof_2_three_node_quic_consensus_and_ebpf_xdp_enforcement() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let ep_a = make_quic_endpoint("127.0.0.1:0".parse()?)?;
    let ep_b = make_quic_endpoint("127.0.0.1:0".parse()?)?;
    let ep_c = make_quic_endpoint("127.0.0.1:0".parse()?)?;
    let ep_d = make_quic_endpoint("127.0.0.1:0".parse()?)?;

    let all_addrs = BTreeMap::from([
        ("node-alpha".to_string(), ep_a.local_addr()?),
        ("node-beta".to_string(), ep_b.local_addr()?),
        ("node-gamma".to_string(), ep_c.local_addr()?),
        ("node-delta".to_string(), ep_d.local_addr()?),
    ]);

    let build_peers = |me: &str| -> BTreeMap<String, SocketAddr> {
        all_addrs
            .iter()
            .filter(|(k, _)| k.as_str() != me)
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    };

    let make_node = |id: &str, ep: quinn::Endpoint| {
        let peers = build_peers(id);
        let peer_ids: Vec<String> = peers.keys().cloned().collect();
        MeshNode {
            node_id: id.into(),
            endpoint: ep,
            state: Arc::new(Mutex::new(ConsensusState::new(id, peer_ids))),
            peer_addrs: peers,
        }
    };

    let node_alpha = make_node("node-alpha", ep_a);
    let node_beta = make_node("node-beta", ep_b);
    let node_gamma = make_node("node-gamma", ep_c);
    let node_delta = make_node("node-delta", ep_d);

    let mut pub_keys = BTreeMap::new();
    for node in [&node_alpha, &node_beta, &node_gamma, &node_delta] {
        let st = node.state.lock().await;
        pub_keys.insert(st.node_id.clone(), st.identity.public_key_bytes.clone());
    }
    for node in [&node_alpha, &node_beta, &node_gamma, &node_delta] {
        node.state
            .lock()
            .await
            .register_peer_public_keys(pub_keys.clone());
    }

    let _srv_a = node_alpha.spawn_server();
    let _srv_b = node_beta.spawn_server();
    let _srv_c = node_gamma.spawn_server();
    let _srv_d = node_delta.spawn_server();

    let elected = node_alpha.campaign().await?;
    assert!(elected, "node-alpha must win quorum over mTLS QUIC");

    let attacker_ip = Ipv4Addr::new(10, 0, 0, 99);
    assert_eq!(
        node_alpha.state.lock().await.check_xdp_packet(attacker_ip),
        XdpVerdict::Pass
    );

    let log_idx = node_alpha
        .propose_and_commit(MeshCommand::set_trust(attacker_ip, false))
        .await?;
    assert_eq!(log_idx, 1);

    for (name, node) in [
        ("node-alpha", &node_alpha),
        ("node-beta", &node_beta),
        ("node-gamma", &node_gamma),
        ("node-delta", &node_delta),
    ] {
        let st = node.state.lock().await;
        assert_eq!(st.commit_index, 1, "{name} commit_index must be 1");
        assert_eq!(st.last_applied, 1, "{name} last_applied must be 1");
        assert!(st.commit_qcs.contains_key(&1));
        assert!(st.commit_qcs[&1].signatures.len() >= 3);
        assert_eq!(st.check_xdp_packet(attacker_ip), XdpVerdict::Drop);
    }

    let log_idx_2 = node_alpha
        .propose_and_commit(MeshCommand::set_trust(attacker_ip, true))
        .await?;
    assert_eq!(log_idx_2, 2);

    for node in [&node_alpha, &node_beta, &node_gamma, &node_delta] {
        let st = node.state.lock().await;
        assert_eq!(st.commit_index, 2);
        assert_eq!(st.check_xdp_packet(attacker_ip), XdpVerdict::Pass);
    }

    Ok(())
}

#[test]
fn proof_3_network_partition_split_brain_prevention_and_recovery() {
    let (mut alpha, mut beta, mut gamma, mut delta) = make_four_node_cluster();

    let rv = alpha.start_signed_election();
    for peer in [&mut beta, &mut gamma, &mut delta] {
        let r = peer.handle_signed_rpc(rv.clone()).unwrap().unwrap();
        let sr = peer.sign_rpc(r);
        alpha.handle_signed_rpc(sr).unwrap();
    }
    assert_eq!(alpha.role, Role::Leader);

    let suspect_ip = Ipv4Addr::new(10, 0, 0, 50);
    alpha
        .propose(MeshCommand::set_trust(suspect_ip, false))
        .unwrap();
    assert_eq!(alpha.commit_index, 0);
    assert_eq!(alpha.check_xdp_packet(suspect_ip), XdpVerdict::Pass);

    let rv_beta = beta.start_signed_election();
    assert_eq!(beta.current_term, 2);
    for peer in [&mut gamma, &mut delta] {
        let r = peer.handle_signed_rpc(rv_beta.clone()).unwrap().unwrap();
        let sr = peer.sign_rpc(r);
        beta.handle_signed_rpc(sr).unwrap();
    }
    assert_eq!(beta.role, Role::Leader);

    let malicious_ip = Ipv4Addr::new(10, 0, 0, 200);
    beta.propose(MeshCommand::set_trust(malicious_ip, false))
        .unwrap();

    for _round in 0..2 {
        for (peer_name, peer) in [("node-gamma", &mut gamma), ("node-delta", &mut delta)] {
            let ae = beta.sign_rpc(beta.make_append_entries(peer_name).unwrap());
            let resp = peer.handle_signed_rpc(ae).unwrap().unwrap();
            let sresp = peer.sign_rpc(resp);
            beta.handle_signed_rpc(sresp).unwrap();
        }
    }
    assert_eq!(beta.commit_index, 1);
    assert_eq!(gamma.commit_index, 1);
    assert_eq!(delta.commit_index, 1);

    let ae_heal = beta.sign_rpc(beta.make_append_entries("node-alpha").unwrap());
    let heal_resp = alpha.handle_signed_rpc(ae_heal).unwrap().unwrap();
    let sheal = alpha.sign_rpc(heal_resp);
    beta.handle_signed_rpc(sheal).unwrap();

    assert_eq!(alpha.role, Role::Follower);
    assert_eq!(alpha.current_term, 2);
    assert_eq!(alpha.commit_index, 1);
    assert_eq!(alpha.check_xdp_packet(suspect_ip), XdpVerdict::Pass);
    assert_eq!(alpha.check_xdp_packet(malicious_ip), XdpVerdict::Drop);
    assert_eq!(alpha.ebpf_policy_map, beta.ebpf_policy_map);
}

#[test]
fn proof_4_ebpf_kernel_bridge_and_lsm_stealth_gc_zeroization() {
    let bridge_alpha = Arc::new(MockEbpfBridge::new_with_secret(vec![
        0xDE, 0xAD, 0xBE, 0xEF,
    ]));
    let bridge_beta = Arc::new(MockEbpfBridge::new_with_secret(vec![
        0xCA, 0xFE, 0xBA, 0xBE,
    ]));

    let (alpha_base, beta_base, mut gamma, mut delta) = make_four_node_cluster();
    let mut alpha = alpha_base.with_ebpf_bridge(bridge_alpha.clone());
    let mut beta = beta_base.with_ebpf_bridge(bridge_beta.clone());

    let rv = alpha.start_signed_election();
    for peer in [&mut beta, &mut gamma, &mut delta] {
        let resp = peer.handle_signed_rpc(rv.clone()).unwrap().unwrap();
        let sresp = peer.sign_rpc(resp);
        alpha.handle_signed_rpc(sresp).unwrap();
    }
    assert_eq!(alpha.role, Role::Leader);

    assert_eq!(alpha.check_lsm_socket_connect(), 0);
    assert_eq!(beta.check_lsm_socket_connect(), 0);
    assert_eq!(bridge_beta.sensitive_key_material.lock().unwrap().len(), 4);

    let attacker_ip = Ipv4Addr::new(10, 0, 0, 77);
    alpha
        .propose(MeshCommand::set_trust(attacker_ip, false))
        .unwrap();
    alpha
        .propose(MeshCommand::set_lockdown("node-beta", true))
        .unwrap();

    for _round in 0..2 {
        for (peer_name, peer) in [
            ("node-beta", &mut beta),
            ("node-gamma", &mut gamma),
            ("node-delta", &mut delta),
        ] {
            let ae = alpha.sign_rpc(alpha.make_append_entries(peer_name).unwrap());
            let resp = peer.handle_signed_rpc(ae).unwrap().unwrap();
            let sresp = peer.sign_rpc(resp);
            alpha.handle_signed_rpc(sresp).unwrap();
        }
    }

    let kernel_peer_policy = *bridge_beta
        .kernel_peer_map
        .lock()
        .unwrap()
        .get(&u32::from(attacker_ip))
        .expect("PeerPolicy must be written into kernel eBPF map");
    assert_eq!(
        evaluate_peer_policy(Some(&kernel_peer_policy)),
        XdpVerdict::Drop
    );

    let kernel_lsm = *bridge_beta.kernel_lsm_policy.lock().unwrap();
    assert_eq!(evaluate_lsm_policy(Some(&kernel_lsm)), -1);
    assert_eq!(beta.check_lsm_socket_connect(), -1);
    assert!(bridge_beta
        .sensitive_key_material
        .lock()
        .unwrap()
        .is_empty());
    assert_eq!(bridge_beta.stealth_gc_purge_count(), 1);
    assert_eq!(alpha.check_lsm_socket_connect(), 0);
}

#[test]
fn proof_5_byzantine_fault_tolerance_ed25519_signature_verification() {
    let (mut alpha, mut beta, mut gamma, mut delta) = make_four_node_cluster();
    let id_attacker = NodeCryptoIdentity::from_seed("node-mallory", &[99u8; 32]);

    let signed_rv = alpha.start_signed_election();
    for peer in [&mut beta, &mut gamma, &mut delta] {
        let resp = peer.handle_signed_rpc(signed_rv.clone()).unwrap().unwrap();
        let signed_resp = peer.sign_rpc(resp);
        alpha.handle_signed_rpc(signed_resp).unwrap();
    }
    assert_eq!(alpha.role, Role::Leader);

    let forged_ae = ConsensusRpc::AppendEntries {
        term: 99,
        leader_id: "node-alpha".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        entries: vec![],
        leader_commit: 0,
        commit_qc: None,
    };
    let spoofed_envelope = gamma.sign_rpc(forged_ae.clone());
    let err_a = beta.handle_signed_rpc(spoofed_envelope).unwrap_err();
    assert!(err_a.contains("Identity Spoofing"));

    let mut fake_sig_envelope = id_attacker.sign_rpc(forged_ae);
    fake_sig_envelope.signer_id = "node-alpha".into();
    let err_b = beta.handle_signed_rpc(fake_sig_envelope).unwrap_err();
    assert!(err_b.contains("invalid Ed25519 signature"));

    let target_ip = Ipv4Addr::new(10, 0, 0, 88);
    alpha
        .propose(MeshCommand::set_trust(target_ip, false))
        .unwrap();
    let valid_ae = alpha.make_append_entries("node-beta").unwrap();
    let mut tampered_envelope = alpha.sign_rpc(valid_ae);

    if let ConsensusRpc::AppendEntries {
        ref mut entries, ..
    } = tampered_envelope.rpc
    {
        entries[0].command = MeshCommand::set_trust(target_ip, true);
    }

    let err_c = beta.handle_signed_rpc(tampered_envelope).unwrap_err();
    assert!(err_c.contains("invalid Ed25519 signature or tampered RPC payload"));

    assert_eq!(beta.rejected_byzantine_packets, 3);
    assert_eq!(beta.current_term, 1);
    assert!(beta.ebpf_policy_map.is_empty());
}

#[test]
fn proof_6_tui_arena_mock_attack_process_spawn_and_extermination() -> Result<()> {
    let mut app = tui::TuiArenaApp::new();
    assert_eq!(app.nodes.len(), 4);
    assert_eq!(app.rogues.len(), 0);

    app.spawn_rogue_attacker()?;
    assert_eq!(app.rogues.len(), 1);
    let pid = app.rogues[0].pid;
    let attacker_ip = app.rogues[0].attacker_ip;
    assert!(pid > 0);
    assert_eq!(app.rogues[0].status, tui::RogueStatus::Infiltrating);

    app.on_tick();
    assert_eq!(
        app.nodes["node-beta"].check_xdp_packet(attacker_ip),
        XdpVerdict::Pass
    );

    app.inject_byzantine_spoof();
    for name in ["node-alpha", "node-beta", "node-gamma", "node-delta"] {
        assert_eq!(app.nodes[name].rejected_byzantine_packets, 1);
    }

    app.exterminate_rogues_by_consensus();

    assert_eq!(app.rogues[0].status, tui::RogueStatus::Exterminated);
    for name in ["node-alpha", "node-beta", "node-gamma", "node-delta"] {
        assert_eq!(
            app.nodes[name].check_xdp_packet(attacker_ip),
            XdpVerdict::Drop
        );
        assert!(app.nodes[name].commit_qcs.contains_key(&2));
    }
    assert_eq!(app.nodes["node-beta"].check_lsm_socket_connect(), -1);
    assert!(app.bridges["node-beta"]
        .sensitive_key_material
        .lock()
        .unwrap()
        .is_empty());

    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn proof_7_live_linux_kernel_xdp_drop_on_veth() -> Result<()> {
    let obj_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("assets/veil_xdp_kern.o");
    if !obj_path.exists() || !std::path::Path::new("/sys/class/net/veth-victim").exists() {
        return Ok(());
    }

    let bridge = match crate::ebpf_bridge::AyaKernelBridge::load_and_attach(
        obj_path.to_str().unwrap(),
        "veth-victim",
    ) {
        Ok(b) => Arc::new(b),
        Err(_) => return Ok(()),
    };

    let attacker_ip = Ipv4Addr::new(10, 99, 0, 66);

    let ping_before = std::process::Command::new("ip")
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
        .status()?;
    assert!(ping_before.success());

    bridge.sync_peer_policy(
        u32::from(attacker_ip),
        veil_common::PeerPolicy::new(false, 1),
    )?;

    let ping_blocked = std::process::Command::new("ip")
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
        .status()?;
    assert!(!ping_blocked.success());

    let (_pass_cnt, drop_cnt) = bridge.read_xdp_counters().unwrap_or((0, 0));
    assert!(drop_cnt >= 1);

    bridge.sync_peer_policy(
        u32::from(attacker_ip),
        veil_common::PeerPolicy::new(true, 2),
    )?;
    let ping_restored = std::process::Command::new("ip")
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
        .status()?;
    assert!(ping_restored.success());

    Ok(())
}

#[test]
fn proof_8_leader_equivocation_and_multisig_quorum_certificate_defense() {
    let (alpha, mut beta, gamma, _delta) = make_four_node_cluster();
    assert_eq!(beta.quorum_size(), 3);

    let honest_entry = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(Ipv4Addr::new(10, 0, 0, 99), false),
    };
    let ae_honest = ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "node-alpha".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        entries: vec![honest_entry.clone()],
        leader_commit: 0,
        commit_qc: None,
    };
    let resp = beta
        .handle_signed_rpc(alpha.sign_rpc(ae_honest))
        .unwrap()
        .unwrap();
    if let ConsensusRpc::AppendEntriesResponse {
        success,
        entry_hash,
        ..
    } = resp
    {
        assert!(success);
        assert_eq!(entry_hash, honest_entry.digest());
    }

    let conflicting_entry = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(Ipv4Addr::new(10, 0, 0, 1), false),
    };
    let ae_equivocate = ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "node-alpha".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        entries: vec![conflicting_entry],
        leader_commit: 0,
        commit_qc: None,
    };
    let resp_eq = beta
        .handle_signed_rpc(alpha.sign_rpc(ae_equivocate))
        .unwrap()
        .unwrap();
    if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_eq {
        assert!(!success);
    }
    assert_eq!(beta.rejected_byzantine_packets, 1);

    let ae_no_qc = ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "node-alpha".into(),
        prev_log_index: 1,
        prev_log_term: 1,
        entries: vec![],
        leader_commit: 1,
        commit_qc: None,
    };
    let resp_no_qc = beta
        .handle_signed_rpc(alpha.sign_rpc(ae_no_qc))
        .unwrap()
        .unwrap();
    if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_no_qc {
        assert!(!success);
    }
    assert_eq!(beta.commit_index, 0);
    assert_eq!(beta.rejected_byzantine_packets, 2);

    let hash = honest_entry.digest();
    let unilateral_qc = QuorumCertificate {
        term: 1,
        index: 1,
        entry_hash: hash,
        signatures: BTreeMap::from([(
            "node-alpha".into(),
            alpha.identity.sign_qc_vote(1, 1, &hash),
        )]),
    };
    let ae_unilateral_qc = ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "node-alpha".into(),
        prev_log_index: 1,
        prev_log_term: 1,
        entries: vec![],
        leader_commit: 1,
        commit_qc: Some(unilateral_qc),
    };
    let resp_unilateral = beta
        .handle_signed_rpc(alpha.sign_rpc(ae_unilateral_qc))
        .unwrap()
        .unwrap();
    if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_unilateral {
        assert!(!success);
    }
    assert_eq!(beta.commit_index, 0);
    assert_eq!(beta.rejected_byzantine_packets, 3);

    let valid_qc = QuorumCertificate {
        term: 1,
        index: 1,
        entry_hash: hash,
        signatures: BTreeMap::from([
            (
                "node-alpha".into(),
                alpha.identity.sign_qc_vote(1, 1, &hash),
            ),
            ("node-beta".into(), beta.identity.sign_qc_vote(1, 1, &hash)),
            (
                "node-gamma".into(),
                gamma.identity.sign_qc_vote(1, 1, &hash),
            ),
        ]),
    };
    let ae_valid_commit = ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "node-alpha".into(),
        prev_log_index: 1,
        prev_log_term: 1,
        entries: vec![],
        leader_commit: 1,
        commit_qc: Some(valid_qc),
    };
    let resp_valid = beta
        .handle_signed_rpc(alpha.sign_rpc(ae_valid_commit))
        .unwrap()
        .unwrap();
    if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp_valid {
        assert!(success);
    }
    assert_eq!(beta.commit_index, 1);
}

#[tokio::test]
async fn proof_9_network_quic_mtls_and_signed_rpc_rejects_untrusted_tls_and_forged_wire_packets(
) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let ep_victim = make_quic_endpoint("127.0.0.1:0".parse()?)?;
    let victim_addr = ep_victim.local_addr()?;
    let victim_node = MeshNode {
        node_id: "node-beta".into(),
        endpoint: ep_victim,
        state: Arc::new(Mutex::new(ConsensusState::new(
            "node-beta",
            vec![
                "node-alpha".into(),
                "node-gamma".into(),
                "node-delta".into(),
            ],
        ))),
        peer_addrs: BTreeMap::new(),
    };
    let _srv = victim_node.spawn_server();

    let rogue_pki = ClusterMtlsPki::generate()?;
    let ep_rogue_tls = rogue_pki.make_endpoint("127.0.0.1:0".parse()?)?;
    let rogue_conn_res = tokio::time::timeout(
        Duration::from_millis(250),
        ep_rogue_tls.connect(victim_addr, "veil.mesh")?,
    )
    .await;
    if let Ok(Ok(conn)) = rogue_conn_res {
        let bi_res = conn.open_bi().await;
        if let Ok((mut s, mut r)) = bi_res {
            let _ = s.write_all(b"test").await;
            let _ = s.finish();
            assert!(r.read_to_end(1024).await.is_err());
        }
    }

    let ep_insider = make_quic_endpoint("127.0.0.1:0".parse()?)?;
    let attacker_identity = NodeCryptoIdentity::from_seed("mallory", &[0x77; 32]);
    let forged_lockdown_rpc = ConsensusRpc::AppendEntries {
        term: 500,
        leader_id: "node-alpha".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        entries: vec![LogEntry {
            term: 500,
            index: 1,
            command: MeshCommand::set_lockdown("*", true),
        }],
        leader_commit: 1,
        commit_qc: None,
    };
    let mut forged_signed = attacker_identity.sign_rpc(forged_lockdown_rpc);
    forged_signed.signer_id = "node-alpha".into();

    let _ = send_rpc_over_quic(&ep_insider, victim_addr, forged_signed).await;

    let st = victim_node.state.lock().await;
    assert_eq!(st.commit_index, 0);
    assert_eq!(st.check_lsm_socket_connect(), 0);
    assert_eq!(st.rejected_byzantine_packets, 1);

    Ok(())
}

#[test]
fn proof_10_lying_follower_match_index_cannot_trigger_two_node_commit() {
    let (mut alpha, mut beta, mut gamma, mut delta) = make_four_node_cluster();

    let rv = alpha.start_signed_election();
    for peer in [&mut beta, &mut gamma, &mut delta] {
        let r = peer.handle_signed_rpc(rv.clone()).unwrap().unwrap();
        let sr = peer.sign_rpc(r);
        alpha.handle_signed_rpc(sr).unwrap();
    }
    assert_eq!(alpha.role, Role::Leader);

    alpha
        .propose(MeshCommand::set_trust(Ipv4Addr::new(10, 0, 0, 55), false))
        .unwrap();
    assert_eq!(alpha.commit_index, 0);

    let lying_resp = ConsensusRpc::AppendEntriesResponse {
        term: 1,
        follower_id: "node-gamma".into(),
        success: true,
        match_index: 1,
        entry_hash: [0xFF; 32],
        qc_vote_signature: vec![0x11; 64],
    };
    alpha.handle_signed_rpc(gamma.sign_rpc(lying_resp)).unwrap();
    assert_eq!(alpha.commit_index, 0);
    assert_eq!(alpha.rejected_byzantine_packets, 1);

    let ae_gamma = alpha.sign_rpc(alpha.make_append_entries("node-gamma").unwrap());
    let valid_gamma_resp = gamma.handle_signed_rpc(ae_gamma).unwrap().unwrap();
    alpha
        .handle_signed_rpc(gamma.sign_rpc(valid_gamma_resp))
        .unwrap();
    assert_eq!(alpha.commit_index, 0);

    let ae_beta = alpha.sign_rpc(alpha.make_append_entries("node-beta").unwrap());
    let valid_beta_resp = beta.handle_signed_rpc(ae_beta).unwrap().unwrap();
    alpha
        .handle_signed_rpc(beta.sign_rpc(valid_beta_resp))
        .unwrap();
    assert_eq!(alpha.commit_index, 1);
}

#[test]
fn proof_11_boundary_validation_index_zero_and_term_max_no_panic_or_saturation() {
    let (mut alpha, mut beta, _gamma, _delta) = make_four_node_cluster();

    let bad_index_zero_ae = ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "node-alpha".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        entries: vec![LogEntry {
            term: 1,
            index: 0,
            command: MeshCommand::NoOp,
        }],
        leader_commit: 0,
        commit_qc: None,
    };
    let resp = beta
        .handle_signed_rpc(alpha.sign_rpc(bad_index_zero_ae))
        .unwrap()
        .unwrap();
    if let ConsensusRpc::AppendEntriesResponse { success, .. } = resp {
        assert!(!success);
    }
    assert_eq!(beta.rejected_byzantine_packets, 1);
    assert!(beta.log.is_empty());

    for bad_term in [u64::MAX, crate::domain::MAX_SAFE_TERM] {
        let term_max_rv = ConsensusRpc::RequestVote {
            term: bad_term,
            candidate_id: "node-alpha".into(),
            last_log_index: 0,
            last_log_term: 0,
        };
        let err = beta
            .handle_signed_rpc(alpha.sign_rpc(term_max_rv))
            .unwrap_err();
        assert!(err.contains("MAX_SAFE_TERM"));
        assert_eq!(beta.current_term, 0);
    }

    // ノード名 ("node-alpha") から SHA-256 で秘密鍵を推測しようとする攻撃 -> CSPRNG 鍵のため失敗する！
    let mut guessed_input = Vec::from(&b"VEIL_MESH_NODE_DEFAULT_SEED_V1:node-alpha"[..]);
    let guessed_digest = ring::digest::digest(&ring::digest::SHA256, &guessed_input);
    guessed_input.clear();
    let mut guessed_seed = [0u8; 32];
    guessed_seed.copy_from_slice(guessed_digest.as_ref());
    let guessed_alpha = NodeCryptoIdentity::from_seed("node-alpha", &guessed_seed);
    let forged_rv = ConsensusRpc::RequestVote {
        term: 1,
        candidate_id: "node-alpha".into(),
        last_log_index: 0,
        last_log_term: 0,
    };
    assert!(beta
        .handle_signed_rpc(guessed_alpha.sign_rpc(forged_rv))
        .is_err());
    assert_eq!(beta.rejected_byzantine_packets, 4);

    // ドメイン方針ガード（Protected IP: 127.0.0.1 の誤遮断防止）のテスト
    alpha.role = Role::Leader;
    alpha.current_term = 1;
    assert!(
        alpha
            .propose(MeshCommand::set_trust(Ipv4Addr::LOCALHOST, false))
            .is_err(),
        "Domain SecurityPolicyGuard must block attempts to XDP_DROP localhost (127.0.0.1)!"
    );
}

// ==============================================================================
// R1〜R10 セキュリティ監査（2026-10-23）回帰テストスイート
// ==============================================================================

#[allow(clippy::too_many_arguments)]
fn audit_append(
    leader: &ConsensusState,
    follower: &mut ConsensusState,
    term: u64,
    prev_log_index: u64,
    prev_log_term: u64,
    entries: Vec<LogEntry>,
    leader_commit: u64,
    commit_qc: Option<QuorumCertificate>,
) -> ConsensusRpc {
    follower
        .handle_signed_rpc(leader.sign_rpc(ConsensusRpc::AppendEntries {
            term,
            leader_id: leader.node_id.clone(),
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit,
            commit_qc,
        }))
        .unwrap()
        .unwrap()
}

fn audit_sig(resp: ConsensusRpc) -> Vec<u8> {
    match resp {
        ConsensusRpc::AppendEntriesResponse {
            success: true,
            qc_vote_signature,
            ..
        } => qc_vote_signature,
        other => panic!("expected accepted AppendEntries, got {other:?}"),
    }
}

fn audit_qc(
    a: &ConsensusState,
    term: u64,
    index: u64,
    entry_hash: [u8; 32],
    others: Vec<(String, Vec<u8>)>,
) -> QuorumCertificate {
    let mut signatures: BTreeMap<String, Vec<u8>> = others.into_iter().collect();
    signatures.insert(
        a.node_id.clone(),
        a.identity.sign_qc_vote(term, index, &entry_hash),
    );
    QuorumCertificate {
        term,
        index,
        entry_hash,
        signatures,
    }
}

#[test]
fn regression_r1_tail_qc_rejects_conflicting_prefixes() {
    let (a, mut b, mut c, _d) = make_four_node_cluster();
    let ip = Ipv4Addr::new(203, 0, 113, 55);
    let x = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(ip, false),
    };
    let y = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(ip, true),
    };
    let tail = LogEntry {
        term: 1,
        index: 2,
        command: MeshCommand::NoOp,
    };

    let bs = audit_sig(audit_append(
        &a,
        &mut b,
        1,
        0,
        0,
        vec![x, tail.clone()],
        0,
        None,
    ));
    let cs = audit_sig(audit_append(
        &a,
        &mut c,
        1,
        0,
        0,
        vec![y, tail.clone()],
        0,
        None,
    ));

    // R1 検証: B と C の index=2 の累積チェーンハッシュは必ず異なる
    let b_chain_hash = b.entry_hash_at(2).unwrap();
    let c_chain_hash = c.entry_hash_at(2).unwrap();
    assert_ne!(b_chain_hash, c_chain_hash);

    // 悪意あるリーダー A が tail.digest() や b_chain_hash で QC を偽造しても 3/4 定足数が成立せず棄却される
    for forged_hash in [tail.digest(), b_chain_hash, c_chain_hash] {
        let qc = audit_qc(
            &a,
            1,
            2,
            forged_hash,
            vec![
                (b.node_id.clone(), bs.clone()),
                (c.node_id.clone(), cs.clone()),
            ],
        );
        let resp_b = audit_append(&a, &mut b, 1, 2, 1, vec![], 2, Some(qc.clone()));
        let resp_c = audit_append(&a, &mut c, 1, 2, 1, vec![], 2, Some(qc));
        assert!(matches!(
            resp_b,
            ConsensusRpc::AppendEntriesResponse { success: false, .. }
        ));
        assert!(matches!(
            resp_c,
            ConsensusRpc::AppendEntriesResponse { success: false, .. }
        ));
    }

    assert_eq!((b.commit_index, c.commit_index), (0, 0));
}

#[test]
fn regression_r2_hidden_qc_rejects_conflicting_commits_across_terms() {
    let (a, mut b, mut c, mut d) = make_four_node_cluster();
    let ip = Ipv4Addr::new(203, 0, 113, 55);
    let x = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(ip, false),
    };
    let y = LogEntry {
        term: 2,
        index: 1,
        command: MeshCommand::set_trust(ip, true),
    };

    let bs = audit_sig(audit_append(&a, &mut b, 1, 0, 0, vec![x.clone()], 0, None));
    let cs = audit_sig(audit_append(&a, &mut c, 1, 0, 0, vec![x.clone()], 0, None));
    let qx = audit_qc(
        &a,
        1,
        1,
        x.digest(),
        vec![(b.node_id.clone(), bs), (c.node_id.clone(), cs)],
    );
    // B だけに qx を配送して確定させる
    audit_sig(audit_append(&a, &mut b, 1, 1, 1, vec![], 1, Some(qx)));
    assert_eq!(b.commit_index, 1);
    assert_eq!(b.check_xdp_packet(ip), XdpVerdict::Drop);

    // A が Term 2 で C の未コミット（だが投票ロック済み）の x を QC なしで y に上書きしようとしても C は拒否する
    let resp_c = audit_append(&a, &mut c, 2, 0, 0, vec![y.clone()], 0, None);
    assert!(matches!(
        resp_c,
        ConsensusRpc::AppendEntriesResponse { success: false, .. }
    ));
    assert!(c.rejected_byzantine_packets >= 1);

    // D だけが y に投票しても 2 票（A と D）しか集まらず、3/4 定足数に満たないため C・D ともに y を確定できない
    let ds = audit_sig(audit_append(&a, &mut d, 2, 0, 0, vec![y.clone()], 0, None));
    let qy_insufficient = audit_qc(&a, 2, 1, y.digest(), vec![(d.node_id.clone(), ds)]);
    let resp_d_commit = audit_append(&a, &mut d, 2, 1, 2, vec![], 1, Some(qy_insufficient));
    assert!(matches!(
        resp_d_commit,
        ConsensusRpc::AppendEntriesResponse { success: false, .. }
    ));
    assert_eq!(c.commit_index, 0);
    assert_eq!(d.commit_index, 0);
}

#[test]
fn regression_r2_term_sandwich_prevents_double_voting_on_same_slot() {
    let (a, mut b, _c, _d) = make_four_node_cluster();
    let x = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::NoOp,
    };
    let z = LogEntry {
        term: 2,
        index: 1,
        command: MeshCommand::NoOp,
    };
    let y = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust(Ipv4Addr::new(203, 0, 113, 55), false),
    };

    let _sx = audit_sig(audit_append(&a, &mut b, 1, 0, 0, vec![x], 0, None));
    // Term 2 の QC なし上書き自体が拒否され、さらに Term 1 の別内容 y の混ぜ込みも拒否される
    let resp_z = audit_append(&a, &mut b, 2, 0, 0, vec![z], 0, None);
    assert!(matches!(
        resp_z,
        ConsensusRpc::AppendEntriesResponse { success: false, .. }
    ));
    let resp_y = audit_append(&a, &mut b, 2, 0, 0, vec![y.clone()], 0, None);
    match resp_y {
        ConsensusRpc::AppendEntriesResponse {
            success,
            qc_vote_signature,
            ..
        } => {
            assert!(!success);
            assert!(qc_vote_signature.is_empty());
            assert!(!NodeCryptoIdentity::verify_signature(
                &b.identity.public_key_bytes,
                &crate::domain::qc_vote_payload(1, 1, &y.digest()),
                &qc_vote_signature
            ));
        }
        other => panic!("unexpected response: {other:?}"),
    }
}

#[tokio::test]
async fn regression_r3_multi_process_shared_ca_pki_handshakes_over_mtls() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let temp_dir =
        std::env::temp_dir().join(format!("veil-mesh-r3-pki-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);

    // 独立した2つの ClusterMtlsPki インスタンスが同じディレクトリから共通 CA をロードして mTLS 通信できることを検証
    let pki_proc_1 = ClusterMtlsPki::load_or_generate_dir(&temp_dir)?;
    let pki_proc_2 = ClusterMtlsPki::load_or_generate_dir(&temp_dir)?;

    let ep_1 = pki_proc_1.make_endpoint("127.0.0.1:0".parse()?)?;
    let ep_2 = pki_proc_2.make_endpoint("127.0.0.1:0".parse()?)?;
    let addr_1 = ep_1.local_addr()?;

    let node_1 = MeshNode {
        node_id: "node-alpha".into(),
        endpoint: ep_1,
        state: Arc::new(Mutex::new(ConsensusState::new(
            "node-alpha",
            vec!["node-beta".into()],
        ))),
        peer_addrs: BTreeMap::new(),
    };
    let _srv = node_1.spawn_server();

    crate::infra::send_ping_round(&ep_2, addr_1, "node-beta", 1).await?;
    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(())
}

#[test]
fn regression_r4_apply_error_is_not_reported_as_applied() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct FailingBridge(AtomicUsize);
    impl crate::usecase::EbpfBridge for FailingBridge {
        fn sync_peer_policy(&self, _: u32, _: veil_common::PeerPolicy) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("injected map update failure")
        }
        fn sync_lsm_policy(&self, _: veil_common::NodeLsmPolicy) -> Result<()> {
            anyhow::bail!("injected map update failure")
        }
        fn stealth_gc_purge_count(&self) -> u64 {
            0
        }
    }

    let bridge = Arc::new(FailingBridge(AtomicUsize::new(0)));
    let mut node = ConsensusState::new("single-node", vec![]).with_ebpf_bridge(bridge.clone());
    node.start_election();
    let ip = Ipv4Addr::new(203, 0, 113, 55);
    node.propose(MeshCommand::set_trust(ip, false)).unwrap();

    assert_eq!(bridge.0.load(Ordering::SeqCst), 1);
    assert_eq!(node.commit_index, 1);
    // R4 修正後: ブリッジ書込み失敗時は last_applied が進まず、誤って Drop 適用済みとして報告しない
    assert_eq!(node.last_applied, 0);
    assert!(node.last_apply_error.is_some());
    assert_eq!(node.check_xdp_packet(ip), XdpVerdict::Pass);
}

#[tokio::test]
async fn regression_r4_propose_and_commit_fails_when_peers_unreachable() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (mut alpha, mut beta, mut gamma, _delta) = make_four_node_cluster();
    let vote = alpha.start_signed_election();
    for peer in [&mut beta, &mut gamma] {
        let response = peer.handle_signed_rpc(vote.clone()).unwrap().unwrap();
        alpha.handle_signed_rpc(peer.sign_rpc(response)).unwrap();
    }
    assert_eq!(alpha.role, Role::Leader);

    let unavailable_peer = std::net::UdpSocket::bind("127.0.0.1:0")?;
    let unavailable_addr = unavailable_peer.local_addr()?;
    let endpoint = make_quic_endpoint("127.0.0.1:0".parse()?)?;
    let node = MeshNode {
        node_id: "node-alpha".into(),
        endpoint,
        state: Arc::new(Mutex::new(alpha)),
        peer_addrs: BTreeMap::from([
            ("node-beta".into(), unavailable_addr),
            ("node-gamma".into(), unavailable_addr),
            ("node-delta".into(), unavailable_addr),
        ]),
    };

    let res = node
        .propose_and_commit(MeshCommand::set_trust(Ipv4Addr::new(10, 0, 0, 99), false))
        .await;
    assert!(
        res.is_err(),
        "propose_and_commit must fail when peers are unreachable and entry is uncommitted"
    );
    let state = node.state.lock().await;
    assert_eq!(state.commit_index, 0);
    assert_eq!(state.last_applied, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn regression_r8_insecure_key_file_permissions_are_rejected() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let unique_dir =
        std::env::temp_dir().join(format!("veil-mesh-r8-key-perm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&unique_dir);
    std::fs::create_dir_all(&unique_dir)?;
    let key_path = unique_dir.join("review-generated-identity.pk8");

    let _original = NodeCryptoIdentity::load_or_generate_file("permission-check", &key_path)?;
    let created_mode = std::fs::metadata(&key_path)?.permissions().mode() & 0o777;
    assert_eq!(created_mode, 0o600);

    // 0644 に緩められた鍵ファイルは読み込み時に PermissionDenied で拒否される
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644))?;
    let err = NodeCryptoIdentity::load_or_generate_file("permission-check", &key_path)
        .err()
        .expect("0644 key file must be rejected");
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

    let _ = std::fs::remove_dir_all(&unique_dir);
    Ok(())
}

#[test]
fn regression_r9_catch_up_batch_is_chunked_within_receiver_limit() {
    let (mut a, mut b, _c, _d) = make_four_node_cluster();
    a.role = Role::Leader;
    a.current_term = 1;
    for _ in 0..=crate::domain::MAX_LOG_ENTRIES_PER_RPC {
        a.propose(MeshCommand::NoOp).unwrap();
    }

    // 1回目の送信で MAX_LOG_ENTRIES_PER_RPC (1024件) がチャンク送信され、フォロワー B に受理される
    let msg1 = a.sign_rpc(a.make_append_entries(&b.node_id).unwrap());
    let resp1 = b.handle_signed_rpc(msg1).unwrap().unwrap();
    assert!(matches!(
        resp1,
        ConsensusRpc::AppendEntriesResponse {
            success: true,
            match_index: 1024,
            ..
        }
    ));
    a.handle_signed_rpc(b.sign_rpc(resp1)).unwrap();

    // 2回目の送信で残りの1件 (index=1025) が送信され、1025件すべて追随完了する
    let msg2 = a.sign_rpc(a.make_append_entries(&b.node_id).unwrap());
    let resp2 = b.handle_signed_rpc(msg2).unwrap().unwrap();
    assert!(matches!(
        resp2,
        ConsensusRpc::AppendEntriesResponse {
            success: true,
            match_index: 1025,
            ..
        }
    ));
    assert_eq!(b.log.len(), 1025);
}

#[test]
fn regression_r10_last_accepted_term_never_revotes_in_same_term() {
    let (a, mut b, _c, _d) = make_four_node_cluster();
    b.handle_signed_rpc(a.sign_rpc(ConsensusRpc::RequestVote {
        term: crate::domain::MAX_SAFE_TERM - 1,
        candidate_id: a.node_id.clone(),
        last_log_index: 0,
        last_log_term: 0,
    }))
    .unwrap();
    assert_eq!(b.voted_for, Some(a.node_id.clone()));

    let previous = b.current_term;
    b.start_election();
    // MAX_SAFE_TERM - 1 からの選挙では Term が MAX_SAFE_TERM へ単調増加する
    assert_eq!(b.current_term, previous + 1);
    assert_eq!(b.voted_for, Some(b.node_id.clone()));

    // さらに MAX_SAFE_TERM 上限に張り付いた状態では、それ以上 Term を増やせないため voted_for を上書きしない
    b.voted_for = Some(a.node_id.clone());
    b.start_election();
    assert_eq!(b.current_term, crate::domain::MAX_SAFE_TERM);
    assert_eq!(b.voted_for, Some(a.node_id));
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn regression_r6_r7_linux_xdp_source_port_no_bypass_and_map_capacity() -> Result<()> {
    use std::os::fd::{AsFd, AsRawFd};
    #[repr(C)]
    #[derive(Default)]
    struct TestRun {
        prog_fd: u32,
        retval: u32,
        data_size_in: u32,
        data_size_out: u32,
        data_in: u64,
        data_out: u64,
        repeat: u32,
        duration: u32,
        ctx_size_in: u32,
        ctx_size_out: u32,
        ctx_in: u64,
        ctx_out: u64,
        flags: u32,
        cpu: u32,
        batch_size: u32,
        padding: u32,
    }
    unsafe extern "C" {
        fn syscall(number: std::os::raw::c_long, ...) -> std::os::raw::c_long;
    }
    fn verdict(fd: i32, source_port: u16) -> Result<u32> {
        let mut packet = [0u8; 64];
        packet[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        packet[14] = 0x45;
        packet[16..18].copy_from_slice(&40u16.to_be_bytes());
        packet[22] = 64;
        packet[23] = 6;
        packet[26..30].copy_from_slice(&[203, 0, 113, 55]);
        packet[30..34].copy_from_slice(&[198, 51, 100, 10]);
        packet[34..36].copy_from_slice(&source_port.to_be_bytes());
        packet[36..38].copy_from_slice(&443u16.to_be_bytes());
        packet[46] = 0x50;
        packet[47] = 0x02;
        let mut output = [0u8; 128];
        let mut attr = TestRun {
            prog_fd: fd as u32,
            data_size_in: packet.len() as u32,
            data_size_out: output.len() as u32,
            data_in: packet.as_ptr() as u64,
            data_out: output.as_mut_ptr() as u64,
            repeat: 1,
            ..Default::default()
        };
        let result = unsafe { syscall(321, 10u32, &mut attr, std::mem::size_of::<TestRun>()) };
        if result < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(attr.retval)
    }
    let object = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("assets/veil_xdp_kern.o");
    let mut bpf = match aya::Ebpf::load_file(object) {
        Ok(b) => b,
        Err(_) => return Ok(()),
    };
    let program: &mut aya::programs::Xdp =
        bpf.program_mut("veil_xdp_filter").unwrap().try_into()?;
    if program.load().is_err() {
        return Ok(());
    }
    let fd = program.fd()?.as_fd().as_raw_fd();
    let blocked_ip = u32::from(Ipv4Addr::new(203, 0, 113, 55));
    let mut map: aya::maps::HashMap<_, u32, veil_common::PeerPolicy> =
        aya::maps::HashMap::try_from(bpf.map_mut("PEER_POLICY_MAP").unwrap())?;
    map.insert(blocked_ip, veil_common::PeerPolicy::new(false, 1), 0)?;

    // R7 修正後: 送信元ポートが 40000 でも 22 でも 10023 でもすべて XDP_DROP (1) となる
    assert_eq!(verdict(fd, 40000)?, 1);
    assert_eq!(verdict(fd, 22)?, 1);
    assert_eq!(verdict(fd, 10023)?, 1);

    // R6 修正後: 257件以上のエントリ挿入も成功する（容量 4096）
    for i in 0..300u32 {
        map.insert(0xc0000200u32 + i, veil_common::PeerPolicy::new(false, 1), 0)?;
    }
    Ok(())
}
