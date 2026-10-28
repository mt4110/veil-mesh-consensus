use super::regression_tests::fixture_dir;
use super::two_phase_tests::{elect, nodes, restore, round, send, view};
use super::*;
use crate::domain::{PhaseBlock, PhaseMessage, CHECKPOINT_INTERVAL};

pub(super) fn gossip(n: &mut [ConsensusState], active: &[usize]) {
    for _ in 0..3 {
        for &i in active {
            if let Some(rpc) = n[i].drive_stable_checkpoint().unwrap() {
                for &j in active {
                    if i != j {
                        if let Some(reply) = n[j].handle_signed_rpc(rpc.clone()).unwrap() {
                            send(n, j, i, reply);
                        }
                    }
                }
            }
        }
    }
}
pub(super) fn fill(n: &mut [ConsensusState], leader: usize, peers: &[usize], count: u64) {
    for _ in 0..count {
        n[leader].propose(MeshCommand::NoOp).unwrap();
        for _ in 0..4 {
            round(n, leader, peers);
        }
    }
}
#[test]
fn checkpoint_quorum_replaces_prefix_and_recovers_late_candidate() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    fill(&mut n, 0, &[1, 2], CHECKPOINT_INTERVAL);
    assert!(n[0].propose(MeshCommand::NoOp).is_err());
    let before = n[0].wal_bytes().unwrap().len();
    gossip(&mut n, &[0, 1, 2]);
    for s in &n[..3] {
        assert_eq!(s.log_base(), 32);
        assert!(s.log.is_empty());
        assert_eq!(s.two_phase.as_ref().unwrap().durable.commits.len(), 1);
    }
    let after = n[0].wal_bytes().unwrap().len();
    assert!(after < before);
    println!("checkpoint bytes: before={before} after={after}");
    // A candidate that missed the entire prefix obtains certified state, then
    // resumes the same hash chain rather than assuming an empty log.
    elect(&mut n, 3, &[1, 2]);
    for _ in 0..3 {
        if let Some(r) = n[3].drive_two_phase().unwrap() {
            if let Some(reply) = n[0].handle_signed_rpc(r).unwrap() {
                send(&mut n, 0, 3, reply);
            }
        }
        round(&mut n, 3, &[0, 1, 2]);
    }
    assert_eq!(n[3].log_base(), 32);
    assert!(n[3].commit_index >= 33);
    for s in &n {
        assert_eq!(s.entry_hash_at(33), n[3].entry_hash_at(33));
    }
    let path = fixture_dir().join("state.wal");
    n[3].save_wal(&path).unwrap();
    let recovered = restore(&n[3], &path);
    assert_eq!(recovered.log_base(), 32);
    assert_eq!(recovered.commit_index, n[3].commit_index);
}
#[test]
fn repeated_checkpoints_bound_wal_and_old_slots_cannot_vote_again() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    let original_view = view(&n, 0);
    let mut peaks = Vec::new();
    for _ in 0..4 {
        fill(&mut n, 0, &[1, 2, 3], CHECKPOINT_INTERVAL);
        gossip(&mut n, &[0, 1, 2, 3]);
        peaks.push(n[0].wal_bytes().unwrap().len());
    }
    assert_eq!(n[0].log_base(), 128);
    assert!(peaks.iter().max().unwrap() - peaks.iter().min().unwrap() < 2000);
    let path = fixture_dir().join("floor.wal");
    n[1].save_wal(&path).unwrap();
    n[1] = restore(&n[1], &path);
    let before = n[1].wal_bytes().unwrap();
    let rpc = ConsensusRpc::TwoPhase {
        term: 1,
        sender_id: "a".into(),
        message: Box::new(PhaseMessage::Proposal {
            view: original_view,
            block: PhaseBlock {
                entry: LogEntry {
                    term: 1,
                    index: 1,
                    command: MeshCommand::NoOp,
                },
                parent: [0; 32],
            },
        }),
    };
    let reply = send(&mut n, 0, 1, rpc).unwrap();
    assert!(
        matches!(reply,ConsensusRpc::TwoPhase {message,..} if matches!(*message,PhaseMessage::Receipt{index:128,..}))
    );
    assert_eq!(n[1].two_phase.as_ref().unwrap().durable.votes.len(), 0);
    assert_eq!(n[1].wal_bytes().unwrap(), before);
    println!("stable WAL sizes over 4 checkpoints: {peaks:?}");
}
#[test]
fn tampered_foreign_and_subquorum_snapshots_do_not_mutate_state() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    fill(&mut n, 0, &[1, 2], 32);
    gossip(&mut n, &[0, 1, 2]);
    let good = n[0]
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .stable
        .clone()
        .unwrap();
    for kind in 0..4 {
        let mut bad = good.clone();
        match kind {
            0 => bad.state.hash[0] ^= 1,
            1 => {
                bad.votes.pop_last();
            }
            2 => bad.votes.values_mut().next().unwrap().signature[0] ^= 1,
            _ => bad
                .state
                .lockdown
                .insert("d".into(), (1, true))
                .map(|_| ())
                .unwrap_or(()),
        }
        let before = n[3].wal_bytes().unwrap();
        assert!(n[3].install_snapshot(bad).is_err());
        assert_eq!(n[3].wal_bytes().unwrap(), before);
    }
    let context = n[3].signing_context.clone();
    n[3].signing_context.config_epoch += 1;
    assert!(n[3].install_snapshot(good.clone()).is_err());
    n[3].signing_context = context;
    n[3].install_snapshot(good).unwrap();
    assert_eq!(n[3].commit_index, 32);
}
#[test]
fn snapshot_transfer_and_restart_do_not_renew_expired_ban_or_clear_bypass() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    let ip = Ipv4Addr::new(192, 0, 2, 71);
    n[0].propose(MeshCommand::set_trust_with_ttl(ip, false, 20))
        .unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2]);
    }
    fill(&mut n, 0, &[1, 2], 31);
    gossip(&mut n, &[0, 1, 2]);
    n[3].advance_monotonic_clock(30);
    n[3].emergency_local_bypass = true;
    let snapshot = n[0]
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .stable
        .clone()
        .unwrap();
    n[3].install_snapshot(snapshot).unwrap();
    assert_eq!(n[3].lease_deadlines[&(1, 0)], 1);
    assert!(n[3].emergency_local_bypass);
    let path = fixture_dir().join("deadline.wal");
    n[3].save_wal(&path).unwrap();
    let recovered = restore(&n[3], &path);
    assert_eq!(recovered.lease_deadlines[&(1, 0)], 1);
    assert!(recovered.emergency_local_bypass);
}
#[test]
fn wal_limit_failure_releases_no_vote_and_keeps_last_durable_file() {
    let path = fixture_dir().join("cap.wal");
    let mut n = nodes();
    n[1].enable_persistence(Arc::new(
        crate::infra::wal::FileWal::create_new(&path).unwrap(),
    ))
    .unwrap();
    let initial = std::fs::read(&path).unwrap();
    n[1].max_wal_bytes = initial.len() as u64;
    let request = n[0].start_signed_election().unwrap();
    assert!(n[1].handle_signed_rpc(request).is_err());
    assert!(n[1].persistence_error.is_some());
    assert_eq!(std::fs::read(&path).unwrap(), initial);
    assert!(n[1].start_signed_election().is_err());
    let mut recovered = restore(&n[1], &path);
    assert_eq!(recovered.current_term, 0);
    recovered.max_wal_bytes = 1;
    assert!(recovered.load_wal(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), initial);
}

