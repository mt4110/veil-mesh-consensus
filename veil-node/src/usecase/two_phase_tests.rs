use super::regression_tests::{cluster, fixture_dir};
use super::*;
use crate::domain::{
    phase_vote_payload, NewView, Phase, PhaseBlock, PhaseCertificate, PhaseMessage,
};

#[tokio::test]
async fn durable_quorum_advances_phases_before_the_slow_peer_finishes() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    n[0].propose(MeshCommand::NoOp).unwrap();
    let peers: BTreeMap<_, _> = ["b", "c", "d"]
        .into_iter()
        .map(|id| (id.to_owned(), "127.0.0.1:1".parse().unwrap()))
        .collect();
    let mut work = Vec::new();
    for (id, addr) in &peers {
        let rpc = n[0].make_append_entries(id).unwrap();
        work.push((id.clone(), *addr, n[0].sign_rpc_durable(rpc).unwrap()));
    }
    let leader = Arc::new(tokio::sync::Mutex::new(n.remove(0)));
    let followers: BTreeMap<_, _> = n
        .into_iter()
        .map(|s| (s.node_id.clone(), Arc::new(tokio::sync::Mutex::new(s))))
        .collect();
    let followers = Arc::new(followers);
    let calls = Arc::new(AtomicUsize::new(0));
    let slow_finished = Arc::new(AtomicBool::new(false));
    let initial_barrier = Arc::new(tokio::sync::Barrier::new(3));
    let prepared_seen = Arc::new(tokio::sync::Notify::new());
    let seen = prepared_seen.clone();
    let l = leader.clone();
    let f = followers.clone();
    let c = calls.clone();
    let slow = slow_finished.clone();
    let task = tokio::spawn(async move {
        crate::infra::daemon::exchange_round(l, &peers, work, move |id, _, rpc| {
            let f = f.clone();
            let c = c.clone();
            let slow = slow.clone();
            let barrier = initial_barrier.clone();
            let seen = seen.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                let ConsensusRpc::TwoPhase { message, .. } = &rpc.rpc else {
                    panic!("phase RPC")
                };
                let initial = matches!(message.as_ref(), PhaseMessage::Proposal { .. });
                let prepared = matches!(message.as_ref(), PhaseMessage::Prepared { .. });
                if initial {
                    barrier.wait().await;
                }
                if id == "d" {
                    struct Finished(Arc<AtomicBool>);
                    impl Drop for Finished {
                        fn drop(&mut self) {
                            self.0.store(true, Ordering::SeqCst);
                        }
                    }
                    let _finished = Finished(slow);
                    std::future::pending::<()>().await;
                } else if prepared {
                    assert!(
                        !slow.load(Ordering::SeqCst),
                        "do not wait for the slow peer's deadline"
                    );
                    seen.notify_one();
                }
                crate::infra::state_worker::run(f[&id].clone(), move |s| {
                    let reply = s.handle_signed_rpc(rpc).map_err(anyhow::Error::msg)?;
                    reply
                        .map(|r| s.sign_rpc_durable(r).map_err(anyhow::Error::msg))
                        .transpose()
                })
                .await?
            }
        })
        .await
    });
    let mut task = task;
    tokio::select! {
        _ = prepared_seen.notified() => {},
        result = &mut task => panic!("round ended without prompt Prepared delivery: {result:?}"),
        _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => panic!("no prompt Prepared delivery"),
    }
    task.await.unwrap().unwrap();
    assert_eq!(leader.lock().await.commit_index, 1);
    for id in ["b", "c"] {
        assert_eq!(followers[id].lock().await.commit_index, 1);
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        9,
        "initial round plus at most two phase followups"
    );
}

