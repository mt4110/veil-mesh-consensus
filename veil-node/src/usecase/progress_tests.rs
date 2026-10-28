use super::super::regression_tests::{cluster, elect, fixture_dir};
use super::*;
use crate::infra::wal::FileWal;

fn replicate(nodes: &mut [ConsensusState], leader: usize) {
    for _ in 0..3 {
        for i in 0..nodes.len() {
            if i == leader {
                continue;
            }
            let rpc = nodes[leader]
                .make_append_entries(&nodes[i].node_id)
                .unwrap();
            let signed = nodes[leader].sign_rpc(rpc);
            let reply = nodes[i].handle_signed_rpc(signed).unwrap().unwrap();
            let signed = nodes[i].sign_rpc(reply);
            nodes[leader].handle_signed_rpc(signed).unwrap();
        }
    }
}
fn setup(ttl: u32) -> (Vec<ConsensusState>, [u8; 32]) {
    let mut n = cluster();
    elect(&mut n);
    n[0].censor_ban_requests = true;
    n[0].propose(MeshCommand::NoOp).unwrap();
    replicate(&mut n, 0);
    let id = n[1]
        .accept_ban_request(Ipv4Addr::new(192, 0, 2, 71), ttl)
        .unwrap();
    let rpc = n[1].drive_ban_progress().unwrap().unwrap();
    for i in [0, 2, 3] {
        n[i].handle_signed_rpc(rpc.clone()).unwrap();
    }
    for node in &mut n[1..] {
        node.drive_ban_progress().unwrap();
    }
    (n, id)
}
fn age(n: &mut [ConsensusState]) {
    for node in n {
        for watch in node.progress.watches.values_mut() {
            watch.2 = Instant::now() - Duration::from_secs(6);
        }
    }
}
fn certify(n: &mut [ConsensusState]) -> ProgressCertificate {
    age(&mut n[1..]);
    let votes: Vec<_> = n[1..]
        .iter_mut()
        .map(|s| s.drive_ban_progress().unwrap().unwrap())
        .collect();
    for rpc in votes {
        for node in &mut n[1..] {
            node.handle_signed_rpc(rpc.clone()).unwrap();
        }
    }
    let cert = n[1].progress.durable.exclusions["a"].clone();
    for node in n.iter_mut() {
        node.receive_progress(None, None, Some(cert.clone()))
            .unwrap();
        node.checkpoint().unwrap();
    }
    cert
}
#[test]
fn fresh_ack_and_unrelated_commits_cannot_hide_pending_ban_handoff_preserves_qc_locks() {
    let (mut n, id) = setup(60);
    let locks = n[1].voted_entry_hashes.clone();
    let before = n[1].election_activity();
    for _ in 0..3 {
        let (_, probe) = n[1].make_liveness_probe().unwrap().unwrap();
        let ack = n[0].handle_signed_rpc(probe).unwrap().unwrap();
        let signed = n[0].sign_rpc(ack);
        n[1].handle_signed_rpc(signed).unwrap();
        n[0].propose(MeshCommand::NoOp).unwrap();
        replicate(&mut n, 0);
    }
    assert!(n[1].election_activity() > before);
    assert!(n[1].ban_request_status(&id).contains("queued"));
    let cert = certify(&mut n);
    assert!(n
        .iter()
        .all(|s| s.current_term == 2 && s.leader_excluded("a")));
    for (key, hash) in locks {
        assert_eq!(n[1].voted_entry_hashes[&key], hash);
    }
    let original = cert.request.command.clone();
    assert!(n[0].start_signed_election().is_err());
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
    n[1].drive_ban_progress().unwrap();
    replicate(&mut n, 1);
    assert!(n.iter().all(|s| s.request_committed(&cert.request)));
    assert_eq!(n[1].log.last().unwrap().command, original);
    assert!(n[1].ban_request_status(&id).contains("applied"));
    let term = n[1].current_term;
    let bytes = n[1].wal_bytes().unwrap();
    for _ in 0..5 {
        n[1].receive_progress(None, None, Some(cert.clone()))
            .unwrap();
    }
    assert_eq!(n[1].current_term, term);
    assert_eq!(n[1].wal_bytes().unwrap(), bytes);
}
#[test]
fn progress_requires_distinct_context_bound_quorum_and_outer_term_is_not_authority() {
    let (mut n, _) = setup(60);
    age(&mut n[1..]);
    let v = n[1].drive_ban_progress().unwrap().unwrap();
    n[2].drive_ban_progress().unwrap(); // Two distinct voters still cannot form a 3/4 certificate.
    for _ in 0..6 {
        n[2].handle_signed_rpc(v.clone()).unwrap();
    }
    assert_eq!(n[2].current_term, 1);
    let mut cert = ProgressCertificate {
        term: 1,
        leader: "a".into(),
        request: n[1].progress.durable.requests["b"].clone(),
        votes: BTreeMap::new(),
    };
    let payload = progress_payload(
        &n[0].signing_context,
        1,
        "a",
        &cert.request.id(&n[0].signing_context),
    );
    cert.votes
        .insert("b".into(), n[1].identity.sign_progress_payload(&payload));
    let before = n[2].wal_bytes().unwrap();
    assert!(n[2]
        .receive_progress(None, None, Some(cert.clone()))
        .is_err());
    cert.votes.insert("c".into(), cert.votes["b"].clone());
    cert.votes.insert("d".into(), cert.votes["b"].clone());
    assert!(n[2]
        .receive_progress(None, None, Some(cert.clone()))
        .is_err());
    for node in &n[1..] {
        cert.votes.insert(
            node.node_id.clone(),
            node.identity.sign_progress_payload(&payload),
        );
    }
    let foreign = SigningContext::new("other-cluster", 1).unwrap();
    let foreign_payload =
        progress_payload(&foreign, 1, "a", &cert.request.id(&n[0].signing_context));
    let mut foreign_cert = cert.clone();
    for node in &n[1..] {
        foreign_cert.votes.insert(
            node.node_id.clone(),
            node.identity.sign_progress_payload(&foreign_payload),
        );
    }
    assert!(n[2]
        .receive_progress(None, None, Some(foreign_cert))
        .is_err());
    let mut bad = cert.clone();
    bad.term = 100;
    assert!(n[2].receive_progress(None, None, Some(bad)).is_err());
    let mut bad = cert.clone();
    bad.request.origin = "a".into();
    assert!(n[2].receive_progress(None, None, Some(bad)).is_err());
    let rpc = n[0].sign_rpc(ConsensusRpc::ProgressGossip {
        term: MAX_SAFE_TERM - 1,
        sender_id: "a".into(),
        request: None,
        vote: None,
        certificate: None,
    });
    n[2].handle_signed_rpc(rpc).unwrap();
    assert_eq!(n[2].wal_bytes().unwrap(), before);
    n[2].receive_progress(None, None, Some(cert)).unwrap();
    assert_eq!(n[2].current_term, 2);
}
#[test]
fn progress_wal_restores_request_exclusion_and_never_renews_expiry() {
    let (mut n, id) = setup(30);
    let path = fixture_dir().join("request.wal");
    n[1].enable_persistence(Arc::new(FileWal::open(&path).unwrap()))
        .unwrap();
    let cert = certify(&mut n);
    let locks = n[1].voted_entry_hashes.clone();
    let mut restored = ConsensusState::new("b", vec!["a".into(), "c".into(), "d".into()]);
    restored.set_crypto_identity(n[1].identity.clone()).unwrap();
    restored
        .register_peer_public_keys(n[1].peer_public_keys.clone())
        .unwrap();
    restored.load_wal(&path).unwrap();
    assert!(restored.leader_excluded("a"));
    assert_eq!(restored.voted_entry_hashes, locks);
    assert_eq!(
        restored.progress.durable.requests["b"].command,
        cert.request.command
    );
    assert!(restored.progress.watches.is_empty());
    restored.monotonic_now_ns = 31_000_000_000;
    assert!(!restored.leader_excluded("a"));
    assert!(restored.ban_request_status(&id).contains("expired"));
    let before = restored.log.len();
    restored.role = Role::Leader;
    restored.drive_ban_progress().unwrap();
    assert_eq!(restored.log.len(), before);
    restored
        .receive_progress(Some(cert.request.clone()), None, Some(cert))
        .unwrap();
    assert_eq!(restored.log.len(), before);
}
#[test]
fn witness_rejects_invalid_or_overcapacity_requests_and_preserves_wal_on_failure() {
    struct Fail;
    impl ConsensusPersistence for Fail {
        fn persist(&self, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("injected"))
        }
    }
    let (mut n, _) = setup(60);
    for (ip, ttl) in [
        (Ipv4Addr::LOCALHOST, 30),
        (Ipv4Addr::new(192, 0, 2, 1), 0),
        (Ipv4Addr::new(192, 0, 2, 1), 86401),
    ] {
        assert!(n[1].accept_ban_request(ip, ttl).is_err());
    }
    assert!(n[1]
        .accept_ban_request(Ipv4Addr::new(192, 0, 2, 72), 60)
        .is_err());
    assert_eq!(n[1].progress.durable.requests.len(), 1);
    n[2].persistence = Some(Arc::new(Fail));
    n[2].persisted_snapshot_hash = None;
    assert!(n[2]
        .accept_ban_request(Ipv4Addr::new(192, 0, 2, 73), 60)
        .is_err());
    assert!(n[2].drive_ban_progress().is_err());
    assert!(n[2].start_signed_election().is_err());
    assert!(n[2].persistence_error.is_some());
}