#[test]
fn uncertain_checkpoint_write_preserves_old_or_new_recoverable_state() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Store {
        disk: crate::infra::wal::FileWal,
        fail: AtomicBool,
        after_write: bool,
    }
    impl ConsensusPersistence for Store {
        fn persist(&self, bytes: &[u8]) -> std::io::Result<()> {
            if self.fail.load(Ordering::SeqCst) {
                if self.after_write {
                    self.disk.persist(bytes)?;
                }
                return Err(std::io::Error::other("injected uncertain checkpoint write"));
            }
            self.disk.persist(bytes)
        }
    }
    for after_write in [false, true] {
        let path = fixture_dir().join("atomic.wal");
        let mut n = nodes();
        elect(&mut n, 0, &[1, 2, 3]);
        fill(&mut n, 0, &[1, 2], 32);
        let store = Arc::new(Store {
            disk: crate::infra::wal::FileWal::create_new(&path).unwrap(),
            fail: AtomicBool::new(false),
            after_write,
        });
        n[2].enable_persistence(store.clone()).unwrap();
        // Save the signing fence and the full committed prefix first.
        n[2].drive_stable_checkpoint().unwrap();
        let full_prefix = std::fs::read(&path).unwrap();
        gossip(&mut n, &[0, 1, 2]);
        let stable = n[0]
            .two_phase
            .as_ref()
            .unwrap()
            .durable
            .stable
            .clone()
            .unwrap();
        // Recover a full-prefix node, including its prepared/commit evidence.
        let mut target = nodes().remove(2);
        target.set_crypto_identity(n[2].identity.clone()).unwrap();
        target
            .register_peer_public_keys(n[2].peer_public_keys.clone())
            .unwrap();
        // A fresh receiver persists its old empty state before the injected failure.
        let other_path = fixture_dir().join("receiver.wal");
        let receiver_store = Arc::new(Store {
            disk: crate::infra::wal::FileWal::create_new(&other_path).unwrap(),
            fail: AtomicBool::new(false),
            after_write,
        });
        receiver_store.disk.persist(&full_prefix).unwrap();
        target.load_wal(&other_path).unwrap();
        target.enable_persistence(receiver_store.clone()).unwrap();
        let old = std::fs::read(&other_path).unwrap();
        receiver_store.fail.store(true, Ordering::SeqCst);
        assert!(target.install_snapshot(stable).is_err());
        assert!(target.persistence_error.is_some());
        assert!(target.start_signed_election().is_err());
        if !after_write {
            assert_eq!(std::fs::read(&other_path).unwrap(), old);
        }
        let recovered = restore(&target, &other_path);
        assert_eq!(recovered.log_base(), if after_write { 32 } else { 0 });
        assert_eq!(recovered.last_applied, recovered.commit_index);
    }
}