#[tokio::test]
async fn failed_prepare_save_never_dispatches_phase_followups() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    n[0].propose(MeshCommand::NoOp).unwrap();
    let (path, store) = view_vote_store();
    n[0].enable_persistence(store.clone()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let peers: BTreeMap<_, _> = ["b", "c", "d"]
        .into_iter()
        .map(|id| (id.to_owned(), "127.0.0.1:1".parse().unwrap()))
        .collect();
    let mut work = Vec::new();
    for (id, addr) in &peers {
        let rpc = n[0].make_append_entries(id).unwrap();
        work.push((id.clone(), *addr, n[0].sign_rpc_durable(rpc).unwrap()));
    }
    store.fail.store(true, Ordering::SeqCst);
    let leader = Arc::new(tokio::sync::Mutex::new(n.remove(0)));
    let followers: Arc<BTreeMap<_, _>> = Arc::new(
        n.into_iter()
            .map(|s| (s.node_id.clone(), Arc::new(tokio::sync::Mutex::new(s))))
            .collect(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    crate::infra::daemon::exchange_round(leader.clone(), &peers, work, move |id, _, rpc| {
        let st = followers[&id].clone();
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            crate::infra::state_worker::run(st, move |s| {
                let response = s.handle_signed_rpc(rpc).map_err(anyhow::Error::msg)?;
                response
                    .map(|r| s.sign_rpc_durable(r).map_err(anyhow::Error::msg))
                    .transpose()
            })
            .await?
        }
    })
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(leader.lock().await.persistence_error.is_some());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

pub(super) fn nodes() -> Vec<ConsensusState> {
    let mut n = cluster();
    for s in &mut n {
        s.enable_two_phase().unwrap();
    }
    n
}
pub(super) fn send(
    n: &mut [ConsensusState],
    from: usize,
    to: usize,
    rpc: ConsensusRpc,
) -> Option<ConsensusRpc> {
    let signed = n[from].sign_rpc_durable(rpc).unwrap();
    n[to].handle_signed_rpc(signed).unwrap()
}
pub(super) fn elect(n: &mut [ConsensusState], leader: usize, peers: &[usize]) {
    if n[leader].current_term > 0 {
        let pre = n[leader].start_pre_vote().unwrap();
        for &i in peers {
            let r = n[i].handle_signed_rpc(pre.clone()).unwrap().unwrap();
            send(n, i, leader, r);
        }
        let req = n[leader].finish_pre_vote().unwrap().unwrap();
        for &i in peers {
            if let Some(r) = n[i].handle_signed_rpc(req.clone()).unwrap() {
                send(n, i, leader, r);
            }
        }
    } else {
        let req = n[leader].start_signed_election().unwrap();
        for &i in peers {
            let r = n[i].handle_signed_rpc(req.clone()).unwrap().unwrap();
            send(n, i, leader, r);
        }
    }
    assert_eq!(n[leader].role, Role::Leader);
}
pub(super) fn round(n: &mut [ConsensusState], leader: usize, peers: &[usize]) {
    for &i in peers {
        if let Some(rpc) = n[leader].make_append_entries(&n[i].node_id) {
            if let Some(r) = send(n, leader, i, rpc) {
                send(n, i, leader, r);
            }
        }
    }
}
pub(super) fn view(n: &[ConsensusState], i: usize) -> NewView {
    n[i].two_phase
        .as_ref()
        .unwrap()
        .durable
        .active
        .clone()
        .unwrap()
}
fn proposal(n: &[ConsensusState], block: PhaseBlock) -> ConsensusRpc {
    ConsensusRpc::TwoPhase {
        term: n[0].current_term,
        sender_id: "a".into(),
        message: Box::new(PhaseMessage::Proposal {
            view: view(n, 0),
            block,
        }),
    }
}

struct ViewVoteWal {
    disk: crate::infra::wal::FileWal,
    saves: std::sync::atomic::AtomicUsize,
    fail: std::sync::atomic::AtomicBool,
}
impl ConsensusPersistence for ViewVoteWal {
    fn persist(&self, bytes: &[u8]) -> std::io::Result<()> {
        use std::sync::atomic::Ordering;
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("injected view/vote save failure"));
        }
        self.disk.persist(bytes)?;
        self.saves.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn view_vote_store() -> (std::path::PathBuf, Arc<ViewVoteWal>) {
    let path = fixture_dir().join("view-vote.wal");
    let store = Arc::new(ViewVoteWal {
        disk: crate::infra::wal::FileWal::create_new(&path).unwrap(),
        saves: 0.into(),
        fail: false.into(),
    });
    (path, store)
}

fn first_view_request(phase: Phase) -> (Vec<ConsensusState>, ConsensusRpc) {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2]);
    n[0].propose(MeshCommand::NoOp).unwrap();
    if phase == Phase::Commit {
        for i in [1, 2] {
            let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
            let reply = send(&mut n, 0, i, rpc).unwrap();
            send(&mut n, i, 0, reply);
        }
    }
    let rpc = n[0].make_append_entries("d").unwrap();
    assert_eq!(n[3].current_term, 0);
    (n, rpc)
}

#[test]
fn first_view_and_phase_vote_share_one_durable_snapshot() {
    use std::sync::atomic::Ordering;
    for phase in [Phase::Prepare, Phase::Commit] {
        let (mut n, rpc) = first_view_request(phase);
        let (path, store) = view_vote_store();
        n[3].enable_persistence(store.clone()).unwrap();
        let before = store.saves.load(Ordering::SeqCst);
        let reply = send(&mut n, 0, 3, rpc).unwrap();
        assert!(matches!(reply, ConsensusRpc::TwoPhase {message, ..}
            if matches!(*message, PhaseMessage::Vote {phase: p, index: 1, ..} if p == phase)));
        assert_eq!(
            store.saves.load(Ordering::SeqCst) - before,
            1,
            "view and vote have no intervening reply and need only one durable snapshot"
        );
        let restored = restore(&n[3], &path);
        assert_eq!(restored.current_term, 1);
        assert_eq!(restored.commit_index, 0);
        let d = &restored.two_phase.as_ref().unwrap().durable;
        assert_eq!(d.active.as_ref().unwrap(), &view(&n, 0));
        assert!(d.votes.contains_key(&(phase, 1, 1)));
        if phase == Phase::Commit {
            assert!(d.prepared.is_some());
        }
    }
}

#[test]
fn steady_view_prepare_saves_once_and_duplicate_saves_nothing() {
    use std::sync::atomic::Ordering;
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    n[0].propose(MeshCommand::NoOp).unwrap();
    for _ in 0..5 {
        round(&mut n, 0, &[1, 2, 3]);
    }
    let (_, store) = view_vote_store();
    n[3].enable_persistence(store.clone()).unwrap();
    n[0].propose(MeshCommand::NoOp).unwrap();
    let rpc = n[0].make_append_entries("d").unwrap();
    let before = store.saves.load(Ordering::SeqCst);
    let first = send(&mut n, 0, 3, rpc.clone()).unwrap();
    assert_eq!(store.saves.load(Ordering::SeqCst) - before, 1);
    let repeat = send(&mut n, 0, 3, rpc).unwrap();
    assert_eq!(
        bincode::serialize(&first).unwrap(),
        bincode::serialize(&repeat).unwrap()
    );
    assert_eq!(store.saves.load(Ordering::SeqCst) - before, 1);
}

