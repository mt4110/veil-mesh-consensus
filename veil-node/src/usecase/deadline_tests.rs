use super::*;
use crate::{ebpf_bridge::MockEbpfBridge, infra::wal::FileWal};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

const SECOND: u64 = 1_000_000_000;
const IP: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 71);

struct Clock {
    sample: Mutex<LeaseTime>,
    healthy: AtomicBool,
}
impl Clock {
    fn new(boot_seconds: u64, wall_seconds: u64) -> Arc<Self> {
        Arc::new(Self {
            sample: Mutex::new(LeaseTime {
                boot_ns: boot_seconds * SECOND,
                unix_ns: SIMULATION_WALL_EPOCH_NS + wall_seconds * SECOND,
                uncertainty_ns: 0,
                max_slow_rate_ppm: 0,
            }),
            healthy: AtomicBool::new(true),
        })
    }
    fn advance(&self, seconds: u64) {
        let mut s = self.sample.lock().unwrap();
        s.boot_ns += seconds * SECOND;
        s.unix_ns += seconds * SECOND;
    }
}
impl PolicyClock for Clock {
    fn boot_id(&self) -> &str {
        "test-boot"
    }
    fn now_ns(&self) -> std::io::Result<u64> {
        Ok(self.sample.lock().unwrap().boot_ns)
    }
    fn lease_time(&self) -> std::io::Result<LeaseTime> {
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("clock quality unavailable"));
        }
        Ok(*self.sample.lock().unwrap())
    }
}

fn cluster() -> (Vec<ConsensusState>, Vec<Arc<Clock>>) {
    let mut nodes = super::regression_tests::cluster();
    let clocks: Vec<_> = (0..4).map(|i| Clock::new(100 + i * 1_000, 100)).collect();
    for (node, clock) in nodes.iter_mut().zip(&clocks) {
        node.configure_policy_clock(clock.clone()).unwrap();
    }
    super::regression_tests::elect(&mut nodes);
    (nodes, clocks)
}
fn deliver(nodes: &mut [ConsensusState], to: usize) {
    let rpc = nodes[0].make_append_entries(&nodes[to].node_id).unwrap();
    let signed = nodes[0].sign_rpc_durable(rpc).unwrap();
    let reply = nodes[to].handle_signed_rpc(signed).unwrap().unwrap();
    let signed = nodes[to].sign_rpc_durable(reply).unwrap();
    nodes[0].handle_signed_rpc(signed).unwrap();
}
fn commit_without_late_node(nodes: &mut [ConsensusState], ttl: u32) {
    nodes[0]
        .propose(MeshCommand::set_trust_with_ttl(IP, false, ttl))
        .unwrap();
    for i in [1, 2, 1, 2] {
        deliver(nodes, i);
    }
    assert_eq!(nodes[0].commit_index, 1);
    assert_eq!(nodes[3].last_log_index(), 0);
}

#[test]
fn delayed_catch_up_accepts_history_but_never_reissues_expired_ban() {
    let (mut nodes, clocks) = cluster();
    let bridge = Arc::new(MockEbpfBridge::default());
    nodes[3].ebpf_bridge = Some(bridge.clone());
    commit_without_late_node(&mut nodes, 10);
    for c in &clocks {
        c.advance(11);
    }
    deliver(&mut nodes, 3);
    assert_eq!(nodes[3].commit_index, 1);
    assert_eq!(nodes[3].last_applied, 1);
    assert_eq!(nodes[3].entry_hash_at(1), nodes[0].entry_hash_at(1));
    assert_eq!(nodes[3].check_xdp_packet(IP), XdpVerdict::Pass);
    assert!(bridge
        .kernel_peer_map
        .lock()
        .unwrap()
        .values()
        .all(|p| p.trusted != 0));
    for c in &clocks {
        c.advance(100);
    }
    deliver(&mut nodes, 3);
    assert_eq!(nodes[3].check_xdp_packet(IP), XdpVerdict::Pass);
    assert_eq!(nodes[3].lease_deadlines[&(1, 0)], 1);
}