#[test]
fn checkpoint_vote_is_not_released_when_its_wal_write_fails() {
    struct Fail;
    impl ConsensusPersistence for Fail {
        fn persist(&self, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("disk failure"))
        }
    }
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    fill(&mut n, 0, &[1, 2], 32);
    let path = fixture_dir().join("vote.wal");
    n[1].enable_persistence(Arc::new(
        crate::infra::wal::FileWal::create_new(&path).unwrap(),
    ))
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    n[1].persistence = Some(Arc::new(Fail));
    assert!(n[1].drive_stable_checkpoint().is_err());
    assert!(n[1].persistence_error.is_some());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(restore(&n[1], &path)
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .checkpoint_vote
        .is_none());
}

#[test]
fn snapshot_policy_capacity_is_bounded_and_explicit_unban_releases_a_slot() {
    let mut s = ConsensusState::new("solo", vec![]);
    s.enable_two_phase().unwrap();
    s.start_signed_election().unwrap();
    for i in 0..256 {
        s.propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(192, 0, 2, i as u8),
            false,
            60,
        ))
        .unwrap();
        s.drive_stable_checkpoint().unwrap();
    }
    assert_eq!(s.log_base(), 256);
    let before = s.wal_bytes().unwrap();
    assert!(s
        .propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(198, 51, 100, 1),
            false,
            60
        ))
        .is_err());
    assert_eq!(s.wal_bytes().unwrap(), before);
    let rpc = s.snapshot_rpc().unwrap();
    assert!(s.rpc_wire_size(&rpc).unwrap() < MAX_WIRE_MESSAGE_BYTES as u64);
    println!(
        "256-rule snapshot wire bytes={}",
        s.rpc_wire_size(&rpc).unwrap()
    );
    s.propose(MeshCommand::set_trust(Ipv4Addr::new(192, 0, 2, 0), true))
        .unwrap();
    s.propose(MeshCommand::set_trust_with_ttl(
        Ipv4Addr::new(198, 51, 100, 1),
        false,
        60,
    ))
    .unwrap();
}