#[test]
fn view_receipt_without_a_vote_still_persists_activation() {
    use std::sync::atomic::Ordering;
    let (mut n, _) = first_view_request(Phase::Commit);
    for _ in 0..3 {
        round(&mut n, 0, &[1, 2]);
    }
    // Commit catch-up does not install the leader's view proof by itself.
    let rpc = n[0].make_append_entries("d").unwrap();
    let receipt = send(&mut n, 0, 3, rpc).unwrap();
    send(&mut n, 3, 0, receipt);
    assert_eq!(n[3].commit_index, 1);
    assert!(n[3].two_phase.as_ref().unwrap().durable.active.is_none());
    let (path, store) = view_vote_store();
    n[3].enable_persistence(store.clone()).unwrap();
    let before = store.saves.load(Ordering::SeqCst);
    let rpc = n[0].make_append_entries("d").unwrap();
    let receipt = send(&mut n, 0, 3, rpc).unwrap();
    assert!(matches!(receipt, ConsensusRpc::TwoPhase {message, ..}
        if matches!(*message, PhaseMessage::Receipt {index: 1, ..})));
    assert_eq!(store.saves.load(Ordering::SeqCst) - before, 1);
    let restored = restore(&n[3], &path);
    let d = &restored.two_phase.as_ref().unwrap().durable;
    assert_eq!(d.active.as_ref().unwrap(), &view(&n, 0));
    assert!(d.votes.is_empty());
}

