use super::*;
use crate::adapter::WireMessage;
use crate::ebpf_bridge::MockEbpfBridge;
use crate::infra::wal::FileWal;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(super) fn fixture_dir() -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("veil-fix-{}-{stamp}-{serial}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    dir
}

struct FaultingStore {
    disk: FileWal,
    fail: AtomicBool,
    fail_commit: bool,
}

impl ConsensusPersistence for FaultingStore {
    fn persist(&self, bytes: &[u8]) -> std::io::Result<()> {
        let envelope: WalEnvelope = bincode::deserialize(bytes).unwrap();
        let payload: WalPayload = bincode::deserialize(&envelope.payload_bytes).unwrap();
        if self.fail.load(Ordering::SeqCst) || (self.fail_commit && payload.commit_index > 0) {
            return Err(std::io::Error::other("injected disk failure"));
        }
        self.disk.persist(bytes)
    }
}

#[test]
fn failed_commit_persistence_prevents_kernel_apply_and_latches_stop() {
    let path = fixture_dir().join("node.wal");
    let bridge = Arc::new(MockEbpfBridge::default());
    let store = Arc::new(FaultingStore {
        disk: FileWal::open(&path).unwrap(),
        fail: AtomicBool::new(false),
        fail_commit: true,
    });
    let mut s = ConsensusState::new("solo", vec![]).with_ebpf_bridge(bridge.clone());
    s.enable_persistence(store).unwrap();
    s.start_signed_election().unwrap();
    assert!(s
        .propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(192, 0, 2, 5),
            false,
            300
        ))
        .is_err());
    assert!(s.persistence_error.is_some());
    assert_eq!(s.last_applied, 0);
    assert!(bridge.kernel_peer_map.lock().unwrap().is_empty());
    assert!(s.start_signed_election().is_err());
    assert!(s.trigger_view_timeout().is_err());
    assert!(s
        .sign_rpc_durable(ConsensusRpc::RequestVote {
            term: 2,
            candidate_id: "solo".into(),
            last_log_index: 0,
            last_log_term: 0
        })
        .is_err());
    let mut restored = ConsensusState::new("solo", vec![]);
    restored.load_wal(&path).unwrap();
    assert_eq!(restored.commit_index, 0);
    assert!(restored.voted_entry_hashes.contains_key(&(1, 1)));
}