#[test]
fn compacted_witness_receipt_remains_terminal_after_restart() {
    let mut s = ConsensusState::new("solo", vec![]);
    s.enable_two_phase().unwrap();
    s.start_signed_election().unwrap();
    let id = s
        .accept_ban_request(Ipv4Addr::new(192, 0, 2, 73), 60)
        .unwrap();
    s.drive_ban_progress().unwrap();
    for _ in 0..31 {
        s.propose(MeshCommand::NoOp).unwrap();
    }
    s.drive_stable_checkpoint().unwrap();
    assert_eq!(s.log_base(), 32);
    assert!(s.ban_request_status(&id).contains("request_phase=applied"));
    let path = fixture_dir().join("receipt.wal");
    s.save_wal(&path).unwrap();
    let mut recovered = restore(&s, &path);
    assert!(recovered
        .ban_request_status(&id)
        .contains("request_phase=applied"));
    let before = recovered.commit_index;
    recovered.drive_ban_progress().unwrap();
    assert_eq!(recovered.commit_index, before);
}

#[test]
fn pending_retirement_does_not_report_a_committed_cleanup_fence() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    n[0].propose(MeshCommand::set_trust_with_ttl(
        Ipv4Addr::new(192, 0, 2, 77),
        false,
        2,
    ))
    .unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2, 3]);
    }
    for s in &mut n {
        s.advance_monotonic_clock(3);
    }
    n[0].retire_expired_if_idle().unwrap();
    assert!(n[0].last_log_index() > n[0].commit_index);
    assert_eq!(n[0].retired_through_unix_ns(), 0);
    assert!(n[0]
        .propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(192, 0, 2, 78),
            false,
            2
        ))
        .is_err());
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2, 3]);
    }
    assert_eq!(n[0].last_log_index(), n[0].commit_index);
    assert_eq!(
        n[0].retired_through_unix_ns(),
        SIMULATION_WALL_EPOCH_NS + 3_000_000_000
    );
    let path = fixture_dir().join("retirement-fence.wal");
    n[0].save_wal(&path).unwrap();
    assert_eq!(
        restore(&n[0], &path).retired_through_unix_ns(),
        n[0].retired_through_unix_ns()
    );
}

#[test]
fn expiration_retirement_requires_past_time_and_reclaims_distinct_ip_capacity() {
    let mut s = ConsensusState::new("solo", vec![]);
    s.enable_two_phase().unwrap();
    s.start_signed_election().unwrap();
    let before = s.wal_bytes().unwrap();
    assert!(s
        .propose(MeshCommand::RetireExpired {
            through_unix_ns: SIMULATION_WALL_EPOCH_NS + 1
        })
        .is_err());
    assert_eq!(s.wal_bytes().unwrap(), before);
    let mut samples = Vec::new();
    for i in 0..512u32 {
        if i % 16 == 0 {
            s.advance_monotonic_clock(2);
            s.retire_expired_if_idle().unwrap();
            s.drive_stable_checkpoint().unwrap();
        }
        s.propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(192, 0, 2 + (i / 256) as u8, i as u8),
            false,
            1,
        ))
        .unwrap();
        s.drive_stable_checkpoint().unwrap();
        assert!(s.log.len() <= 32);
        assert!(s.two_phase.as_ref().unwrap().durable.commits.len() <= 33);
        samples.push(s.wal_bytes().unwrap().len());
    }
    assert!(s.log_base() > 480);
    assert!(samples.iter().max().unwrap() < &(crate::domain::DEFAULT_MAX_WAL_BYTES as usize));
    println!(
        "512 distinct expiring BANs: base={} max_WAL_bytes={} last_WAL_bytes={}",
        s.log_base(),
        samples.iter().max().unwrap(),
        samples.last().unwrap()
    );
    let path = fixture_dir().join("retired.wal");
    s.save_wal(&path).unwrap();
    let mut r = restore(&s, &path);
    assert_eq!(r.retired_through_unix_ns(), s.retired_through_unix_ns());
    r.advance_monotonic_clock(100);
    r.retire_expired_if_idle().unwrap();
}