#[test]
fn live_catch_up_uses_only_remaining_window_on_a_different_boot_clock() {
    let (mut nodes, clocks) = cluster();
    commit_without_late_node(&mut nodes, 10);
    for c in &clocks {
        c.advance(7);
    }
    deliver(&mut nodes, 3);
    assert_eq!(
        nodes[3].peer_rule_deadlines_ns[&u32::from(IP)],
        3_110 * SECOND
    );
    assert_eq!(nodes[3].check_xdp_packet(IP), XdpVerdict::Drop);
    clocks[3].advance(3);
    assert_eq!(nodes[3].check_xdp_packet(IP), XdpVerdict::Pass);
}

#[test]
fn deadline_is_bound_by_rpc_signature_and_quorum_hash() {
    let (mut nodes, _) = cluster();
    commit_without_late_node(&mut nodes, 10);
    let rpc = nodes[0].make_append_entries("d").unwrap();
    let mut signed = nodes[0].sign_rpc(rpc);
    if let ConsensusRpc::AppendEntries { entries, .. } = &mut signed.rpc {
        if let MeshCommand::SetPeerTrustLeaseBatch {
            issued_at_unix_ns,
            rules,
        } = &mut entries[0].command
        {
            *issued_at_unix_ns += SECOND;
            rules[0].not_after_unix_ns += SECOND;
        } else {
            panic!("TTL proposal was not stamped");
        }
    }
    assert!(nodes[3].handle_signed_rpc(signed.clone()).is_err());
    // Even a leader able to replace its outer signature cannot reuse the old QC.
    let rebound = nodes[0].sign_rpc(signed.rpc);
    let refusal = nodes[3].handle_signed_rpc(rebound).unwrap().unwrap();
    assert!(
        matches!(refusal, ConsensusRpc::AppendEntriesResponse { success: false, ref qc_vote_signature, .. } if qc_vote_signature.is_empty())
    );
    assert!(nodes[3].voted_entry_hashes.is_empty());
    assert!(nodes[3].log.is_empty());
    assert_eq!(nodes[3].commit_index, 0);
}

#[test]
fn malformed_windows_and_expired_or_future_local_proposals_are_rejected_before_mutation() {
    let clock = Clock::new(100, 100);
    let mut node = ConsensusState::new("solo", vec![]);
    node.configure_policy_clock(clock.clone()).unwrap();
    node.start_signed_election().unwrap();
    let issued = clock.lease_time().unwrap().unix_ns;
    let make = |issued_at_unix_ns, not_after_unix_ns, ttl| MeshCommand::SetPeerTrustLeaseBatch {
        issued_at_unix_ns,
        rules: vec![DeadlinePeerTrustRule {
            rule: PeerTrustRule::new(IP, false, ttl),
            not_after_unix_ns,
        }],
    };
    for bad in [
        make(0, 10 * SECOND, 10),
        make(issued, issued + 11 * SECOND, 10),
        make(u64::MAX, 0, 1),
        make(issued, issued + SECOND, 0),
        make(issued, issued + 86_401 * SECOND, 86_401),
        make(issued - 20 * SECOND, issued - 10 * SECOND, 10),
        make(issued + 20 * SECOND, issued + 30 * SECOND, 10),
    ] {
        assert!(node.propose(bad).is_err());
        assert!(node.log.is_empty());
        assert!(node.voted_entry_hashes.is_empty());
    }
}

