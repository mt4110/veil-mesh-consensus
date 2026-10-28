// Append to usecase/consensus_engine.rs in an isolated copy of commit 7e9473c.
// Passing tests demonstrate defects, not safety. No existing files are deleted.
#[cfg(test)]
mod v091_review {
    use super::*;
    use crate::adapter::WireMessage;
    use crate::ebpf_bridge::MockEbpfBridge;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn rejected_unrelated_qc_mutates_log_and_enables_conflicting_commit() {
        let mut n = cluster();
        elect(&mut n);
        let ip = Ipv4Addr::new(192, 0, 2, 99);
        let x = n[0].propose(MeshCommand::set_trust(ip, false)).unwrap();
        // A collects the first QC but only B learns its commit.
        for i in 1..4 {
            let rpc = ConsensusRpc::AppendEntries {
                term: 1,
                leader_id: "a".into(),
                prev_log_index: 0,
                prev_log_term: 0,
                entries: vec![x.clone()],
                leader_commit: 0,
                commit_qc: None,
            };
            let signed = n[0].sign_rpc(rpc);
            let resp = n[i].handle_signed_rpc(signed).unwrap().unwrap();
            let signed = n[i].sign_rpc(resp);
            n[0].handle_signed_rpc(signed).unwrap();
        }
        let rpc = n[0].make_append_entries("b").unwrap();
        let signed = n[0].sign_rpc(rpc);
        n[1].handle_signed_rpc(signed).unwrap();
        assert_eq!(n[1].commit_index, 1);
        // A can legitimately obtain a higher QC for a NoOp extending X.
        elect(&mut n);
        let noop = n[0].propose(MeshCommand::NoOp).unwrap();
        for i in 2..4 {
            let rpc = ConsensusRpc::AppendEntries {
                term: 2,
                leader_id: "a".into(),
                prev_log_index: 1,
                prev_log_term: 1,
                entries: vec![noop.clone()],
                leader_commit: 0,
                commit_qc: None,
            };
            let signed = n[0].sign_rpc(rpc);
            let resp = n[i].handle_signed_rpc(signed).unwrap().unwrap();
            let signed = n[i].sign_rpc(resp);
            n[0].handle_signed_rpc(signed).unwrap();
        }
        let unrelated_qc = n[0].commit_qcs[&2].clone();
        let y = LogEntry {
            term: 2,
            index: 1,
            command: MeshCommand::set_trust(ip, true),
        };
        let mut signatures = BTreeMap::new();
        signatures.insert("a".into(), n[0].identity.sign_qc_vote(2, 1, &y.digest()));
        for i in 2..4 {
            let rpc = ConsensusRpc::AppendEntries {
                term: 2,
                leader_id: "a".into(),
                prev_log_index: 0,
                prev_log_term: 0,
                entries: vec![y.clone()],
                leader_commit: 0,
                commit_qc: Some(unrelated_qc.clone()),
            };
            let signed = n[0].sign_rpc(rpc);
            let rejected = n[i].handle_signed_rpc(signed).unwrap().unwrap();
            assert!(matches!(
                rejected,
                ConsensusRpc::AppendEntriesResponse { success: false, .. }
            ));
            // Rejection happens after replacing the previously locked log.
            assert_eq!(n[i].log, vec![y.clone()]);
            let rpc = ConsensusRpc::AppendEntries {
                term: 2,
                leader_id: "a".into(),
                prev_log_index: 1,
                prev_log_term: 2,
                entries: vec![],
                leader_commit: 0,
                commit_qc: None,
            };
            let signed = n[0].sign_rpc(rpc);
            let resp = n[i].handle_signed_rpc(signed).unwrap().unwrap();
            if let ConsensusRpc::AppendEntriesResponse {
                success: true,
                qc_vote_signature,
                ..
            } = resp
            {
                signatures.insert(n[i].node_id.clone(), qc_vote_signature);
            } else {
                panic!("expected signed success");
            }
        }
        let qc_y = QuorumCertificate {
            term: 2,
            index: 1,
            entry_hash: y.digest(),
            signatures,
        };
        for i in 2..4 {
            let rpc = ConsensusRpc::AppendEntries {
                term: 2,
                leader_id: "a".into(),
                prev_log_index: 1,
                prev_log_term: 2,
                entries: vec![],
                leader_commit: 1,
                commit_qc: Some(qc_y.clone()),
            };
            let signed = n[0].sign_rpc(rpc);
            n[i].handle_signed_rpc(signed).unwrap();
            assert_eq!(n[i].commit_index, 1);
            assert_eq!(n[i].check_xdp_packet(ip), XdpVerdict::Pass);
        }
        assert_eq!(n[1].commit_index, 1);
        assert_eq!(n[1].check_xdp_packet(ip), XdpVerdict::Drop);
        println!(
            "one Byzantine node; honest B committed DROP, honest C/D committed PASS at index 1"
        );
    }

