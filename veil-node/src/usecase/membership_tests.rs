use super::*;
use crate::{
    domain::{Member, MAX_MEMBERS},
    ebpf_bridge::MockEbpfBridge,
    infra::wal::FileWal,
};

fn fixture() -> (Vec<Arc<NodeCryptoIdentity>>, Vec<Member>) {
    let ids: Vec<_> = ["a", "b", "c", "d"]
        .into_iter()
        .map(|n| Arc::new(NodeCryptoIdentity::generate_random(n)))
        .collect();
    let members = ids
        .iter()
        .enumerate()
        .map(|(i, id)| Member {
            node_id: id.node_id.clone(),
            address: format!("127.0.0.1:{}", 21001 + i).parse().unwrap(),
            public_key: id.public_key_bytes.as_slice().try_into().unwrap(),
        })
        .collect();
    (ids, members)
}
fn config(members: Vec<Member>, protected: Vec<Ipv4Addr>, error: u64) -> FixedConfiguration {
    FixedConfiguration::new("site", 9, members, protected, error).unwrap()
}
fn state(config: &FixedConfiguration, identity: &Arc<NodeCryptoIdentity>) -> ConsensusState {
    let mut node = ConsensusState::new(&identity.node_id, vec!["must-be-replaced".into()]);
    node.configure_fixed_membership(config.clone(), identity.clone())
        .unwrap();
    node
}
fn committed(cfg: &FixedConfiguration, ids: &[Arc<NodeCryptoIdentity>]) -> Vec<ConsensusState> {
    let mut nodes: Vec<_> = ids.iter().map(|id| state(cfg, id)).collect();
    super::regression_tests::elect(&mut nodes);
    nodes[0]
        .propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(192, 0, 2, 9),
            false,
            300,
        ))
        .unwrap();
    for i in [1, 2, 1] {
        let rpc = nodes[0].make_append_entries(&nodes[i].node_id).unwrap();
        let signed = nodes[0].sign_rpc(rpc);
        let reply = nodes[i].handle_signed_rpc(signed).unwrap().unwrap();
        let signed_reply = nodes[i].sign_rpc(reply);
        nodes[0].handle_signed_rpc(signed_reply).unwrap();
    }
    assert_eq!(nodes[0].commit_index, 1);
    nodes
}

#[test]
fn canonical_configuration_ignores_order_and_normalizes_addresses() {
    let (_, members) = fixture();
    let policy = vec![Ipv4Addr::new(192, 0, 2, 50), Ipv4Addr::new(192, 0, 2, 51)];
    let original = config(members.clone(), policy.clone(), 1000);
    let mut reordered = members.clone();
    reordered.reverse();
    reordered[0].address = "[::ffff:127.0.0.1]:21004".parse().unwrap();
    let mut reordered_policy = policy;
    reordered_policy.reverse();
    reordered_policy.push(reordered_policy[0]);
    let equivalent = config(reordered, reordered_policy, 1000);
    assert_eq!(original.context(), equivalent.context());
    assert_eq!(original.members(), equivalent.members());
    assert_ne!(original.context().config_digest, [0; 32]);
    let key: String = members[0]
        .public_key
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    let parsed: Member = format!("a@127.0.0.1:21001={key}").parse().unwrap();
    assert_eq!(parsed, members[0]);
}