#[tokio::test]
async fn failed_vote_persistence_returns_no_wire_vote_and_never_resumes() {
    use crate::infra::{send_rpc_over_quic, ClusterMtlsPki, MeshNode};
    use tokio::sync::Mutex;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut n = cluster();
    let mut b = n.remove(1);
    let path = fixture_dir().join("b.wal");
    let store = Arc::new(FaultingStore {
        disk: FileWal::open(&path).unwrap(),
        fail: AtomicBool::new(false),
        fail_commit: false,
    });
    b.enable_persistence(store.clone()).unwrap();
    let before = std::fs::read(&path).unwrap();
    store.fail.store(true, Ordering::SeqCst);
    let pki = ClusterMtlsPki::generate().unwrap();
    let server = MeshNode {
        node_id: "b".into(),
        endpoint: pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap(),
        state: Arc::new(Mutex::new(b)),
        peer_addrs: BTreeMap::new(),
    };
    let task = server.spawn_server();
    let client = pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let vote = n[0].start_signed_election().unwrap();
    assert!(
        send_rpc_over_quic(&client, server.local_addr().unwrap(), vote.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    store.fail.store(false, Ordering::SeqCst);
    assert!(
        send_rpc_over_quic(&client, server.local_addr().unwrap(), vote)
            .await
            .unwrap()
            .is_none()
    );
    assert!(server.state.lock().await.persistence_error.is_some());
    task.abort();
    server.endpoint.close(0u32.into(), b"done");
    client.close(0u32.into(), b"done");
}

#[test]
fn qc_vote_is_durable_before_it_leaves_the_state_machine() {
    let mut n = cluster();
    elect(&mut n);
    let path = fixture_dir().join("b.wal");
    n[1].enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
        .unwrap();
    n[0].propose(MeshCommand::NoOp).unwrap();
    let rpc = n[0].make_append_entries("b").unwrap();
    let signed = n[0].sign_rpc(rpc);
    let resp = n[1].handle_signed_rpc(signed).unwrap().unwrap();
    assert!(matches!(
        resp,
        ConsensusRpc::AppendEntriesResponse { success: true, .. }
    ));
    let mut restored = ConsensusState::new("b", vec!["a".into(), "c".into(), "d".into()]);
    restored.load_wal(&path).unwrap();
    assert_eq!(restored.voted_entry_hashes, n[1].voted_entry_hashes);
    assert_eq!(restored.log, n[1].log);
}

#[test]
fn wal_lock_excludes_second_writer_and_retains_unrelated_temporary_file() {
    let dir = fixture_dir();
    let path = dir.join("node.wal");
    let stale = path.with_extension("wal.tmp");
    std::fs::write(&stale, b"retained evidence").unwrap();
    let first = FileWal::open(&path).unwrap();
    assert!(FileWal::open(&path).is_err());
    first.persist(b"first snapshot").unwrap();
    first.persist(b"second snapshot").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second snapshot");
    assert_eq!(std::fs::read(&stale).unwrap(), b"retained evidence");
    drop(first);
    assert!(FileWal::open(&path).is_ok());
}

#[test]
fn partial_batch_retry_does_not_reapply_successful_rule_or_extend_deadline() {
    struct BatchBridge {
        calls: std::sync::Mutex<BTreeMap<u32, usize>>,
        fail: AtomicBool,
    }
    impl EbpfBridge for BatchBridge {
        fn sync_peer_policy(&self, ip: u32, _: PeerPolicy) -> anyhow::Result<()> {
            *self.calls.lock().unwrap().entry(ip).or_default() += 1;
            if ip == u32::from(Ipv4Addr::new(192, 0, 2, 2)) && self.fail.load(Ordering::SeqCst) {
                anyhow::bail!("second rule failed");
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
    let bridge = Arc::new(BatchBridge {
        calls: Default::default(),
        fail: AtomicBool::new(true),
    });
    let mut s = ConsensusState::new("solo", vec![]).with_ebpf_bridge(bridge.clone());
    s.start_election();
    let first = Ipv4Addr::new(192, 0, 2, 1);
    s.propose(MeshCommand::set_trust_batch(vec![
        PeerTrustRule::new(first, false, 60),
        PeerTrustRule::new(Ipv4Addr::new(192, 0, 2, 2), false, 60),
    ]))
    .unwrap();
    assert_eq!(s.last_applied, 0);
    s.advance_monotonic_clock(30);
    bridge.fail.store(false, Ordering::SeqCst);
    s.retry_pending_apply();
    assert_eq!(s.last_applied, 1);
    assert_eq!(bridge.calls.lock().unwrap()[&u32::from(first)], 1);
    assert_eq!(s.peer_rule_deadlines_ns[&u32::from(first)], 60_000_000_000);
}

#[test]
fn oversized_single_command_is_rejected_before_log_and_vote_changes() {
    let mut s = ConsensusState::new("solo", vec![]);
    s.start_election();
    assert!(s
        .propose(MeshCommand::set_lockdown(
            "x".repeat(MAX_WIRE_MESSAGE_BYTES),
            true
        ))
        .is_err());
    assert!(s.log.is_empty());
    assert!(s.voted_entry_hashes.is_empty());
}

#[test]
fn entry_and_large_qc_can_be_delivered_separately_without_stalling() {
    let names = [
        "a".to_string(),
        "b".repeat(10_000),
        "c".repeat(10_000),
        "d".to_string(),
    ];
    let mut nodes: Vec<_> = names
        .iter()
        .map(|id| ConsensusState::new(id, names.iter().filter(|n| *n != id).cloned().collect()))
        .collect();
    let keys: BTreeMap<_, _> = nodes
        .iter()
        .map(|s| (s.node_id.clone(), s.identity.public_key_bytes.clone()))
        .collect();
    for s in &mut nodes {
        s.register_peer_public_keys(keys.clone()).unwrap();
    }
    elect(&mut nodes);
    nodes[0]
        .propose(MeshCommand::set_lockdown("unmatched".repeat(7_000), false))
        .unwrap();
    let mut saw_deferred_qc = false;
    for _ in 0..3 {
        for i in 1..4 {
            let rpc = nodes[0].make_append_entries(&nodes[i].node_id).unwrap();
            if let ConsensusRpc::AppendEntries {
                entries, commit_qc, ..
            } = &rpc
            {
                if !entries.is_empty() && nodes[0].commit_index > 0 && commit_qc.is_none() {
                    saw_deferred_qc = true;
                }
            }
            let signed = nodes[0].sign_rpc(rpc);
            assert!(
                bincode::serialized_size(&WireMessage::Consensus(signed.clone().into())).unwrap()
                    <= MAX_WIRE_MESSAGE_BYTES as u64
            );
            let response = nodes[i].handle_signed_rpc(signed).unwrap().unwrap();
            let signed = nodes[i].sign_rpc(response);
            nodes[0].handle_signed_rpc(signed).unwrap();
        }
    }
    assert!(saw_deferred_qc);
    assert!(nodes
        .iter()
        .all(|s| s.commit_index == 1 && s.last_applied == 1));
}

#[test]
fn invalid_commit_certificate_does_not_append_or_change_term() {
    let mut n = cluster();
    let entry = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::NoOp,
    };
    let rpc = ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: "a".into(),
        prev_log_index: 0,
        prev_log_term: 0,
        prev_log_hash: [0; 32],
        entries: vec![entry.clone()],
        leader_commit: 1,
        commit_qc: Some(QuorumCertificate {
            term: 1,
            index: 1,
            entry_hash: entry.digest(),
            signatures: BTreeMap::new(),
        }),
    };
    let signed = n[0].sign_rpc(rpc);
    let resp = n[1].handle_signed_rpc(signed).unwrap().unwrap();
    assert!(matches!(
        resp,
        ConsensusRpc::AppendEntriesResponse { success: false, .. }
    ));
    assert!(n[1].log.is_empty());
    assert!(n[1].voted_entry_hashes.is_empty());
    assert_eq!(n[1].current_term, 0);
}

// Subprocess fixture: the parent sends a real QUIC RPC, kills this process
// after its response, then starts it again using the same WAL and identity.
#[tokio::test]
#[ignore = "subprocess fixture; executed explicitly by the parent recovery test"]
async fn crash_recovery_process_fixture() {
    let root =
        std::env::var_os("VEIL_TEST_RECOVERY_ROOT").expect("parent supplies fixture directory");
    let root = std::path::PathBuf::from(root);
    let phase = std::env::var("VEIL_TEST_RECOVERY_PHASE").unwrap();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut b = process_state("b");
    let path = root.join("b.wal");
    let store = Arc::new(FileWal::open(&path).unwrap());
    if path.exists() {
        b.load_wal(&path).unwrap();
    }
    b.enable_persistence(store).unwrap();
    let pki = crate::infra::ClusterMtlsPki::load_or_generate_dir(&root.join("pki")).unwrap();
    let node = crate::infra::MeshNode {
        node_id: "b".into(),
        endpoint: pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap(),
        state: Arc::new(tokio::sync::Mutex::new(b)),
        peer_addrs: BTreeMap::new(),
    };
    let _server = node.spawn_server();
    std::fs::write(
        root.join(format!("{phase}.addr")),
        node.local_addr().unwrap().to_string(),
    )
    .unwrap();
    std::future::pending::<()>().await;
}

fn process_state(id: &str) -> ConsensusState {
    let mut s = ConsensusState::new(
        id,
        ["a", "b", "c", "d"]
            .iter()
            .filter(|&&n| n != id)
            .map(|n| n.to_string())
            .collect(),
    );
    let mut keys = BTreeMap::new();
    for (i, name) in ["a", "b", "c", "d"].into_iter().enumerate() {
        // Public, deterministic test-only keys, never used by the daemon.
        let identity = NodeCryptoIdentity::from_seed(name, &[i as u8 + 1; 32]);
        keys.insert(name.into(), identity.public_key_bytes.clone());
        if name == id {
            s.set_crypto_identity(identity).unwrap();
        }
    }
    s.register_peer_public_keys(keys).unwrap();
    s
}

struct TestChild(std::process::Child);
impl Drop for TestChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn launch_child(root: &std::path::Path, phase: &str) -> (TestChild, std::net::SocketAddr) {
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "usecase::consensus_engine::regression_tests::crash_recovery_process_fixture",
            "--nocapture",
        ])
        .env("VEIL_TEST_RECOVERY_ROOT", root)
        .env("VEIL_TEST_RECOVERY_PHASE", phase)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut child = TestChild(child);
    let ready = root.join(format!("{phase}.addr"));
    for _ in 0..200 {
        if let Ok(addr) = std::fs::read_to_string(&ready) {
            if let Ok(addr) = addr.parse() {
                return (child, addr);
            }
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before ready"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("child readiness deadline exceeded");
}

#[tokio::test]
async fn acknowledged_vote_survives_process_kill_and_restart_over_quic() {
    use crate::infra::{send_rpc_over_quic, ClusterMtlsPki};
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root = fixture_dir();
    let pki = ClusterMtlsPki::load_or_generate_dir(&root.join("pki")).unwrap();
    let client = pki.make_endpoint("127.0.0.1:0".parse().unwrap()).unwrap();
    let (first, addr) = launch_child(&root, "first").await;
    let mut a = process_state("a");
    let response = send_rpc_over_quic(&client, addr, a.start_signed_election().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        response.rpc,
        ConsensusRpc::RequestVoteResponse {
            term: 1,
            vote_granted: true,
            ..
        }
    ));
    drop(first); // kill + wait; no orderly state flush or Drop in the child.
    let (second, addr) = launch_child(&root, "second").await;
    let mut c = process_state("c");
    let response = send_rpc_over_quic(&client, addr, c.start_signed_election().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        response.rpc,
        ConsensusRpc::RequestVoteResponse {
            term: 1,
            vote_granted: false,
            ..
        }
    ));
    drop(second);
    client.close(0u32.into(), b"done");
}

pub(super) fn cluster() -> Vec<ConsensusState> {
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
        s.register_peer_public_keys(keys.clone()).unwrap();
    }
    states
}

