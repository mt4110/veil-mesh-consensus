use super::*;
use crate::{adapter::WireMessage, ebpf_bridge::MockEbpfBridge, infra::wal::FileWal};

fn context(cluster: &str, epoch: u64) -> SigningContext {
    SigningContext::new(cluster, epoch).unwrap()
}
fn cluster(ctx: SigningContext, source: Option<&[ConsensusState]>) -> Vec<ConsensusState> {
    let mut nodes = super::regression_tests::cluster();
    for (i, node) in nodes.iter_mut().enumerate() {
        node.configure_signing_context(ctx.clone()).unwrap();
        if let Some(source) = source {
            // Deliberately reuse every identity/key: rejection must come from binding.
            node.identity = source[i].identity.clone();
            node.register_peer_public_keys(source[i].peer_public_keys.clone())
                .unwrap();
        }
    }
    nodes
}
fn committed(nodes: &mut [ConsensusState]) -> ConsensusRpc {
    super::regression_tests::elect(nodes);
    nodes[0]
        .propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(192, 0, 2, 9),
            false,
            300,
        ))
        .unwrap();
    for i in [1, 2] {
        let signed = nodes[0].sign_rpc(nodes[0].make_append_entries(&nodes[i].node_id).unwrap());
        let reply = nodes[i].handle_signed_rpc(signed).unwrap().unwrap();
        nodes[0]
            .handle_signed_rpc(nodes[i].sign_rpc(reply))
            .unwrap();
    }
    assert_eq!(nodes[0].commit_index, 1);
    nodes[0].make_append_entries("d").unwrap()
}

#[test]
fn invalid_context_and_live_context_changes_are_rejected() {
    for id in ["", "space id", "クラスタ", &"a".repeat(129)] {
        assert!(SigningContext::new(id, 1).is_err());
    }
    assert!(SigningContext::new("site", 0).is_err());
    assert!(SigningContext::new("a".repeat(128), u64::MAX).is_ok());
    let mut version = context("site", 1);
    version.protocol_version = 1;
    assert!(version.validate().is_err());
    let mut node = cluster(context("site", 1), None).remove(0);
    node.start_signed_election().unwrap();
    assert!(node.configure_signing_context(context("site", 2)).is_err());
    assert_eq!(node.signing_context(), &context("site", 1));
    let mut fresh = cluster(context("site", 1), None).remove(0);
    let path = super::regression_tests::fixture_dir().join("empty.wal");
    fresh
        .enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
        .unwrap();
    assert!(fresh.configure_signing_context(context("site", 2)).is_err());
}

#[test]
fn every_rpc_kind_and_header_relabel_are_bound_before_any_state_change() {
    let mut source = cluster(context("site-a", 7), None);
    let append = committed(&mut source);
    let messages = vec![
        ConsensusRpc::RequestVote {
            term: 99,
            candidate_id: "a".into(),
            last_log_index: 0,
            last_log_term: 0,
        },
        ConsensusRpc::RequestVoteResponse {
            term: 99,
            voter_id: "a".into(),
            vote_granted: true,
        },
        append,
        ConsensusRpc::AppendEntriesResponse {
            term: 99,
            follower_id: "a".into(),
            success: true,
            match_index: 1,
            entry_hash: [0; 32],
            qc_vote_signature: vec![],
        },
        ConsensusRpc::TimeoutVote {
            term: 99,
            voter_id: "a".into(),
            highest_qc: None,
            tc_vote_signature: vec![],
        },
    ];
    for target in [context("site-b", 7), context("site-a", 8)] {
        let mut receiver = cluster(target.clone(), Some(&source)).remove(3);
        let before = receiver.wal_bytes().unwrap();
        for rpc in &messages {
            let signed = source[0].sign_rpc(rpc.clone());
            assert!(receiver.handle_signed_rpc(signed.clone()).is_err());
            let mut relabeled = signed;
            relabeled.context = target.clone();
            assert!(receiver.handle_signed_rpc(relabeled).is_err());
            assert_eq!(receiver.wal_bytes().unwrap(), before);
        }
        assert!(receiver.timeout_votes.is_empty());
        assert_eq!(receiver.role, Role::Follower);
        assert!(receiver.ebpf_policy_map.is_empty());
    }
    let mut receiver = cluster(context("site-a", 7), Some(&source)).remove(3);
    let mut wrong_version = source[0].sign_rpc(messages[0].clone());
    wrong_version.context.protocol_version = 1;
    assert!(receiver.handle_signed_rpc(wrong_version).is_err());
    assert_eq!(receiver.current_term, 0);
}

