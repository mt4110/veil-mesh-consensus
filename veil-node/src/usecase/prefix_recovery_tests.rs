//! Counterexamples and safety constraints for prefix recovery. A passing
//! `reproduces_*` test means the limitation was reproduced, not repaired.
use super::regression_tests::{cluster, elect, fixture_dir};
use super::*;

fn proposal(command: MeshCommand) -> ConsensusRpc {
    ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "a".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        prev_log_hash: [0; 32],
        entries: vec![LogEntry {
            term: 1,
            index: 1,
            command,
        }],
        leader_commit: 0,
        commit_qc: None,
    }
}
fn fork() -> Vec<ConsensusState> {
    let mut n = cluster();
    elect(&mut n);
    let a = n[0].sign_rpc(proposal(MeshCommand::NoOp));
    let b = n[0].sign_rpc(proposal(MeshCommand::set_trust(
        Ipv4Addr::new(192, 0, 2, 82),
        true,
    )));
    for i in [1, 3] {
        n[i].handle_signed_rpc(a.clone()).unwrap();
    }
    n[2].handle_signed_rpc(b).unwrap();
    assert_eq!(n[1].entry_hash_at(1), n[3].entry_hash_at(1));
    assert_ne!(n[1].entry_hash_at(1), n[2].entry_hash_at(1));
    n
}
fn elect_b(n: &mut [ConsensusState]) {
    let pre = n[1].start_pre_vote().unwrap();
    for i in [2, 3] {
        let reply = n[i].handle_signed_rpc(pre.clone()).unwrap().unwrap();
        let signed = n[i].sign_rpc(reply);
        n[1].handle_signed_rpc(signed).unwrap();
    }
    let request = n[1].finish_pre_vote().unwrap().unwrap();
    for i in [2, 3] {
        let reply = n[i].handle_signed_rpc(request.clone()).unwrap().unwrap();
        let signed = n[i].sign_rpc(reply);
        n[1].handle_signed_rpc(signed).unwrap();
    }
    assert_eq!(n[1].role, Role::Leader);
}
#[test]
fn reproduces_conflicting_uncommitted_prefix_stall_after_successful_election() {
    let mut n = fork();
    elect_b(&mut n);
    n[1].propose(MeshCommand::NoOp).unwrap();
    for _ in 0..8 {
        for i in [2, 3] {
            let rpc = n[1].make_append_entries(&n[i].node_id).unwrap();
            let signed = n[1].sign_rpc(rpc);
            let response = n[i].handle_signed_rpc(signed).unwrap().unwrap();
            let signed = n[i].sign_rpc(response);
            n[1].handle_signed_rpc(signed).unwrap();
        }
    }
    assert_eq!(n[1].current_term, 2);
    assert_eq!(n[1].commit_index, 0);
    assert_eq!(n[1].qc_votes[&2].len(), 2);
    assert_ne!(n[1].entry_hash_at(1), n[2].entry_hash_at(1));
    println!("REPRODUCED: b elected in term 2; 8 replication rounds; commit_index=0; tip votes=2/3; divergent prefix retained");
}
#[test]
fn hidden_qc_survives_wal_and_naive_higher_term_rebase_can_create_conflicting_qcs() {
    let mut n = fork();
    let hash_a = n[1].entry_hash_at(1).unwrap();
    let qc_a = QuorumCertificate {
        term: 1,
        index: 1,
        entry_hash: hash_a,
        signatures: [0, 1, 3]
            .into_iter()
            .map(|i| {
                (
                    n[i].node_id.clone(),
                    n[i].identity
                        .sign_qc_vote(&n[i].signing_context, 1, 1, &hash_a),
                )
            })
            .collect(),
    };
    n[2].verify_qc_signatures_only(&qc_a).unwrap();
    // The faulty collector can retain a complete QC without distributing it.
    assert!(n[1..]
        .iter()
        .all(|s| s.commit_index == 0 && s.commit_qcs.is_empty()));
    let path = fixture_dir().join("b.wal");
    n[1].save_wal(&path).unwrap();
    let mut recovered = ConsensusState::new("b", vec!["a".into(), "c".into(), "d".into()]);
    recovered
        .set_crypto_identity(n[1].identity.clone())
        .unwrap();
    recovered
        .register_peer_public_keys(n[1].peer_public_keys.clone())
        .unwrap();
    recovered.load_wal(&path).unwrap();
    assert_eq!(recovered.commit_index, 0);
    assert!(recovered.commit_qcs.is_empty());
    assert_eq!(recovered.voted_entry_hashes[&(1, 1)], hash_a);
    recovered.verify_qc_signatures_only(&qc_a).unwrap();
    // Counterfactual only: a patch that permits a new-term overwrite with no
    // view-change proof can keep every (term,index) vote unique and still break
    // cross-term decision uniqueness. No product guard is disabled here.
    let replacement = LogEntry {
        term: 2,
        index: 1,
        command: MeshCommand::set_trust(Ipv4Addr::new(192, 0, 2, 82), true),
    };
    let hash_b = replacement.digest_with_parent(&[0; 32]);
    assert_ne!(hash_a, hash_b);
    let mut signatures = BTreeMap::new();
    for node in &mut n[1..] {
        assert!(!node.voted_entry_hashes.contains_key(&(2, 1)));
        node.voted_entry_hashes.insert((2, 1), hash_b);
        signatures.insert(
            node.node_id.clone(),
            node.identity
                .sign_qc_vote(&node.signing_context, 2, 1, &hash_b),
        );
    }
    let qc_b = QuorumCertificate {
        term: 2,
        index: 1,
        entry_hash: hash_b,
        signatures,
    };
    n[2].verify_qc_signatures_only(&qc_b).unwrap();
    n[2].verify_qc_signatures_only(&qc_a).unwrap();
    assert_eq!(n[1].voted_entry_hashes[&(1, 1)], hash_a);
    println!("REPRODUCED COUNTERFACTUAL: hidden QC(term=1,index=1,A) and new QC(term=2,index=1,B) both verify; preserving per-term vote history alone is insufficient");
}