#[test]
fn failed_combined_view_vote_save_returns_no_vote_and_stops_signing() {
    use std::sync::atomic::Ordering;
    for phase in [Phase::Prepare, Phase::Commit] {
        let (mut n, rpc) = first_view_request(phase);
        let (path, store) = view_vote_store();
        n[3].enable_persistence(store.clone()).unwrap();
        let before = std::fs::read(&path).unwrap();
        store.fail.store(true, Ordering::SeqCst);
        let signed = n[0].sign_rpc_durable(rpc.clone()).unwrap();
        assert!(n[3].handle_signed_rpc(signed.clone()).is_err());
        assert!(n[3].persistence_error.is_some());
        assert!(n[3].sign_rpc_durable(rpc).is_err());
        assert!(n[3].handle_signed_rpc(signed).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let restored = restore(&n[3], &path);
        assert_eq!(restored.current_term, 0);
        assert!(restored
            .two_phase
            .as_ref()
            .unwrap()
            .durable
            .votes
            .is_empty());
    }
}

#[test]
fn rejected_proposal_still_persists_the_admitted_view_without_a_vote() {
    let (mut n, rpc) = first_view_request(Phase::Prepare);
    let ConsensusRpc::TwoPhase {
        term,
        sender_id,
        mut message,
    } = rpc
    else {
        unreachable!()
    };
    let PhaseMessage::Proposal { block, .. } = message.as_mut() else {
        unreachable!()
    };
    block.parent = [9; 32];
    let bad = ConsensusRpc::TwoPhase {
        term,
        sender_id,
        message,
    };
    let (path, store) = view_vote_store();
    n[3].enable_persistence(store).unwrap();
    let signed = n[0].sign_rpc_durable(bad).unwrap();
    assert!(n[3].handle_signed_rpc(signed).is_err());
    let restored = restore(&n[3], &path);
    assert_eq!(restored.current_term, 1);
    let d = &restored.two_phase.as_ref().unwrap().durable;
    assert_eq!(d.active.as_ref().unwrap(), &view(&n, 0));
    assert!(d.votes.is_empty());
}

pub(crate) fn split(n: &mut [ConsensusState]) {
    elect(n, 0, &[1, 2, 3]);
    let a = PhaseBlock {
        entry: LogEntry {
            term: 1,
            index: 1,
            command: MeshCommand::NoOp,
        },
        parent: [0; 32],
    };
    let b = PhaseBlock {
        entry: LogEntry {
            term: 1,
            index: 1,
            command: MeshCommand::set_trust(Ipv4Addr::new(192, 0, 2, 90), true),
        },
        parent: [0; 32],
    };
    for (i, b) in [(1, a.clone()), (2, b), (3, a)] {
        let rpc = proposal(n, b);
        send(n, 0, i, rpc);
    }
}
pub(super) fn restore(old: &ConsensusState, path: &std::path::Path) -> ConsensusState {
    let mut s = ConsensusState::new(&old.node_id, old.peers.clone());
    s.set_crypto_identity(old.identity.clone()).unwrap();
    s.register_peer_public_keys(old.peer_public_keys.clone())
        .unwrap();
    s.enable_two_phase().unwrap();
    s.load_wal(path).unwrap();
    s
}
#[test]
fn unprepared_split_recovers_in_new_view_without_erasing_old_votes() {
    let mut n = nodes();
    split(&mut n);
    let old: Vec<_> = n
        .iter()
        .map(|s| s.two_phase.as_ref().unwrap().durable.votes.clone())
        .collect();
    elect(&mut n, 1, &[2, 3]);
    n[1].drive_two_phase().unwrap();
    for _ in 0..5 {
        round(&mut n, 1, &[2, 3]);
    }
    for i in 1..4 {
        assert_eq!(n[i].commit_index, 1);
        assert_eq!(n[i].entry_hash_at(1), n[1].entry_hash_at(1));
        for (k, v) in &old[i] {
            assert_eq!(
                n[i].two_phase.as_ref().unwrap().durable.votes.get(k),
                Some(v)
            );
        }
    }
}
#[test]
fn prepare_does_not_apply_and_commit_evidence_survives_wal_before_vote() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    let path = fixture_dir().join("b.wal");
    n[1].enable_persistence(Arc::new(
        crate::infra::wal::FileWal::create_new(&path).unwrap(),
    ))
    .unwrap();
    n[0].propose(MeshCommand::set_trust_with_ttl(
        Ipv4Addr::new(192, 0, 2, 91),
        false,
        30,
    ))
    .unwrap();
    // Only gather prepares. No commit certificate or application yet.
    for i in [1, 2] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let r = send(&mut n, 0, i, rpc).unwrap();
        send(&mut n, i, 0, r);
    }
    assert_eq!(n[0].commit_index, 0);
    assert_eq!(n[0].last_applied, 0);
    let rpc = n[0].make_append_entries("b").unwrap();
    let reply = send(&mut n, 0, 1, rpc).unwrap();
    assert!(
        matches!(reply,ConsensusRpc::TwoPhase {message,..} if matches!(*message,PhaseMessage::Vote {phase:Phase::Commit,..}))
    );
    let restored = restore(&n[1], &path);
    assert!(restored
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .prepared
        .is_some());
    assert!(restored
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .votes
        .contains_key(&(Phase::Commit, 1, 1)));
    assert_eq!(restored.commit_index, 0);
}
pub(super) fn sign_cert(
    n: &[ConsensusState],
    phase: Phase,
    view: u64,
    block: PhaseBlock,
    ids: &[usize],
) -> PhaseCertificate {
    let payload = phase_vote_payload(
        &n[0].signing_context,
        phase,
        view,
        block.entry.index,
        &block.hash(),
    );
    PhaseCertificate {
        phase,
        view,
        block,
        votes: ids
            .iter()
            .map(|&i| {
                (
                    n[i].node_id.clone(),
                    n[i].identity.sign_progress_payload(&payload),
                )
            })
            .collect(),
    }
}
#[test]
fn hidden_commit_is_carried_across_two_views_and_restarts_with_original_deadline() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    n[0].propose(MeshCommand::set_trust_with_ttl(
        Ipv4Addr::new(192, 0, 2, 92),
        false,
        60,
    ))
    .unwrap();
    let block = PhaseBlock {
        entry: n[0].log[0].clone(),
        parent: [0; 32],
    };
    // a/b/d prepare, and a receives the QC. Deliver it to b/d, collect commit
    // votes externally, but withhold the completed commit QC from all replicas.
    for i in [1, 3] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let r = send(&mut n, 0, i, rpc).unwrap();
        send(&mut n, i, 0, r);
    }
    let mut votes = BTreeMap::new();
    for i in [1, 3] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let r = send(&mut n, 0, i, rpc).unwrap();
        if let ConsensusRpc::TwoPhase { message, .. } = r {
            if let PhaseMessage::Vote { signature, .. } = *message {
                votes.insert(n[i].node_id.clone(), signature);
            }
        }
    }
    let mut hidden = sign_cert(&n, Phase::Commit, 1, block.clone(), &[0]);
    hidden.votes.extend(votes);
    n[2].verify_phase_certificate(&hidden).unwrap();
    assert!(n.iter().all(|s| s.commit_index == 0));
    let path = fixture_dir().join("b.wal");
    n[1].save_wal(&path).unwrap();
    n[1] = restore(&n[1], &path);
    elect(&mut n, 2, &[1, 3]);
    n[2].drive_two_phase().unwrap();
    // Second handoff before any new-view commit. a is now merely an eligible
    // responder (faulty collector is no longer needed for a quorum).
    n[2].role = Role::Follower;
    elect(&mut n, 1, &[2, 3]);
    n[1].drive_two_phase().unwrap();
    for _ in 0..5 {
        round(&mut n, 1, &[2, 3]);
    }
    for s in &n[1..] {
        assert_eq!(s.log[0], block.entry);
        assert_eq!(s.commit_index, 1);
    }
    // Delayed original proof remains a commit of exactly the same value.
    n[3].install_phase_commit(hidden).unwrap();
    assert_eq!(n[3].log[0], block.entry);
}
#[test]
fn new_view_rejects_omitted_wrong_phase_wrong_candidate_and_stale_signatures() {
    let mut n = nodes();
    split(&mut n);
    elect(&mut n, 1, &[2, 3]);
    let v = view(&n, 1);
    for attack in 0..4 {
        let mut bad = v.clone();
        match attack {
            0 => {
                bad.reports.pop_first();
            }
            1 => bad.leader = "d".into(),
            2 => bad.view += 1,
            _ => {
                bad.reports.values_mut().next().unwrap().signature[0] ^= 1;
            }
        }
        assert!(n[2].select_new_view(&bad).is_err());
    }
    let b = PhaseBlock {
        entry: LogEntry {
            term: 1,
            index: 1,
            command: MeshCommand::NoOp,
        },
        parent: [0; 32],
    };
    let mut c = sign_cert(&n, Phase::Prepare, 1, b, &[0, 1, 2]);
    c.phase = Phase::Commit;
    assert!(n[2].verify_phase_certificate(&c).is_err());
}
#[test]
fn old_wal_is_preserved_and_cannot_become_a_new_protocol_identity() {
    for committed in [false, true] {
        let mut old = cluster();
        super::regression_tests::elect(&mut old);
        old[0].propose(MeshCommand::NoOp).unwrap();
        if committed {
            for i in [1, 2] {
                let rpc = old[0].make_append_entries(&old[i].node_id).unwrap();
                let r = send(&mut old, 0, i, rpc).unwrap();
                send(&mut old, i, 0, r);
            }
        }
        let path = fixture_dir().join("legacy.wal");
        old[0].save_wal(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut s = ConsensusState::new("a", old[0].peers.clone());
        s.set_crypto_identity(old[0].identity.clone()).unwrap();
        s.register_peer_public_keys(old[0].peer_public_keys.clone())
            .unwrap();
        s.enable_two_phase().unwrap();
        assert!(s.load_wal(&path).is_err());
        assert!(s.persistence_error.is_some());
        assert!(s.start_signed_election().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(s.last_applied, 0);
    }
}

#[test]
fn durable_view_fence_blocks_delayed_old_prepare_and_wrong_selected_value() {
    let mut n = nodes();
    split(&mut n);
    let block = PhaseBlock {
        entry: n[1].log[0].clone(),
        parent: [0; 32],
    };
    let prep = sign_cert(&n, Phase::Prepare, 1, block.clone(), &[0, 1, 3]);
    let old_rpc = ConsensusRpc::TwoPhase {
        term: 1,
        sender_id: "a".into(),
        message: Box::new(PhaseMessage::Prepared {
            view: view(&n, 0),
            certificate: prep,
        }),
    };
    // B gets prepare evidence and can emit an old-view commit vote; D does not.
    send(&mut n, 0, 1, old_rpc.clone());
    let path = fixture_dir().join("d.wal");
    n[3].enable_persistence(Arc::new(
        crate::infra::wal::FileWal::create_new(&path).unwrap(),
    ))
    .unwrap();
    elect(&mut n, 2, &[1, 3]);
    let mut d = restore(&n[3], &path);
    let before = d.two_phase.as_ref().unwrap().durable.votes.clone();
    assert!(d.handle_signed_rpc(n[0].sign_rpc(old_rpc)).is_err());
    assert_eq!(d.two_phase.as_ref().unwrap().durable.votes, before);
    let v = view(&n, 2);
    assert_eq!(n[2].select_new_view(&v).unwrap(), Some(block));
    let wrong = PhaseBlock {
        entry: LogEntry {
            term: 2,
            index: 1,
            command: MeshCommand::NoOp,
        },
        parent: [0; 32],
    };
    let rpc = ConsensusRpc::TwoPhase {
        term: 2,
        sender_id: "c".into(),
        message: Box::new(PhaseMessage::Proposal {
            view: v,
            block: wrong,
        }),
    };
    assert!(d.handle_signed_rpc(n[2].sign_rpc(rpc)).is_err());
    assert_eq!(d.two_phase.as_ref().unwrap().durable.votes, before);
}

#[test]
fn prepare_and_view_report_write_failures_release_no_signed_response() {
    struct Fail;
    impl ConsensusPersistence for Fail {
        fn persist(&self, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("injected write failure"))
        }
    }
    for report in [false, true] {
        let mut n = nodes();
        elect(&mut n, 0, &[1, 2, 3]);
        if !report {
            let b = PhaseBlock {
                entry: LogEntry {
                    term: 1,
                    index: 1,
                    command: MeshCommand::NoOp,
                },
                parent: [0; 32],
            };
            let rpc = proposal(&n, b);
            send(&mut n, 0, 1, rpc);
        }
        let before_votes = n[1].two_phase.as_ref().unwrap().durable.votes.clone();
        let path = fixture_dir().join("b.wal");
        n[1].enable_persistence(Arc::new(
            crate::infra::wal::FileWal::create_new(&path).unwrap(),
        ))
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let rpc = if report {
            n[1].step_down(2); // Model independently admitted next term, before durable report.
            ConsensusRpc::TwoPhase {
                term: 2,
                sender_id: "c".into(),
                message: Box::new(PhaseMessage::ViewRequest),
            }
        } else {
            let b = PhaseBlock {
                entry: LogEntry {
                    term: 1,
                    index: 1,
                    command: MeshCommand::NoOp,
                },
                parent: [0; 32],
            };
            let c = sign_cert(&n, Phase::Prepare, 1, b, &[0, 2, 3]);
            ConsensusRpc::TwoPhase {
                term: 1,
                sender_id: "a".into(),
                message: Box::new(PhaseMessage::Prepared {
                    view: view(&n, 0),
                    certificate: c,
                }),
            }
        };
        n[1].persistence = Some(Arc::new(Fail));
        let signed = n[if report { 2 } else { 0 }].sign_rpc(rpc);
        assert!(n[1].handle_signed_rpc(signed).is_err());
        assert!(n[1].persistence_error.is_some());
        assert_eq!(n[1].last_applied, 0);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let restored = restore(&n[1], &path);
        assert_eq!(restored.current_term, 1);
        assert_eq!(
            restored.two_phase.as_ref().unwrap().durable.votes,
            before_votes
        );
        assert!(restored
            .two_phase
            .as_ref()
            .unwrap()
            .durable
            .prepared
            .is_none());
    }
}

#[test]
fn all_prepare_delivery_subsets_preserve_any_hidden_commit_on_handoff() {
    let mut runs = 0;
    for mask in 0..8 {
        for deliveries in 0..8 {
            for leader in 1..4 {
                let mut n = nodes();
                elect(&mut n, 0, &[1, 2, 3]);
                let a = PhaseBlock {
                    entry: LogEntry {
                        term: 1,
                        index: 1,
                        command: MeshCommand::NoOp,
                    },
                    parent: [0; 32],
                };
                let b = PhaseBlock {
                    entry: LogEntry {
                        term: 1,
                        index: 1,
                        command: MeshCommand::set_trust(Ipv4Addr::new(192, 0, 2, 93), true),
                    },
                    parent: [0; 32],
                };
                let mut a_ids = vec![0];
                let mut b_ids = vec![0];
                for i in 1..4 {
                    let block = if mask & (1 << (i - 1)) != 0 {
                        a_ids.push(i);
                        a.clone()
                    } else {
                        b_ids.push(i);
                        b.clone()
                    };
                    let rpc = proposal(&n, block);
                    send(&mut n, 0, i, rpc);
                }
                let (block, ids) = if a_ids.len() >= 3 {
                    (a, a_ids)
                } else {
                    (b, b_ids)
                };
                let prepare = sign_cert(&n, Phase::Prepare, 1, block.clone(), &ids);
                let mut commit_ids = vec![0];
                for i in 1..4 {
                    if deliveries & (1 << (i - 1)) != 0 {
                        let rpc = ConsensusRpc::TwoPhase {
                            term: 1,
                            sender_id: "a".into(),
                            message: Box::new(PhaseMessage::Prepared {
                                view: view(&n, 0),
                                certificate: prepare.clone(),
                            }),
                        };
                        send(&mut n, 0, i, rpc);
                        commit_ids.push(i);
                    }
                }
                let hidden = (commit_ids.len() >= 3)
                    .then(|| sign_cert(&n, Phase::Commit, 1, block.clone(), &commit_ids));
                let peers: Vec<_> = (1..4).filter(|i| *i != leader).collect();
                elect(&mut n, leader, &peers);
                n[leader].drive_two_phase().unwrap();
                for _ in 0..5 {
                    round(&mut n, leader, &peers);
                }
                let hash = n[leader].entry_hash_at(1).unwrap();
                for s in &mut n[1..] {
                    assert_eq!(s.commit_index, 1);
                    assert_eq!(s.entry_hash_at(1), Some(hash));
                    if let Some(c) = &hidden {
                        assert_eq!(hash, c.block.hash());
                        s.install_phase_commit(c.clone()).unwrap();
                    }
                }
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 192);
    println!("PASS: 192 branch/prepare-delivery/leader combinations; hidden commits retained");
}

#[test]
fn sequential_catchup_fetches_committed_parents_and_does_not_extend_expired_ban() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    let ip = Ipv4Addr::new(192, 0, 2, 94);
    n[0].propose(MeshCommand::set_trust_with_ttl(ip, false, 3))
        .unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 3]);
    }
    n[0].propose(MeshCommand::NoOp).unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 3]);
    }
    n[0].propose(MeshCommand::NoOp).unwrap();
    for i in [1, 3] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let r = send(&mut n, 0, i, rpc).unwrap();
        send(&mut n, i, 0, r);
    }
    let rpc = n[0].make_append_entries("b").unwrap();
    send(&mut n, 0, 1, rpc); // prepared height 3, parent 2 committed
    for s in &mut n {
        s.advance_monotonic_clock(5);
    }
    elect(&mut n, 2, &[1, 3]);
    for _ in 0..3 {
        if let Some(query) = n[2].drive_two_phase().unwrap() {
            let reply = n[1].handle_signed_rpc(query).unwrap().unwrap();
            send(&mut n, 1, 2, reply);
        }
    }
    for _ in 0..6 {
        round(&mut n, 2, &[1, 3]);
    }
    for s in &n[1..] {
        assert_eq!(s.commit_index, 3);
        assert_eq!(s.check_xdp_packet(ip), veil_common::XdpVerdict::Pass);
        assert_eq!(s.log[0], n[0].log[0]);
    }
}