#[test]
fn qc_and_timeout_signatures_cannot_be_transplanted_under_a_fresh_outer_signature() {
    let mut source = cluster(context("site-a", 7), None);
    let append = committed(&mut source);
    let qc = source[0].commit_qcs[&1].clone();
    let tc = TimeoutCertificate {
        term: 10,
        highest_qc: None,
        votes: source[..3]
            .iter()
            .map(|n| {
                (
                    n.node_id.clone(),
                    (
                        0,
                        0,
                        n.identity.sign_timeout_vote(n.signing_context(), 10, 0, 0),
                    ),
                )
            })
            .collect(),
    };
    source[0].verify_timeout_certificate(&tc).unwrap();
    for target in [context("site-b", 7), context("site-a", 8)] {
        let mut nodes = cluster(target.clone(), Some(&source));
        let before = nodes[3].wal_bytes().unwrap();
        assert!(nodes[3].verify_qc_signatures_only(&qc).is_err());
        assert!(nodes[3].apply_timeout_certificate(tc.clone()).is_err());
        let rebound = nodes[0].sign_rpc(append.clone());
        let reply = nodes[3].handle_signed_rpc(rebound).unwrap().unwrap();
        assert!(
            matches!(reply, ConsensusRpc::AppendEntriesResponse { success: false, ref qc_vote_signature, .. } if qc_vote_signature.is_empty())
        );
        // Test detached timeout signatures even with a correctly signed new-context RPC.
        let rpc = ConsensusRpc::TimeoutVote {
            term: 10,
            voter_id: "a".into(),
            highest_qc: None,
            tc_vote_signature: tc.votes["a"].2.clone(),
        };
        let signed = nodes[0].sign_rpc(rpc);
        assert!(nodes[3].handle_signed_rpc(signed).is_err());
        assert!(nodes[3].timeout_votes.is_empty());
        assert_eq!(nodes[3].wal_bytes().unwrap(), before);
        // Positive control: same keys and same log bytes can agree under the new context.
        let new_append = committed(&mut nodes);
        assert_eq!(nodes[0].entry_hash_at(1), source[0].entry_hash_at(1));
        let signed = nodes[0].sign_rpc(new_append);
        nodes[3].handle_signed_rpc(signed).unwrap();
        assert_eq!(nodes[3].commit_index, 1);
    }
}