#[test]
fn excluded_leader_cannot_reenter_via_votes_or_append_even_with_admitted_term() {
    let (mut n, _) = setup(60);
    certify(&mut n);
    let votes = n[..3]
        .iter()
        .map(|s| {
            (
                s.node_id.clone(),
                s.identity.sign_term_support(&s.signing_context, 3),
            )
        })
        .collect();
    let cert = TermCertificate::PreVote { term: 3, votes };
    let inner = ConsensusRpc::RequestVote {
        term: 3,
        candidate_id: "a".into(),
        last_log_index: n[0].last_log_index(),
        last_log_term: n[0].last_log_term(),
    };
    let rpc = n[0].sign_rpc(ConsensusRpc::TermCertified {
        term: 3,
        sender_id: "a".into(),
        certificate: cert.clone(),
        rpc_bytes: bincode::serialize(&inner).unwrap(),
    });
    let response = n[1].handle_signed_rpc(rpc).unwrap().unwrap();
    assert!(matches!(
        response,
        ConsensusRpc::RequestVoteResponse {
            vote_granted: false,
            ..
        }
    ));
    assert!(n[1].voted_for.is_none());
    let before = n[1].voted_entry_hashes.clone();
    let inner = ConsensusRpc::AppendEntries {
        term: 3,
        leader_id: "a".into(),
        prev_log_index: n[0].last_log_index(),
        prev_log_term: n[0].last_log_term(),
        prev_log_hash: n[0].entry_hash_at(n[0].last_log_index()).unwrap_or([0; 32]),
        entries: vec![],
        leader_commit: 0,
        commit_qc: None,
    };
    let rpc = n[0].sign_rpc(ConsensusRpc::TermCertified {
        term: 3,
        sender_id: "a".into(),
        certificate: cert,
        rpc_bytes: bincode::serialize(&inner).unwrap(),
    });
    let response = n[1].handle_signed_rpc(rpc).unwrap().unwrap();
    assert!(matches!(
        response,
        ConsensusRpc::AppendEntriesResponse { success: false, .. }
    ));
    assert_eq!(n[1].voted_entry_hashes, before);
    assert_ne!(n[1].leader_id.as_deref(), Some("a"));
}