fn qc_a(n: &[ConsensusState]) -> QuorumCertificate {
    let hash = n[1].entry_hash_at(1).unwrap();
    QuorumCertificate {
        term: 1,
        index: 1,
        entry_hash: hash,
        signatures: [0, 1, 3]
            .into_iter()
            .map(|i| {
                (
                    n[i].node_id.clone(),
                    n[i].identity
                        .sign_qc_vote(&n[i].signing_context, 1, 1, &hash),
                )
            })
            .collect(),
    }
}
fn certified_prefix(n: &[ConsensusState]) -> ConsensusRpc {
    let mut rpc = proposal(MeshCommand::NoOp);
    if let ConsensusRpc::AppendEntries {
        leader_commit,
        commit_qc,
        ..
    } = &mut rpc
    {
        *leader_commit = 1;
        *commit_qc = Some(qc_a(n));
    }
    rpc
}
fn deliver_from_b(n: &mut [ConsensusState], i: usize) -> ConsensusRpc {
    let rpc = n[1].make_append_entries(&n[i].node_id).unwrap();
    let signed = n[1].sign_rpc_durable(rpc).unwrap();
    let reply = n[i].handle_signed_rpc(signed).unwrap().unwrap();
    let signed = n[i].sign_rpc_durable(reply.clone()).unwrap();
    n[1].handle_signed_rpc(signed).unwrap();
    reply
}
fn restore(old: &ConsensusState, path: &std::path::Path) -> ConsensusState {
    let mut recovered = ConsensusState::new(&old.node_id, old.peers.clone());
    recovered.set_crypto_identity(old.identity.clone()).unwrap();
    recovered
        .register_peer_public_keys(old.peer_public_keys.clone())
        .unwrap();
    recovered.load_wal(path).unwrap();
    recovered
}