#[test]
fn new_phase_context_and_legacy_qc_domains_cannot_be_reused() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    let b = PhaseBlock {
        entry: LogEntry {
            term: 1,
            index: 1,
            command: MeshCommand::NoOp,
        },
        parent: [0; 32],
    };
    let c = sign_cert(&n, Phase::Prepare, 1, b.clone(), &[0, 1, 2]);
    for changed in 0..3 {
        let saved = n[3].signing_context.clone();
        match changed {
            0 => n[3].signing_context.cluster_id = "other".into(),
            1 => n[3].signing_context.config_epoch += 1,
            _ => n[3].signing_context.config_digest[0] ^= 1,
        }
        assert!(n[3].verify_phase_certificate(&c).is_err());
        n[3].signing_context = saved;
    }
    let mut legacy = c;
    for st in n.iter().take(3) {
        legacy.votes.insert(
            st.node_id.clone(),
            st.identity
                .sign_qc_vote(&st.signing_context, 1, 1, &b.hash()),
        );
    }
    assert!(n[3].verify_phase_certificate(&legacy).is_err());
}

#[test]
fn seven_member_worst_case_view_and_maximum_command_fit_wire_and_oversize_is_rejected() {
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
    // 1024-byte serialized policy payload (4-byte tag + 8-byte len + 1011 bytes + bool).
    let command = MeshCommand::set_lockdown("x".repeat(1011), false);
    assert_eq!(bincode::serialized_size(&command).unwrap(), 1024);
    let b = PhaseBlock {
        entry: LogEntry {
            term: 1,
            index: 1,
            command,
        },
        parent: [0; 32],
    };
    let p = sign_cert(&n, Phase::Prepare, 1, b.clone(), &[0, 1, 2, 3, 4, 5, 6]);
    let c = sign_cert(&n, Phase::Commit, 1, b.clone(), &[0, 1, 2, 3, 4, 5, 6]);
    let mut reports = BTreeMap::new();
    for st in &n {
        let mut r = crate::domain::ViewReport {
            view: 2,
            candidate: n[0].node_id.clone(),
            voter: st.node_id.clone(),
            prepared: Some(p.clone()),
            committed: Some(c.clone()),
            signature: vec![],
        };
        r.signature = st
            .identity
            .sign_progress_payload(&r.payload(&st.signing_context));
        reports.insert(st.node_id.clone(), r);
    }
    let v = NewView {
        view: 2,
        leader: n[0].node_id.clone(),
        reports,
    };
    n[0].select_new_view(&v).unwrap();
    let rpc = ConsensusRpc::TwoPhase {
        term: 2,
        sender_id: n[0].node_id.clone(),
        message: Box::new(PhaseMessage::Prepared {
            view: v,
            certificate: p,
        }),
    };
    let bytes = n[0].rpc_wire_size(&rpc).unwrap();
    assert!(bytes < MAX_WIRE_MESSAGE_BYTES as u64);
    println!("7-member maximal evidence envelope: {bytes} bytes (limit {MAX_WIRE_MESSAGE_BYTES})");
    elect(&mut n, 0, &[1, 2, 3, 4, 5, 6]);
    assert!(n[0]
        .propose(MeshCommand::set_lockdown("x".repeat(1012), false))
        .is_err());
    assert_eq!(n[0].last_log_index(), 0);
}