#[test]
fn legacy_v5_wal_upgrade_retains_commit_qc_vote_history_and_exclusive_vote() {
    let (n, _) = setup(60);
    let bytes = n[1].wal_bytes().unwrap();
    let mut env: WalEnvelope = bincode::deserialize(&bytes).unwrap();
    let (mut payload, certificate, _): (WalPayload, Option<TermCertificate>, ProgressDurable) =
        bincode::deserialize(&env.payload_bytes).unwrap();
    payload.version = 5;
    env.payload_bytes = bincode::serialize(&(payload, certificate)).unwrap();
    env.sha256_checksum = ring::digest::digest(&ring::digest::SHA256, &env.payload_bytes)
        .as_ref()
        .try_into()
        .unwrap();
    let path = fixture_dir().join("legacy5.wal");
    let legacy = bincode::serialize(&env).unwrap();
    FileWal::open(&path).unwrap().persist(&legacy).unwrap();
    let mut restored = ConsensusState::new("b", vec!["a".into(), "c".into(), "d".into()]);
    restored.set_crypto_identity(n[1].identity.clone()).unwrap();
    restored
        .register_peer_public_keys(n[1].peer_public_keys.clone())
        .unwrap();
    restored.load_wal(&path).unwrap();
    assert_eq!(restored.current_term, n[1].current_term);
    assert_eq!(restored.voted_for, n[1].voted_for);
    assert_eq!(restored.commit_qcs, n[1].commit_qcs);
    assert_eq!(restored.voted_entry_hashes, n[1].voted_entry_hashes);
    assert_eq!(restored.log, n[1].log);
    assert_eq!(std::fs::read(path).unwrap(), legacy);
    assert!(restored.progress.durable.requests.is_empty());
}