#[test]
fn certified_conflict_receipt_preserves_old_vote_across_restart_without_resigning() {
    let mut n = fork();
    let old_votes = n[2].voted_entry_hashes.clone();
    let path = fixture_dir().join("c.wal");
    n[2].enable_persistence(Arc::new(
        crate::infra::wal::FileWal::create_new(&path).unwrap(),
    ))
    .unwrap();
    let rpc = certified_prefix(&n);
    let signed = n[0].sign_rpc(rpc.clone());
    let reply = n[2].handle_signed_rpc(signed).unwrap().unwrap();
    assert!(
        matches!(reply, ConsensusRpc::AppendEntriesResponse { success: true, qc_vote_signature, .. } if qc_vote_signature.is_empty())
    );
    assert_eq!(n[2].voted_entry_hashes, old_votes);
    assert_eq!(n[2].commit_index, 1);
    assert_eq!(n[2].entry_hash_at(1), n[1].entry_hash_at(1));
    let mut recovered = restore(&n[2], &path);
    assert_eq!(recovered.voted_entry_hashes, old_votes);
    assert_eq!(recovered.commit_index, 1);
    let reply = recovered
        .handle_signed_rpc(n[0].sign_rpc(rpc))
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, ConsensusRpc::AppendEntriesResponse { success: true, qc_vote_signature, .. } if qc_vote_signature.is_empty())
    );
    assert_eq!(recovered.voted_entry_hashes, old_votes);
}

#[test]
fn known_qc_recovers_conflict_after_election_and_commits_new_finite_ban() {
    let mut n = fork();
    let old_votes = n[2].voted_entry_hashes.clone();
    let rpc = certified_prefix(&n);
    for i in [1, 3] {
        let signed = n[0].sign_rpc(rpc.clone());
        n[i].handle_signed_rpc(signed).unwrap();
    }
    elect_b(&mut n);
    n[1].propose(MeshCommand::NoOp).unwrap();
    // Same term/index but wrong parent: no mutation or vote on the minority fork.
    let old_log = n[2].log.clone();
    assert!(matches!(
        deliver_from_b(&mut n, 2),
        ConsensusRpc::AppendEntriesResponse { success: false, .. }
    ));
    assert_eq!(n[2].log, old_log);
    assert_eq!(n[2].voted_entry_hashes, old_votes);
    for _ in 0..3 {
        for i in [2, 3] {
            deliver_from_b(&mut n, i);
        }
    }
    assert!(n[1..].iter().all(|s| s.commit_index == 2));
    assert_eq!(n[2].voted_entry_hashes[&(1, 1)], old_votes[&(1, 1)]);
    assert_eq!(
        n[2].voted_entry_hashes[&(2, 2)],
        n[1].entry_hash_at(2).unwrap()
    );
    let ip = Ipv4Addr::new(192, 0, 2, 83);
    n[1].propose(MeshCommand::set_trust_with_ttl(ip, false, 30))
        .unwrap();
    for _ in 0..3 {
        for i in [2, 3] {
            deliver_from_b(&mut n, i);
        }
    }
    for state in &n[1..] {
        assert_eq!(state.commit_index, 3);
        assert_eq!(state.last_applied, 3);
        assert_eq!(state.check_xdp_packet(ip), veil_common::XdpVerdict::Drop);
        state
            .policy_guard
            .validate_log_command(&state.log[2].command)
            .unwrap();
    }
}

#[test]
fn forged_qc_uncovered_suffix_and_committed_conflict_cannot_unlock() {
    for attack in 0..3 {
        let mut n = fork();
        let mut rpc = certified_prefix(&n);
        match attack {
            0 => {
                if let ConsensusRpc::AppendEntries {
                    commit_qc: Some(qc),
                    ..
                } = &mut rpc
                {
                    qc.signatures.values_mut().next().unwrap()[0] ^= 1;
                }
            }
            1 => {
                // A QC at position 1 cannot authorize conflicting votes at position 2.
                let old = LogEntry {
                    term: 1,
                    index: 2,
                    command: MeshCommand::NoOp,
                };
                let hash = n[2].push_log_entry(old.clone());
                n[2].voted_entry_hashes.insert((1, 2), hash);
                if let ConsensusRpc::AppendEntries { entries, .. } = &mut rpc {
                    entries.push(old);
                }
            }
            _ => {
                n[2].commit_index = 1;
            } // Deliberate contradictory committed state: fail closed.
        }
        let log = n[2].log.clone();
        let votes = n[2].voted_entry_hashes.clone();
        let signed = n[0].sign_rpc(rpc);
        let reply = n[2].handle_signed_rpc(signed).unwrap().unwrap();
        assert!(
            matches!(reply, ConsensusRpc::AppendEntriesResponse { success: false, qc_vote_signature, .. } if qc_vote_signature.is_empty())
        );
        assert_eq!(n[2].log, log);
        assert_eq!(n[2].voted_entry_hashes, votes);
    }
}