#[test]
fn pending_temporary_snapshot_stops_repeated_accumulation_without_deleting_evidence() {
    let dir = fixture_dir();
    let path = dir.join("node.wal");
    let store = crate::infra::wal::FileWal::create_new(&path).unwrap();
    store.persist(b"durable").unwrap();
    let temp = dir.join("node.wal.123-456.tmp");
    std::fs::write(&temp, b"partial").unwrap();
    assert!(store.persist(b"replacement").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"durable");
    assert_eq!(std::fs::read(&temp).unwrap(), b"partial");
    drop(store);
    assert!(crate::infra::wal::FileWal::open_existing(&path).is_err());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
}

#[test]
fn hidden_commit_above_checkpoint_survives_two_views_and_restart() {
    use super::two_phase_tests::sign_cert;
    use crate::domain::Phase;
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    fill(&mut n, 0, &[1, 2, 3], 32);
    gossip(&mut n, &[0, 1, 2, 3]);
    n[0].propose(MeshCommand::set_trust_with_ttl(
        Ipv4Addr::new(192, 0, 2, 92),
        false,
        60,
    ))
    .unwrap();
    let block = PhaseBlock {
        entry: n[0].log[0].clone(),
        parent: n[0].base_hash(),
    };
    assert_eq!(block.entry.index, 33);
    for i in [1, 3] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let r = send(&mut n, 0, i, rpc).unwrap();
        send(&mut n, i, 0, r);
    }
    let mut hidden = sign_cert(&n, Phase::Commit, n[0].current_term, block.clone(), &[0]);
    for i in [1, 3] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let r = send(&mut n, 0, i, rpc).unwrap();
        let ConsensusRpc::TwoPhase { message, .. } = r else {
            panic!("phase response")
        };
        let PhaseMessage::Vote { signature, .. } = *message else {
            panic!("commit vote")
        };
        hidden.votes.insert(n[i].node_id.clone(), signature);
    }
    n[2].verify_phase_certificate(&hidden).unwrap();
    assert!(n.iter().all(|s| s.commit_index == 32));
    let path = fixture_dir().join("above-floor.wal");
    n[1].save_wal(&path).unwrap();
    n[1] = restore(&n[1], &path);
    elect(&mut n, 2, &[1, 3]);
    n[2].drive_two_phase().unwrap();
    n[2].role = Role::Follower;
    elect(&mut n, 1, &[2, 3]);
    n[1].drive_two_phase().unwrap();
    for _ in 0..5 {
        round(&mut n, 1, &[2, 3]);
    }
    for s in &n[1..] {
        assert_eq!(s.log_base(), 32);
        assert_eq!(s.log[0], block.entry);
        assert_eq!(s.commit_index, 33);
    }
    n[3].install_phase_commit(hidden).unwrap();
    assert_eq!(n[3].log[0], block.entry);
}