#[test]
fn uncertainty_and_rate_budget_expire_conservatively_at_the_boundary() {
    let s = LeaseTime {
        boot_ns: 100 * SECOND,
        unix_ns: 1_000 * SECOND,
        uncertainty_ns: 2 * SECOND,
        max_slow_rate_ppm: 1_000,
    };
    assert_eq!(s.boot_deadline(999 * SECOND, 1_002 * SECOND).unwrap(), 1);
    assert_eq!(
        s.boot_deadline(999 * SECOND, 1_012 * SECOND).unwrap(),
        109_990_000_000
    );
    assert_eq!(s.boot_deadline(1_003 * SECOND, 1_012 * SECOND).unwrap(), 1);
    assert!(LeaseTime {
        unix_ns: u64::MAX,
        ..s
    }
    .boot_deadline(1, 2)
    .is_err());
    assert!(LeaseTime {
        boot_ns: u64::MAX,
        ..s
    }
    .boot_deadline(999 * SECOND, 1_012 * SECOND)
    .is_err());
}

#[test]
fn unknown_clock_suppresses_new_apply_durably_but_does_not_stall_catch_up() {
    let (mut nodes, clocks) = cluster();
    commit_without_late_node(&mut nodes, 10);
    let path = super::regression_tests::fixture_dir().join("d.wal");
    nodes[3]
        .enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
        .unwrap();
    clocks[3].healthy.store(false, Ordering::SeqCst);
    deliver(&mut nodes, 3);
    assert_eq!(nodes[3].commit_index, 1);
    assert_eq!(nodes[3].check_xdp_packet(IP), XdpVerdict::Pass);
    assert!(nodes[3].lease_clock_error.is_some());
    assert!(nodes[3]
        .render_prometheus_metrics()
        .contains("veil_node_degraded{node=\"d\"} 1"));
    let identity = nodes[3].identity.clone();
    drop(nodes);
    clocks[3].healthy.store(true, Ordering::SeqCst);
    let mut restored = ConsensusState::new("d", vec![]);
    restored.identity = identity;
    restored.configure_policy_clock(clocks[3].clone()).unwrap();
    restored.load_wal(&path).unwrap();
    assert_eq!(restored.check_xdp_packet(IP), XdpVerdict::Pass);
    assert_eq!(restored.lease_deadlines[&(1, 0)], 1);
}

#[test]
fn unknown_issuer_clock_rejects_proposal_and_retry_does_not_restamp() {
    let clock = Clock::new(100, 100);
    let mut node = ConsensusState::new("solo", vec![]);
    node.configure_policy_clock(clock.clone()).unwrap();
    node.start_signed_election().unwrap();
    clock.healthy.store(false, Ordering::SeqCst);
    assert!(node
        .propose(MeshCommand::set_trust_with_ttl(IP, false, 10))
        .is_err());
    assert!(node.log.is_empty());
    clock.healthy.store(true, Ordering::SeqCst);
    let entry = node
        .propose(MeshCommand::set_trust_with_ttl(IP, false, 10))
        .unwrap();
    clock.advance(11);
    assert!(node.propose(entry.command.clone()).is_err());
    assert_eq!(node.log.len(), 1);
}

#[test]
fn wal_recovery_takes_minimum_of_saved_deadline_and_absolute_deadline() {
    for wall_step in [-5i64, 20] {
        let clock = Clock::new(100, 100);
        let path = super::regression_tests::fixture_dir().join("solo.wal");
        let mut node = ConsensusState::new("solo", vec![]);
        node.configure_policy_clock(clock.clone()).unwrap();
        node.enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
            .unwrap();
        node.start_signed_election().unwrap();
        node.propose(MeshCommand::set_trust_with_ttl(IP, false, 10))
            .unwrap();
        drop(node);
        clock.advance(7);
        let wall = clock.sample.lock().unwrap().unix_ns;
        clock.sample.lock().unwrap().unix_ns =
            wall.checked_add_signed(wall_step * SECOND as i64).unwrap();
        let mut restored = ConsensusState::new("solo", vec![]);
        restored.configure_policy_clock(clock).unwrap();
        restored.load_wal(&path).unwrap();
        assert_eq!(
            restored.lease_deadlines[&(1, 0)],
            if wall_step < 0 { 110 * SECOND } else { 1 }
        );
    }
}