#[test]
fn request_receipt_is_withheld_when_new_request_checkpoint_fails() {
    use std::sync::Mutex;
    struct FailNewRequest(Mutex<Vec<u8>>);
    impl ConsensusPersistence for FailNewRequest {
        fn persist(&self, bytes: &[u8]) -> std::io::Result<()> {
            let env: WalEnvelope = bincode::deserialize(bytes).unwrap();
            let (_, _, progress): (WalPayload, Option<TermCertificate>, ProgressDurable) =
                bincode::deserialize(&env.payload_bytes).unwrap();
            if !progress.requests.is_empty() {
                return Err(std::io::Error::other("injected request write failure"));
            }
            *self.0.lock().unwrap() = bytes.to_vec();
            Ok(())
        }
    }
    let mut n = cluster();
    let store = Arc::new(FailNewRequest(Mutex::new(vec![])));
    n[1].enable_persistence(store.clone()).unwrap();
    let before = store.0.lock().unwrap().clone();
    assert!(n[1]
        .accept_ban_request(Ipv4Addr::new(192, 0, 2, 10), 60)
        .is_err());
    assert_eq!(*store.0.lock().unwrap(), before);
    assert!(n[1].persistence_error.is_some());
    assert!(n[1].drive_ban_progress().is_err());
    assert!(n[1].start_signed_election().is_err());
}