#[test]
fn malformed_membership_cannot_create_a_voter_set() {
    let (_, members) = fixture();
    let reject = |m| assert!(FixedConfiguration::new("site", 9, m, vec![], 1000).is_err());
    reject(vec![]);
    reject(vec![members[0].clone(); MAX_MEMBERS + 1]);
    for variant in 0..7 {
        let mut bad = members.clone();
        match variant {
            0 => bad[1].node_id = bad[0].node_id.clone(),
            1 => bad[1].public_key = bad[0].public_key,
            2 => bad[1].address = bad[0].address,
            3 => bad[1].address = "[::ffff:127.0.0.1]:21001".parse().unwrap(),
            4 => bad[0].node_id = "../outside".into(),
            5 => bad[0].public_key = [0; 32],
            _ => bad[0].node_id = "x".repeat(129),
        }
        reject(bad);
    }
    for endpoint in [
        "0.0.0.0:1",
        "127.0.0.1:0",
        "224.0.0.1:1",
        "255.255.255.255:1",
        "[::]:1",
        "[ff02::1]:1",
        "[fe80::1%2]:1",
    ] {
        let mut bad = members.clone();
        bad[0].address = endpoint.parse().unwrap();
        reject(bad);
    }
    for raw in [
        "a",
        "a@host:5000=abcd",
        "a@127.0.0.1:5000=xyz",
        &format!("a@127.0.0.1:5000={}", "g".repeat(64)),
    ] {
        assert!(raw.parse::<Member>().is_err());
    }
    assert!(FixedConfiguration::new("site", 9, members.clone(), vec![], 0).is_err());
    assert!(
        FixedConfiguration::new("site", 9, members, vec![Ipv4Addr::LOCALHOST; 257], 1000).is_err()
    );
}

#[test]
fn local_identity_must_match_and_installed_membership_cannot_reload() {
    let (ids, members) = fixture();
    let cfg = config(members.clone(), vec![], 1000);
    let mut missing = ConsensusState::new("absent", vec![]);
    let missing_id = NodeCryptoIdentity::generate_random("absent");
    let before = missing.wal_bytes().unwrap();
    assert!(missing
        .configure_fixed_membership(cfg.clone(), missing_id)
        .is_err());
    assert_eq!(missing.wal_bytes().unwrap(), before);
    let mut wrong = ConsensusState::new("a", vec![]);
    assert!(wrong
        .configure_signing_context(cfg.context().clone())
        .is_err());
    assert!(wrong
        .configure_fixed_membership(cfg.clone(), ids[1].clone())
        .is_err());
    assert!(wrong
        .configure_fixed_membership(cfg.clone(), NodeCryptoIdentity::generate_random("a"))
        .is_err());
    assert_eq!(wrong.signing_context().config_digest, [0; 32]);
    let mut installed = state(&cfg, &ids[0]);
    assert_eq!(installed.peers, vec!["b", "c", "d"]);
    assert_eq!(installed.cluster_size(), 4);
    assert_eq!(installed.quorum_size(), 3);
    assert_eq!(installed.peer_public_keys, cfg.public_keys());
    let before = installed.wal_bytes().unwrap();
    assert!(installed
        .register_peer_public_keys(BTreeMap::new())
        .is_err());
    assert!(installed.set_crypto_identity(ids[1].clone()).is_err());
    assert!(installed
        .configure_signing_context(SigningContext::simulation())
        .is_err());
    assert!(installed
        .configure_fixed_membership(cfg.clone(), ids[0].clone())
        .is_err());
    assert_eq!(installed.wal_bytes().unwrap(), before);
    installed.start_signed_election().unwrap();
    // Unknown IDs cannot acquire votes by possessing a valid but unlisted key.
    let outsider = NodeCryptoIdentity::generate_random("outsider");
    let rpc = ConsensusRpc::RequestVote {
        term: 99,
        candidate_id: "outsider".into(),
        last_log_term: 0,
        last_log_index: 0,
    };
    let before = installed.wal_bytes().unwrap();
    assert!(installed
        .handle_signed_rpc(outsider.sign_rpc(cfg.context(), rpc))
        .is_err());
    assert_eq!(installed.wal_bytes().unwrap(), before);
    // All IPv4 member endpoints are protected by the installed guard.
    assert!(installed
        .policy_guard
        .validate_command(&MeshCommand::set_trust_with_ttl(
            Ipv4Addr::LOCALHOST,
            false,
            300
        ))
        .is_err());
}