#[test]
fn committed_application_retries_without_new_consensus_or_deadline_renewal() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Bridge(AtomicBool);
    impl EbpfBridge for Bridge {
        fn sync_peer_policy(&self, _: u32, _: PeerPolicy) -> anyhow::Result<()> {
            if self.0.load(Ordering::SeqCst) {
                anyhow::bail!("injected backend failure")
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
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    let bridge = Arc::new(Bridge(AtomicBool::new(true)));
    n[1].ebpf_bridge = Some(bridge.clone());
    n[0].propose(MeshCommand::set_trust_with_ttl(
        Ipv4Addr::new(192, 0, 2, 95),
        false,
        30,
    ))
    .unwrap();
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2, 3]);
    }
    assert_eq!(n[1].commit_index, 1);
    assert_eq!(n[1].last_applied, 0);
    let deadline = n[1].lease_deadlines[&(1, 0)];
    let votes = n[1].two_phase.as_ref().unwrap().durable.votes.clone();
    bridge.0.store(false, Ordering::SeqCst);
    n[1].advance_monotonic_clock(5);
    n[1].drive_two_phase().unwrap();
    assert_eq!(n[1].last_applied, 1);
    assert_eq!(n[1].lease_deadlines[&(1, 0)], deadline);
    assert_eq!(n[1].two_phase.as_ref().unwrap().durable.votes, votes);
}