#[test]
fn certified_censorship_handoff_can_interrupt_an_unrelated_old_leader_noop() {
    use super::super::two_phase_tests as phase;
    let mut n = phase::nodes();
    phase::elect(&mut n, 0, &[1, 2, 3]);
    n[0].drive_two_phase().unwrap();
    for _ in 0..4 {
        phase::round(&mut n, 0, &[1, 2, 3]);
    }
    n[0].censor_ban_requests = true;
    let id = n[1]
        .accept_ban_request(Ipv4Addr::new(192, 0, 2, 72), 25)
        .unwrap();
    let request = n[1].progress.durable.requests["b"].clone();
    let gossip = n[1].drive_ban_progress().unwrap().unwrap();
    for node in &mut n[2..] {
        node.handle_signed_rpc(gossip.clone()).unwrap();
        node.drive_ban_progress().unwrap();
    }
    let old_term = n[0].current_term;
    let noop = n[0].propose(MeshCommand::NoOp).unwrap();
    phase::round(&mut n, 0, &[1, 2, 3]);
    assert!(n[0].commit_index < noop.index);
    let old_rpc = n[0].make_append_entries("b").unwrap();
    let old_rpc = n[0].sign_rpc_durable(old_rpc).unwrap();
    // Deterministic ordering: the request grace expires while an unrelated
    // old-view NoOp is in flight. Do not change the real driver's timer.
    age(&mut n[1..]);
    let votes: Vec<_> = n[1..]
        .iter_mut()
        .map(|s| s.drive_ban_progress().unwrap().unwrap())
        .collect();
    for vote in votes {
        for node in &mut n[1..] {
            node.handle_signed_rpc(vote.clone()).unwrap();
        }
    }
    for node in &mut n[1..] {
        assert!(node.current_term > old_term);
        assert!(node.certified_handoff_for(&id, old_term, "a"));
        assert!(!node.certified_handoff_for(&[0; 32], old_term, "a"));
        assert!(!node.certified_handoff_for(&id, old_term + 1, "a"));
        assert!(!node.certified_handoff_for(&id, old_term, "b"));
        assert!(node.leader_excluded("a"));
        assert!(node.handle_signed_rpc(old_rpc.clone()).is_err());
    }
    assert_eq!(n[0].current_term, old_term);
    assert!(n[0].commit_index < noop.index);
    phase::elect(&mut n, 1, &[2, 3]);
    for _ in 0..10 {
        n[1].drive_two_phase().unwrap();
        n[1].drive_ban_progress().unwrap();
        phase::round(&mut n, 1, &[2, 3]);
    }
    for node in &n[1..] {
        assert!(node.request_committed(&request));
        assert!(node.ban_request_status(&id).contains("applied"));
        assert!(node.log.iter().any(|e| e.command == request.command));
    }
}

#[test]
fn censorship_candidate_retries_lost_view_report_without_a_new_term_or_vote() {
    use super::super::two_phase_tests as phase;
    let mut n = phase::nodes();
    phase::elect(&mut n, 0, &[1, 2, 3]);
    n[0].drive_two_phase().unwrap();
    for _ in 0..4 {
        phase::round(&mut n, 0, &[1, 2, 3]);
    }
    n[0].censor_ban_requests = true;
    n[1].accept_ban_request(Ipv4Addr::new(192, 0, 2, 73), 25)
        .unwrap();
    let gossip = n[1].drive_ban_progress().unwrap().unwrap();
    for node in &mut n[2..] {
        node.handle_signed_rpc(gossip.clone()).unwrap();
        node.drive_ban_progress().unwrap();
    }
    age(&mut n[1..]);
    let votes: Vec<_> = n[1..]
        .iter_mut()
        .map(|s| s.drive_ban_progress().unwrap().unwrap())
        .collect();
    for vote in votes {
        for node in &mut n[1..] {
            node.handle_signed_rpc(vote.clone()).unwrap();
        }
    }
    let pre = n[1].start_pre_vote().unwrap();
    for i in [2, 3] {
        let reply = n[i].handle_signed_rpc(pre.clone()).unwrap().unwrap();
        phase::send(&mut n, i, 1, reply);
    }
    let req = n[1].finish_pre_vote().unwrap().unwrap();
    let reply = n[2].handle_signed_rpc(req.clone()).unwrap().unwrap();
    phase::send(&mut n, 2, 1, reply);
    let lost = n[3].handle_signed_rpc(req).unwrap().unwrap();
    let frozen = n[3].wal_bytes().unwrap();
    let candidate = n[1].wal_bytes().unwrap();
    let term = n[1].current_term;
    assert_eq!(n[1].role, Role::Candidate);
    let retry = n[1]
        .drive_two_phase()
        .unwrap()
        .expect("candidate must retry lost report in the same term");
    assert_eq!(n[1].current_term, term);
    assert_eq!(n[1].wal_bytes().unwrap(), candidate);
    let replay = n[3].handle_signed_rpc(retry).unwrap().unwrap();
    assert_eq!(
        bincode::serialize(&replay).unwrap(),
        bincode::serialize(&lost).unwrap()
    );
    assert_eq!(n[3].wal_bytes().unwrap(), frozen);
    phase::send(&mut n, 3, 1, replay);
    assert_eq!(n[1].role, Role::Leader);
    assert_eq!(n[1].current_term, term);
}