#[test]
fn checkpoint_keeps_failed_kernel_expiration_for_retry() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Bridge(AtomicBool);
    impl EbpfBridge for Bridge {
        fn sync_peer_policy(&self, _: u32, _: PeerPolicy) -> anyhow::Result<()> {
            if self.0.load(Ordering::SeqCst) {
                anyhow::bail!("injected removal failure")
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
    let mut s = ConsensusState::new("solo", vec![]);
    s.enable_two_phase().unwrap();
    s.start_signed_election().unwrap();
    let bridge = Arc::new(Bridge(AtomicBool::new(false)));
    s.ebpf_bridge = Some(bridge.clone());
    s.propose(MeshCommand::set_trust_with_ttl(
        Ipv4Addr::new(192, 0, 2, 77),
        false,
        1,
    ))
    .unwrap();
    bridge.0.store(true, Ordering::SeqCst);
    s.advance_monotonic_clock(2);
    assert_eq!(s.peer_rule_deadlines_ns.len(), 1);
    s.propose(MeshCommand::RetireExpired {
        through_unix_ns: SIMULATION_WALL_EPOCH_NS + 2_000_000_000,
    })
    .unwrap();
    for _ in 0..30 {
        s.propose(MeshCommand::NoOp).unwrap();
    }
    s.drive_stable_checkpoint().unwrap();
    assert_eq!(s.log_base(), 32);
    assert!(s
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .stable
        .as_ref()
        .unwrap()
        .state
        .rules
        .is_empty());
    assert_eq!(s.peer_rule_deadlines_ns.len(), 1);
    bridge.0.store(false, Ordering::SeqCst);
    assert_eq!(s.expire_stale_policies(), 1);
    assert!(s.peer_rule_deadlines_ns.is_empty());
    assert!(s.ebpf_policy_map.is_empty());
}

#[test]
fn seven_member_maximum_snapshot_proof_fits_single_wire_frame() {
    use super::two_phase_tests::sign_cert;
    use crate::domain::{CertifiedSnapshot, CheckpointVote, Phase};
    let mut solo = ConsensusState::new("solo", vec![]);
    solo.enable_two_phase().unwrap();
    solo.start_signed_election().unwrap();
    for i in 0..256 {
        solo.propose(MeshCommand::set_trust_with_ttl(
            Ipv4Addr::new(192, 0, 2, i as u8),
            false,
            60,
        ))
        .unwrap();
        solo.drive_stable_checkpoint().unwrap();
    }
    let mut state = solo
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .stable
        .as_ref()
        .unwrap()
        .state
        .clone();
    let names: Vec<_> = (0..7).map(|i| format!("{i}{}", "x".repeat(127))).collect();
    let mut n: Vec<_> = names
        .iter()
        .map(|id| ConsensusState::new(id, names.iter().filter(|p| *p != id).cloned().collect()))
        .collect();
    let keys = n
        .iter()
        .map(|s| (s.node_id.clone(), s.identity.public_key_bytes.clone()))
        .collect::<BTreeMap<_, _>>();
    for s in &mut n {
        s.register_peer_public_keys(keys.clone()).unwrap();
        s.enable_two_phase().unwrap();
    }
    let block = PhaseBlock {
        entry: LogEntry {
            term: 1,
            index: 256,
            command: MeshCommand::set_lockdown("x".repeat(1011), false),
        },
        parent: [1; 32],
    };
    state.hash = block.hash();
    state.lockdown = names.iter().map(|id| (id.clone(), (255, false))).collect();
    let commit = sign_cert(&n, Phase::Commit, 1, block, &[0, 1, 2, 3, 4, 5, 6]);
    let votes = n
        .iter()
        .map(|s| {
            let mut v = CheckpointVote {
                height: state.height,
                hash: state.hash,
                state_hash: state.digest(),
                voter: s.node_id.clone(),
                signature: vec![],
            };
            v.signature = s
                .identity
                .sign_progress_payload(&v.payload(&s.signing_context));
            (s.node_id.clone(), v)
        })
        .collect();
    // Deliberately maximal serialized evidence; this fixture measures/verifies
    // the certificate envelope rather than simulating these 256 decisions.
    let snapshot = CertifiedSnapshot {
        state,
        commit,
        votes,
    };
    n[0].verify_snapshot(&snapshot).unwrap();
    let rpc = n[0].phase_rpc(PhaseMessage::Snapshot(Box::new(snapshot)));
    let bytes = n[0].rpc_wire_size(&rpc).unwrap();
    assert!(bytes < MAX_WIRE_MESSAGE_BYTES as u64);
    println!("7-member 256-rule maximum snapshot wire bytes={bytes}");
}