    fn cluster() -> Vec<ConsensusState> {
        let names = ["a", "b", "c", "d"];
        let mut states: Vec<_> = names
            .iter()
            .map(|me| {
                ConsensusState::new(
                    *me,
                    names
                        .iter()
                        .filter(|n| *n != me)
                        .map(|n| n.to_string())
                        .collect(),
                )
            })
            .collect();
        let keys = states
            .iter()
            .map(|s| (s.node_id.clone(), s.identity.public_key_bytes.clone()))
            .collect::<BTreeMap<_, _>>();
        for s in &mut states {
            s.register_peer_public_keys(keys.clone());
        }
        states
    }

    fn elect(states: &mut [ConsensusState]) {
        let req = states[0].start_signed_election();
        for i in 1..states.len() {
            let resp = states[i].handle_signed_rpc(req.clone()).unwrap().unwrap();
            let signed = states[i].sign_rpc(resp);
            states[0].handle_signed_rpc(signed).unwrap();
        }
    }

    fn replicate(states: &mut [ConsensusState]) {
        for i in 1..states.len() {
            let rpc = states[0].make_append_entries(&states[i].node_id).unwrap();
            let signed = states[0].sign_rpc(rpc);
            let resp = states[i].handle_signed_rpc(signed).unwrap().unwrap();
            let signed = states[i].sign_rpc(resp);
            states[0].handle_signed_rpc(signed).unwrap();
        }
    }