pub(super) fn elect(states: &mut [ConsensusState]) {
    let req = if states[0].current_term == 0 {
        states[0].start_signed_election().unwrap()
    } else {
        let pre = states[0].start_pre_vote().unwrap();
        for i in 1..states.len() {
            if let Some(reply) = states[i].handle_signed_rpc(pre.clone()).unwrap() {
                let signed = states[i].sign_rpc(reply);
                states[0].handle_signed_rpc(signed).unwrap();
            }
        }
        states[0].finish_pre_vote().unwrap().unwrap()
    };
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
fn unrelated_qc_rejection_preserves_log_and_cannot_obtain_conflicting_votes() {
    let mut n = cluster();
    elect(&mut n);
    let ip = Ipv4Addr::new(192, 0, 2, 99);
    let x = n[0]
        .propose(MeshCommand::set_trust_with_ttl(ip, false, 300))
        .unwrap();
    // A collects the first QC but only B learns its commit.
    for i in 1..4 {
        let rpc = ConsensusRpc::AppendEntries {
            term: 1,
            leader_id: "a".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            prev_log_hash: [0; 32],
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
            prev_log_hash: x.digest(),
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
    for i in 2..4 {
        let rpc = ConsensusRpc::AppendEntries {
            term: 2,
            leader_id: "a".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            prev_log_hash: [0; 32],
            entries: vec![y.clone()],
            leader_commit: 0,
            commit_qc: Some(unrelated_qc.clone()),
        };
        let old_log = n[i].log.clone();
        let old_hashes = n[i].chain_hashes.clone();
        let old_votes = n[i].voted_entry_hashes.clone();
        let signed = n[0].sign_rpc(rpc);
        let rejected = n[i].handle_signed_rpc(signed).unwrap().unwrap();
        assert!(matches!(
            rejected,
            ConsensusRpc::AppendEntriesResponse { success: false, .. }
        ));
        assert_eq!(n[i].log, old_log);
        assert_eq!(n[i].chain_hashes, old_hashes);
        assert_eq!(n[i].voted_entry_hashes, old_votes);
        let rpc = ConsensusRpc::AppendEntries {
            term: 2,
            leader_id: "a".into(),
            prev_log_index: 1,
            prev_log_term: 2,
            prev_log_hash: y.digest(),
            entries: vec![],
            leader_commit: 0,
            commit_qc: None,
        };
        let signed = n[0].sign_rpc(rpc);
        let resp = n[i].handle_signed_rpc(signed).unwrap().unwrap();
        assert!(
            matches!(resp, ConsensusRpc::AppendEntriesResponse { success: false, qc_vote_signature, .. } if qc_vote_signature.is_empty())
        );
        assert_eq!(n[i].commit_index, 0);
    }
    assert_eq!(n[1].commit_index, 1);
    assert_eq!(n[1].check_xdp_packet(ip), XdpVerdict::Drop);
}

#[test]
fn heartbeat_retries_failed_follower_apply() {
    let mut nodes = cluster();
    let bridge = Arc::new(RecoverableBridge {
        fail: AtomicBool::new(true),
        calls: AtomicUsize::new(0),
    });
    nodes[1].ebpf_bridge = Some(bridge.clone());
    elect(&mut nodes);
    nodes[0]
        .propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(192, 0, 2, 4),
            false,
            300,
        ))
        .unwrap();
    replicate(&mut nodes);
    replicate(&mut nodes);
    assert_eq!((nodes[1].commit_index, nodes[1].last_applied), (1, 0));
    bridge.fail.store(false, Ordering::SeqCst);
    for _ in 0..5 {
        replicate(&mut nodes);
    }
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 2);
    assert_eq!(nodes[1].last_applied, 1);
    assert!(nodes[1].last_apply_error.is_none());
}