#[test]
fn relative_only_wal_is_preserved_and_refused_without_rearming_or_voting() {
    let path = super::regression_tests::fixture_dir().join("legacy.wal");
    let mut old = ConsensusState::new("legacy", vec![]);
    let entry = LogEntry {
        term: 1,
        index: 1,
        command: MeshCommand::set_trust_with_ttl(IP, false, 10),
    };
    let hash = old.push_log_entry(entry);
    old.current_term = 1;
    old.voted_for = Some("legacy".into());
    old.voted_entry_hashes.insert((1, 1), hash);
    old.commit_index = 1;
    old.lease_deadlines.insert((1, 0), 10 * SECOND);
    old.save_wal(&path).unwrap();
    let mut restored = ConsensusState::new("legacy", vec![]);
    let original = std::fs::read(&path).unwrap();
    assert!(restored.load_wal(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(restored.log.is_empty());
    assert!(restored.start_signed_election().is_err());
    assert_eq!(old.voted_entry_hashes[&(1, 1)], hash);
    assert_eq!(old.voted_for.as_deref(), Some("legacy"));
    assert_eq!(restored.check_xdp_packet(IP), XdpVerdict::Pass);
    assert!(restored.lease_deadlines.is_empty());
}

#[test]
fn signed_deadline_batches_still_chunk_and_expired_chunks_do_not_stall_commit() {
    let (mut nodes, clocks) = cluster();
    for batch in 0..32u32 {
        nodes[0]
            .propose(MeshCommand::set_trust_batch(
                (0..128u32)
                    .map(|i| {
                        PeerTrustRule::new(
                            Ipv4Addr::from(0xc0000200u32 + batch * 128 + i),
                            false,
                            60,
                        )
                    })
                    .collect(),
            ))
            .unwrap();
    }
    for _ in 0..6 {
        for i in [1, 2] {
            deliver(&mut nodes, i);
        }
    }
    assert_eq!(nodes[0].commit_index, 32);
    clocks[3].advance(61);
    let mut messages = 0;
    for _ in 0..6 {
        let rpc = nodes[0].make_append_entries("d").unwrap();
        let signed = nodes[0].sign_rpc(rpc);
        let wire = crate::adapter::WireMessage::Consensus(signed.into());
        assert!(bincode::serialized_size(&wire).unwrap() <= MAX_WIRE_MESSAGE_BYTES as u64);
        deliver(&mut nodes, 3);
        messages += 1;
        if nodes[3].commit_index == 32 {
            break;
        }
    }
    assert!(messages > 1);
    assert_eq!(nodes[3].commit_index, 32);
    assert_eq!(nodes[3].last_applied, 32);
    assert!(nodes[3].ebpf_policy_map.is_empty());
}

#[test]
fn malformed_signed_remote_window_does_not_mutate_log_or_vote_history() {
    let (mut nodes, _) = cluster();
    nodes[0]
        .propose(MeshCommand::set_trust_with_ttl(IP, false, 10))
        .unwrap();
    let mut rpc = nodes[0].make_append_entries("d").unwrap();
    if let ConsensusRpc::AppendEntries { entries, .. } = &mut rpc {
        if let MeshCommand::SetPeerTrustLeaseBatch { rules, .. } = &mut entries[0].command {
            rules[0].not_after_unix_ns += SECOND;
        }
    }
    let signed = nodes[0].sign_rpc(rpc);
    let response = nodes[3].handle_signed_rpc(signed).unwrap().unwrap();
    assert!(matches!(response, ConsensusRpc::AppendEntriesResponse {
        success: false, ref qc_vote_signature, ..
    } if qc_vote_signature.is_empty()));
    assert!(nodes[3].log.is_empty());
    assert!(nodes[3].voted_entry_hashes.is_empty());
    assert_eq!(nodes[3].commit_index, 0);
}