#[test]
fn receipt_without_vote_advances_replication_only_for_committed_prefix() {
    let mut n = fork();
    let rpc = certified_prefix(&n);
    let signed = n[0].sign_rpc(rpc);
    n[1].handle_signed_rpc(signed).unwrap();
    elect_b(&mut n);
    let reply = ConsensusRpc::AppendEntriesResponse {
        term: n[1].current_term,
        follower_id: "c".into(),
        success: true,
        match_index: 1,
        entry_hash: n[1].entry_hash_at(1).unwrap(),
        qc_vote_signature: vec![],
    };
    let signed = n[2].sign_rpc(reply);
    n[1].handle_signed_rpc(signed).unwrap();
    assert_eq!(n[1].match_index["c"], 1);
    assert!(!n[1].qc_votes.get(&1).is_some_and(|v| v.contains_key("c")));
    n[1].propose(MeshCommand::NoOp).unwrap();
    let reply = ConsensusRpc::AppendEntriesResponse {
        term: n[1].current_term,
        follower_id: "c".into(),
        success: true,
        match_index: 2,
        entry_hash: n[1].entry_hash_at(2).unwrap(),
        qc_vote_signature: vec![],
    };
    let signed = n[2].sign_rpc(reply);
    n[1].handle_signed_rpc(signed).unwrap();
    assert_eq!(n[1].match_index["c"], 1);
    assert_eq!(n[1].commit_index, 1);
    assert_eq!(n[1].qc_votes[&2].len(), 1);
}

#[test]
fn recovered_leader_cannot_overwrite_vote_for_a_discarded_suffix_slot() {
    let mut n = fork();
    elect_b(&mut n);
    let old = n[1].propose(MeshCommand::NoOp).unwrap();
    let vote = n[1].voted_entry_hashes[&(old.term, old.index)];
    // Model a certified repair that discards a suffix but retains its vote fence.
    n[1].truncate_log(1);
    assert!(n[1]
        .propose(MeshCommand::set_trust(Ipv4Addr::new(192, 0, 2, 84), true))
        .is_err());
    assert_eq!(n[1].last_log_index(), 1);
    assert_eq!(n[1].voted_entry_hashes[&(old.term, old.index)], vote);
}

#[test]
fn repair_persistence_failure_leaks_no_vote_or_receipt_and_old_wal_survives() {
    struct Fail;
    impl ConsensusPersistence for Fail {
        fn persist(&self, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("injected repair write failure"))
        }
    }
    let mut n = fork();
    let path = fixture_dir().join("c.wal");
    n[2].save_wal(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let old_log = n[2].log.clone();
    let old_votes = n[2].voted_entry_hashes.clone();
    n[2].persistence = Some(Arc::new(Fail));
    let rpc = certified_prefix(&n);
    let signed = n[0].sign_rpc(rpc);
    assert!(n[2].handle_signed_rpc(signed).is_err());
    assert!(n[2].persistence_error.is_some());
    assert_eq!(n[2].last_applied, 0);
    assert_eq!(n[2].voted_entry_hashes, old_votes);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let recovered = restore(&n[2], &path);
    assert_eq!(recovered.log, old_log);
    assert_eq!(recovered.voted_entry_hashes, old_votes);
    assert_eq!(recovered.commit_index, 0);
}