#[test]
fn large_rule_batches_catch_up_within_wire_byte_limit() {
    let mut nodes = cluster();
    elect(&mut nodes);
    elect(&mut nodes); // Include the quorum term certificate in every wire budget.
    for _ in 0..64 {
        nodes[0]
            .propose(MeshCommand::set_trust_batch(
                (1..=128)
                    .map(|i| PeerTrustRule::new(Ipv4Addr::new(192, 0, 2, i), false, 60))
                    .collect(),
            ))
            .unwrap();
    }
    let mut rounds = 0;
    while nodes[1].last_log_index() < nodes[0].last_log_index() {
        let rpc = nodes[0].make_append_entries("b").unwrap();
        let signed = nodes[0].sign_rpc(rpc);
        let bytes = bincode::serialize(&WireMessage::Consensus(signed.clone().into())).unwrap();
        assert!(bytes.len() <= MAX_WIRE_MESSAGE_BYTES);
        let resp = nodes[1].handle_signed_rpc(signed).unwrap().unwrap();
        assert!(matches!(
            resp,
            ConsensusRpc::AppendEntriesResponse { success: true, .. }
        ));
        let signed = nodes[1].sign_rpc(resp);
        nodes[0].handle_signed_rpc(signed).unwrap();
        rounds += 1;
        assert!(rounds <= 64);
    }
    assert!(rounds > 1);
    assert_eq!(nodes[1].log, nodes[0].log);
}