    // Encode precisely the current snapshot format without calling save_wal's
    // remove_file/rename path. This fixture tests load/replay, not fsync durability.
    fn retained_wal_fixture(s: &ConsensusState, label: &str) -> std::path::PathBuf {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        static SERIAL: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "veil-v091-review-{label}-{}-{}.wal",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::SeqCst)
        ));
        let payload = WalPayload {
            version: WAL_MAGIC_VERSION,
            node_id: s.node_id.clone(),
            current_term: s.current_term,
            voted_for: s.voted_for.clone(),
            log: s.log.clone(),
            commit_index: s.commit_index,
            commit_qcs: s.commit_qcs.clone(),
            voted_entry_hashes: s
                .voted_entry_hashes
                .iter()
                .map(|(&(t, i), &h)| (t, i, h))
                .collect(),
        };
        let payload_bytes = bincode::serialize(&payload).unwrap();
        let checksum = ring::digest::digest(&ring::digest::SHA256, &payload_bytes);
        let envelope = WalEnvelope {
            payload_bytes: payload_bytes.clone(),
            sha256_checksum: checksum.as_ref().try_into().unwrap(),
        };
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        f.write_all(&bincode::serialize(&envelope).unwrap())
            .unwrap();
        f.sync_all().unwrap();
        println!("retained_fixture={}", path.display());
        path
    }

    #[test]
    fn bypass_changes_simulator_but_leaves_bridge_blocked() {
        let bridge = Arc::new(MockEbpfBridge::default());
        let mut s = ConsensusState::new("solo", vec![]).with_ebpf_bridge(bridge.clone());
        s.start_election();
        let ip = Ipv4Addr::new(192, 0, 2, 1);
        s.propose(MeshCommand::set_trust_with_ttl(ip, false, 60))
            .unwrap();
        s.set_emergency_local_bypass(true);
        assert_eq!(s.check_xdp_packet(ip), XdpVerdict::Pass);
        assert_eq!(
            evaluate_peer_policy(bridge.kernel_peer_map.lock().unwrap().get(&u32::from(ip))),
            XdpVerdict::Drop
        );
    }

    #[test]
    fn elapsed_time_does_not_advance_ttl() {
        let mut s = ConsensusState::new("solo", vec![]);
        s.start_election();
        let ip = Ipv4Addr::new(192, 0, 2, 2);
        s.propose(MeshCommand::set_trust_with_ttl(ip, false, 1))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert_eq!(s.monotonic_now_ns, 0);
        assert_eq!(s.expire_stale_policies(), 0);
        assert_eq!(s.check_xdp_packet(ip), XdpVerdict::Drop);
    }

    #[test]
    fn expired_rule_and_bypass_are_lost_on_replay() {
        let mut s = ConsensusState::new("solo", vec![]);
        s.start_election();
        let ip = Ipv4Addr::new(192, 0, 2, 3);
        s.propose(MeshCommand::set_trust_with_ttl(ip, false, 60))
            .unwrap();
        assert_eq!(s.advance_monotonic_clock(61), 1);
        s.set_emergency_local_bypass(true);
        let path = retained_wal_fixture(&s, "expiry");
        let mut restored = ConsensusState::new("solo", vec![]);
        restored.identity = s.identity.clone();
        restored.register_peer_public_keys(s.peer_public_keys.clone());
        restored.load_wal(&path).unwrap();
        assert_eq!(restored.check_xdp_packet(ip), XdpVerdict::Drop);
        assert_eq!(
            restored.peer_rule_deadlines_ns[&u32::from(ip)],
            60_000_000_000
        );
        assert!(!restored.emergency_local_bypass);
    }

    struct RecoverableBridge {
        fail: AtomicBool,
        calls: AtomicUsize,
    }
    impl EbpfBridge for RecoverableBridge {
        fn sync_peer_policy(&self, _: u32, _: PeerPolicy) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                anyhow::bail!("injected apply failure");
            }
            Ok(())
        }
        fn sync_lsm_policy(&self, _: NodeLsmPolicy) -> anyhow::Result<()> {
            Ok(())
        }
        fn stealth_gc_purge_count(&self) -> u64 {
            0
        }
    }

    #[test]
    fn heartbeat_does_not_retry_failed_follower_apply() {
        let mut nodes = cluster();
        let bridge = Arc::new(RecoverableBridge {
            fail: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
        });
        nodes[1].ebpf_bridge = Some(bridge.clone());
        elect(&mut nodes);
        nodes[0]
            .propose(MeshCommand::set_trust(Ipv4Addr::new(192, 0, 2, 4), false))
            .unwrap();
        replicate(&mut nodes);
        replicate(&mut nodes);
        assert_eq!((nodes[1].commit_index, nodes[1].last_applied), (1, 0));
        bridge.fail.store(false, Ordering::SeqCst);
        for _ in 0..5 {
            replicate(&mut nodes);
        }
        assert_eq!(bridge.calls.load(Ordering::SeqCst), 1);
        assert_eq!(nodes[1].last_applied, 0);
    }

    #[test]
    fn entry_chunking_still_exceeds_wire_byte_limit() {
        let mut nodes = cluster();
        elect(&mut nodes);
        for _ in 0..64 {
            nodes[0]
                .propose(MeshCommand::set_trust_batch(
                    (1..=128)
                        .map(|i| PeerTrustRule::new(Ipv4Addr::new(192, 0, 2, i), false, 60))
                        .collect(),
                ))
                .unwrap();
        }
        let rpc = nodes[0].make_append_entries("b").unwrap();
        let signed = nodes[0].sign_rpc(rpc);
        let bytes = bincode::serialize(&WireMessage::Consensus(signed.clone())).unwrap();
        println!(
            "wire_bytes={};limit={}",
            bytes.len(),
            MAX_WIRE_MESSAGE_BYTES
        );
        assert!(bytes.len() > MAX_WIRE_MESSAGE_BYTES);
        assert!(nodes[1].handle_signed_rpc(signed).is_err());
        assert_eq!(nodes[1].last_log_index(), 0);
    }

    #[test]
    fn divergent_uncommitted_prefixes_stall_honest_successor() {
        let mut nodes = cluster();
        // Only a is Byzantine; it sends one different, individually valid prefix
        // to each honest node. All later honest-to-honest messages are delivered.
        for i in 1..4 {
            let entry = LogEntry {
                term: 1,
                index: 1,
                command: MeshCommand::set_trust(Ipv4Addr::new(192, 0, 2, i as u8), false),
            };
            let rpc = ConsensusRpc::AppendEntries {
                term: 1,
                leader_id: "a".into(),
                prev_log_index: 0,
                prev_log_term: 0,
                entries: vec![entry],
                leader_commit: 0,
                commit_qc: None,
            };
            let signed = nodes[0].sign_rpc(rpc);
            let resp = nodes[i].handle_signed_rpc(signed).unwrap().unwrap();
            assert!(matches!(
                resp,
                ConsensusRpc::AppendEntriesResponse { success: true, .. }
            ));
        }
        for _ in 0..3 {
            let votes: Vec<_> = nodes[1..]
                .iter_mut()
                .map(|s| s.trigger_view_timeout())
                .collect();
            for vote in votes {
                for s in &mut nodes[1..] {
                    s.handle_signed_rpc(vote.clone()).unwrap();
                }
            }
            let leader = (1..4).find(|&i| nodes[i].role == Role::Leader).unwrap();
            nodes[leader].propose(MeshCommand::NoOp).unwrap();
            for _ in 0..8 {
                for i in 1..4 {
                    if i == leader {
                        continue;
                    }
                    let rpc = nodes[leader]
                        .make_append_entries(&nodes[i].node_id)
                        .unwrap();
                    let signed = nodes[leader].sign_rpc(rpc);
                    let resp = nodes[i].handle_signed_rpc(signed).unwrap().unwrap();
                    let signed = nodes[i].sign_rpc(resp);
                    nodes[leader].handle_signed_rpc(signed).unwrap();
                }
            }
            assert!(nodes[1..].iter().all(|s| s.commit_index == 0));
        }
        println!("three honest leader views, full delivery, all commit_index=0");
    }

    #[tokio::test]
    async fn wire_vote_is_returned_without_durable_vote_record() {
        use crate::infra::{send_rpc_over_quic, ClusterMtlsPki, MeshNode};
        use tokio::sync::Mutex;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut nodes = cluster();
        let follower = nodes.remove(1);
        let path = retained_wal_fixture(&follower, "initial");
        let before = std::fs::read(&path).unwrap();
        let follower_identity = follower.identity.clone();
        let keys = follower.peer_public_keys.clone();
        let pki = ClusterMtlsPki::generate().unwrap();
        let endpoint = pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let client = pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
        let server = MeshNode {
            node_id: "b".into(),
            endpoint,
            state: Arc::new(Mutex::new(follower)),
            peer_addrs: BTreeMap::new(),
        };
        let handle = server.spawn_server();
        let req = nodes[0].start_signed_election();
        let response = send_rpc_over_quic(&client, server.local_addr().unwrap(), req)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            response.rpc,
            ConsensusRpc::RequestVoteResponse {
                vote_granted: true,
                ..
            }
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut restored = ConsensusState::new("b", vec!["a".into(), "c".into(), "d".into()]);
        restored.identity = follower_identity;
        restored.register_peer_public_keys(keys);
        restored.load_wal(&path).unwrap();
        let req = nodes[1].start_signed_election();
        let response = restored.handle_signed_rpc(req).unwrap().unwrap();
        assert!(matches!(
            response,
            ConsensusRpc::RequestVoteResponse {
                term: 1,
                vote_granted: true,
                ..
            }
        ));
        handle.abort();
        server.endpoint.close(0u32.into(), b"review complete");
        client.close(0u32.into(), b"review complete");
    }
}