#[tokio::test]
async fn certified_prefix_recovery_over_node_tls_quic_and_normal_driver() -> anyhow::Result<()> {
    use crate::domain::{FixedConfiguration, Member};
    use crate::infra::{
        daemon, local_control, node_tls::NodeTls, node_tls_tests::Authority, wal::FileWal,
        ClusterMtlsPki,
    };
    use std::time::{Duration, Instant};
    use tokio::sync::Mutex;
    struct Clock {
        start: Instant,
    }
    impl PolicyClock for Clock {
        fn boot_id(&self) -> &str {
            "prefix-network-fixture"
        }
        fn now_ns(&self) -> std::io::Result<u64> {
            Ok(1_000_000_000 + self.start.elapsed().as_nanos() as u64)
        }
        fn lease_time(&self) -> std::io::Result<LeaseTime> {
            let boot_ns = self.now_ns()?;
            Ok(LeaseTime {
                boot_ns,
                unix_ns: SIMULATION_WALL_EPOCH_NS + boot_ns,
                uncertainty_ns: 0,
                max_slow_rate_ppm: 0,
            })
        }
    }
    struct Tasks(Vec<tokio::task::JoinHandle<()>>);
    impl Drop for Tasks {
        fn drop(&mut self) {
            for t in &self.0 {
                t.abort();
            }
        }
    }
    let _ = rustls::crypto::ring::default_provider().install_default();
    use std::os::unix::fs::DirBuilderExt;
    let root = std::env::temp_dir().join(format!(
        "vp-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let authority = Authority::new();
    let pki = ClusterMtlsPki::generate()?;
    let mut endpoints: Vec<_> = (0..4)
        .map(|_| pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap())
        .collect();
    let mut n = cluster();
    let members = n
        .iter()
        .zip(&endpoints)
        .map(|(st, ep)| Member {
            node_id: st.node_id.clone(),
            address: ep.local_addr().unwrap(),
            public_key: st.identity.public_key_bytes.as_slice().try_into().unwrap(),
        })
        .collect();
    let config = FixedConfiguration::new("prefix-network", 1, members, vec![], 1000)
        .map_err(anyhow::Error::msg)?;
    let clock = Arc::new(Clock {
        start: Instant::now(),
    });
    let mut tls = Vec::new();
    for (st, endpoint) in n.iter_mut().zip(&mut endpoints) {
        st.configure_fixed_membership(config.clone(), st.identity.clone())
            .map_err(anyhow::Error::msg)?;
        st.configure_policy_clock(clock.clone())?;
        st.ebpf_bridge = Some(Arc::new(crate::ebpf_bridge::MockEbpfBridge::default()));
        st.enable_persistence(Arc::new(FileWal::create_new(
            &root.join(format!("{}.wal", st.node_id)),
        )?))
        .map_err(anyhow::Error::msg)?;
        let bundle = authority.bundle(
            &root.join(format!("tls-{}", st.node_id)),
            &config,
            &st.node_id,
        );
        let credential = Arc::new(NodeTls::load(&bundle, &config, &st.node_id)?);
        credential.configure_test_endpoint(endpoint);
        tls.push(credential);
    }
    elect(&mut n);
    let a = n[0].sign_rpc(proposal(MeshCommand::NoOp));
    let b = n[0].sign_rpc(proposal(MeshCommand::set_trust(
        Ipv4Addr::new(192, 0, 2, 82),
        true,
    )));
    for i in [1, 3] {
        n[i].handle_signed_rpc(a.clone()).unwrap();
    }
    n[2].handle_signed_rpc(b).unwrap();
    let old_vote = n[2].voted_entry_hashes[&(1, 1)];
    let rpc = n[0].sign_rpc(certified_prefix(&n));
    for i in [1, 3] {
        n[i].handle_signed_rpc(rpc.clone()).unwrap();
    }
    // Seed the elected leader through the normal signed election path. From
    // here, repair/NoOp/BAN replication is driven exclusively by normal daemon.
    elect_b(&mut n);
    let states: Vec<_> = n.into_iter().map(|st| Arc::new(Mutex::new(st))).collect();
    endpoints[0].close(0u32.into(), b"old faulty leader offline");
    let mut tasks = Tasks(vec![]);
    for i in 1..4 {
        let endpoint = endpoints[i].clone();
        let state = states[i].clone();
        let credential = tls[i].clone();
        let id = state.lock().await.node_id.clone();
        let server_id = id.clone();
        tasks.0.push(tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let st = state.clone();
                let tls = credential.clone();
                let id = server_id.clone();
                tokio::spawn(async move {
                    if let Ok(conn) = incoming.await {
                        let _ =
                            crate::infra::handle_authenticated_connection(conn, &id, st, tls).await;
                    }
                });
            }
        }));
        let listener = local_control::bind(&root.join(format!("{id}.sock")))?;
        let state = states[i].clone();
        tasks.0.push(tokio::spawn(async move {
            local_control::serve(listener, state).await.unwrap();
        }));
        let endpoint = endpoints[i].clone();
        let state = states[i].clone();
        let tls = tls[i].clone();
        let peers = config.peer_addrs(&id);
        tasks.0.push(tokio::spawn(async move {
            daemon::run(endpoint, state, peers, tls).await.unwrap();
        }));
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut done = true;
            for st in &states[1..] {
                done &= st.lock().await.last_applied >= 2;
            }
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await?;
    let ip = Ipv4Addr::new(192, 0, 2, 85);
    let receipt = local_control::submit_ban(&root.join("b.sock"), ip, 4).await?;
    assert!(receipt.starts_with("ACCEPTED "));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let mut done = true;
            for st in &states[1..] {
                done &= st.lock().await.check_xdp_packet(ip) == veil_common::XdpVerdict::Drop;
            }
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await?;
    let expected_command = states[1].lock().await.log[2].command.clone();
    for st in &states[1..] {
        assert_eq!(st.lock().await.log[2].command, expected_command);
    }
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let mut done = true;
            for st in &states[1..] {
                done &= st.lock().await.check_xdp_packet(ip) == veil_common::XdpVerdict::Pass;
            }
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await?;
    let c = states[2].lock().await;
    assert_eq!(c.voted_entry_hashes[&(1, 1)], old_vote);
    let mut recovered = ConsensusState::new("c", vec![]);
    recovered
        .configure_fixed_membership(config, c.identity.clone())
        .map_err(anyhow::Error::msg)?;
    recovered.configure_policy_clock(clock)?;
    recovered.load_wal(&root.join("c.wal"))?;
    assert_eq!(recovered.voted_entry_hashes[&(1, 1)], old_vote);
    assert_eq!(recovered.log, c.log);
    assert_eq!(recovered.commit_index, c.commit_index);
    assert_eq!(
        recovered.check_xdp_packet(ip),
        veil_common::XdpVerdict::Pass
    );
    println!("PASS: node-specific TLS/QUIC, normal daemon repaired QC-covered fork, committed finite BAN, expired original command, WAL retained minority vote; packet backend is Mock");
    Ok(())
}