#[test]
fn same_term_leader_contact_preserves_durable_election_vote() {
    let mut nodes = cluster();
    let path = fixture_dir().join("same-term.wal");
    nodes[1]
        .enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
        .unwrap();
    nodes[1].start_signed_election().unwrap();
    let voted_for = nodes[1].voted_for.clone();
    let heartbeat = nodes[0].sign_rpc(ConsensusRpc::AppendEntries {
        term: 1,
        leader_id: nodes[0].node_id.clone(),
        prev_log_index: 0,
        prev_log_term: 0,
        prev_log_hash: [0; 32],
        entries: vec![],
        leader_commit: 0,
        commit_qc: None,
    });
    assert!(matches!(
        nodes[1].handle_signed_rpc(heartbeat),
        Ok(Some(ConsensusRpc::AppendEntriesResponse {
            success: true,
            ..
        }))
    ));
    assert_eq!(nodes[1].role, Role::Follower);
    assert_eq!(nodes[1].voted_for, voted_for);
    let request = nodes[2].start_signed_election().unwrap();
    assert!(matches!(
        nodes[1].handle_signed_rpc(request),
        Ok(Some(ConsensusRpc::RequestVoteResponse {
            vote_granted: false,
            ..
        }))
    ));
    let mut restored = ConsensusState::new(nodes[1].node_id.clone(), vec![]);
    restored.load_wal(&path).unwrap();
    assert_eq!(restored.voted_for, voted_for);
}