#[test]
fn restarted_follower_learns_idle_leader_after_certified_catchup() {
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    n[0].drive_two_phase().unwrap();
    for _ in 0..5 {
        round(&mut n, 0, &[1, 2, 3]);
    }
    let path = fixture_dir().join("idle-restart.wal");
    n[0].save_wal(&path).unwrap();
    elect(&mut n, 1, &[2, 3]);
    n[1].drive_two_phase().unwrap();
    for _ in 0..5 {
        round(&mut n, 1, &[2, 3]);
    }
    assert_eq!(n[1].commit_index, 2);
    n[0] = restore(&n[0], &path);
    let votes = n[0].two_phase.as_ref().unwrap().durable.votes.clone();
    for _ in 0..6 {
        round(&mut n, 1, &[0]);
    }
    assert_eq!(n[0].commit_index, n[1].commit_index);
    assert_eq!(n[0].last_applied, n[1].commit_index);
    assert_eq!(n[0].leader_id.as_deref(), Some("b"));
    assert_eq!(n[0].role, Role::Follower);
    assert_eq!(n[0].two_phase.as_ref().unwrap().durable.votes, votes);
}

#[test]
fn lost_prepare_and_commit_replies_retry_without_rewriting_durable_votes() {
    use crate::infra::wal::FileWal;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountedWal {
        disk: FileWal,
        writes: AtomicUsize,
    }
    impl ConsensusPersistence for CountedWal {
        fn persist(&self, bytes: &[u8]) -> std::io::Result<()> {
            self.disk.persist(bytes)?;
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    let root = fixture_dir();
    let stores: Vec<_> = (1..=2)
        .map(|i| {
            let path = root.join(format!("{i}.wal"));
            let store = Arc::new(CountedWal {
                disk: FileWal::create_new(&path).unwrap(),
                writes: AtomicUsize::new(0),
            });
            n[i].enable_persistence(store.clone()).unwrap();
            (path, store)
        })
        .collect();
    n[0].propose(MeshCommand::NoOp).unwrap();
    for phase in [Phase::Prepare, Phase::Commit] {
        // Both receivers finish durable handling, but neither reply reaches
        // the leader. Re-drive exactly those requests after the response loss.
        let requests: Vec<_> = (1..=2)
            .map(|i| n[0].make_append_entries(&n[i].node_id).unwrap())
            .collect();
        let replies: Vec<_> = (1..=2)
            .map(|i| send(&mut n, 0, i, requests[i - 1].clone()).unwrap())
            .collect();
        assert_eq!(n[0].commit_index, 0);
        for i in 1..=2 {
            let (path, store) = &stores[i - 1];
            let before = std::fs::read(path).unwrap();
            let writes = store.writes.load(Ordering::SeqCst);
            let recovered = restore(&n[i], path);
            assert!(recovered
                .two_phase
                .as_ref()
                .unwrap()
                .durable
                .votes
                .contains_key(&(phase, 1, 1)));
            assert_eq!(recovered.commit_index, 0);
            let retry = send(&mut n, 0, i, requests[i - 1].clone()).unwrap();
            assert_eq!(
                bincode::serialize(&retry).unwrap(),
                bincode::serialize(&replies[i - 1]).unwrap()
            );
            assert_eq!(store.writes.load(Ordering::SeqCst), writes);
            assert_eq!(std::fs::read(path).unwrap(), before);
            send(&mut n, i, 0, retry);
        }
    }
    assert_eq!(n[0].commit_index, 1);
    round(&mut n, 0, &[1, 2]);
    assert!(n[..3]
        .iter()
        .all(|s| s.commit_index == 1 && s.last_applied == 1));
}

#[test]
fn late_prepare_votes_preserve_durable_certificate_without_extra_sync() {
    use crate::infra::wal::FileWal;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountedWal {
        disk: FileWal,
        writes: AtomicUsize,
    }
    impl ConsensusPersistence for CountedWal {
        fn persist(&self, bytes: &[u8]) -> std::io::Result<()> {
            self.disk.persist(bytes)?;
            self.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let mut n = nodes();
    let path = fixture_dir().join("leader.wal");
    let store = Arc::new(CountedWal {
        disk: FileWal::open(&path).unwrap(),
        writes: AtomicUsize::new(0),
    });
    n[0].enable_persistence(store.clone()).unwrap();
    elect(&mut n, 0, &[1, 2, 3]);
    n[0].propose(MeshCommand::NoOp).unwrap();
    let mut replies = Vec::new();
    // All followers see the same prepare proposal before any response arrives.
    for i in 1..4 {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        replies.push(send(&mut n, 0, i, rpc).unwrap());
    }
    send(&mut n, 1, 0, replies[0].clone());
    send(&mut n, 2, 0, replies[1].clone());
    let before = std::fs::read(&path).unwrap();
    let writes = store.writes.load(Ordering::SeqCst);
    for _ in 0..3 {
        send(&mut n, 3, 0, replies[2].clone());
    }
    assert_eq!(
        store.writes.load(Ordering::SeqCst),
        writes,
        "late prepare votes must not rewrite an already durable quorum"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    n[0] = restore(&n[0], &path);
    assert_eq!(n[0].commit_index, 0);
    let prepared = n[0]
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .prepared
        .as_ref()
        .unwrap();
    n[0].verify_phase_certificate(prepared).unwrap();
    assert_eq!(prepared.votes.len(), 3);
    assert!(n[0]
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .votes
        .contains_key(&(Phase::Commit, 1, 1)));
    // Restore retains the lock and selected block; a new leader can finish it.
    elect(&mut n, 1, &[0, 2, 3]);
    n[1].drive_two_phase().unwrap();
    for _ in 0..5 {
        round(&mut n, 1, &[0, 2, 3]);
    }
    assert!(n.iter().all(|s| s.commit_index >= 1));
}

#[test]
fn append_prototype_recovery_preserves_votes_certificates_and_expired_deadlines() {
    use crate::infra::snapshot_append_prototype::AppendWal;
    use crate::usecase::ConsensusPersistence;
    let mut n = nodes();
    elect(&mut n, 0, &[1, 2, 3]);
    let root = fixture_dir();
    let path = root.join("append.wal");
    let store =
        Arc::new(AppendWal::create_new(&path, crate::domain::DEFAULT_MAX_WAL_BYTES).unwrap());
    n[1].enable_persistence(store.clone()).unwrap();
    let ip = Ipv4Addr::new(192, 0, 2, 91);
    n[0].propose(MeshCommand::set_trust_with_ttl(ip, false, 3))
        .unwrap();
    for i in [1, 2] {
        let rpc = n[0].make_append_entries(&n[i].node_id).unwrap();
        let reply = send(&mut n, 0, i, rpc).unwrap();
        send(&mut n, i, 0, reply);
    }
    let rpc = n[0].make_append_entries("b").unwrap();
    let reply = send(&mut n, 0, 1, rpc).unwrap();
    let expected = n[1].wal_bytes().unwrap();
    // Force at least one frame-count compaction with the same complete state.
    for _ in 0..20 {
        store.persist(&expected).unwrap();
    }
    n[1].persistence.take();
    drop(store);
    let (store, payload) =
        AppendWal::open_existing(&path, crate::domain::DEFAULT_MAX_WAL_BYTES).unwrap();
    assert_eq!(payload, expected);
    let extracted = root.join("extracted.wal");
    crate::infra::wal::FileWal::create_new(&extracted)
        .unwrap()
        .persist(&payload)
        .unwrap();
    let recovered = restore(&n[1], &extracted);
    let d = &recovered.two_phase.as_ref().unwrap().durable;
    assert!(d.prepared.is_some());
    assert!(d.votes.contains_key(&(Phase::Prepare, 1, 1)));
    assert!(d.votes.contains_key(&(Phase::Commit, 1, 1)));
    assert_eq!(recovered.commit_index, 0);
    n[1] = recovered;
    n[1].enable_persistence(Arc::new(store)).unwrap();
    let conflicting = proposal(
        &n,
        PhaseBlock {
            entry: LogEntry {
                term: 1,
                index: 1,
                command: MeshCommand::NoOp,
            },
            parent: [0; 32],
        },
    );
    let signed = n[0].sign_rpc_durable(conflicting).unwrap();
    assert!(n[1].handle_signed_rpc(signed).is_err());
    send(&mut n, 1, 0, reply);
    for _ in 0..4 {
        round(&mut n, 0, &[1, 2]);
    }
    assert_eq!(n[1].commit_index, 1);
    let original = n[1].log[0].clone();
    n[1].advance_monotonic_clock(5);
    n[1].checkpoint().unwrap();
    assert_eq!(n[1].check_xdp_packet(ip), veil_common::XdpVerdict::Pass);
    n[1].persistence.take();
    let (_store, payload) =
        AppendWal::open_existing(&path, crate::domain::DEFAULT_MAX_WAL_BYTES).unwrap();
    let extracted = root.join("expired.wal");
    crate::infra::wal::FileWal::create_new(&extracted)
        .unwrap()
        .persist(&payload)
        .unwrap();
    // Recovery runs at the current simulated time, not the helper's default
    // time zero. Production supplies its policy clock before loading the WAL.
    let mut recovered = ConsensusState::new(&n[1].node_id, n[1].peers.clone());
    recovered
        .set_crypto_identity(n[1].identity.clone())
        .unwrap();
    recovered
        .register_peer_public_keys(n[1].peer_public_keys.clone())
        .unwrap();
    recovered.enable_two_phase().unwrap();
    recovered.monotonic_now_ns = n[1].monotonic_now_ns;
    recovered.load_wal(&extracted).unwrap();
    assert_eq!(recovered.log[0], original);
    assert_eq!(
        recovered.check_xdp_packet(ip),
        veil_common::XdpVerdict::Pass
    );
    assert!(recovered
        .two_phase
        .as_ref()
        .unwrap()
        .durable
        .commits
        .contains_key(&1));
}