#[test]
fn wal_context_mismatch_and_legacy_versions_fail_before_apply_without_rewriting_file() {
    let path = super::regression_tests::fixture_dir().join("a.wal");
    let mut source = cluster(context("site-a", 7), None);
    committed(&mut source);
    source[0].save_wal(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    for target in [context("site-b", 7), context("site-a", 8)] {
        let bridge = Arc::new(MockEbpfBridge::default());
        let mut receiver = cluster(target, Some(&source))
            .remove(0)
            .with_ebpf_bridge(bridge.clone());
        let before = receiver.wal_bytes().unwrap();
        assert!(receiver
            .load_wal(&path)
            .unwrap_err()
            .to_string()
            .contains("context mismatch"));
        assert_eq!(receiver.wal_bytes().unwrap(), before);
        assert!(bridge.kernel_peer_map.lock().unwrap().is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    let mut receiver = cluster(context("site-a", 7), Some(&source)).remove(0);
    receiver.load_wal(&path).unwrap();
    assert_eq!(receiver.commit_index, 1);
    assert_eq!(receiver.voted_entry_hashes, source[0].voted_entry_hashes);
    assert_eq!(receiver.commit_qcs, source[0].commit_qcs);
    for version in [1u32, 2, 3] {
        let old = super::regression_tests::fixture_dir().join("old.wal");
        let mut envelope: WalEnvelope = bincode::deserialize(&bytes).unwrap();
        envelope.payload_bytes[..4].copy_from_slice(&version.to_le_bytes());
        envelope.sha256_checksum =
            ring::digest::digest(&ring::digest::SHA256, &envelope.payload_bytes)
                .as_ref()
                .try_into()
                .unwrap();
        let old_bytes = bincode::serialize(&envelope).unwrap();
        FileWal::open(&old).unwrap().persist(&old_bytes).unwrap();
        let mut fresh = cluster(context("site-a", 7), Some(&source)).remove(0);
        assert!(fresh
            .load_wal(&old)
            .unwrap_err()
            .to_string()
            .contains("explicit migration"));
        assert_eq!(std::fs::read(&old).unwrap(), old_bytes);
        assert!(fresh.log.is_empty());
    }
}

#[test]
fn signed_wire_budget_accounts_for_maximum_context_length() {
    let mut nodes = cluster(context(&"a".repeat(128), u64::MAX), None);
    super::regression_tests::elect(&mut nodes);
    for _ in 0..32 {
        nodes[0]
            .propose(MeshCommand::set_trust_batch(
                (0..128)
                    .map(|i| PeerTrustRule::new(Ipv4Addr::from(0xc0000200u32 + i), false, 60))
                    .collect(),
            ))
            .unwrap();
    }
    let mut messages = 0;
    for _ in 0..10 {
        let signed = nodes[0].sign_rpc(nodes[0].make_append_entries("b").unwrap());
        assert!(
            bincode::serialized_size(&WireMessage::Consensus(signed.clone().into())).unwrap()
                <= MAX_WIRE_MESSAGE_BYTES as u64
        );
        let reply = nodes[1].handle_signed_rpc(signed).unwrap().unwrap();
        let signed_reply = nodes[1].sign_rpc(reply);
        nodes[0].handle_signed_rpc(signed_reply).unwrap();
        messages += 1;
        if nodes[1].last_log_index() == 32 {
            break;
        }
    }
    assert!(messages > 1);
    assert_eq!(nodes[1].last_log_index(), 32);
}

#[tokio::test]
async fn mtls_quic_rejects_cross_cluster_and_epoch_replay_with_reused_keys_and_ca() {
    use crate::infra::{send_rpc_over_quic, ClusterMtlsPki, MeshNode};
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut source = cluster(context("site-a", 7), None);
    let append = committed(&mut source);
    let pki = ClusterMtlsPki::generate().unwrap();
    let client = pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    for target in [context("site-b", 7), context("site-a", 8)] {
        let mut nodes = cluster(target.clone(), Some(&source));
        let mut receiver = nodes.pop().unwrap();
        let path = super::regression_tests::fixture_dir().join("d.wal");
        receiver
            .enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let server = MeshNode {
            node_id: "d".into(),
            endpoint: pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap(),
            state: Arc::new(tokio::sync::Mutex::new(receiver)),
            peer_addrs: BTreeMap::new(),
        };
        let task = server.spawn_server();
        let replay = source[0].sign_rpc(append.clone());
        assert!(
            send_rpc_over_quic(&client, server.local_addr().unwrap(), replay)
                .await
                .unwrap()
                .is_none()
        );
        let rebound = nodes[0].sign_rpc(append.clone());
        let refusal = send_rpc_over_quic(&client, server.local_addr().unwrap(), rebound)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(refusal.rpc, ConsensusRpc::AppendEntriesResponse {success:false, ref qc_vote_signature, ..} if qc_vote_signature.is_empty())
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(server.state.lock().await.ebpf_policy_map.is_empty());
        // Authenticate the refusal and then deliver a current-context quorum.
        // A fresh receiver remains at term 0 on rejection; authenticate its NACK
        // without feeding that out-of-range term into the consensus state machine.
        assert!(NodeCryptoIdentity::verify_signature(
            &nodes[0].peer_public_keys["d"],
            &rpc_signing_payload(&target, "d", &refusal.rpc),
            &refusal.signature,
        ));
        let new_append = committed(&mut nodes);
        let signed = nodes[0].sign_rpc(new_append);
        let accepted = send_rpc_over_quic(&client, server.local_addr().unwrap(), signed)
            .await
            .unwrap()
            .unwrap();
        nodes[0].handle_signed_rpc(accepted).unwrap();
        let state = server.state.lock().await;
        assert_eq!(state.commit_index, 1);
        assert_eq!(state.last_applied, 1);
        assert_eq!(
            state.check_xdp_packet(Ipv4Addr::new(192, 0, 2, 9)),
            XdpVerdict::Drop
        );
        println!("context_wire: cluster={} epoch={} old RPC/QC rejected; current QC committed; rejected WAL unchanged", target.cluster_id, target.config_epoch);
        drop(state);
        task.abort();
        server.endpoint.close(0u32.into(), b"test complete");
        server.endpoint.wait_idle().await;
    }
    client.close(0u32.into(), b"test complete");
    client.wait_idle().await;
}

#[test]
fn legacy_unbound_signatures_and_cross_purpose_signatures_are_rejected() {
    use ring::signature::{Ed25519KeyPair, KeyPair};
    let pair = Ed25519KeyPair::from_seed_unchecked(&[31; 32]).unwrap();
    let ctx = context("site", 1);
    let rpc = ConsensusRpc::RequestVote {
        term: 1,
        candidate_id: "a".into(),
        last_log_index: 0,
        last_log_term: 0,
    };
    let mut old_qc = b"VEIL_QC_VOTE_V1:".to_vec();
    old_qc.extend_from_slice(&1u64.to_be_bytes());
    old_qc.extend_from_slice(&1u64.to_be_bytes());
    old_qc.extend_from_slice(&[7; 32]);
    let mut old_tc = b"VEIL_TC_VOTE_V1:".to_vec();
    for n in [1u64, 0, 0] {
        old_tc.extend_from_slice(&n.to_be_bytes());
    }
    let old = [bincode::serialize(&rpc).unwrap(), old_qc, old_tc];
    let new = [
        rpc_signing_payload(&ctx, "a", &rpc),
        qc_vote_payload(&ctx, 1, 1, &[7; 32]),
        timeout_vote_payload(&ctx, 1, 0, 0),
    ];
    for (i, payload) in new.iter().enumerate() {
        assert!(!NodeCryptoIdentity::verify_signature(
            pair.public_key().as_ref(),
            payload,
            pair.sign(&old[i]).as_ref()
        ));
        for (j, signed_payload) in new.iter().enumerate() {
            assert_eq!(
                NodeCryptoIdentity::verify_signature(
                    pair.public_key().as_ref(),
                    payload,
                    pair.sign(signed_payload).as_ref()
                ),
                i == j
            );
        }
    }
}