#[test]
fn election_contact_signal_ignores_rejected_rpc_and_term_limit_fails_closed() {
    let mut nodes = cluster();
    let request = nodes[0].start_signed_election().unwrap();
    let mut forged = request.clone();
    forged.signature[0] ^= 1;
    assert!(nodes[1].handle_signed_rpc(forged).is_err());
    assert_eq!(nodes[1].election_activity(), 0);
    nodes[1].handle_signed_rpc(request).unwrap();
    assert_eq!(nodes[1].election_activity(), 1);
    let conflicting = nodes[2].start_signed_election().unwrap();
    nodes[1].handle_signed_rpc(conflicting).unwrap();
    assert_eq!(nodes[1].election_activity(), 1);
    nodes[1].current_term = MAX_SAFE_TERM - 1;
    assert!(nodes[1].start_signed_election().is_err());
    assert_eq!(nodes[1].current_term, MAX_SAFE_TERM - 1);
}

#[test]
fn missing_wal_cannot_reset_a_vote_and_exact_recovery_rejects_a_second_candidate() {
    let mut nodes = cluster();
    let root = fixture_dir();
    let wal = root.join("b.wal");
    nodes[1]
        .enable_persistence(Arc::new(FileWal::create_new(&wal).unwrap()))
        .unwrap();
    let first = nodes[0].start_signed_election().unwrap();
    assert!(matches!(
        nodes[1].handle_signed_rpc(first).unwrap().unwrap(),
        ConsensusRpc::RequestVoteResponse {
            vote_granted: true,
            ..
        }
    ));
    let identity = nodes[1].identity.clone();
    let registry = nodes[1].peer_public_keys.clone();
    let before = std::fs::read(&wal).unwrap();
    nodes.remove(1); // releases this fixture's WAL lock, as process exit would
    let retained = root.join("retained.wal");
    std::fs::rename(&wal, &retained).unwrap();
    assert!(FileWal::open_existing(&wal).is_err());
    assert!(!wal.exists());
    assert_eq!(std::fs::read(&retained).unwrap(), before);
    std::fs::rename(&retained, &wal).unwrap();
    let store = Arc::new(FileWal::open_existing(&wal).unwrap());
    let mut recovered = ConsensusState::new("b", vec!["a".into(), "c".into(), "d".into()]);
    recovered.set_crypto_identity(identity).unwrap();
    recovered.register_peer_public_keys(registry).unwrap();
    recovered.load_wal(&wal).unwrap();
    recovered.enable_persistence(store).unwrap();
    let conflicting = nodes[1].start_signed_election().unwrap(); // c, same term
    assert!(matches!(
        recovered.handle_signed_rpc(conflicting).unwrap().unwrap(),
        ConsensusRpc::RequestVoteResponse {
            vote_granted: false,
            ..
        }
    ));
    assert_eq!(recovered.voted_for.as_deref(), Some("a"));
    assert_eq!(recovered.current_term, 1);
}