#[test]
fn every_security_relevant_config_change_rejects_rpc_qc_tc_and_wal() {
    let (ids, members) = fixture();
    let original = config(members.clone(), vec![], 1000);
    let mut source = committed(&original, &ids);
    source[0].next_index.insert("b".into(), 1);
    let qc = source[0].commit_qcs[&1].clone();
    let append = source[0].make_append_entries("b").unwrap();
    let old_rpc = source[0].sign_rpc(append.clone());
    let tc = TimeoutCertificate {
        term: 10,
        highest_qc: None,
        votes: ids[..3]
            .iter()
            .map(|id| {
                (
                    id.node_id.clone(),
                    (0, 0, id.sign_timeout_vote(original.context(), 10, 0, 0)),
                )
            })
            .collect(),
    };
    source[0].verify_timeout_certificate(&tc).unwrap();
    let path = super::regression_tests::fixture_dir().join("b.wal");
    source[1].save_wal(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    for variant in 0..7 {
        let mut members = members.clone();
        let mut protected = vec![];
        let mut error = 1000;
        match variant {
            0 => {
                members[3].public_key = NodeCryptoIdentity::generate_random("d")
                    .public_key_bytes
                    .as_slice()
                    .try_into()
                    .unwrap()
            }
            1 => members[3].address = "127.0.0.1:22004".parse().unwrap(),
            2 => members[3].node_id = "d-renamed".into(),
            3 => {
                members.pop();
            }
            4 => protected.push(Ipv4Addr::new(192, 0, 2, 77)),
            5 => error = 500,
            _ => members.push(Member {
                node_id: "e".into(),
                address: "127.0.0.1:21005".parse().unwrap(),
                public_key: NodeCryptoIdentity::generate_random("e")
                    .public_key_bytes
                    .as_slice()
                    .try_into()
                    .unwrap(),
            }),
        }
        let changed = config(members, protected, error);
        assert_eq!(changed.context().cluster_id, original.context().cluster_id);
        assert_eq!(
            changed.context().config_epoch,
            original.context().config_epoch
        );
        assert_ne!(
            changed.context().config_digest,
            original.context().config_digest
        );
        let bridge = Arc::new(MockEbpfBridge::default());
        let mut receiver = state(&changed, &ids[1]).with_ebpf_bridge(bridge.clone());
        let before = receiver.wal_bytes().unwrap();
        assert!(receiver.handle_signed_rpc(old_rpc.clone()).is_err());
        let mut relabeled = old_rpc.clone();
        relabeled.context = changed.context().clone();
        assert!(receiver.handle_signed_rpc(relabeled).is_err());
        // A listed Byzantine leader can re-sign the envelope, but not the old quorum.
        let rebound = ids[0].sign_rpc(changed.context(), append.clone());
        let reply = receiver.handle_signed_rpc(rebound).unwrap().unwrap();
        assert!(
            matches!(reply,ConsensusRpc::AppendEntriesResponse{success:false,ref qc_vote_signature,..} if qc_vote_signature.is_empty())
        );
        assert!(receiver.verify_qc_signatures_only(&qc).is_err());
        assert!(receiver.apply_timeout_certificate(tc.clone()).is_err());
        assert!(receiver.load_wal(&path).is_err());
        assert_eq!(receiver.wal_bytes().unwrap(), before);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(bridge.kernel_peer_map.lock().unwrap().is_empty());
        assert!(receiver.timeout_votes.is_empty());
    }
    let mut reordered = members;
    reordered.reverse();
    let equivalent = config(reordered, vec![], 1000);
    let mut restored = state(&equivalent, &ids[1]);
    restored.load_wal(&path).unwrap();
    assert_eq!(restored.log, source[1].log);
    assert_eq!(restored.voted_entry_hashes, source[1].voted_entry_hashes);
}

#[test]
fn fixed_membership_quorum_counts_only_distinct_listed_keys() {
    let (ids, members) = fixture();
    let cfg = config(members, vec![], 1000);
    let mut nodes: Vec<_> = ids.iter().map(|id| state(&cfg, id)).collect();
    super::regression_tests::elect(&mut nodes);
    nodes[0].propose(MeshCommand::NoOp).unwrap();
    for i in [1, 1, 2] {
        let signed = nodes[0].sign_rpc(nodes[0].make_append_entries(&nodes[i].node_id).unwrap());
        let reply = nodes[i].handle_signed_rpc(signed).unwrap().unwrap();
        let signed = nodes[i].sign_rpc(reply);
        nodes[0].handle_signed_rpc(signed).unwrap();
        assert_eq!(nodes[0].commit_index, if i == 2 { 1 } else { 0 });
    }
    let mut qc = nodes[0].commit_qcs[&1].clone();
    let extra = NodeCryptoIdentity::generate_random("outsider");
    qc.signatures.remove("c");
    qc.signatures.insert(
        "outsider".into(),
        extra.sign_qc_vote(cfg.context(), qc.term, qc.index, &qc.entry_hash),
    );
    assert!(nodes[3].verify_qc_signatures_only(&qc).is_err());
}

#[tokio::test]
async fn fixed_config_digest_rejects_same_epoch_replay_over_real_mtls_quic() {
    use crate::infra::{send_rpc_over_quic, ClusterMtlsPki, MeshNode};
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (ids, mut members) = fixture();
    let pki = ClusterMtlsPki::generate().unwrap();
    let endpoints: Vec<_> = (0..4)
        .map(|_| pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap())
        .collect();
    for (member, endpoint) in members.iter_mut().zip(&endpoints) {
        member.address = endpoint.local_addr().unwrap();
    }
    let old_cfg = config(members.clone(), vec![], 1000);
    let source = committed(&old_cfg, &ids);
    let old_append = source[0].make_append_entries("d").unwrap();
    let cfg = config(members, vec![Ipv4Addr::new(192, 0, 2, 77)], 1000);
    let mut nodes: Vec<_> = ids.iter().map(|id| state(&cfg, id)).collect();
    let mut receiver = nodes.pop().unwrap();
    let path = super::regression_tests::fixture_dir().join("d.wal");
    receiver
        .enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut endpoints = endpoints;
    let server = MeshNode {
        node_id: "d".into(),
        endpoint: endpoints.pop().unwrap(),
        state: Arc::new(tokio::sync::Mutex::new(receiver)),
        peer_addrs: cfg.peer_addrs("d"),
    };
    let task = server.spawn_server();
    struct Abort(tokio::task::JoinHandle<()>);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _guard = Abort(task);
    assert!(send_rpc_over_quic(
        &endpoints[0],
        server.local_addr().unwrap(),
        source[0].sign_rpc(old_append.clone())
    )
    .await
    .unwrap()
    .is_none());
    let rejected = send_rpc_over_quic(
        &endpoints[0],
        server.local_addr().unwrap(),
        nodes[0].sign_rpc(old_append),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        rejected.rpc,
        ConsensusRpc::AppendEntriesResponse { success: false, .. }
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(server.state.lock().await.commit_index, 0);
    // The new, matching configuration still forms a quorum with the same keys.
    let current = committed(&cfg, &ids);
    let reply = send_rpc_over_quic(
        &endpoints[0],
        server.local_addr().unwrap(),
        current[0].sign_rpc(current[0].make_append_entries("d").unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        reply.rpc,
        ConsensusRpc::AppendEntriesResponse { success: true, .. }
    ));
    let s = server.state.lock().await;
    assert_eq!(s.commit_index, 1);
    assert_eq!(s.last_applied, 1);
    println!("fixed_membership_wire: same ID/epoch/keys/CA; foreign digest RPC/QC rejected; WAL unchanged; matching QC committed");
    drop(s);
    server.endpoint.close(0u32.into(), b"done");
    for e in &endpoints {
        e.close(0u32.into(), b"done");
    }
    server.endpoint.wait_idle().await;
    for e in &endpoints {
        e.wait_idle().await;
    }
}