#[test]
fn old_rpc_domain_is_rejected_without_changing_qc_or_wal_context() {
    let mut n = cluster();
    let seed = [93; 32]; // Public test seed, never a runtime identity.
    n[0].set_crypto_identity(Arc::new(NodeCryptoIdentity::from_seed("a", &seed)))
        .unwrap();
    let keys = n
        .iter()
        .map(|st| (st.node_id.clone(), st.identity.public_key_bytes.clone()))
        .collect::<BTreeMap<_, _>>();
    for st in &mut n {
        st.register_peer_public_keys(keys.clone()).unwrap();
    }
    let rpc = ConsensusRpc::RequestVote {
        term: 1,
        candidate_id: "a".into(),
        last_log_index: 0,
        last_log_term: 0,
    };
    let legacy_payload =
        bincode::serialize(&("VEIL_RPC_V3", &n[0].signing_context, "a", &rpc)).unwrap();
    let key = ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    let legacy = SignedConsensusRpc {
        context: n[0].signing_context.clone(),
        signer_id: "a".into(),
        rpc: rpc.clone(),
        signature: key.sign(&legacy_payload).as_ref().to_vec(),
    };
    assert!(n[1].handle_signed_rpc(legacy).is_err());
    assert_eq!(n[1].current_term, 0);
    assert!(n[1].voted_for.is_none());
    let signed = n[0].sign_rpc(rpc);
    assert!(matches!(
        n[1].handle_signed_rpc(signed).unwrap(),
        Some(ConsensusRpc::RequestVoteResponse {
            vote_granted: true,
            ..
        })
    ));
    assert_eq!(n[1].signing_context.protocol_version, 3);
}
